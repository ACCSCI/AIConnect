//! Signed member replicas. Signatures prove authorship, not consensus or wall-clock time.
use super::*;

impl Core {
    fn replica_budget(&self) -> Duration {
        Duration::from_secs(if self.0.direct_only { 2 } else { 20 })
    }
    fn replica_access(&self, gid: &str, peer: &str) -> Result<Value> {
        let state = self.group_load(gid)?;
        ensure!(
            state["replication"] == "members-v1",
            "group_replication_required"
        );
        active(&state)?;
        member(&state, &self.id())?;
        member(&state, peer)?;
        self.allowed(peer, "chat")?;
        Ok(state)
    }
    fn replica_snapshot(&self, gid: &str, reference: &str) -> Result<Value> {
        let db = self.0.db.lock().unwrap();
        let raw: String = db
            .query_row(
                "SELECT data FROM group_snapshots WHERE group_id=? AND reference=?",
                params![gid, reference],
                |r| r.get(0),
            )
            .context("group_missing_snapshot")?;
        Ok(serde_json::from_str(&raw)?)
    }
    fn replica_card(&self, gid: &str, peer: &str, card: &Value) -> Result<()> {
        let body = Self::verify(card)?;
        ensure!(
            body["kind"] == "contact" && body["id"] == peer,
            "group_invalid_card"
        );
        ensure!(
            EndpointTicket::from_str(text(&body, "ticket")?)?
                .endpoint_addr()
                .id
                .to_string()
                == peer,
            "group_invalid_card"
        );
        self.0.db.lock().unwrap().execute("INSERT INTO group_peer_cards(group_id,peer,data) VALUES(?,?,?) ON CONFLICT(group_id,peer) DO UPDATE SET data=excluded.data",params![gid,peer,card.to_string()])?;
        Ok(())
    }
    fn replica_ticket(&self, state: &Value, peer: &str) -> Result<String> {
        let gid = text(state, "groupId")?;
        let raw: Option<String> = self
            .0
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT data FROM group_peer_cards WHERE group_id=? AND peer=?",
                params![gid, peer],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(raw) = raw {
            let card = Self::verify(&serde_json::from_str::<Value>(&raw)?)?;
            return Ok(text(&card, "ticket")?.into());
        }
        Ok(text(member(state, peer)?, "ticket")?.into())
    }
    fn replica_current_snapshot(&self, gid: &str) -> Result<Value> {
        self.load("chat_groups", gid)
    }

