//! Explicit task confirmation; these events never invoke an agent or grant execution rights.
use super::*;
use rusqlite::{OptionalExtension, Transaction};

const MAX_TEXT: usize = 8000;
fn bounded(v: &Value, key: &str, max: usize) -> Result<String> {
    let s = v[key].as_str().context("workflow_missing_field")?;
    ensure!(
        !s.trim().is_empty() && s.len() <= max,
        "workflow_invalid_field"
    );
    Ok(s.to_owned())
}
fn load(db: &Connection, id: &str) -> Result<Value> {
    let raw: String = db
        .query_row("SELECT data FROM workflow_tasks WHERE id=?", [id], |r| {
            r.get(0)
        })
        .optional()?
        .context("workflow_task_not_found")?;
    Ok(serde_json::from_str(&raw)?)
}
fn visible(mut task: Value) -> Value {
    if task["deadline"].as_i64().unwrap_or(0) <= now()
        && !matches!(task["state"].as_str(), Some("completed" | "declined"))
    {
        task["recordedState"] = task["state"].clone();
        task["state"] = json!("expired");
    }
    task
}

/// Applied in the same SQLite transaction as message insertion, before ACK.
pub(super) fn apply(
    tx: &Transaction<'_>,
    local: &str,
    message_id: &str,
    sid: &str,
    sender: &str,
    body: &str,
) -> Result<()> {
    let event: Value = serde_json::from_str(body).context("workflow_invalid_event")?;
    ensure!(
        event["v"] == 1
            && event["eventId"] == message_id
            && event["sessionId"] == sid
            && event["actor"] == sender,
        "workflow_invalid_envelope"
    );
    let task_id = bounded(&event, "taskId", 80)?;
    ensure!(
        uuid::Uuid::parse_str(&task_id).is_ok() && uuid::Uuid::parse_str(message_id).is_ok(),
        "workflow_invalid_id"
    );
    let timestamp = event["created"]
        .as_i64()
        .context("workflow_invalid_timestamp")?;
    ensure!(timestamp <= now() + 30000, "workflow_future_event");
    let raw: String = tx.query_row("SELECT data FROM sessions WHERE id=?", [sid], |r| r.get(0))?;
    let session: Session = serde_json::from_str(&raw)?;
    ensure!(
        !session.temporary && session.status == "open",
        "workflow_persistent_session_required"
    );
    let peer = session.peer_id.context("workflow_peer_required")?;
    ensure!(
        sender == local || sender == peer,
        "workflow_actor_not_allowed"
    );
    let kind = bounded(&event, "type", 40)?;
    let mut task = if kind == "proposed" {
        ensure!(
            tx.query_row(
                "SELECT COUNT(*) FROM workflow_tasks WHERE id=?",
                [&task_id],
                |r| r.get::<_, i64>(0)
            )? == 0,
            "workflow_task_id_conflict"
        );
        ensure!(
            event["revision"] == 1 && event["requesterId"] == sender,
            "workflow_invalid_proposal"
        );
        let assignee = if sender == local { &peer } else { local };
        ensure!(event["assigneeId"] == assignee, "workflow_invalid_assignee");
        let deadline = event["deadline"]
            .as_i64()
            .context("workflow_invalid_deadline")?;
        ensure!(
            deadline > now() && deadline > timestamp && deadline <= timestamp + 86400000,
            "workflow_invalid_deadline"
        );
        let title = bounded(&event, "title", 160)?;
        let text = bounded(&event, "text", MAX_TEXT)?;
        ensure!(
            event["contractId"].is_null()
                || event["contractId"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty() && s.len() <= 128),
            "workflow_invalid_contract_id"
        );
        json!({"v":1,"taskId":task_id,"sessionId":sid,"requesterId":sender,"assigneeId":assignee,"title":title,"text":text,"deadline":deadline,"contractId":event["contractId"],"state":"proposed","revision":0,"created":timestamp,"result":null,"review":null})
    } else {
        let mut task = load(tx, &task_id)?;
        ensure!(
            task["sessionId"] == sid && task["deadline"].as_i64().unwrap_or(0) > now(),
            "workflow_task_expired_or_wrong_session"
        );
        ensure!(
            event["revision"].as_u64() == task["revision"].as_u64().and_then(|n| n.checked_add(1)),
            "workflow_revision_conflict"
        );
        let state = task["state"].as_str().context("workflow_invalid_state")?;
        match kind.as_str() {
            "accepted" | "declined" => {
                ensure!(
                    task["assigneeId"] == sender && state == "proposed",
                    "workflow_transition_denied"
                );
                task["state"] = json!(if kind == "accepted" {
                    "accepted"
                } else {
                    "declined"
                });
                if kind == "declined" {
                    task["declineReason"] = json!(bounded(&event, "text", MAX_TEXT)?);
                }
            }
            "delivered" => {
                ensure!(
                    task["assigneeId"] == sender
                        && matches!(state, "accepted" | "changes_requested"),
                    "workflow_transition_denied"
                );
                let text = bounded(&event, "text", MAX_TEXT)?;
                if !event["sha256"].is_null() {
                    ensure!(
                        event["sha256"].as_str().is_some_and(
                            |s| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit())
                        ),
                        "workflow_invalid_digest"
                    );
                }
                task["result"] = json!({"text":text,"sha256":event["sha256"],"eventId":message_id,"submittedBy":sender});
                task["state"] = json!("delivered");
            }
            "approved" | "changes_requested" => {
                ensure!(
                    task["requesterId"] == sender && state == "delivered",
                    "workflow_transition_denied"
                );
                task["review"] = json!({"text":bounded(&event, "text", MAX_TEXT)?,"eventId":message_id,"reviewedBy":sender});
                task["state"] = json!(if kind == "approved" {
                    "completed"
                } else {
                    "changes_requested"
                });
            }
            _ => bail!("workflow_unknown_transition"),
        }
        task
    };
    task["revision"] = event["revision"].clone();
    task["lastEventId"] = json!(message_id);
    task["lastActor"] = json!(sender);
    task["updated"] = json!(timestamp);
    tx.execute("INSERT INTO workflow_tasks(id,data) VALUES(?,?) ON CONFLICT(id) DO UPDATE SET data=excluded.data", params![task_id,serde_json::to_string(&task)?])?;
    Ok(())
}

