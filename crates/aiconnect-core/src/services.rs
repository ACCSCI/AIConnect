//! Durable collaboration jobs. Creating a job never grants execution rights.
use super::*;
use rusqlite::OptionalExtension;
use sha2::{Digest, Sha256};

fn init(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS service_jobs(job_id TEXT PRIMARY KEY,idempotency_key TEXT UNIQUE NOT NULL,request_json TEXT NOT NULL,data TEXT NOT NULL);CREATE INDEX IF NOT EXISTS service_jobs_order ON service_jobs(job_id);")?;
    Ok(())
}
fn field(r: &Value, key: &str, max: usize) -> Result<String> {
    let s = text(r, key)?;
    ensure!(
        !s.trim().is_empty() && s.len() <= max,
        "service_invalid_{key}"
    );
    Ok(s.to_owned())
}
fn free_price(price: &Value) -> bool {
    price["amount"].as_f64() == Some(0.0)
        && price["currency"]
            .as_str()
            .is_some_and(|s| !s.is_empty() && s.len() <= 12)
}
fn check_schema(schema: &Value, input: &Value) -> Result<()> {
    fn bounded(v: &Value, depth: usize) -> Result<()> {
        ensure!(depth < 20, "service_schema_too_deep");
        match v {
            Value::Object(m) => {
                ensure!(m.len() <= 128, "service_schema_too_wide");
                for (k, v) in m {
                    ensure!(
                        k != "$ref" && k != "$dynamicRef",
                        "service_remote_schema_disabled"
                    );
                    bounded(v, depth + 1)?;
                }
            }
            Value::Array(a) => {
                ensure!(a.len() <= 128, "service_schema_too_wide");
                for v in a {
                    bounded(v, depth + 1)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    bounded(schema, 0)?;
    let validator = jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .with_pattern_options(
            jsonschema::PatternOptions::fancy_regex()
                .backtrack_limit(10000)
                .size_limit(1048576)
                .dfa_size_limit(1048576),
        )
        .build(schema)
        .map_err(|error| anyhow::anyhow!("service_invalid_input_schema: {error}"))?;
    ensure!(validator.is_valid(input), "service_input_schema_mismatch");
    Ok(())
}
fn load(db: &Connection, id: &str) -> Result<Value> {
    let raw: String = db
        .query_row("SELECT data FROM service_jobs WHERE job_id=?", [id], |r| {
            r.get(0)
        })
        .optional()?
        .context("service_job_not_found")?;
    Ok(serde_json::from_str(&raw)?)
}
fn save(db: &Connection, job: &mut Value) -> Result<()> {
    let previous = job["storageRevision"].as_u64().unwrap_or(0);
    let mut next = job.clone();
    next["storageRevision"] = json!(previous + 1);
    let encoded = serde_json::to_string(&next)?;
    let old_size: i64 = db.query_row(
        "SELECT length(CAST(data AS BLOB)) FROM service_jobs WHERE job_id=?",
        [text(job, "jobId")?],
        |row| row.get(0),
    )?;
    if encoded.len() > old_size as usize {
        super::governance::storage_check(db, encoded.len() - old_size as usize)?;
    }
    let changed=db.execute("UPDATE service_jobs SET data=? WHERE job_id=? AND COALESCE(json_extract(data,'$.storageRevision'),0)=?", params![encoded,text(job,"jobId")?,previous])?;
    ensure!(changed == 1, "service_concurrent_update_retry_read");
    *job = next;
    Ok(())
}
fn terminal(job: &Value) -> bool {
    matches!(
        job["state"].as_str(),
        Some("completed" | "failed" | "canceled" | "expired")
    )
}
fn artifacts(value: &Value) -> Result<Value> {
    if value.is_null() {
        return Ok(json!([]));
    }
    let items = value.as_array().context("service_invalid_artifacts")?;
    ensure!(items.len() <= 32, "service_too_many_artifacts");
    for item in items {
        field(item, "name", 256)?;
        ensure!(
            item["sha256"]
                .as_str()
                .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())),
            "service_invalid_artifact_digest"
        );
        ensure!(
            item["size"]
                .as_u64()
                .is_some_and(|n| n <= 1_099_511_627_776),
            "service_invalid_artifact_size"
        );
        if !item["uri"].is_null() {
            field(item, "uri", 2048)?;
        }
    }
    Ok(value.clone())
}

impl Core {
    fn service_cached_descriptor(&self, agent: &str) -> Result<Option<Value>> {
        let db = self.0.db.lock().unwrap();
        let exists: i64 = db.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='discovery_cache'",
            [],
            |row| row.get(0),
        )?;
        if exists == 0 {
            return Ok(None);
        }
        let mut q =
            db.prepare("SELECT descriptor FROM discovery_cache WHERE agent_id=? LIMIT 129")?;
        let rows = q
            .query_map([agent], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        ensure!(rows.len() <= 128, "service_too_many_descriptor_sources");
        let mut best: Option<Value> = None;
        for raw in rows {
            let envelope: Value = serde_json::from_str(&raw)?;
            let payload = super::network_identity::verify_native_descriptor_signature(&envelope)?;
            ensure!(
                payload["agentId"] == agent,
                "service_descriptor_provider_mismatch"
            );
            if let Some(current) = &best {
                let revision = payload["revision"].as_u64().unwrap_or(0);
                let old = current["payload"]["revision"].as_u64().unwrap_or(0);
                if revision == old {
                    ensure!(
                        envelope["payload"] == current["payload"],
                        "service_descriptor_revision_fork"
                    );
                }
                if revision <= old {
                    continue;
                }
            }
            best = Some(envelope);
        }
        Ok(best)
    }
    fn service_load(&self, id: &str) -> Result<Value> {
        let mut job = {
            let db = self.0.db.lock().unwrap();
            init(&db)?;
            load(&db, id)?
        };
        if !terminal(&job) && job["deadline"].as_i64().unwrap_or(0) <= now() {
            job["state"] = json!("expired");
            job["updated"] = json!(now());
            let db = self.0.db.lock().unwrap();
            save(&db, &mut job)?;
        }
        Ok(job)
    }
    /// Authenticated local IPC only; provider metadata is untrusted data, never instructions.
    pub async fn service_request(&self, r: &Value) -> Result<Value> {
        ensure!(
            serde_json::to_vec(r)?.len() <= 262144,
            "service_request_too_large"
        );
        let action = text(r, "action")?;
        let result = match action {
            "service.inspect" | "service.quote" => {
                let provider = r["provider"].as_str().unwrap_or("");
                let envelope = if !r["descriptor"].is_null() {
                    Some(r["descriptor"].clone())
                } else if provider == self.id() {
                    self.load("settings", "nativeDescriptor").ok()
                } else {
                    self.service_cached_descriptor(provider)?
                };
                let descriptor = envelope
                    .as_ref()
                    .map(super::network_identity::verify_native_descriptor)
                    .transpose()?;
                if let Some(d) = &descriptor {
                    ensure!(
                        provider.is_empty() || d["agentId"] == provider,
                        "service_descriptor_provider_mismatch"
                    );
                }
                let skill = descriptor
                    .as_ref()
                    .and_then(|d| d["skills"].as_array())
                    .and_then(|skills| {
                        skills
                            .iter()
                            .find(|skill| skill["skillId"] == r["capability"])
                    })
                    .cloned();
                let price = skill
                    .as_ref()
                    .map(|s| s["declaredPrice"].clone())
                    .filter(|p| !p.is_null())
                    .unwrap_or(r["declaredPrice"].clone());
                let free = free_price(&price);
                Ok(
                    json!({"provider":r["provider"],"capability":r["capability"],"descriptor":envelope,"capabilityInfo":skill,"declaredPrice":price,"priceEvidence":"provider_declaration_only","projectPays":false,"allowedByFreeOnly":free,"executionAuthorized":false,"transports":["native","a2a"],"warning":"Capability and price declarations are not verified service quality or settlement guarantees"}),
                )
            }
            "service.invoke" => self.service_invoke(r).await,
            "service.dispatch" => {
                let mut job = self.service_load(text(r, "jobId")?)?;
                ensure!(
                    job["transport"] == "a2a"
                        && job["state"] == "queued"
                        && job["cancelRequested"] != true,
                    "service_dispatch_not_queued"
                );
                let grant = self.validate_service_grant(
                    text(&job, "grantId")?,
                    text(&job, "audience")?,
                    &format!("service:{}", text(&job, "capability")?),
                )?;
                let reservation = self.governance_admit(
                    text(&job, "audience")?,
                    grant["ownerId"].as_str(),
                    "execute",
                    text(&job, "jobId")?,
                )?;
                ensure!(
                    reservation["status"] == "reserved",
                    "service_execution_reservation_expired"
                );
                self.governance_provider_check(text(&job, "provider")?)?;
                job["state"] = json!("dispatching");
                job["updated"] = json!(now());
                let revision = job["storageRevision"].as_u64().unwrap_or(0);
                job["storageRevision"] = json!(revision + 1);
                let db = self.0.db.lock().unwrap();
                let changed=db.execute("UPDATE service_jobs SET data=? WHERE job_id=? AND json_extract(data,'$.state')='queued' AND json_extract(data,'$.cancelRequested')=0 AND COALESCE(json_extract(data,'$.storageRevision'),0)=?",params![serde_json::to_string(&job)?,text(&job,"jobId")?,revision])?;
                ensure!(changed == 1, "service_dispatch_already_claimed");
                Ok(json!({"job":job,"claimed":true,"automaticRetryAllowed":false}))
            }
            "service.snapshot" => {
                let mut job = self.service_load(text(r, "jobId")?)?;
                ensure!(
                    !r["externalTask"].is_null()
                        && serde_json::to_vec(&r["externalTask"])?.len() <= 65536,
                    "service_external_task_too_large"
                );
                job["externalTask"] = r["externalTask"].clone();
                let db = self.0.db.lock().unwrap();
                save(&db, &mut job)?;
                Ok(json!({"job":job}))
            }
            "service.status" | "service.result" => {
                let mut job = self.service_load(text(r, "jobId")?)?;
                if job["transport"] == "native" && !terminal(&job) {
                    let task = self
                        .workflow_action(json!({"action":"tasks.show","taskId":job["jobId"]}))
                        .await;
                    if let Ok(task) = task {
                        let task = &task["task"];
                        let s = task["state"].as_str().unwrap_or("proposed");
                        job["providerStatus"] = json!(s);
                        job["state"] = json!(match s {
                            "completed" => "completed",
                            "delivered" => "delivered",
                            "declined" => "failed",
                            "expired" => "expired",
                            "accepted" => "working",
                            "changes_requested" => "changes_requested",
                            _ => "submitted",
                        });
                        job["localReviewStatus"] = json!(match s {
                            "completed" => "approved",
                            "delivered" => "awaiting_review",
                            "changes_requested" => "changes_requested",
                            _ => "pending",
                        });
                        job["nativeTask"] = task.clone();
                        if s == "delivered" {
                            job["result"] = task["result"].clone();
                        }
                        let db = self.0.db.lock().unwrap();
                        save(&db, &mut job)?;
                    }
                }
                Ok(json!({"job":job}))
            }
            "service.list" | "service.pending" => {
                let offset = r["offset"].as_u64().unwrap_or(0).min(i64::MAX as u64) as i64;
                let rows = {
                    let db = self.0.db.lock().unwrap();
                    init(&db)?;
                    let mut q = db.prepare(
                        "SELECT data FROM service_jobs ORDER BY rowid LIMIT 33 OFFSET ?",
                    )?;
                    q.query_map([offset], |x| x.get::<_, String>(0))?
                        .collect::<std::result::Result<Vec<_>, _>>()?
                };
                let total = rows.len();
                let mut jobs = Vec::new();
                let mut scanned = 0;
                let mut bytes = 0;
                for raw in rows.into_iter().take(32) {
                    let job: Value = serde_json::from_str(&raw)?;
                    if bytes + raw.len() > 300000 && !jobs.is_empty() {
                        break;
                    }
                    scanned += 1;
                    if !r["transport"].is_null() && job["transport"] != r["transport"] {
                        continue;
                    }
                    if r["action"] == "service.pending"
                        && (terminal(&job) || job["state"] == "delivered")
                    {
                        continue;
                    }
                    bytes += raw.len();
                    jobs.push(job);
                }
                Ok(json!({"jobs":jobs,"hasMore":scanned<total,"nextOffset":offset+scanned as i64}))
            }
            "service.lookup" => {
                let wanted = field(r, "externalTaskId", 256)?;
                let db = self.0.db.lock().unwrap();
                init(&db)?;
                let raw:Option<String>=db.query_row("SELECT data FROM service_jobs WHERE json_extract(data,'$.externalTaskId')=? LIMIT 1",[wanted],|x|x.get(0)).optional()?;
                Ok(json!({"job":raw.map(|x|serde_json::from_str::<Value>(&x)).transpose()?}))
            }
            "service.cancel" => {
                let mut job = self.service_load(text(r, "jobId")?)?;
                ensure!(!terminal(&job), "service_job_terminal");
                job["cancelRequested"] = json!(true);
                job["cancelRequestedAt"] = json!(now());
                job["updated"] = json!(now());
                let not_dispatched = job["transport"] == "a2a" && job["state"] == "queued";
                if not_dispatched {
                    job["state"] = json!("canceled");
                    job["providerStatus"] = json!("canceled");
                }
                {
                    let db = self.0.db.lock().unwrap();
                    save(&db, &mut job)?;
                }
                if not_dispatched {
                    if let Some(id) = job["reservationId"].as_str() {
                        self.governance_complete(id, false)?;
                    }
                }
                Ok(
                    json!({"job":job,"guaranteedRollback":false,"note":"Cancellation is a request; external effects and existing native tasks are not undone"}),
                )
            }
            "service.update" => self.service_update(r),
            "service.review" => {
                let mut job = self.service_load(text(r, "jobId")?)?;
                ensure!(
                    job["state"] == "delivered",
                    "service_review_requires_delivery"
                );
                let approve = r["approve"]
                    .as_bool()
                    .context("service_review_approve_required")?;
                let note = r["text"].as_str().unwrap_or("Reviewed locally");
                ensure!(note.len() <= 8000, "service_text_too_large");
                if job["transport"] == "native" {
                    self.workflow_action(json!({"action":if approve {"tasks.approve"}else{"tasks.reject"},"taskId":job["jobId"],"text":note})).await?;
                }
                job["state"] = json!(if approve {
                    "completed"
                } else {
                    "changes_requested"
                });
                job["localReviewStatus"] = json!(if approve {
                    "approved"
                } else {
                    "changes_requested"
                });
                job["review"] = json!({"actor":self.id(),"at":now(),"text":note});
                let reservation = job["reservationId"].as_str().map(str::to_owned);
                {
                    let db = self.0.db.lock().unwrap();
                    save(&db, &mut job)?;
                }
                if approve {
                    if let Some(id) = reservation {
                        self.governance_complete(&id, true)?;
                    }
                }
                Ok(json!({"job":job}))
            }
            _ => bail!("service_unknown_action"),
        };
        if matches!(
            action,
            "service.invoke"
                | "service.dispatch"
                | "service.update"
                | "service.cancel"
                | "service.review"
        ) {
            if let Ok(value) = &result {
                self.governance_audit_event(action,json!({"jobId":value["job"]["jobId"],"state":value["job"]["state"],"idempotent":value["idempotent"],"projectPays":false}))?;
            }
        }
        result
    }

    async fn service_invoke(&self, r: &Value) -> Result<Value> {
        let key = field(r, "idempotencyKey", 200)?;
        let transport = field(r, "transport", 20)?;
        ensure!(
            matches!(transport.as_str(), "native" | "a2a"),
            "service_invalid_transport"
        );
        let capability = field(r, "capability", 256)?;
        ensure!(
            !r["input"].is_null() && serde_json::to_vec(&r["input"])?.len() <= 6000,
            "service_invalid_input"
        );
        ensure!(
            free_price(&r["declaredPrice"]),
            "service_free_only_price_unknown_or_paid"
        );
        let grant = field(r, "grantId", 128)?;
        let audience = field(r, "audience", 2048)?;
        if transport == "a2a" {
            ensure!(
                r["provider"] == audience,
                "service_provider_audience_mismatch"
            );
            self.governance_provider_check(&audience)?;
        } else {
            let session = self.active(&field(r, "sessionId", 80)?)?;
            ensure!(
                !session.temporary && session.peer_id.as_deref() == Some(audience.as_str()),
                "service_session_audience_mismatch"
            );
        }
        if !r["inputSchema"].is_null() {
            check_schema(&r["inputSchema"], &r["input"])?;
        }
        let cached_descriptor = if transport == "native" {
            self.service_cached_descriptor(&audience)?
        } else {
            None
        };
        if let Some(cached) = &cached_descriptor {
            if !r["descriptor"].is_null() {
                ensure!(
                    r["descriptor"]["payload"]["revision"].as_u64()
                        >= cached["payload"]["revision"].as_u64(),
                    "service_descriptor_revision_rollback"
                );
                if r["descriptor"]["payload"]["revision"] == cached["payload"]["revision"] {
                    ensure!(
                        r["descriptor"]["payload"] == cached["payload"],
                        "service_descriptor_revision_fork"
                    );
                }
            }
        }
        let advertised = if !r["descriptor"].is_null() {
            Some(r["descriptor"].clone())
        } else {
            cached_descriptor
        };
        if transport == "native" {
            ensure!(advertised.is_some(), "service_signed_descriptor_required");
        }
        if let Some(envelope) = advertised {
            let descriptor = super::network_identity::verify_native_descriptor(&envelope)?;
            ensure!(
                transport == "native" && descriptor["agentId"] == audience,
                "service_descriptor_provider_mismatch"
            );
            let skill = descriptor["skills"]
                .as_array()
                .context("service_invalid_descriptor")?
                .iter()
                .find(|s| s["skillId"] == capability)
                .context("service_capability_not_advertised")?;
            ensure!(
                !skill["inputSchema"].is_null(),
                "service_input_schema_required"
            );
            check_schema(&skill["inputSchema"], &r["input"])?;
            ensure!(
                free_price(&skill["declaredPrice"]),
                "service_descriptor_price_unknown_or_paid"
            );
        }
        let ttl = r["ttlSeconds"].as_i64().unwrap_or(3600);
        ensure!((1..=86400).contains(&ttl), "service_invalid_ttl");
        let mut normalized = r.clone();
        normalized
            .as_object_mut()
            .context("service_invalid_request")?
            .remove("action");
        let request_json = serde_json::to_string(&normalized)?;
        let existing = {
            let db = self.0.db.lock().unwrap();
            init(&db)?;
            db.query_row(
                "SELECT request_json,data FROM service_jobs WHERE idempotency_key=?",
                [&key],
                |x| Ok((x.get::<_, String>(0)?, x.get::<_, String>(1)?)),
            )
            .optional()?
        };
        if let Some((original, raw)) = existing {
            ensure!(original == request_json, "service_idempotency_conflict");
            return Ok(json!({"job":serde_json::from_str::<Value>(&raw)?,"idempotent":true}));
        }
        let auth =
            self.validate_service_grant(&grant, &audience, &format!("service:{capability}"))?;
        let job_id = uid();
        let digest = hex::encode(Sha256::digest(serde_jcs::to_vec(&r["input"])?));
        let mut job = json!({"v":1,"jobId":job_id,"requestId":job_id,"transport":transport,"provider":r["provider"],"audience":audience,"capability":capability,"sessionId":r["sessionId"],"input":r["input"],"inputDigest":digest,"grantId":grant,"deadline":now()+ttl*1000,"created":now(),"updated":now(),"idempotencyKey":key,"declaredPrice":r["declaredPrice"],"projectPays":false,"state":"authorizing","providerStatus":"queued","localReviewStatus":"pending","cancelRequested":false,"artifacts":[],"evidenceLevel":if transport=="native"{"native_task_protocol"}else{"local_adapter_observation"},"externalTaskId":r["externalTaskId"]});
        {
            let db = self.0.db.lock().unwrap();
            let count: i64 =
                db.query_row("SELECT COUNT(*) FROM service_jobs", [], |row| row.get(0))?;
            ensure!(count < 10000, "service_job_storage_limit");
            super::governance::storage_check(
                &db,
                serde_json::to_vec(&job)?.len() + request_json.len(),
            )?;
            let inserted=db.execute("INSERT OR IGNORE INTO service_jobs(job_id,idempotency_key,request_json,data) VALUES(?,?,?,?)",params![job_id,key,request_json,serde_json::to_string(&job)?])?;
            if inserted == 0 {
                let (original, raw): (String, String) = db.query_row(
                    "SELECT request_json,data FROM service_jobs WHERE idempotency_key=?",
                    [&key],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
                ensure!(original == request_json, "service_idempotency_conflict");
                return Ok(json!({"job":serde_json::from_str::<Value>(&raw)?,"idempotent":true}));
            }
        }
        let authorization = (|| -> Result<Value> {
            self.consume_service_grant(
                &grant,
                &audience,
                &format!("service:{capability}"),
                &job_id,
            )?;
            self.governance_admit(&audience, auth["ownerId"].as_str(), "execute", &job_id)
        })();
        match authorization {
            Ok(reservation) => {
                job["reservationId"] = reservation["reservationId"].clone();
                job["state"] = json!("queued");
            }
            Err(error) => {
                job["state"] = json!("failed");
                job["error"] = json!(error.to_string());
                let db = self.0.db.lock().unwrap();
                save(&db, &mut job)?;
                return Err(error);
            }
        }
        {
            let db = self.0.db.lock().unwrap();
            save(&db, &mut job)?;
        }
        if transport == "native" {
            let proposal=self.workflow_action(json!({"action":"tasks.propose","sessionId":field(r,"sessionId",80)?,"eventId":job_id,"title":format!("Service: {capability}"),"text":serde_json::to_string(&r["input"])? ,"ttlSeconds":ttl})).await;
            match proposal {
                Ok(result) => {
                    job["state"] = json!("submitted");
                    job["nativeTask"] = result["task"].clone();
                    job["delivery"] = result["delivery"].clone();
                }
                Err(error) => {
                    job["error"] = json!(error.to_string());
                    // A transport error after local commit is not proof that the
                    // proposal did not exist. Preserve the task and query it;
                    // never create a second side-effecting proposal.
                    if let Ok(task) = self
                        .workflow_action(json!({"action":"tasks.show","taskId":job_id}))
                        .await
                    {
                        job["state"] = json!("submitted");
                        job["nativeTask"] = task["task"].clone();
                        job["delivery"] = json!({"status":"unknown"});
                    } else {
                        job["state"] = json!("failed");
                        if let Some(id) = job["reservationId"].as_str() {
                            self.governance_complete(id, false)?;
                        }
                    }
                }
            }
            let db = self.0.db.lock().unwrap();
            save(&db, &mut job)?;
        }
        Ok(json!({"job":job,"idempotent":false,"executionStarted":false}))
    }

    fn service_update(&self, r: &Value) -> Result<Value> {
        let mut job = self.service_load(text(r, "jobId")?)?;
        ensure!(
            job["transport"] == "a2a",
            "service_native_state_uses_workflow"
        );
        ensure!(
            !terminal(&job) && job["localReviewStatus"] != "approved",
            "service_job_terminal"
        );
        ensure!(
            matches!(
                job["state"].as_str(),
                Some(
                    "dispatching"
                        | "submitted"
                        | "working"
                        | "input_required"
                        | "changes_requested"
                        | "delivered"
                )
            ),
            "service_dispatch_required"
        );
        let status = field(r, "providerStatus", 32)?;
        ensure!(
            matches!(
                status.as_str(),
                "submitted" | "working" | "input-required" | "completed" | "failed" | "canceled"
            ),
            "service_invalid_provider_status"
        );
        // A completed job can only be replaced after explicit requester review requesting changes.
        ensure!(
            job["state"] != "delivered" || status == "completed",
            "service_delivery_cannot_regress"
        );
        if let Some(progress) = r["progress"].as_u64() {
            ensure!(progress <= 100, "service_invalid_progress");
            job["progress"] = json!(progress);
        }
        if !r["text"].is_null() {
            let note = field(r, "text", 8000)?;
            job["result"] = json!({"text":note});
        }
        if !r["artifacts"].is_null() {
            job["artifacts"] = artifacts(&r["artifacts"])?;
        }
        if !r["externalTaskId"].is_null() {
            job["externalTaskId"] = json!(field(r, "externalTaskId", 256)?);
        }
        if !r["externalTask"].is_null() {
            ensure!(
                serde_json::to_vec(&r["externalTask"])?.len() <= 65536,
                "service_external_task_too_large"
            );
            job["externalTask"] = r["externalTask"].clone();
        }
        job["providerStatus"] = json!(status);
        job["state"] = json!(match status.as_str() {
            "completed" => "delivered",
            "input-required" => "input_required",
            s => s,
        });
        job["updated"] = json!(now());
        if status == "completed" {
            job["localReviewStatus"] = json!("awaiting_review");
        }
        {
            let db = self.0.db.lock().unwrap();
            save(&db, &mut job)?;
        }
        if matches!(status.as_str(), "failed" | "canceled") {
            if let Some(id) = job["reservationId"].as_str() {
                self.governance_complete(id, false)?;
            }
        }
        Ok(json!({"job":job}))
    }
}
