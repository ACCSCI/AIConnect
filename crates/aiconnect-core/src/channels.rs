//! Channels are subscriptions, never grants to execute remote text.
use super::*;
use rusqlite::OptionalExtension;

const MANIFEST_LIMIT: usize = 1_048_576;
const RETAIN: usize = 64;

impl Core {
    fn channels_schema(&self) -> Result<()> {
        self.0.db.lock().unwrap().execute_batch("CREATE TABLE IF NOT EXISTS channels_local(id TEXT PRIMARY KEY,data TEXT NOT NULL);CREATE TABLE IF NOT EXISTS channel_subscriptions(id TEXT PRIMARY KEY,data TEXT NOT NULL);CREATE TABLE IF NOT EXISTS channel_events(sequence INTEGER PRIMARY KEY AUTOINCREMENT,source_id TEXT NOT NULL,event_id TEXT NOT NULL,data TEXT NOT NULL,UNIQUE(source_id,event_id));CREATE INDEX IF NOT EXISTS channel_events_source ON channel_events(source_id,sequence);")?;
        Ok(())
    }
    fn channel_group_snapshot(&self, gid: &str) -> Result<Value> {
        let body = Self::verify_bounded(&self.load("chat_groups", gid)?, 44000)?;
        ensure!(body["objectType"] == "channel", "channel_not_a_channel");
        ensure!(
            body["members"][self.id()]["active"] == true,
            "group_not_member"
        );
        Ok(body)
    }
    fn channel_rows(&self, source: &str, after: i64) -> Result<Value> {
        let db = self.0.db.lock().unwrap();
        let mut q = db.prepare("SELECT sequence,data FROM channel_events WHERE source_id=? AND sequence>? ORDER BY sequence LIMIT 129")?;
        let mut rows = q
            .query_map(params![source, after], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let has_more = rows.len() > 128;
        rows.truncate(128);
        let mut events = vec![];
        let mut bytes = 0;
        let mut has_more = has_more;
        for (s, d) in rows {
            if !events.is_empty() && bytes + d.len() > 350000 {
                has_more = true;
                break;
            }
            bytes += d.len();
            events.push(json!({"cursor":s,"event":serde_json::from_str::<Value>(&d)?}));
        }
        let cursor = events
            .last()
            .and_then(|e| e["cursor"].as_i64())
            .unwrap_or(after);
        Ok(json!({"events":events,"cursor":cursor,"hasMore":has_more,"executed":false}))
    }
    fn channel_import_body(&self, source_id: &str, body: &str, provenance: &str) -> Result<Value> {
        ensure!(body.len() <= MANIFEST_LIMIT, "channel_response_too_large");
        let mut sub = self.load("channel_subscriptions", source_id)?;
        ensure!(sub["active"] == true, "channel_unsubscribed");
        let mut events = vec![];
        let mut native_manifest = false;
        let mut gap = false;
        let mut max_sequence = sub["maxSequence"].as_u64().unwrap_or(0);
        if let Ok(envelope) = serde_json::from_str::<Value>(body) {
            if envelope.get("signature").is_some() {
                native_manifest = true;
                let manifest = Self::verify_bounded(&envelope, MANIFEST_LIMIT)?;
                ensure!(
                    manifest["v"] == 1 && manifest["kind"] == "channel_manifest",
                    "channel_invalid_manifest"
                );
                let publisher = text(&manifest, "id")?;
                let channel_id = text(&manifest, "channelId")?;
                if let Some(pin) = sub["publisher"].as_str() {
                    ensure!(pin == publisher, "channel_publisher_key_conflict");
                }
                if let Some(pin) = sub["channelId"].as_str() {
                    ensure!(pin == channel_id, "channel_source_conflict");
                }
                let list = manifest["events"]
                    .as_array()
                    .context("channel_invalid_events")?;
                ensure!(list.len() <= RETAIN, "channel_too_many_events");
                let declared_last = manifest["lastSequence"]
                    .as_u64()
                    .context("channel_invalid_sequence")?;
                ensure!(declared_last >= max_sequence, "channel_manifest_rollback");
                let mut prior: Option<(u64, String)> = None;
                for signed in list {
                    let event = Self::verify(signed)?;
                    ensure!(
                        event["v"] == 1
                            && event["kind"] == "channel_event"
                            && event["id"] == publisher
                            && event["channelId"] == channel_id,
                        "channel_invalid_event"
                    );
                    let seq = event["sequence"]
                        .as_u64()
                        .filter(|s| *s > 0 && *s <= 1_000_000_000_000)
                        .context("channel_invalid_sequence")?;
                    let eid = text(&event, "eventId")?;
                    ensure!(
                        uuid::Uuid::parse_str(eid).is_ok(),
                        "channel_invalid_event_id"
                    );
                    ensure!(text(&event, "text")?.len() <= 8000, "channel_invalid_text");
                    ensure!(
                        event["created"]
                            .as_i64()
                            .is_some_and(|t| t >= 0 && t <= now() + 30000),
                        "channel_invalid_time"
                    );
                    if let Some((prev_seq, prev_ref)) = &prior {
                        ensure!(
                            seq == prev_seq + 1 && event["previous"] == *prev_ref,
                            "channel_invalid_chain"
                        );
                    } else {
                        gap = seq > max_sequence.saturating_add(1);
                        if seq == 1 {
                            ensure!(event["previous"] == "", "channel_invalid_chain");
                        } else {
                            let previous = text(&event, "previous")?;
                            ensure!(
                                previous.len() == 86
                                    && B64.decode(previous).is_ok_and(|v| v.len() == 64),
                                "channel_invalid_chain"
                            );
                        }
                    }
                    prior = Some((seq, text(signed, "signature")?.into()));
                    events.push((eid.to_owned(),json!({"body":event,"signature":signed["signature"],"provenance":"author_signature","publisher":publisher})));
                    max_sequence = max_sequence.max(seq);
                }
                ensure!(
                    list.is_empty() && declared_last == 0
                        || prior.as_ref().is_some_and(|p| p.0 == declared_last),
                    "channel_invalid_manifest_tail"
                );
                sub["publisher"] = json!(publisher);
                sub["channelId"] = json!(channel_id);
                sub["keyPin"] = json!(if sub["expectedPublisher"].is_string() {
                    "explicit"
                } else {
                    "tofu_not_owner_verified"
                });
            } else {
                ensure!(sub["publisher"].is_null(), "channel_signature_downgrade");
                events = parse_feed(body, provenance)?;
            }
        } else {
            ensure!(sub["publisher"].is_null(), "channel_signature_downgrade");
            events = parse_feed(body, provenance)?;
        }
        let mut db = self.0.db.lock().unwrap();
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let latest_raw: String = tx.query_row(
            "SELECT data FROM channel_subscriptions WHERE id=?",
            [source_id],
            |r| r.get(0),
        )?;
        let mut latest: Value = serde_json::from_str(&latest_raw)?;
        ensure!(latest["active"] == true, "channel_unsubscribed");
        if native_manifest {
            if let Some(pin) = latest["publisher"].as_str() {
                ensure!(sub["publisher"] == pin, "channel_publisher_key_conflict");
            }
            if let Some(pin) = latest["channelId"].as_str() {
                ensure!(sub["channelId"] == pin, "channel_source_conflict");
            }
            ensure!(
                max_sequence >= latest["maxSequence"].as_u64().unwrap_or(0),
                "channel_manifest_rollback"
            );
            for key in ["publisher", "channelId", "keyPin"] {
                latest[key] = sub[key].clone();
            }
        } else {
            ensure!(latest["publisher"].is_null(), "channel_signature_downgrade");
        }
        sub = latest;
        let mut inserted = 0;
        for (eid, event) in events {
            if event["provenance"] == "author_signature" {
                let fork:Option<String>=tx.query_row("SELECT data FROM channel_events WHERE source_id=? AND json_extract(data,'$.body.sequence')=?",params![source_id,event["body"]["sequence"].as_u64()],|r|r.get(0)).optional()?;
                if let Some(fork) = fork {
                    ensure!(
                        serde_json::from_str::<Value>(&fork)? == event,
                        "channel_sequence_conflict"
                    );
                }
            }
            let existing: Option<String> = tx
                .query_row(
                    "SELECT data FROM channel_events WHERE source_id=? AND event_id=?",
                    params![source_id, eid],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(existing) = existing {
                // Ordinary RSS entries may change under a stable ID; native signed events may not.
                if event["provenance"] == "author_signature" {
                    ensure!(
                        serde_json::from_str::<Value>(&existing)? == event,
                        "channel_event_conflict"
                    );
                }
            } else {
                super::governance::storage_check(&tx, event.to_string().len())?;
                inserted += tx.execute(
                    "INSERT INTO channel_events(source_id,event_id,data) VALUES(?,?,?)",
                    params![source_id, eid, event.to_string()],
                )?;
            }
        }
        sub["maxSequence"] = json!(max_sequence);
        gap = gap || sub["gap"] == true;
        sub["gap"] = json!(gap);
        sub["lastSync"] = json!(now());
        tx.execute(
            "UPDATE channel_subscriptions SET data=? WHERE id=?",
            params![sub.to_string(), source_id],
        )?;
        let removed=tx.execute("DELETE FROM channel_events WHERE source_id=? AND sequence NOT IN (SELECT sequence FROM channel_events WHERE source_id=? ORDER BY sequence DESC LIMIT 5000)",params![source_id,source_id])?;
        tx.commit()?;
        Ok(
            json!({"sourceId":source_id,"inserted":inserted,"historyGap":gap,"retentionRemoved":removed,"notificationsReady":if sub["muted"]==true{0}else{inserted},"executed":false,"wakeRequested":false}),
        )
    }
    pub async fn channel_request(&self, r: &Value) -> Result<Value> {
        self.channels_schema()?;
        match text(r, "action")? {
            "channels.create" => {
                let name = text(r, "name")?;
                ensure!(
                    !name.trim().is_empty() && name.len() <= 160,
                    "channel_invalid_name"
                );
                let visibility = r["visibility"].as_str().unwrap_or("private");
                ensure!(
                    matches!(visibility, "private" | "public"),
                    "channel_invalid_visibility"
                );
                if visibility == "private" {
                    let group=self.group_action(json!({"action":"groups.create","name":name,"channel":true,"maxMembers":r["maxMembers"].as_u64().unwrap_or(16)})).await?;
                    Ok(
                        json!({"channelId":group["group"]["groupId"],"visibility":"private","group":group["group"]}),
                    )
                } else {
                    let id = uid();
                    let channel = json!({"channelId":id,"name":name,"visibility":"public","publisher":self.id(),"lastSequence":0,"created":now()});
                    self.put("channels_local", &id, &channel)?;
                    Ok(json!({"channelId":id,"channel":channel,"publishedExternally":false}))
                }
            }
            "channels.list" => {
                let mut private = vec![];
                for env in self.list("chat_groups")? {
                    let s = Self::verify_bounded(&env, 44000)?;
                    if s["objectType"] == "channel" {
                        private.push(s);
                    }
                }
                Ok(json!({"public":self.list("channels_local")?,"private":private}))
            }
            "channels.show" => {
                let id = text(r, "channelId")?;
                match self.load("channels_local", id) {
                    Ok(c) => Ok(json!({"channel":c})),
                    Err(_) => Ok(json!({"channel":self.channel_group_snapshot(id)?})),
                }
            }
            "channels.invite" | "channels.publisher" => {
                let gid = text(r, "channelId")?;
                let s = self.channel_group_snapshot(gid)?;
                ensure!(s["id"] == self.id(), "channel_owner_required");
                let peer = text(r, "peer")?;
                let req = if r["action"] == "channels.invite" {
                    json!({"action":"groups.invite","groupId":gid,"peer":peer})
                } else {
                    json!({"action":"groups.policy","groupId":gid,"peer":peer,"canSend":r["canPublish"].as_bool().context("channel_invalid_policy")?})
                };
                self.group_action(req).await
            }
            "channels.join" => {
                let result = self
                    .group_action(json!({"action":"groups.join","invite":text(r,"invite")?}))
                    .await?;
                ensure!(
                    result["group"]["objectType"] == "channel",
                    "channel_not_a_channel"
                );
                Ok(result)
            }
            "channels.publish" => {
                let id = text(r, "channelId")?;
                let content = text(r, "text")?;
                ensure!(
                    !content.trim().is_empty() && content.len() <= 8000,
                    "channel_invalid_text"
                );
                if self.load("channels_local", id).is_ok() {
                    let eid = r["id"].as_str().map(str::to_owned).unwrap_or_else(uid);
                    ensure!(
                        uuid::Uuid::parse_str(&eid).is_ok(),
                        "channel_invalid_event_id"
                    );
                    let mut db = self.0.db.lock().unwrap();
                    let tx =
                        db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                    if let Some(existing) = tx
                        .query_row(
                            "SELECT data FROM channel_events WHERE source_id=? AND event_id=?",
                            params![id, eid],
                            |r| r.get::<_, String>(0),
                        )
                        .optional()?
                    {
                        let existing: Value = serde_json::from_str(&existing)?;
                        ensure!(
                            Self::verify(&existing)?["text"] == content,
                            "channel_event_conflict"
                        );
                        return Ok(
                            json!({"eventId":eid,"duplicate":true,"publishedExternally":false}),
                        );
                    }
                    // Re-read state inside the write transaction to serialize concurrent publishers.
                    let mut channel: Value = serde_json::from_str(&tx.query_row(
                        "SELECT data FROM channels_local WHERE id=?",
                        [id],
                        |r| r.get::<_, String>(0),
                    )?)?;
                    let seq = channel["lastSequence"].as_u64().unwrap_or(0) + 1;
                    let event=self.signed(json!({"v":1,"kind":"channel_event","id":self.id(),"channelId":id,"eventId":eid,"sequence":seq,"previous":channel["lastRef"].as_str().unwrap_or(""),"text":content,"created":now()}))?;
                    channel["lastSequence"] = json!(seq);
                    channel["lastRef"] = event["signature"].clone();
                    super::governance::storage_check(&tx, event.to_string().len())?;
                    tx.execute(
                        "INSERT INTO channel_events(source_id,event_id,data) VALUES(?,?,?)",
                        params![id, eid, event.to_string()],
                    )?;
                    tx.execute(
                        "UPDATE channels_local SET data=? WHERE id=?",
                        params![channel.to_string(), id],
                    )?;
                    tx.execute("DELETE FROM channel_events WHERE source_id=? AND sequence NOT IN (SELECT sequence FROM channel_events WHERE source_id=? ORDER BY sequence DESC LIMIT 64)",params![id,id])?;
                    tx.commit()?;
                    Ok(
                        json!({"eventId":eid,"sequence":seq,"publishedExternally":false,"executed":false}),
                    )
                } else {
                    self.channel_group_snapshot(id)?;
                    let mut req = json!({"action":"groups.send","groupId":id,"text":content});
                    if let Some(eid) = r["id"].as_str() {
                        req["id"] = json!(eid);
                    }
                    self.group_action(req).await
                }
            }
            "channels.export" => {
                let id = text(r, "channelId")?;
                let channel = self.load("channels_local", id)?;
                let mut events: Vec<Value> = {
                    let db = self.0.db.lock().unwrap();
                    let mut q = db.prepare(
                        "SELECT data FROM channel_events WHERE source_id=? ORDER BY sequence",
                    )?;
                    q.query_map([id], |r| r.get::<_, String>(0))?
                        .map(|r| Ok(serde_json::from_str(&r?)?))
                        .collect::<Result<_>>()?
                };
                // Bound the complete escaped IPC response, including the feed mirror.
                loop {
                    let mut items = vec![];
                    for event in &events {
                        let e = Self::verify(event)?;
                        items.push(json!({"id":e["eventId"],"content_text":e["text"]}));
                    }
                    let signed=self.signed(json!({"v":1,"kind":"channel_manifest","id":self.id(),"channelId":id,"name":channel["name"],"lastSequence":channel["lastSequence"],"events":events,"generatedAt":now(),"retention":events.len()}))?;
                    let mirror = json!({"version":"https://jsonfeed.org/version/1.1","title":channel["name"],"items":items,"_aiconnect":{"channelId":id,"signatureLocation":"channel.json","provenance":"unsigned_feed_mirror"}});
                    let result = json!({"files":{"channel.json":serde_json::to_string_pretty(&signed)?,"feed.json":serde_json::to_string_pretty(&mirror)?},"publishedExternally":false,"retention":events.len(),"storedRetention":RETAIN});
                    if serde_json::to_vec(&result)?.len() <= 900000 {
                        return Ok(result);
                    }
                    ensure!(events.len() > 1, "channel_export_too_large");
                    events.remove(0);
                }
            }
            "channels.subscribe" => {
                let id = r["sourceId"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(uid);
                ensure!(
                    !id.is_empty() && id.len() <= 200,
                    "channel_invalid_source_id"
                );
                let url = r["url"].as_str();
                let gid = r["channelId"].as_str();
                ensure!(
                    url.is_some() ^ gid.is_some(),
                    "channel_supply_url_or_channel"
                );
                if let Some(url) = url {
                    let parsed = reqwest::Url::parse(url)?;
                    ensure!(
                        parsed.scheme() == "https"
                            && parsed.username().is_empty()
                            && parsed.password().is_none(),
                        "channel_https_required"
                    );
                }
                if let Some(gid) = gid {
                    self.channel_group_snapshot(gid)?;
                }
                let existing = self.load("channel_subscriptions", &id).ok();
                ensure!(
                    self.load("channels_local", &id).is_err(),
                    "channel_source_id_is_owned_channel"
                );
                if let Some(existing) = &existing {
                    ensure!(
                        existing["url"] == json!(url)
                            && (url.is_some() || existing["channelId"] == json!(gid)),
                        "channel_source_conflict"
                    );
                }
                let mut sub=existing.unwrap_or(json!({"sourceId":id,"url":url,"channelId":gid,"active":true,"muted":false,"wake":false,"readCursor":0,"maxSequence":0,"nextPoll":0,"failures":0,"created":now()}));
                if let Some(pin) = r["expectedPublisher"].as_str() {
                    EndpointId::from_str(pin)?;
                    if let Some(old) = sub["publisher"].as_str() {
                        ensure!(old == pin, "channel_publisher_key_conflict");
                    }
                    sub["publisher"] = json!(pin);
                    sub["expectedPublisher"] = json!(pin);
                }
                sub["active"] = json!(true);
                if let Some(wake) = r["wake"].as_bool() {
                    self.channel_set_wake(&mut sub, wake)?;
                }
                self.put("channel_subscriptions", &id, &sub)?;
                Ok(json!({"subscription":sub,"fetched":false,"executed":false}))
            }
            "channels.subscriptions" => {
                Ok(json!({"subscriptions":self.list("channel_subscriptions")?}))
            }
            "channels.wake" => {
                let id = text(r, "sourceId")?;
                let mut sub = self.load("channel_subscriptions", id)?;
                self.channel_set_wake(
                    &mut sub,
                    r["wake"]
                        .as_bool()
                        .context("channel_wake_boolean_required")?,
                )?;
                self.put("channel_subscriptions", id, &sub)?;
                Ok(
                    json!({"subscription":sub,"taskPermission":false,"requiredSandbox":"read-only","automaticReply":false}),
                )
            }
            "channels.notification.authorize" => {
                let source = text(r, "sourceId")?;
                let cursor = r["cursor"]
                    .as_i64()
                    .context("channel_notification_cursor_required")?;
                let sub = self.load("channel_subscriptions", source)?;
                ensure!(
                    sub["active"] == true && sub["muted"] != true && sub["wake"] == true,
                    "channel_notification_not_authorized"
                );
                ensure!(
                    cursor > sub["wakeAfter"].as_i64().unwrap_or(i64::MAX),
                    "channel_notification_before_grant"
                );
                let raw: String = self.0.db.lock().unwrap().query_row(
                    "SELECT data FROM channel_events WHERE source_id=? AND sequence=?",
                    params![source, cursor],
                    |r| r.get(0),
                )?;
                let event: Value = serde_json::from_str(&raw)?;
                if let Some(author) = event["body"]["id"].as_str() {
                    self.allowed(author, "chat")?;
                }
                if sub["url"].is_null() {
                    let state = self.channel_group_snapshot(text(&sub, "channelId")?)?;
                    ensure!(state["status"] == "active", "group_dissolved");
                    let author = text(&event["body"], "id")?;
                    ensure!(
                        state["members"][author]["active"] == true
                            && state["members"][author]["canSend"] == true,
                        "channel_publisher_revoked"
                    );
                    ensure!(
                        event["provenance"] == "group_member_signature",
                        "channel_unsigned_notification_denied"
                    );
                    ensure!(
                        event["body"]["epoch"] == state["epoch"],
                        "channel_historical_notification_denied"
                    );
                }
                Ok(
                    json!({"allowed":true,"wake":true,"notification":true,"task":false,"sandbox":"read-only","maxTurns":1,"maxTimeoutMs":30000,"event":event}),
                )
            }
            "channels.notifications" => {
                let after = r["after"].as_i64().unwrap_or(0);
                let db = self.0.db.lock().unwrap();
                let mut q=db.prepare("SELECT e.sequence,e.source_id,e.data FROM channel_events e JOIN channel_subscriptions s ON s.id=e.source_id WHERE e.sequence>? AND json_extract(s.data,'$.active')=1 AND json_extract(s.data,'$.muted')=0 ORDER BY e.sequence LIMIT 33")?;
                let rows = q
                    .query_map([after], |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                        ))
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                let mut notices = vec![];
                let mut cursor = after;
                let mut bytes = 0;
                let mut more = false;
                for (sequence, source, data) in rows {
                    if notices.len() >= 32 || !notices.is_empty() && bytes + data.len() > 350000 {
                        more = true;
                        break;
                    }
                    bytes += data.len();
                    cursor = sequence;
                    notices.push(json!({"cursor":sequence,"sourceId":source,"event":serde_json::from_str::<Value>(&data)?}));
                }
                if notices.is_empty() {
                    cursor = db
                        .query_row(
                            "SELECT COALESCE(MAX(sequence),0) FROM channel_events",
                            [],
                            |r| r.get::<_, i64>(0),
                        )?
                        .max(after);
                }
                Ok(
                    json!({"notifications":notices,"cursor":cursor,"hasMore":more,"executed":false,"wakeRequested":false}),
                )
            }
            "channels.unsubscribe" | "channels.mute" => {
                let id = text(r, "sourceId")?;
                let mut sub = self.load("channel_subscriptions", id)?;
                if r["action"] == "channels.unsubscribe" {
                    sub["active"] = json!(false);
                    sub["wake"] = json!(false);
                } else {
                    sub["muted"] = json!(r["muted"].as_bool().unwrap_or(true));
                }
                self.put("channel_subscriptions", id, &sub)?;
                Ok(json!({"subscription":sub,"wakeRequested":false}))
            }
            "channels.read" => {
                let mut sub = None;
                let id = if let Some(source) = r["sourceId"].as_str() {
                    sub = Some(self.load("channel_subscriptions", source)?);
                    source
                } else {
                    text(r, "channelId")?
                };
                let mut result = if let Some(gid) = sub
                    .as_ref()
                    .and_then(|s| s["channelId"].as_str())
                    .filter(|_| sub.as_ref().is_some_and(|s| s["url"].is_null()))
                {
                    self.channel_group_snapshot(gid)?;
                    self.group_action(json!({"action":"groups.messages","groupId":gid,"after":r["after"].as_i64().unwrap_or(sub.as_ref().unwrap()["readCursor"].as_i64().unwrap_or(0))})).await?
                } else if sub.is_none() && self.load("channels_local", id).is_err() {
                    self.channel_group_snapshot(id)?;
                    self.group_action(json!({"action":"groups.messages","groupId":id,"after":r["after"].as_i64().unwrap_or(0)})).await?
                } else {
                    self.channel_rows(
                        id,
                        r["after"].as_i64().unwrap_or(
                            sub.as_ref()
                                .and_then(|s| s["readCursor"].as_i64())
                                .unwrap_or(0),
                        ),
                    )?
                };
                if let Some(mut sub) = sub {
                    let cursor = result["cursor"]
                        .as_i64()
                        .or_else(|| {
                            result["messages"]
                                .as_array()
                                .and_then(|a| a.last())
                                .and_then(|v| v["sequence"].as_i64())
                        })
                        .unwrap_or(sub["readCursor"].as_i64().unwrap_or(0));
                    sub["readCursor"] = json!(cursor);
                    self.put("channel_subscriptions", id, &sub)?;
                    result["historyGap"] = sub["gap"].clone();
                }
                result["executed"] = json!(false);
                Ok(result)
            }
            "channels.import" => {
                self.channel_import_body(text(r, "sourceId")?, text(r, "body")?, "unverified_local")
            }
            "channels.sync" => {
                let id = text(r, "sourceId")?;
                let mut sub = self.load("channel_subscriptions", id)?;
                ensure!(sub["active"] == true, "channel_unsubscribed");
                if sub["url"].is_null() {
                    let gid = text(&sub, "channelId")?;
                    self.channel_group_snapshot(gid)?;
                    return self
                        .group_action(json!({"action":"groups.sync","groupId":gid}))
                        .await;
                }
                ensure!(
                    now() >= sub["nextPoll"].as_i64().unwrap_or(0),
                    "channel_poll_backoff"
                );
                let response = self
                    .governance_http_get(
                        text(&sub, "url")?,
                        sub["etag"].as_str(),
                        sub["lastModified"].as_str(),
                        MANIFEST_LIMIT,
                    )
                    .await
                    .and_then(|response| {
                        let status = response["status"].as_u64().unwrap_or(0);
                        ensure!(
                            status == 200 || status == 304,
                            "channel_http_status_{status}"
                        );
                        let result = if status == 304 {
                            json!({"sourceId":id,"notModified":true,"inserted":0,"executed":false})
                        } else {
                            self.channel_import_body(id, text(&response, "body")?, "https_source")?
                        };
                        Ok((response, result))
                    });
                match response {
                    Ok((response, result)) => {
                        sub = self.load("channel_subscriptions", id)?;
                        sub["etag"] = response["etag"].clone();
                        sub["lastModified"] = response["lastModified"].clone();
                        sub["nextPoll"] = json!(now() + 60000);
                        sub["failures"] = json!(0);
                        sub["lastSync"] = json!(now());
                        self.put("channel_subscriptions", id, &sub)?;
                        Ok(result)
                    }
                    Err(error) => {
                        let failures = sub["failures"]
                            .as_u64()
                            .unwrap_or(0)
                            .saturating_add(1)
                            .min(16);
                        sub["failures"] = json!(failures);
                        sub["nextPoll"] =
                            json!(now() + (60000i64 * (1i64 << failures.min(6))).min(3600000));
                        sub["lastError"] = json!(error.to_string());
                        self.put("channel_subscriptions", id, &sub)?;
                        Err(error)
                    }
                }
            }
            _ => bail!("channel_unknown_action"),
        }
    }
    fn channel_set_wake(&self, sub: &mut Value, wake: bool) -> Result<()> {
        if wake && sub["wake"] != true {
            let db = self.0.db.lock().unwrap();
            sub["wakeAfter"] = json!(db.query_row(
                "SELECT COALESCE(MAX(sequence),0) FROM channel_events",
                [],
                |r| r.get::<_, i64>(0)
            )?);
            if sub["url"].is_null() {
                let gid = text(sub, "channelId")?.to_owned();
                sub["privateCursor"] = json!(db.query_row(
                    "SELECT COALESCE(MAX(sequence),0) FROM group_messages WHERE group_id=?",
                    [gid],
                    |r| r.get::<_, i64>(0)
                )?);
            }
        }
        sub["wake"] = json!(wake);
        Ok(())
    }
    fn channel_private_events(&self, source: &str) -> Result<Value> {
        let sub = self.load("channel_subscriptions", source)?;
        self.channel_group_snapshot(text(&sub, "channelId")?)?;
        let mut db = self.0.db.lock().unwrap();
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut sub: Value = serde_json::from_str(&tx.query_row(
            "SELECT data FROM channel_subscriptions WHERE id=?",
            [source],
            |r| r.get::<_, String>(0),
        )?)?;
        ensure!(sub["active"] == true, "channel_unsubscribed");
        let gid = text(&sub, "channelId")?.to_owned();
        let after = sub["privateCursor"].as_i64().unwrap_or(0);
        let records = {
            let mut q=tx.prepare("SELECT m.sequence,m.id,r.data FROM group_messages m LEFT JOIN group_records r ON r.id=m.id WHERE m.group_id=? AND m.sequence>? ORDER BY m.sequence LIMIT 32")?;
            q.query_map(params![gid, after], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut inserted = 0;
        for (sequence, id, raw) in records {
            sub["privateCursor"] = json!(sequence);
            let Some(raw) = raw else {
                continue;
            };
            let signed: Value = serde_json::from_str(&raw)?;
            let event = json!({"body":Self::verify(&signed)?,"signature":signed["signature"],"provenance":"group_member_signature"});
            let exists: i64 = tx.query_row(
                "SELECT COUNT(*) FROM channel_events WHERE source_id=? AND event_id=?",
                params![source, id],
                |r| r.get(0),
            )?;
            if exists == 0 {
                super::governance::storage_check(&tx, event.to_string().len())?;
                inserted += tx.execute(
                    "INSERT INTO channel_events(source_id,event_id,data) VALUES(?,?,?)",
                    params![source, id, event.to_string()],
                )?;
            }
        }
        sub["nextPoll"] = json!(now() + 2000);
        tx.execute(
            "UPDATE channel_subscriptions SET data=? WHERE id=?",
            params![sub.to_string(), source],
        )?;
        tx.commit()?;
        Ok(json!({"inserted":inserted,"executed":false,"wakeRequested":false}))
    }
    /// One due source per tick; serial execution prevents an unbounded poll fanout.
    pub async fn channel_tick(&self) -> Result<Value> {
        self.channels_schema()?;
        let selected = {
            let db = self.0.db.lock().unwrap();
            db.query_row("SELECT id FROM channel_subscriptions WHERE json_extract(data,'$.active')=1 AND COALESCE(json_extract(data,'$.nextPoll'),0)<=? ORDER BY COALESCE(json_extract(data,'$.nextPoll'),0),rowid LIMIT 1",[now()],|r|r.get::<_,String>(0)).optional()?
        };
        if let Some(source) = selected {
            if self.load("channel_subscriptions", &source)?["url"].is_null() {
                let result = self.channel_private_events(&source);
                if let Err(error) = &result {
                    let mut sub = self.load("channel_subscriptions", &source)?;
                    sub["nextPoll"] = json!(now() + 60000);
                    sub["lastError"] = json!(error.to_string());
                    self.put("channel_subscriptions", &source, &sub)?;
                }
                return result;
            }
            self.channel_request(&json!({"action":"channels.sync","sourceId":source}))
                .await
        } else {
            Ok(json!({"polled":false}))
        }
    }
}

fn parse_feed(body: &str, provenance: &str) -> Result<Vec<(String, Value)>> {
    let feed = feed_rs::parser::parse(body.as_bytes()).context("channel_invalid_feed")?;
    ensure!(feed.entries.len() <= 1000, "channel_too_many_events");
    let mut events = vec![];
    for entry in feed.entries {
        ensure!(
            !entry.id.is_empty() && entry.id.len() <= 4096,
            "channel_invalid_event_id"
        );
        let title = entry.title.map(|v| v.content).unwrap_or_default();
        let content = entry
            .content
            .and_then(|v| v.body)
            .or_else(|| entry.summary.map(|v| v.content))
            .unwrap_or_default();
        ensure!(
            title.len() + content.len() <= 64000,
            "channel_entry_too_large"
        );
        events.push((entry.id.clone(),json!({"eventId":entry.id,"title":title,"text":content,"published":entry.published.map(|d|d.to_rfc3339()),"provenance":provenance,"authorSignatureVerified":false})));
    }
    Ok(events)
}
