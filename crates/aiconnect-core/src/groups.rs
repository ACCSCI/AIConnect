//! Owner-authoritative membership; application messages fan out directly between members.
use super::*;
use rusqlite::OptionalExtension;
mod replication;

const MAX_MEMBERS: usize = 64;
const DEFAULT_MEMBERS: usize = 64;
const LEGACY_MEMBERS: usize = 16;
const STATE_BYTES: usize = 44000;
fn group_verify(envelope: &Value) -> Result<Value> {
    Core::verify_bounded(envelope, STATE_BYTES)
}
struct DeliveryClaim {
    inner: Arc<Inner>,
    key: (String, String),
}
impl Drop for DeliveryClaim {
    fn drop(&mut self) {
        self.inner.group_inflight.lock().unwrap().remove(&self.key);
    }
}
fn get(db: &Connection, table: &str, id: &str) -> Result<Value> {
    let raw: String = db
        .query_row(&format!("SELECT data FROM {table} WHERE id=?"), [id], |r| {
            r.get(0)
        })
        .optional()?
        .context("group_not_found")?;
    Ok(serde_json::from_str(&raw)?)
}
fn put(db: &Connection, table: &str, id: &str, value: &Value) -> Result<()> {
    db.execute(&format!("INSERT INTO {table}(id,data) VALUES(?,?) ON CONFLICT(id) DO UPDATE SET data=excluded.data"),params![id,value.to_string()])?;
    Ok(())
}
fn member<'a>(state: &'a Value, id: &str) -> Result<&'a Value> {
    state["members"][id]
        .as_object()
        .filter(|m| m.get("active") == Some(&json!(true)))
        .map(|_| &state["members"][id])
        .context("group_not_member")
}
fn active(state: &Value) -> Result<()> {
    ensure!(state["status"] == "active", "group_dissolved");
    Ok(())
}
fn admin(state: &Value, id: &str) -> Result<()> {
    ensure!(
        matches!(member(state, id)?["role"].as_str(), Some("owner" | "admin")),
        "group_admin_required"
    );
    Ok(())
}
fn validate(envelope: &Value, expected_owner: &str, gid: &str) -> Result<Value> {
    let state = group_verify(envelope).context("group_invalid_signature")?;
    ensure!(
        state["v"] == 1
            && state["kind"] == "group_state"
            && state["id"] == expected_owner
            && state["groupId"] == gid,
        "group_invalid_state"
    );
    let members = state["members"]
        .as_object()
        .context("group_invalid_members")?;
    ensure!(
        (2..=MAX_MEMBERS as u64).contains(
            &state["maxMembers"]
                .as_u64()
                .unwrap_or(LEGACY_MEMBERS as u64)
        ),
        "group_invalid_member_limit"
    );
    ensure!(
        members.len() <= MAX_MEMBERS
            && members.len()
                <= state["maxMembers"]
                    .as_u64()
                    .unwrap_or(LEGACY_MEMBERS as u64) as usize
            && state["revision"].as_u64().is_some(),
        "group_invalid_members"
    );
    ensure!(
        member(&state, expected_owner)?["role"] == "owner",
        "group_invalid_owner"
    );
    for (id, m) in members {
        ensure!(
            m["active"].is_boolean()
                && m["canSend"].is_boolean()
                && matches!(m["role"].as_str(), Some("owner" | "admin" | "member")),
            "group_invalid_member"
        );
        ensure!(
            m["role"] != "owner" || id == expected_owner,
            "group_invalid_owner"
        );
        ensure!(
            EndpointTicket::from_str(text(m, "ticket")?)?
                .endpoint_addr()
                .id
                .to_string()
                == *id,
            "group_invalid_member_ticket"
        );
    }
    Ok(state)
}