    /// A signature commits to the record bytes and serves as its content reference.
    /// Old-epoch history is archived as such, never treated as current authorization.
    fn replica_ingest(&self, gid: &str, record: &Value, historical: bool) -> Result<()> {
        let body = Self::verify(record).context("group_invalid_record_signature")?;
        ensure!(
            body["v"] == 1 && body["kind"] == "group_record" && body["groupId"] == gid,
            "group_invalid_record"
        );
        let sender = text(&body, "id")?;
        self.allowed(sender, "chat")?;
        let mid = text(&body, "messageId")?;
        ensure!(
            uuid::Uuid::parse_str(mid).is_ok(),
            "group_invalid_message_id"
        );
        let content = text(&body, "text")?;
        ensure!(
            !content.trim().is_empty() && content.len() <= 8000,
            "group_invalid_message"
        );
        let created = body["created"]
            .as_i64()
            .context("group_invalid_timestamp")?;
        ensure!(
            created >= 0 && created <= now() + 30000,
            "group_invalid_timestamp"
        );
        let seq = body["authorSequence"]
            .as_i64()
            .filter(|s| *s > 0 && *s <= 1_000_000_000_000)
            .context("group_invalid_author_sequence")?;
        let previous = body["previous"]
            .as_str()
            .context("group_invalid_previous")?;
        ensure!(
            (seq == 1 && previous.is_empty())
                || (seq > 1
                    && previous.len() == 86
                    && B64.decode(previous).is_ok_and(|v| v.len() == 64)),
            "group_invalid_previous"
        );
        let epoch = body["epoch"].as_u64().context("group_invalid_epoch")?;
        let sr = text(&body, "stateRef")?;
        let snapshot = self.replica_snapshot(gid, sr)?;
        let current = self.group_load(gid)?;
        active(&current)?;
        let myself = member(&current, &self.id())?;
        ensure!(
            epoch >= myself["joinedEpoch"].as_u64().unwrap_or(u64::MAX),
            "group_history_before_join"
        );
        let authority = validate(&snapshot, text(&current, "id")?, gid)?;
        active(&authority)?;
        ensure!(
            authority["replication"] == "members-v1" && authority["epoch"] == epoch,
            "group_invalid_record_epoch"
        );
        ensure!(
            member(&authority, sender)?["canSend"] == true,
            "group_sender_muted"
        );
        member(&authority, &self.id())?;
        if !historical {
            ensure!(current["epoch"] == epoch, "group_stale_epoch");
            ensure!(
                member(&current, sender)?["canSend"] == true,
                "group_sender_muted"
            );
        }
        let reference = text(record, "signature")?;
        let mut db = self.0.db.lock().unwrap();
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let old: Option<String> = tx
            .query_row("SELECT data FROM group_records WHERE id=?", [mid], |r| {
                r.get(0)
            })
            .optional()?;
        if let Some(old) = old {
            ensure!(
                serde_json::from_str::<Value>(&old)? == *record,
                "group_message_id_conflict"
            );
            return Ok(());
        }
        let fork:Option<String>=tx.query_row("SELECT reference FROM group_records WHERE group_id=? AND sender=? AND author_sequence=?",params![gid,sender,seq],|r|r.get(0)).optional()?;
        ensure!(fork.is_none(), "group_author_fork");
        crate::governance::storage_check(&tx, record.to_string().len())?;
        if seq > 1 {
            let parent:Option<(String,i64)>=tx.query_row("SELECT sender,author_sequence FROM group_records WHERE group_id=? AND reference=?",params![gid,previous],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            if let Some((who, n)) = parent {
                ensure!(who == sender && n == seq - 1, "group_invalid_previous");
            }
        }
        let mut children = tx.prepare(
            "SELECT data FROM group_records WHERE group_id=? AND sender=? AND author_sequence=?",
        )?;
        let child = children
            .query_map(params![gid, sender, seq + 1], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(children);
        for child in child {
            ensure!(
                Self::verify(&serde_json::from_str::<Value>(&child)?)?["previous"] == reference,
                "group_author_fork"
            );
        }
        // Legacy rows remain local; another author cannot retroactively sign them.
        let existing: Option<(String, String, String)> = tx
            .query_row(
                "SELECT group_id,sender,text FROM group_messages WHERE id=?",
                [mid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        if let Some(old) = existing {
            ensure!(
                old == (gid.into(), sender.into(), content.into()),
                "group_message_id_conflict"
            );
        } else {
            tx.execute(
                "INSERT INTO group_messages(id,group_id,sender,text,created) VALUES(?,?,?,?,?)",
                params![mid, gid, sender, content, created],
            )?;
        }
        tx.execute("INSERT INTO group_records(id,group_id,sender,author_sequence,reference,state_ref,epoch,data) VALUES(?,?,?,?,?,?,?,?)",params![mid,gid,sender,seq,reference,sr,epoch,record.to_string()])?;
        tx.commit()?;
        Ok(())
    }
    pub(super) async fn replica_receive(&self, remote: &str, p: Value) -> Result<Value> {
        let gid = text(&p, "groupId")?;
        let kind = text(&p, "kind")?;
        let pinned = self.group_load(gid)?;
        // Only already-known groups may receive snapshots; the pinned owner cannot change.
        if kind == "group.replica.hello" {
            let incoming = validate(&p["snapshot"], text(&pinned, "id")?, gid)?;
            ensure!(
                incoming["replication"] == "members-v1",
                "group_replication_required"
            );
            // Removed members may learn their removal, but cannot inject snapshots themselves.
            member(&incoming, remote)?;
            self.group_cache(&p["snapshot"], text(&pinned, "id")?, gid)?;
            let state = self.group_load(gid)?;
            if member(&state, &self.id()).is_err() || state["status"] != "active" {
                return Ok(
                    json!({"snapshot":self.replica_current_snapshot(gid)?,"card":self.group_card().await?,"inactive":true}),
                );
            }
            self.replica_access(gid, remote)?;
            self.replica_card(gid, remote, &p["card"])?;
            return Ok(
                json!({"snapshot":self.replica_current_snapshot(gid)?,"card":self.group_card().await?}),
            );
        }
        let state = self.replica_access(gid, remote)?;
        match kind {
            "group.replica.proof" => {
                let proof = validate(&p["snapshot"], text(&state, "id")?, gid)?;
                ensure!(
                    proof["replication"] == "members-v1"
                        && proof["epoch"].as_u64().unwrap_or(0)
                            >= member(&state, &self.id())?["joinedEpoch"]
                                .as_u64()
                                .unwrap_or(u64::MAX),
                    "group_history_before_join"
                );
                self.group_cache(&p["snapshot"], text(&state, "id")?, gid)?;
                Ok(json!({"persisted":true}))
            }
            "group.replica.snapshot" => {
                let snapshot = self.replica_snapshot(gid, text(&p, "reference")?)?;
                let old = group_verify(&snapshot)?;
                ensure!(
                    old["epoch"].as_u64().unwrap_or(0)
                        >= member(&state, remote)?["joinedEpoch"]
                            .as_u64()
                            .unwrap_or(u64::MAX),
                    "group_history_before_join"
                );
                Ok(json!({"snapshot":snapshot}))
            }
            "group.replica.pull" => {
                let after = p["after"]
                    .as_i64()
                    .filter(|a| *a >= 0)
                    .context("group_invalid_cursor")?;
                let floor = member(&state, remote)?["joinedEpoch"]
                    .as_u64()
                    .unwrap_or(u64::MAX);
                let db = self.0.db.lock().unwrap();
                let mut q=db.prepare("SELECT sequence,data FROM group_records WHERE group_id=? AND sequence>? AND epoch>=? ORDER BY sequence LIMIT 9")?;
                let rows = q
                    .query_map(params![gid, after, floor], |r| {
                        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                let mut entries = vec![];
                let mut bytes = 0;
                let mut next = after;
                for (seq, raw) in &rows {
                    if entries.len() == 8 || bytes + raw.len() > 48000 {
                        break;
                    }
                    bytes += raw.len();
                    next = *seq;
                    entries.push(serde_json::from_str::<Value>(raw)?);
                }
                Ok(
                    json!({"records":entries,"nextAfter":next,"hasMore":rows.last().is_some_and(|(s,_)|*s>next)}),
                )
            }
            "group.replica.message" => {
                self.replica_ingest(gid, &p["record"], false)?;
                Ok(json!({"ack":Self::verify(&p["record"])?["messageId"],"persisted":true}))
            }
            _ => bail!("group_unknown_replica_action"),
        }
    }
    async fn replica_hello(&self, gid: &str, peer: &str) -> Result<String> {
        let state = self.replica_access(gid, peer)?;
        let ticket = self.replica_ticket(&state, peer)?;
        let response=self.rpc(&ticket,json!({"kind":"group.replica.hello","groupId":gid,"snapshot":self.replica_current_snapshot(gid)?,"card":self.group_card().await?}),peer).await?;
        self.group_cache(&response["snapshot"], text(&state, "id")?, gid)?;
        self.replica_access(gid, peer)?;
        self.replica_card(gid, peer, &response["card"])?;
        self.replica_ticket(&self.group_load(gid)?, peer)
    }
    pub(super) async fn replica_deliver(&self, mid: &str, peer: &str) -> Result<()> {
        let raw: String = self
            .0
            .db
            .lock()
            .unwrap()
            .query_row("SELECT data FROM group_records WHERE id=?", [mid], |r| {
                r.get(0)
            })
            .context("group_signed_record_required")?;
        let record: Value = serde_json::from_str(&raw)?;
        let body = Self::verify(&record)?;
        let gid = text(&body, "groupId")?;
        let ticket = self.replica_hello(gid, peer).await?;
        let state = self.replica_access(gid, peer)?;
        ensure!(state["epoch"] == body["epoch"], "group_stale_epoch");
        self.rpc(&ticket,json!({"kind":"group.replica.proof","groupId":gid,"snapshot":self.replica_snapshot(gid,text(&body,"stateRef")?)?}),peer).await?;
        let response = self
            .rpc(
                &ticket,
                json!({"kind":"group.replica.message","groupId":gid,"record":record}),
                peer,
            )
            .await?;
        ensure!(
            response["ack"] == mid && response["persisted"] == true,
            "invalid_ack"
        );
        Ok(())
    }
    pub(super) async fn replica_announce(&self, gid: &str) {
        let Ok(state) = self.group_load(gid) else {
            return;
        };
        self.replica_announce_members(gid, &state).await;
    }
    pub(super) async fn replica_announce_members(&self, gid: &str, state: &Value) {
        let mut jobs = tokio::task::JoinSet::new();
        for peer in state["members"]
            .as_object()
            .unwrap()
            .keys()
            .filter(|p| **p != self.id())
        {
            let core = self.clone();
            let gid = gid.to_string();
            let peer = peer.clone();
            let Ok(ticket) = self.replica_ticket(state, &peer) else {
                continue;
            };
            jobs.spawn(async move {
                let _slot = core.0.group_delivery_slots.clone().acquire_owned().await;
                let packet=json!({"kind":"group.replica.hello","groupId":gid,"snapshot":core.replica_current_snapshot(&gid).ok(),"card":core.group_card().await.ok()});
                let _ = timeout(core.replica_budget(), core.rpc(&ticket,packet,&peer)).await;
            });
        }
        if timeout(Duration::from_secs(8), async {
            while jobs.join_next().await.is_some() {}
        })
        .await
        .is_err()
        {
            jobs.abort_all();
        }
    }
    pub(super) async fn replica_send(&self, r: &Value) -> Result<Value> {
        let gid = text(r, "groupId")?;
        let state = self.group_authorize(gid, &self.id()).await?;
        let mid = r["id"].as_str().map(str::to_owned).unwrap_or_else(uid);
        ensure!(
            uuid::Uuid::parse_str(&mid).is_ok(),
            "group_invalid_message_id"
        );
        let content = text(r, "text")?;
        // Serialize local sequence allocation, signing, ingestion and durable fanout creation.
        {
            let _guard = self.0.receive_guard.lock().unwrap();
            let old: Option<String> = self
                .0
                .db
                .lock()
                .unwrap()
                .query_row("SELECT data FROM group_records WHERE id=?", [&mid], |r| {
                    r.get(0)
                })
                .optional()?;
            if let Some(raw) = old {
                let b = Self::verify(&serde_json::from_str::<Value>(&raw)?)?;
                ensure!(
                    b["id"] == self.id() && b["groupId"] == gid && b["text"] == content,
                    "group_message_id_conflict"
                );
            } else {
                let last:Option<(i64,String)>=self.0.db.lock().unwrap().query_row("SELECT author_sequence,reference FROM group_records WHERE group_id=? AND sender=? ORDER BY author_sequence DESC LIMIT 1",params![gid,self.id()],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
                let (sequence, previous) =
                    last.map(|(n, s)| (n + 1, s)).unwrap_or((1, String::new()));
                let snapshot = self.replica_current_snapshot(gid)?;
                let record=self.signed(json!({"v":1,"kind":"group_record","id":self.id(),"messageId":mid,"groupId":gid,"text":content,"created":now(),"authorSequence":sequence,"previous":previous,"epoch":state["epoch"],"stateRef":snapshot["signature"]}))?;
                self.replica_ingest(gid, &record, false)?;
            }
            let db = self.0.db.lock().unwrap();
            for peer in state["members"]
                .as_object()
                .unwrap()
                .keys()
                .filter(|p| **p != self.id())
            {
                db.execute("INSERT OR IGNORE INTO group_outbox(message_id,recipient,status) VALUES(?,?,'pending')",params![mid,peer])?;
            }
        }
        let mut jobs = tokio::task::JoinSet::new();
        for peer in state["members"]
            .as_object()
            .unwrap()
            .keys()
            .filter(|p| **p != self.id())
        {
            let core = self.clone();
            let peer = peer.clone();
            let id = mid.clone();
            jobs.spawn(async move {
                let _ = core.group_deliver(&id, &peer, false).await;
            });
        }
        if timeout(Duration::from_secs(2), async {
            while jobs.join_next().await.is_some() {}
        })
        .await
        .is_err()
        {
            jobs.detach_all();
        }
        let db = self.0.db.lock().unwrap();
        let mut q = db.prepare(
            "SELECT recipient,status,error FROM group_outbox WHERE message_id=? ORDER BY recipient",
        )?;
        let deliveries=q.query_map([&mid],|r|Ok(json!({"peer":r.get::<_,String>(0)?,"status":r.get::<_,String>(1)?,"error":r.get::<_,Option<String>>(2)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
        Ok(
            json!({"id":mid,"groupId":gid,"deliveries":deliveries,"replicated":true,"executed":false}),
        )
    }
    async fn replica_pull_peer(&self, gid: &str, peer: &str) -> Result<Value> {
        let ticket = self.replica_hello(gid, peer).await?;
        let after: i64 = self
            .0
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT cursor FROM group_replica_cursors WHERE group_id=? AND peer=?",
                params![gid, peer],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        let response = self
            .rpc(
                &ticket,
                json!({"kind":"group.replica.pull","groupId":gid,"after":after}),
                peer,
            )
            .await?;
        let records = response["records"]
            .as_array()
            .filter(|a| a.len() <= 8)
            .context("group_invalid_pull")?;
        let mut imported = 0;
        for record in records {
            let b = Self::verify(record)?;
            let sr = text(&b, "stateRef")?;
            if self.replica_snapshot(gid, sr).is_err() {
                let reply = self
                    .rpc(
                        &ticket,
                        json!({"kind":"group.replica.snapshot","groupId":gid,"reference":sr}),
                        peer,
                    )
                    .await?;
                self.group_cache(&reply["snapshot"], text(&self.group_load(gid)?, "id")?, gid)?;
                // group_cache retains monotonic current state; retain older proofs separately.
                let proof = validate(&reply["snapshot"], text(&self.group_load(gid)?, "id")?, gid)?;
                ensure!(
                    reply["snapshot"]["signature"] == sr && proof["replication"] == "members-v1",
                    "group_invalid_snapshot_reference"
                );
                self.0.db.lock().unwrap().execute(
                    "INSERT OR IGNORE INTO group_snapshots(reference,group_id,data) VALUES(?,?,?)",
                    params![sr, gid, reply["snapshot"].to_string()],
                )?;
            }
            self.replica_ingest(gid, record, true)?;
            imported += 1;
        }
        let next = response["nextAfter"]
            .as_i64()
            .filter(|n| *n >= after)
            .context("group_invalid_cursor")?;
        ensure!(next == after || !records.is_empty(), "group_invalid_cursor");
        self.0.db.lock().unwrap().execute("INSERT INTO group_replica_cursors(group_id,peer,cursor) VALUES(?,?,?) ON CONFLICT(group_id,peer) DO UPDATE SET cursor=MAX(cursor,excluded.cursor)",params![gid,peer,next])?;
        Ok(json!({"peer":peer,"imported":imported,"hasMore":response["hasMore"],"nextAfter":next}))
    }
    pub(super) async fn replica_sync(&self, gid: &str) -> Result<Vec<Value>> {
        let state = self.replica_access(gid, &self.id())?;
        let mut jobs = tokio::task::JoinSet::new();
        for peer in state["members"]
            .as_object()
            .unwrap()
            .keys()
            .filter(|p| **p != self.id())
        {
            let core = self.clone();
            let gid = gid.to_string();
            let peer = peer.clone();
            jobs.spawn(async move {
                let _slot = core.0.group_delivery_slots.clone().acquire_owned().await;
                match timeout(core.replica_budget(), core.replica_pull_peer(&gid, &peer)).await {
                    Ok(Ok(v)) => v,
                    Ok(Err(e)) => json!({"peer":peer,"error":e.to_string()}),
                    Err(_) => json!({"peer":peer,"error":"peer_unreachable"}),
                }
            });
        }
        let mut peers = vec![];
        if timeout(Duration::from_secs(24), async {
            while let Some(Ok(v)) = jobs.join_next().await {
                peers.push(v);
            }
        })
        .await
        .is_err()
        {
            jobs.abort_all();
            peers.push(json!({"error":"sync_budget_exhausted","hasMore":true}));
        }
        Ok(peers)
    }
    pub(crate) async fn group_replication_tick(&self) -> Result<()> {
        // Poll one peer per group each tick, round robin persisted with the log.
        for e in self.list("chat_groups")? {
            let state = group_verify(&e)?;
            if state["replication"] != "members-v1"
                || state["status"] != "active"
                || member(&state, &self.id()).is_err()
            {
                continue;
            }
            let gid = text(&state, "groupId")?;
            let peers: Vec<_> = state["members"]
                .as_object()
                .unwrap()
                .keys()
                .filter(|p| **p != self.id())
                .cloned()
                .collect();
            if peers.is_empty() {
                continue;
            }
            let key = format!("replica_tick:{gid}");
            let n = self
                .load("settings", &key)
                .ok()
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            self.put("settings", &key, &json!(n.wrapping_add(1)))?;
            let _slot = self.0.group_retry_slots.clone().acquire_owned().await?;
            let _ = timeout(
                self.replica_budget(),
                self.replica_pull_peer(gid, &peers[n as usize % peers.len()]),
            )
            .await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    async fn peer() -> (TempDir, Core) {
        let d = TempDir::new().unwrap();
        let c = Core::open(Config {
            directory: d.path().into(),
            direct_only: true,
            relay_only: false,
            relay_url: None,
        })
        .await
        .unwrap();
        (d, c)
    }
    async fn join(a: &Core, b: &Core, gid: &str) {
        let i = a
            .dispatch(json!({"action":"groups.invite","groupId":gid,"peer":b.id()}))
            .await
            .unwrap();
        b.dispatch(json!({"action":"groups.join","invite":i["invite"]}))
            .await
            .unwrap();
    }
    #[tokio::test]
    async fn bounded_paged_pull_converges_large_records_and_keeps_author_chain() {
        let (_da, a) = peer().await;
        let (_db, b) = peer().await;
        let gid = a
            .dispatch(json!({"action":"groups.create","name":"Pages"}))
            .await
            .unwrap()["group"]["groupId"]
            .as_str()
            .unwrap()
            .to_owned();
        join(&a, &b, &gid).await;
        let state = a.group_load(&gid).unwrap();
        let proof = a.replica_current_snapshot(&gid).unwrap();
        let mut previous = String::new();
        for n in 1..=12 {
            let record=a.signed(json!({"v":1,"kind":"group_record","id":a.id(),"messageId":uid(),"groupId":gid,"text":"x".repeat(8000),"created":now(),"authorSequence":n,"previous":previous,"epoch":state["epoch"],"stateRef":proof["signature"]})).unwrap();
            a.replica_ingest(&gid, &record, false).unwrap();
            previous = record["signature"].as_str().unwrap().to_string();
        }
        let page = a
            .replica_receive(
                &b.id(),
                json!({"kind":"group.replica.pull","groupId":gid,"after":0}),
            )
            .await
            .unwrap();
        assert!(serde_json::to_vec(&page).unwrap().len() < MAX_PACKET);
        assert_eq!(page["hasMore"], true);
        assert!(page["records"].as_array().unwrap().len() < 8);
        for _ in 0..6 {
            let result = b.replica_pull_peer(&gid, &a.id()).await.unwrap();
            if result["hasMore"] == false {
                break;
            }
        }
        let rows = b.group_message_rows(Some(&gid), 0).unwrap();
        assert_eq!(rows["messages"].as_array().unwrap().len(), 12);
        assert!(
            rows["messages"]
                .as_array()
                .unwrap()
                .iter()
                .all(|m| m["signatureVerified"] == true)
        );
        a.close().await;
        b.close().await;
    }
    #[tokio::test]
    async fn signatures_forks_stale_epochs_and_oversized_sequences_are_checked() {
        let (_da, a) = peer().await;
        let (_db, b) = peer().await;
        let (_dc, c) = peer().await;
        let gid = a
            .dispatch(json!({"action":"groups.create","name":"Security"}))
            .await
            .unwrap()["group"]["groupId"]
            .as_str()
            .unwrap()
            .to_string();
        join(&a, &b, &gid).await;
        join(&a, &c, &gid).await;
        let sent = b
            .dispatch(json!({"action":"groups.send","groupId":gid,"text":"original"}))
            .await
            .unwrap();
        let raw: String =
            b.0.db
                .lock()
                .unwrap()
                .query_row(
                    "SELECT data FROM group_records WHERE id=?",
                    [sent["id"].as_str().unwrap()],
                    |r| r.get(0),
                )
                .unwrap();
        let envelope: Value = serde_json::from_str(&raw).unwrap();
        c.replica_ingest(&gid, &envelope, false).unwrap();
        let mut body = Core::verify(&envelope).unwrap();
        let mut tampered = envelope.clone();
        body["text"] = json!("tampered");
        tampered["body"] = json!(B64.encode(serde_json::to_vec(&body).unwrap()));
        assert!(c.replica_ingest(&gid, &tampered, false).is_err());
        assert!(
            c.replica_ingest(&gid, &c.signed(body.clone()).unwrap(), false)
                .is_err(),
            "relay cannot impersonate author"
        );
        body["messageId"] = json!(uid());
        assert!(
            c.replica_ingest(&gid, &b.signed(body.clone()).unwrap(), false)
                .unwrap_err()
                .to_string()
                .contains("group_author_fork")
        );
        body["authorSequence"] = json!(i64::MAX);
        assert!(
            c.replica_ingest(&gid, &b.signed(body.clone()).unwrap(), false)
                .is_err()
        );
        body["authorSequence"] = json!(2);
        body["previous"] = envelope["signature"].clone();
        a.dispatch(json!({"action":"groups.policy","groupId":gid,"peer":b.id(),"canSend":false}))
            .await
            .unwrap();
        let stale = b.signed(body).unwrap();
        assert!(
            c.replica_ingest(&gid, &stale, false).is_err(),
            "old authority cannot authorize live messages"
        );
        c.replica_ingest(&gid, &stale, true).unwrap();
        let page = c.group_message_rows(Some(&gid), 0).unwrap();
        assert!(
            page["messages"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["authorization"] == "historical_unfinalized"
                    && r["executionAllowed"] == false)
        );
        for p in [&a, &b, &c] {
            p.close().await;
        }
    }
}