impl Core {
    fn workflow_view(&self, id: &str) -> Result<Value> {
        let db = self.0.db.lock().unwrap();
        let mut task = visible(load(&db, id)?);
        task["lastEventDelivery"] = db
            .query_row(
                "SELECT status FROM messages WHERE id=?",
                [text(&task, "lastEventId")?],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .map(Value::String)
            .unwrap_or(Value::Null);
        Ok(task)
    }
    pub(super) async fn workflow_action(&self, r: Value) -> Result<Value> {
        let action = text(&r, "action")?;
        if action == "tasks.show" {
            return Ok(json!({"task":self.workflow_view(text(&r,"taskId")?)?}));
        }
        if action == "tasks.list" {
            let offset = r["offset"].as_u64().unwrap_or(0).min(i64::MAX as u64) as i64;
            let ids = {
                let db = self.0.db.lock().unwrap();
                let mut q =
                    db.prepare("SELECT id FROM workflow_tasks ORDER BY rowid LIMIT 33 OFFSET ?")?;
                q.query_map([offset], |r| r.get::<_, String>(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?
            };
            let total = ids.len();
            let mut tasks = vec![];
            let mut bytes = 0;
            for id in ids.into_iter().take(32) {
                let task = self.workflow_view(&id)?;
                let size = serde_json::to_vec(&task)?.len();
                if !tasks.is_empty() && bytes + size > 400000 {
                    break;
                }
                bytes += size;
                tasks.push(task);
            }
            let more = tasks.len() < total;
            return Ok(
                json!({"nextOffset":offset+tasks.len() as i64,"hasMore":more,"tasks":tasks}),
            );
        }
        if action == "tasks.history" {
            let id = text(&r, "taskId")?;
            let task = self.workflow_view(id)?;
            let after = r["after"].as_i64().unwrap_or(0);
            let rows = self
                .messages(Some(text(&task, "sessionId")?), after, false)?
                .into_iter()
                .filter(|m| {
                    m["kind"] == "task_event"
                        && serde_json::from_str::<Value>(m["text"].as_str().unwrap_or(""))
                            .is_ok_and(|e| e["taskId"] == id)
                })
                .collect();
            return Self::page(rows, after);
        }
        let kind = match action {
            "tasks.propose" => "proposed",
            "tasks.accept" => "accepted",
            "tasks.decline" => "declined",
            "tasks.deliver" => "delivered",
            "tasks.approve" => "approved",
            "tasks.reject" => "changes_requested",
            _ => bail!("workflow_unknown_action"),
        };
        let event_id = r["eventId"].as_str().map(str::to_owned).unwrap_or_else(uid);
        ensure!(
            uuid::Uuid::parse_str(&event_id).is_ok(),
            "workflow_invalid_event_id"
        );
        // Replaying a locally persisted event keeps the original timestamp and revision.
        let previous = {
            let db = self.0.db.lock().unwrap();
            db.query_row(
                "SELECT session_id,sender,text,kind FROM messages WHERE id=?",
                [&event_id],
                |q| {
                    Ok((
                        q.get::<_, String>(0)?,
                        q.get::<_, String>(1)?,
                        q.get::<_, String>(2)?,
                        q.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?
        };
        let (sid, body, task_id) = if let Some((sid, sender, body, mk)) = previous {
            let event: Value = serde_json::from_str(&body).context("workflow_event_id_conflict")?;
            ensure!(
                sender == self.id() && mk == "task_event" && event["type"] == kind,
                "workflow_event_id_conflict"
            );
            if kind != "proposed" {
                ensure!(event["taskId"] == r["taskId"], "workflow_event_id_conflict");
            }
            for key in ["text", "title", "contractId", "sha256", "sessionId"] {
                if !r[key].is_null() {
                    ensure!(event[key] == r[key], "workflow_event_id_conflict");
                }
            }
            if let Some(ttl) = r["ttlSeconds"].as_i64() {
                ensure!(
                    (1..=86400).contains(&ttl)
                        && event["deadline"]
                            .as_i64()
                            .unwrap_or(0)
                            .checked_sub(event["created"].as_i64().unwrap_or(0))
                            == ttl.checked_mul(1000),
                    "workflow_event_id_conflict"
                );
            }
            (sid, body, text(&event, "taskId")?.to_owned())
        } else {
            let timestamp = now();
            let (sid, task_id, revision) = if kind == "proposed" {
                (bounded(&r, "sessionId", 80)?, event_id.clone(), 1)
            } else {
                let task_id = bounded(&r, "taskId", 80)?;
                let task = self.workflow_view(&task_id)?;
                ensure!(
                    task["lastActor"] != self.id()
                        || matches!(
                            task["lastEventDelivery"].as_str(),
                            Some("acked" | "received")
                        ),
                    "workflow_previous_event_not_delivered"
                );
                (
                    text(&task, "sessionId")?.to_owned(),
                    task_id,
                    task["revision"]
                        .as_u64()
                        .context("workflow_invalid_revision")?
                        + 1,
                )
            };
            let s = self.active(&sid)?;
            ensure!(
                !s.temporary && s.status == "open",
                "workflow_persistent_session_required"
            );
            let mut event = json!({"v":1,"eventId":event_id,"taskId":task_id,"sessionId":sid,"actor":self.id(),"created":timestamp,"type":kind,"revision":revision});
            if kind == "proposed" {
                let ttl = r["ttlSeconds"].as_i64().unwrap_or(3600);
                ensure!((1..=86400).contains(&ttl), "workflow_invalid_ttl");
                event["requesterId"] = json!(self.id());
                event["assigneeId"] = json!(s.peer_id.context("workflow_peer_required")?);
                event["deadline"] = json!(timestamp + ttl * 1000);
                event["contractId"] = r["contractId"].clone();
                event["title"] = json!(r["title"].as_str().unwrap_or("Task"));
            }
            if kind != "accepted" {
                event["text"] = r["text"].clone();
            }
            if kind == "delivered" {
                event["sha256"] = r["sha256"].clone();
            }
            (sid, serde_json::to_string(&event)?, task_id)
        };
        let delivery = self
            .send(&json!({"sessionId":sid,"text":body,"kind":"task_event","id":event_id}))
            .await?;
        Ok(json!({"task":self.workflow_view(&task_id)?,"eventId":event_id,"delivery":delivery}))
    }
}