impl Core {
    fn group_load(&self, gid: &str) -> Result<Value> {
        let envelope = self.load("chat_groups", gid).context("group_not_found")?;
        let body = group_verify(&envelope).context("group_invalid_signature")?;
        validate(&envelope, text(&body, "id")?, gid)
    }
    fn group_cache(&self, envelope: &Value, owner: &str, gid: &str) -> Result<Value> {
        let state = validate(envelope, owner, gid)?;
        let mut db = self.0.db.lock().unwrap();
        let tx = db.transaction()?;
        if state["replication"] == "members-v1" {
            tx.execute(
                "INSERT OR IGNORE INTO group_snapshots(reference,group_id,data) VALUES(?,?,?)",
                params![text(envelope, "signature")?, gid, envelope.to_string()],
            )?;
        }
        if let Ok(old) = get(&tx, "chat_groups", gid) {
            let old = group_verify(&old)?;
            ensure!(old["id"] == owner, "group_owner_conflict");
            if state["revision"].as_u64() < old["revision"].as_u64() {
                tx.commit()?;
                return Ok(old);
            }
            if state["revision"] == old["revision"] {
                ensure!(state == old, "group_state_conflict");
            }
        }
        put(&tx, "chat_groups", gid, envelope)?;
        if state["replication"] == "members-v1" {
            tx.execute(
                "INSERT OR IGNORE INTO group_snapshots(reference,group_id,data) VALUES(?,?,?)",
                params![text(envelope, "signature")?, gid, envelope.to_string()],
            )?;
        }
        tx.commit()?;
        Ok(state)
    }
    fn group_signed(&self, state: &Value) -> Result<Value> {
        ensure!(
            serde_json::to_vec(state)?.len() < STATE_BYTES,
            "group_state_too_large"
        );
        self.signed(state.clone())
    }
    async fn group_card(&self) -> Result<Value> {
        let profile = self
            .load("settings", "profile")
            .unwrap_or(json!({"name":"Agent"}));
        self.signed(json!({"v":1,"kind":"contact","id":self.id(),"name":profile["name"],"ticket":self.ticket().await}))
    }
    /// Serialized, atomic owner control. No network calls while holding the database lock.
    fn group_authority(&self, actor: &str, p: &Value) -> Result<Value> {
        self.allowed(actor, "chat")?;
        let _guard = self.0.receive_guard.lock().unwrap();
        let gid = text(p, "groupId")?;
        let mut db = self.0.db.lock().unwrap();
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let envelope = get(&tx, "chat_groups", gid)?;
        let mut state = validate(&envelope, &self.id(), gid)?;
        let kind = text(p, "kind")?;
        if kind == "group.join" {
            active(&state)?;
            let iid = text(p, "inviteId")?;
            let mut invite = get(&tx, "group_invites", iid).context("group_invite_not_found")?;
            ensure!(
                invite["groupId"] == gid && invite["peer"] == actor,
                "group_invite_wrong_peer"
            );
            ensure!(
                invite["expires"].as_i64().unwrap_or(0) > now(),
                "group_invite_expired"
            );
            if invite["status"] == "used" {
                member(&state, actor).context("group_invite_consumed")?;
                return Ok(json!({"snapshot":envelope}));
            }
            ensure!(invite["status"] == "pending", "group_invite_revoked");
            admin(&state, text(&invite, "issuer")?).context("group_inviter_no_longer_admin")?;
            if state["members"].get(actor).is_none() {
                ensure!(
                    state["members"].as_object().unwrap().len()
                        < state["maxMembers"]
                            .as_u64()
                            .unwrap_or(LEGACY_MEMBERS as u64) as usize,
                    "group_member_limit"
                );
            }
            let card = Self::verify(&p["card"]).context("group_invalid_card")?;
            ensure!(
                card["v"] == 1 && card["kind"] == "contact" && card["id"] == actor,
                "group_invalid_card"
            );
            ensure!(
                EndpointTicket::from_str(text(&card, "ticket")?)?
                    .endpoint_addr()
                    .id
                    .to_string()
                    == actor,
                "group_invalid_card"
            );
            if member(&state, actor).is_err() {
                let can_send = state["objectType"] != "channel";
                state["members"][actor] = json!({"name":card["name"].as_str().unwrap_or("Agent").chars().take(80).collect::<String>(),"role":"member","canSend":can_send,"active":true,"ticket":card["ticket"],"joinedEpoch":state["epoch"].as_u64().unwrap_or(0)+1});
            }
            invite["status"] = json!("used");
            put(&tx, "group_invites", iid, &invite)?;
        } else {
            member(&state, actor)?;
            if kind != "group.sync" {
                active(&state)?;
            }
            let mut changed = false;
            // Refresh only the authenticated caller's independently signed endpoint ticket.
            if !p["card"].is_null() {
                let card = Self::verify(&p["card"]).context("group_invalid_card")?;
                ensure!(
                    card["id"] == actor
                        && EndpointTicket::from_str(text(&card, "ticket")?)?
                            .endpoint_addr()
                            .id
                            .to_string()
                            == actor,
                    "group_invalid_card"
                );
                if state["members"][actor]["ticket"] != card["ticket"] {
                    state["members"][actor]["ticket"] = card["ticket"].clone();
                    changed = true;
                }
            }
            match kind {
                "group.sync" => {}
                "group.auth" => {
                    let sender = text(p, "sender")?;
                    let source = member(&state, sender)?;
                    ensure!(source["canSend"] == true, "group_sender_muted");
                }
                "group.invite" => {
                    admin(&state, actor)?;
                    let peer = text(p, "peer")?;
                    EndpointId::from_str(peer).context("group_invalid_peer")?;
                    ensure!(member(&state, peer).is_err(), "group_already_member");
                    let ttl = p["ttlSeconds"].as_i64().unwrap_or(900);
                    ensure!((1..=86400).contains(&ttl), "group_invalid_ttl");
                    let iid = uid();
                    let expires = now() + ttl * 1000;
                    let invite = json!({"groupId":gid,"peer":peer,"issuer":actor,"expires":expires,"status":"pending"});
                    put(&tx, "group_invites", &iid, &invite)?;
                    let signed=self.signed(json!({"v":1,"kind":"group_invite","id":self.id(),"groupId":gid,"inviteId":iid,"peer":peer,"expires":expires,"ownerTicket":state["members"][&self.id()]["ticket"]}))?;
                    let result = json!({"invite":B64.encode(serde_json::to_vec(&signed)?),"expires":expires,"peer":peer});
                    tx.commit()?;
                    return Ok(result);
                }
                "group.remove" | "group.role" | "group.policy" => {
                    admin(&state, actor)?;
                    let target = text(p, "peer")?;
                    let target_member = member(&state, target)?.clone();
                    ensure!(target != self.id(), "group_owner_protected");
                    if actor != self.id() {
                        ensure!(
                            target_member["role"] == "member" && kind != "group.role",
                            "group_owner_required"
                        );
                    }
                    if kind == "group.role" {
                        ensure!(actor == self.id(), "group_owner_required");
                        let role = text(p, "role")?;
                        ensure!(matches!(role, "admin" | "member"), "group_invalid_role");
                        state["members"][target]["role"] = json!(role);
                        if role == "member" {
                            tx.execute("UPDATE group_invites SET data=json_set(data,'$.status','revoked') WHERE json_extract(data,'$.groupId')=? AND json_extract(data,'$.issuer')=? AND json_extract(data,'$.status')='pending'",params![gid,target])?;
                        }
                    } else if kind == "group.policy" {
                        state["members"][target]["canSend"] =
                            json!(p["canSend"].as_bool().context("group_invalid_policy")?);
                    } else {
                        state["members"].as_object_mut().unwrap().remove(target);
                        tx.execute("UPDATE group_invites SET data=json_set(data,'$.status','revoked') WHERE json_extract(data,'$.groupId')=? AND (json_extract(data,'$.peer')=? OR (json_extract(data,'$.issuer')=? AND json_extract(data,'$.status')='pending'))",params![gid,target,target])?;
                    }
                    changed = true;
                }
                "group.leave" => {
                    ensure!(actor != self.id(), "group_owner_must_dissolve");
                    state["members"].as_object_mut().unwrap().remove(actor);
                    tx.execute("UPDATE group_invites SET data=json_set(data,'$.status','revoked') WHERE json_extract(data,'$.groupId')=? AND (json_extract(data,'$.peer')=? OR (json_extract(data,'$.issuer')=? AND json_extract(data,'$.status')='pending'))",params![gid,actor,actor])?;
                    changed = true;
                }
                "group.dissolve" => {
                    ensure!(actor == self.id(), "group_owner_required");
                    state["status"] = json!("dissolved");
                    changed = true;
                }
                "group.limit" => {
                    ensure!(actor == self.id(), "group_owner_required");
                    let limit = p["maxMembers"]
                        .as_u64()
                        .context("group_invalid_member_limit")?;
                    ensure!(
                        (2..=MAX_MEMBERS as u64).contains(&limit),
                        "group_invalid_member_limit"
                    );
                    ensure!(
                        state["members"].as_object().unwrap().len() <= limit as usize,
                        "group_limit_below_membership"
                    );
                    if state["maxMembers"] != json!(limit) {
                        state["maxMembers"] = json!(limit);
                        changed = true;
                    }
                }
                "group.replicate" => {
                    ensure!(actor == self.id(), "group_owner_required");
                    state["replication"] = json!("members-v1");
                    if state["epoch"].is_null() {
                        state["epoch"] = json!(0);
                    }
                    for m in state["members"].as_object_mut().unwrap().values_mut() {
                        if m["joinedEpoch"].is_null() {
                            m["joinedEpoch"] = json!(1);
                        }
                    }
                    changed = true;
                }
                _ => bail!("group_unknown_control"),
            }
            if !changed {
                tx.commit()?;
                return Ok(json!({"snapshot":envelope}));
            }
        }
        if state["replication"] == "members-v1" && !matches!(kind, "group.sync" | "group.auth") {
            state["epoch"] = json!(state["epoch"].as_u64().unwrap_or(0) + 1);
        }
        state["revision"] = json!(
            state["revision"]
                .as_u64()
                .context("group_invalid_revision")?
                + 1
        );
        if state["replication"] == "members-v1" {
            state["previousState"] = envelope["signature"].clone();
        }
        let snapshot = self.group_signed(&state)?;
        put(&tx, "chat_groups", gid, &snapshot)?;
        if state["replication"] == "members-v1" {
            tx.execute(
                "INSERT OR IGNORE INTO group_snapshots(reference,group_id,data) VALUES(?,?,?)",
                params![text(&snapshot, "signature")?, gid, snapshot.to_string()],
            )?;
        }
        tx.commit()?;
        Ok(json!({"snapshot":snapshot}))
    }
    async fn group_control(&self, gid: &str, kind: &str, mut fields: Value) -> Result<Value> {
        let state = self.group_load(gid)?;
        let owner = text(&state, "id")?;
        fields["kind"] = json!(kind);
        fields["groupId"] = json!(gid);
        fields["card"] = self.group_card().await?;
        let response = if owner == self.id() {
            self.group_authority(&self.id(), &fields)?
        } else {
            self.allowed(owner, "chat")?;
            self.rpc(text(&state["members"][owner], "ticket")?, fields, owner)
                .await?
        };
        if !response["snapshot"].is_null() {
            self.group_cache(&response["snapshot"], owner, gid)?;
        }
        Ok(response)
    }
    async fn group_sync(&self, gid: &str) -> Result<Value> {
        self.group_control(gid, "group.sync", json!({})).await?;
        self.group_load(gid)
    }
    async fn group_authorize(&self, gid: &str, sender: &str) -> Result<Value> {
        let cached = self.group_load(gid)?;
        if cached["replication"] == "members-v1" {
            active(&cached)?;
            member(&cached, &self.id())?;
            ensure!(
                member(&cached, sender)?["canSend"] == true,
                "group_sender_muted"
            );
            return Ok(cached);
        }
        self.group_control(gid, "group.auth", json!({"sender":sender}))
            .await?;
        let state = self.group_load(gid)?;
        active(&state)?;
        member(&state, &self.id())?;
        ensure!(
            member(&state, sender)?["canSend"] == true,
            "group_sender_muted"
        );
        Ok(state)
    }
    fn group_insert(
        &self,
        id: &str,
        gid: &str,
        sender: &str,
        body: &str,
        created: i64,
        recipients: Option<Vec<String>>,
    ) -> Result<()> {
        let mut db = self.0.db.lock().unwrap();
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let old = tx
            .query_row(
                "SELECT group_id,sender,text FROM group_messages WHERE id=?",
                [id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        if let Some(old) = old {
            ensure!(
                old == (gid.into(), sender.into(), body.into()),
                "group_message_id_conflict"
            );
        } else {
            crate::governance::storage_check(
                &tx,
                body.len() + recipients.as_ref().map_or(0, |r| r.len() * 256),
            )?;
            tx.execute(
                "INSERT INTO group_messages(id,group_id,sender,text,created) VALUES(?,?,?,?,?)",
                params![id, gid, sender, body, created],
            )?;
            if let Some(destinations) = recipients {
                for recipient in destinations {
                    tx.execute("INSERT INTO group_outbox(message_id,recipient,status) VALUES(?,?,'pending')",params![id,recipient])?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }
    fn group_message_rows(&self, gid: Option<&str>, after: i64) -> Result<Value> {
        let db = self.0.db.lock().unwrap();
        let mut q=db.prepare("SELECT sequence,id,group_id,sender,text,created FROM group_messages WHERE sequence>? AND (? IS NULL OR group_id=?) ORDER BY sequence LIMIT 33")?;
        let mut rows=q.query_map(params![after,gid,gid],|r|Ok(json!({"sequence":r.get::<_,i64>(0)?,"id":r.get::<_,String>(1)?,"groupId":r.get::<_,String>(2)?,"sender":r.get::<_,String>(3)?,"text":r.get::<_,String>(4)?,"created":r.get::<_,i64>(5)?,"kind":"group_text","executionAllowed":false})))?.collect::<std::result::Result<Vec<_>,_>>()?;
        drop(q);
        drop(db);
        for row in &mut rows {
            let record: Option<String> = self
                .0
                .db
                .lock()
                .unwrap()
                .query_row(
                    "SELECT data FROM group_records WHERE id=?",
                    [text(row, "id")?],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(raw) = record {
                let envelope: Value = serde_json::from_str(&raw)?;
                let body = Self::verify(&envelope)?;
                let state = self.group_load(text(row, "groupId")?)?;
                row["signatureVerified"] = json!(true);
                row["authorSequence"] = body["authorSequence"].clone();
                row["previous"] = body["previous"].clone();
                row["epoch"] = body["epoch"].clone();
                row["recordRef"] = envelope["signature"].clone();
                row["authorization"] = json!(if state["epoch"] == body["epoch"]
                    && state["status"] == "active"
                {
                    "current_epoch"
                } else {
                    "historical_unfinalized"
                });
            } else {
                row["signatureVerified"] = json!(false);
                row["authorization"] = json!("legacy_unsigned");
            }
        }
        Self::page(rows, after)
    }
    async fn group_deliver(&self, id: &str, recipient: &str, retry: bool) -> Result<()> {
        let key = (id.to_owned(), recipient.to_owned());
        if !self.0.group_inflight.lock().unwrap().insert(key.clone()) {
            return Ok(());
        }
        let _claim = DeliveryClaim {
            inner: self.0.clone(),
            key,
        };
        // Offline retries may use at most one slot, leaving capacity for fresh traffic.
        let _retry = if retry {
            Some(self.0.group_retry_slots.clone().acquire_owned().await?)
        } else {
            None
        };
        let _slot = self.0.group_delivery_slots.clone().acquire_owned().await?;
        let pending = self.0.db.lock().unwrap().query_row(
            "SELECT status FROM group_outbox WHERE message_id=? AND recipient=?",
            params![id, recipient],
            |r| r.get::<_, String>(0),
        )?;
        if pending != "pending" {
            return Ok(());
        }
        self.0.db.lock().unwrap().execute("UPDATE group_outbox SET attempts=attempts+1,last_attempt=? WHERE message_id=? AND recipient=?",params![now(),id,recipient])?;
        let (gid, sender, body, created) = {
            let db = self.0.db.lock().unwrap();
            db.query_row(
                "SELECT group_id,sender,text,created FROM group_messages WHERE id=?",
                [id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                    ))
                },
            )?
        };
        let result:Result<()>=async{
            let state=self.group_authorize(&gid,&sender).await?;member(&state,recipient)?;self.allowed(recipient,"chat")?;
            if state["replication"]=="members-v1" { return self.replica_deliver(id,recipient).await; }
            let response=self.rpc(text(&state["members"][recipient],"ticket")?,json!({"kind":"group.message","groupId":gid,"id":id,"text":body,"created":created}),recipient).await?;
            ensure!(response["ack"]==id&&response["persisted"]==true,"invalid_ack");Ok(())
        }.await;
        let (status, error) = match &result {
            Ok(()) => ("acked", None),
            Err(e) if permanent(e) => ("rejected", Some(e.to_string())),
            Err(e) => ("pending", Some(e.to_string())),
        };
        self.0.db.lock().unwrap().execute(
            "UPDATE group_outbox SET status=?,error=?,retry_after=? + 1000 * MIN(30,2 * (1 << MIN(attempts,4))) WHERE message_id=? AND recipient=?",
            params![status, error, now(), id, recipient],
        )?;
        result
    }
    pub(super) async fn group_retry(&self, force: bool) -> Result<()> {
        let jobs = {
            let db = self.0.db.lock().unwrap();
            let mut q=db.prepare("SELECT message_id,recipient FROM group_outbox WHERE status='pending' AND (? OR retry_after<=?) ORDER BY last_attempt,rowid LIMIT 256")?;
            q.query_map(params![force, now()], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
        };
        let jobs: Vec<_> = {
            let inflight = self.0.group_inflight.lock().unwrap();
            jobs.into_iter()
                .filter(|key| !inflight.contains(key))
                .take(8)
                .collect()
        };
        let mut batch = tokio::task::JoinSet::new();
        for (id, recipient) in jobs {
            let core = self.clone();
            batch.spawn(async move {
                let _ = core.group_deliver(&id, &recipient, true).await;
            });
        }
        while batch.join_next().await.is_some() {}
        Ok(())
    }
    pub(super) async fn group_receive(&self, remote: &str, p: Value) -> Result<Value> {
        self.allowed(remote, "chat")?;
        if p["kind"]
            .as_str()
            .is_some_and(|k| k.starts_with("group.replica."))
        {
            return self.replica_receive(remote, p).await;
        }
        if p["kind"] != "group.message" {
            return self.group_authority(remote, &p);
        }
        let gid = text(&p, "groupId")?;
        ensure!(
            self.group_load(gid)?["replication"] != "members-v1",
            "group_signed_record_required"
        );
        let id = text(&p, "id")?;
        ensure!(
            uuid::Uuid::parse_str(id).is_ok(),
            "group_invalid_message_id"
        );
        let body = text(&p, "text")?;
        ensure!(
            !body.trim().is_empty() && body.len() <= 8000,
            "group_invalid_message"
        );
        let created = p["created"].as_i64().context("group_invalid_timestamp")?;
        ensure!(
            created >= 0 && created <= now() + 30000,
            "group_invalid_timestamp"
        );
        // A fresh owner check makes removed/muted peers fail closed, even with stale snapshots.
        self.group_authorize(gid, remote).await?;
        self.group_insert(id, gid, remote, body, created, None)?;
        Ok(json!({"ack":id,"persisted":true}))
    }
    pub(super) async fn group_action(&self, r: Value) -> Result<Value> {
        let action = text(&r, "action")?;
        match action {
            "groups.create" => {
                let name = text(&r, "name")?;
                let max_members = r["maxMembers"].as_u64().unwrap_or(DEFAULT_MEMBERS as u64);
                ensure!(
                    (2..=MAX_MEMBERS as u64).contains(&max_members),
                    "group_invalid_member_limit"
                );
                ensure!(
                    !name.trim().is_empty() && name.len() <= 160,
                    "group_invalid_name"
                );
                let gid = uid();
                let id = self.id();
                let card = Self::verify(&self.group_card().await?)?;
                let mut state = json!({"v":1,"kind":"group_state","id":id,"groupId":gid,"name":name,"revision":1,"maxMembers":max_members,"status":"active","members":{id.clone():{"name":card["name"],"role":"owner","canSend":true,"active":true,"ticket":card["ticket"],"joinedEpoch":1}},"created":now()});
                if r["channel"] == true {
                    state["objectType"] = json!("channel");
                }
                if r["ownerOnline"] != true {
                    state["replication"] = json!("members-v1");
                    state["epoch"] = json!(1);
                }
                self.group_cache(&self.group_signed(&state)?, &id, &gid)?;
                Ok(json!({"group":state,"fresh":true}))
            }
            "groups.list" => {
                let offset = r["offset"].as_u64().unwrap_or(0) as usize;
                let all = self.list("chat_groups")?;
                let total = all.len();
                let mut groups = vec![];
                let mut bytes = 0;
                for e in all.into_iter().skip(offset).take(16) {
                    let s = group_verify(&e)?;
                    let n = serde_json::to_vec(&s)?.len();
                    if !groups.is_empty() && bytes + n > 350000 {
                        break;
                    }
                    bytes += n;
                    groups.push(s);
                }
                Ok(
                    json!({"nextOffset":offset+groups.len(),"hasMore":offset+groups.len()<total,"groups":groups,"fresh":false}),
                )
            }
            "groups.join" => {
                let raw = text(&r, "invite")?;
                ensure!(raw.len() < 16000, "group_invalid_invite");
                let e: Value =
                    serde_json::from_slice(&B64.decode(raw)?).context("group_invalid_invite")?;
                let invite = Self::verify(&e).context("group_invalid_invite")?;
                ensure!(
                    invite["v"] == 1
                        && invite["kind"] == "group_invite"
                        && invite["peer"] == self.id(),
                    "group_invite_wrong_peer"
                );
                ensure!(
                    invite["expires"].as_i64().unwrap_or(0) > now(),
                    "group_invite_expired"
                );
                let owner = text(&invite, "id")?;
                self.allowed(owner, "chat")?;
                let response=self.rpc(text(&invite,"ownerTicket")?,json!({"kind":"group.join","groupId":invite["groupId"],"inviteId":invite["inviteId"],"card":self.group_card().await?}),owner).await?;
                let state =
                    self.group_cache(&response["snapshot"], owner, text(&invite, "groupId")?)?;
                if state["replication"] == "members-v1" {
                    self.replica_announce(text(&invite, "groupId")?).await;
                }
                Ok(json!({"group":state,"fresh":true}))
            }
            "groups.messages" => self
                .group_message_rows(Some(text(&r, "groupId")?), r["after"].as_i64().unwrap_or(0)),
            "groups.inbox" => self.group_message_rows(None, r["after"].as_i64().unwrap_or(0)),
            "groups.show" | "groups.sync" => {
                let gid = text(&r, "groupId")?;
                if self.group_load(gid)?["replication"] == "members-v1" {
                    let peers = self.replica_sync(gid).await?;
                    return Ok(
                        json!({"group":self.group_load(gid)?,"fresh":false,"replicated":true,"peers":peers,"authorityOnlineRequired":false}),
                    );
                }
                match self.group_sync(gid).await {
                    Ok(state) => Ok(json!({"group":state,"fresh":true})),
                    Err(e) if action == "groups.show" => Ok(
                        json!({"group":self.group_load(gid)?,"fresh":false,"syncError":e.to_string()}),
                    ),
                    Err(e) => Err(e),
                }
            }
            "groups.send" => {
                let gid = text(&r, "groupId")?;
                let body = text(&r, "text")?;
                ensure!(
                    !body.trim().is_empty() && body.len() <= 8000,
                    "group_invalid_message"
                );
                if self.group_load(gid)?["replication"] == "members-v1" {
                    return self.replica_send(&r).await;
                }
                let state = self.group_authorize(gid, &self.id()).await?;
                let id = r["id"].as_str().map(str::to_owned).unwrap_or_else(uid);
                ensure!(
                    uuid::Uuid::parse_str(&id).is_ok(),
                    "group_invalid_message_id"
                );
                let recipients = state["members"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .filter(|(peer, m)| **peer != self.id() && m["active"] == true)
                    .map(|(peer, _)| peer.clone())
                    .collect();
                self.group_insert(&id, gid, &self.id(), body, now(), Some(recipients))?;
                let pending = {
                    let db = self.0.db.lock().unwrap();
                    let mut q=db.prepare("SELECT recipient FROM group_outbox WHERE message_id=? AND status='pending'")?;
                    q.query_map([&id], |r| r.get::<_, String>(0))?
                        .collect::<std::result::Result<Vec<_>, _>>()?
                };
                let mut batch = tokio::task::JoinSet::new();
                for peer in pending {
                    let core = self.clone();
                    let mid = id.clone();
                    batch.spawn(async move {
                        let _ = core.group_deliver(&mid, &peer, false).await;
                    });
                }
                while batch.join_next().await.is_some() {}
                let deliveries = {
                    let db = self.0.db.lock().unwrap();
                    let mut q=db.prepare("SELECT recipient,status,error FROM group_outbox WHERE message_id=? ORDER BY recipient")?;
                    q.query_map([&id],|r|Ok(json!({"peer":r.get::<_,String>(0)?,"status":r.get::<_,String>(1)?,"error":r.get::<_,Option<String>>(2)?})))?.collect::<std::result::Result<Vec<_>,_>>()?
                };
                Ok(json!({"id":id,"groupId":gid,"deliveries":deliveries,"executed":false}))
            }
            "groups.invite" | "groups.remove" | "groups.role" | "groups.policy"
            | "groups.leave" | "groups.dissolve" | "groups.replicate" | "groups.limit" => {
                let gid = text(&r, "groupId")?;
                let kind = format!("group.{}", action.strip_prefix("groups.").unwrap());
                let previous = self.group_load(gid)?;
                let response = self.group_control(gid, &kind, r.clone()).await?;
                if self.group_load(gid)?["replication"] == "members-v1" {
                    self.replica_announce_members(gid, &previous).await;
                }
                if action == "groups.invite" {
                    Ok(response)
                } else {
                    Ok(json!({"group":self.group_load(gid)?,"fresh":true}))
                }
            }
            _ => bail!("group_unknown_action"),
        }
    }
}
