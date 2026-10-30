//! Local resource policy. Provider declarations are local attestations, never payment authorizations.
use super::*;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

fn defaults() -> Value {
    json!({"globalPerMinute":10000,"peerPerMinute":2000,"ownerPerMinute":4000,"dailyExecutions":200,"maxConversationTurns":20,"maxReserved":32,"maxAuditRows":10000,"maxStoredReservations":100000,"maxProviders":1000,"maxInbox":1000,"maxPolicyProposals":1000,"dailyDownloadBytes":67108864,"maxLocalStorageBytes":1073741824})
}
fn schema(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS governance_policy(id TEXT PRIMARY KEY,data TEXT NOT NULL);CREATE TABLE IF NOT EXISTS governance_reservations(id TEXT PRIMARY KEY,request_id TEXT UNIQUE NOT NULL,peer TEXT NOT NULL,owner TEXT,action TEXT NOT NULL,conversation TEXT NOT NULL,status TEXT NOT NULL,created INTEGER NOT NULL);CREATE INDEX IF NOT EXISTS governance_res_created ON governance_reservations(created);CREATE TABLE IF NOT EXISTS governance_audit(sequence INTEGER PRIMARY KEY AUTOINCREMENT,created INTEGER NOT NULL,event TEXT NOT NULL,data TEXT NOT NULL);CREATE TABLE IF NOT EXISTS governance_providers(origin TEXT PRIMARY KEY,data TEXT NOT NULL);CREATE TABLE IF NOT EXISTS governance_inbox(id TEXT PRIMARY KEY,data TEXT NOT NULL);CREATE TABLE IF NOT EXISTS governance_download(day INTEGER PRIMARY KEY,bytes INTEGER NOT NULL);")?;
    Ok(())
}
fn load_policy(db: &Connection) -> Result<Value> {
    let mut q = db.prepare("SELECT data FROM governance_policy WHERE id='active'")?;
    let data = q
        .query_map([], |r| r.get::<_, String>(0))?
        .next()
        .transpose()?;
    Ok(match data {
        Some(s) => serde_json::from_str(&s)?,
        None => defaults(),
    })
}
fn validate_policy(v: &Value) -> Result<Value> {
    ensure!(v.is_object(), "invalid_governance_policy");
    let mut p = defaults();
    for (key, value) in v.as_object().unwrap() {
        ensure!(p.get(key).is_some(), "invalid_governance_policy_field");
        let n = value.as_u64().context("invalid_governance_limit")?;
        let max = if key == "dailyDownloadBytes" || key == "maxLocalStorageBytes" {
            10_737_418_240
        } else {
            1_000_000
        };
        ensure!((1..=max).contains(&n), "invalid_governance_limit");
        p[key] = value.clone();
    }
    Ok(p)
}
/// Conservative SQLite/WAL estimate; call inside the write transaction after idempotency lookup.
pub(super) fn storage_check(db: &Connection, additional_bytes: usize) -> Result<()> {
    let page_count: i64 = db.pragma_query_value(None, "page_count", |r| r.get(0))?;
    let page_size: i64 = db.pragma_query_value(None, "page_size", |r| r.get(0))?;
    let name: String = db.query_row(
        "SELECT file FROM pragma_database_list WHERE name='main'",
        [],
        |r| r.get(0),
    )?;
    let disk = std::fs::metadata(&name).map(|m| m.len()).unwrap_or(0);
    let mut wal = std::ffi::OsString::from(&name);
    wal.push("-wal");
    let wal = std::fs::metadata(std::path::PathBuf::from(wal))
        .map(|m| m.len())
        .unwrap_or(0);
    let pages = (page_count as u64).saturating_mul(page_size as u64);
    let estimated = disk
        .max(pages)
        .saturating_add(wal)
        .saturating_add((additional_bytes as u64).saturating_mul(2))
        .saturating_add(16384);
    let has_policy: i64 = db.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='governance_policy'",
        [],
        |r| r.get(0),
    )?;
    let policy = if has_policy > 0 {
        load_policy(db)?
    } else {
        defaults()
    };
    let limit = policy["maxLocalStorageBytes"].as_u64().unwrap();
    ensure!(estimated <= limit, "governance_local_storage_limit");
    Ok(())
}
fn audit(db: &Connection, event: &str, data: Value) -> Result<()> {
    db.execute(
        "INSERT INTO governance_audit(created,event,data) VALUES(?1,?2,?3)",
        params![now(), event, data.to_string()],
    )?;
    let max = load_policy(db)?["maxAuditRows"].as_i64().unwrap_or(10000);
    db.execute("DELETE FROM governance_audit WHERE sequence <= COALESCE((SELECT MAX(sequence)-?1 FROM governance_audit),0)", [max])?;
    Ok(())
}
fn bounded(s: &str) -> Result<()> {
    ensure!(
        !s.is_empty() && s.len() <= 256,
        "invalid_governance_identifier"
    );
    Ok(())
}
fn origin(url: &str) -> Result<(reqwest::Url, String)> {
    ensure!(url.len() <= 4096, "invalid_http_url");
    let u = reqwest::Url::parse(url)?;
    ensure!(
        u.scheme() == "https"
            && u.username().is_empty()
            && u.password().is_none()
            && u.fragment().is_none()
            && u.host_str().is_some(),
        "http_https_required"
    );
    ensure!(u.port_or_known_default() == Some(443), "http_port_denied");
    let o = u.origin().ascii_serialization();
    Ok((u, o))
}
/// Checks the persisted operator attestation, not the truth of a provider's commercial terms.
pub(super) fn validate_relay_registration(db: &Connection, url: &str) -> Result<()> {
    let (u, o) = origin(url)?;
    ensure!(u.query().is_none(), "relay_credentials_query_denied");
    schema(db)?;
    let raw: String = db
        .query_row(
            "SELECT data FROM governance_providers WHERE origin=?1",
            [o],
            |r| r.get(0),
        )
        .context("relay_free_registration_required")?;
    let p: Value = serde_json::from_str(&raw)?;
    ensure!(
        p["status"] == "enabled"
            && p["free"] == true
            && p["termsAllowed"] == true
            && p["expiresAt"].as_i64().unwrap_or(0) > now()
            && p["purpose"] == "iroh-relay",
        "relay_free_registration_invalid"
    );
    Ok(())
}
fn public_v4(a: Ipv4Addr) -> bool {
    let [x, y, z, _] = a.octets();
    !(a.is_private()
        || a.is_loopback()
        || a.is_link_local()
        || a.is_multicast()
        || a.is_unspecified()
        || x == 0
        || x >= 240
        || (x == 100 && (64..=127).contains(&y))
        || (x == 192 && y == 0 && (z == 0 || z == 2))
        || (x == 192 && y == 88 && z == 99)
        || (x == 198 && (y == 18 || y == 19))
        || (x == 198 && y == 51 && z == 100)
        || (x == 203 && y == 0 && z == 113))
}
pub(crate) fn public_ip(a: IpAddr) -> bool {
    match a {
        IpAddr::V4(a) => public_v4(a),
        IpAddr::V6(a) => {
            if let Some(v4) = a.to_ipv4_mapped() {
                return public_v4(v4);
            }
            let s = a.segments();
            // Only ordinary global-unicast addresses; reject transition, documentation and special-use ranges.
            (s[0] & 0xe000) == 0x2000
                && !(s[0] == 0x2001 && (s[1] < 0x0200 || s[1] == 0x0db8))
                && s[0] != 0x2002
        }
    }
}

impl Core {
    pub(crate) fn governance_audit_event(&self, event: &str, data: Value) -> Result<()> {
        bounded(event)?;
        ensure!(
            serde_json::to_vec(&data)?.len() <= 8192,
            "audit_event_too_large"
        );
        let db = self.0.db.lock().unwrap();
        schema(&db)?;
        audit(&db, event, data)
    }
    pub fn governance_request(&self, v: &Value) -> Result<Value> {
        let op = v["op"]
            .as_str()
            .or_else(|| v["action"].as_str())
            .context("missing_action")?;
        if op == "provider.authorize" {
            return self.governance_provider_check(text(v, "url")?);
        }
        if op == "governance.download.charge" {
            let bytes = v["bytes"].as_u64().context("invalid_download_charge")?;
            ensure!(bytes <= 4 * 1024 * 1024, "invalid_download_charge");
            self.governance_charge_download(bytes as usize)?;
            return Ok(json!({"chargedBytes":bytes,"freeOnly":true}));
        }
        if op == "budget.reserve" {
            return self.governance_reserve(
                text(v, "peer")?,
                v["owner"].as_str(),
                v["resourceAction"]
                    .as_str()
                    .or_else(|| {
                        if v["op"].is_string() {
                            v["action"].as_str()
                        } else {
                            None
                        }
                    })
                    .unwrap_or("execute"),
                text(v, "requestId")?,
                v["conversationId"].as_str(),
            );
        }
        if op == "budget.commit" || op == "budget.release" {
            return self.governance_complete(text(v, "reservationId")?, op == "budget.commit");
        }
        let db = self.0.db.lock().unwrap();
        schema(&db)?;
        match op {
            "policy.inspect" => Ok(
                json!({"policy":load_policy(&db)?,"freeOnly":true,"automaticPaidFallback":false,"sponsoredCredentials":false}),
            ),
            "policy.set" => {
                let p = validate_policy(&v["policy"])?;
                db.execute(
                    "INSERT OR REPLACE INTO governance_policy VALUES('active',?1)",
                    [p.to_string()],
                )?;
                audit(&db, "policy.set", p.clone())?;
                Ok(p)
            }
            "policy.import" => {
                let p = validate_policy(&v["policy"])?;
                let total: i64 = db.query_row(
                    "SELECT COUNT(*) FROM governance_policy WHERE id!='active'",
                    [],
                    |r| r.get(0),
                )?;
                ensure!(
                    total < load_policy(&db)?["maxPolicyProposals"].as_i64().unwrap(),
                    "governance_policy_storage_limit"
                );
                let id = uid();
                db.execute(
                    "INSERT INTO governance_policy VALUES(?1,?2)",
                    params![id, p.to_string()],
                )?;
                audit(
                    &db,
                    "policy.proposed",
                    json!({"proposalId":id,"label":v["label"]}),
                )?;
                Ok(json!({"proposalId":id,"adopted":false}))
            }
            "policy.adopt" => {
                let id = text(v, "proposalId")?;
                ensure!(id != "active", "invalid_policy_proposal");
                let p: String = db.query_row(
                    "SELECT data FROM governance_policy WHERE id=?1",
                    [id],
                    |r| r.get(0),
                )?;
                db.execute(
                    "INSERT OR REPLACE INTO governance_policy VALUES('active',?1)",
                    [&p],
                )?;
                audit(&db, "policy.adopted", json!({"proposalId":id}))?;
                Ok(serde_json::from_str(&p)?)
            }
            "budget.inspect" => {
                let download: i64 = db.query_row(
                    "SELECT COALESCE(SUM(bytes),0) FROM governance_download WHERE day=?1",
                    [now() / 86400000],
                    |r| r.get(0),
                )?;
                let counts:i64=db.query_row("SELECT COUNT(*) FROM governance_reservations WHERE action='execute' AND created>=?1",[now()/86_400_000*86_400_000],|r|r.get(0))?;
                let active: i64 = db.query_row(
                    "SELECT COUNT(*) FROM governance_reservations WHERE status='reserved'",
                    [],
                    |r| r.get(0),
                )?;
                Ok(
                    json!({"policy":load_policy(&db)?,"dailyExecutions":counts,"dailyDownloadedBytes":download,"reserved":active,"currencyBudget":0,"freeOnly":true}),
                )
            }
            "audit.export" => {
                let after = v["after"].as_i64().unwrap_or(0).max(0);
                let limit = v["limit"].as_i64().unwrap_or(200).clamp(1, 1000);
                let mut q=db.prepare("SELECT sequence,created,event,data FROM governance_audit WHERE sequence>?1 ORDER BY sequence LIMIT ?2")?;
                let rows = q
                    .query_map(params![after, limit], |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, i64>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, String>(3)?,
                        ))
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                let events=rows.into_iter().map(|(s,t,e,d)|json!({"sequence":s,"created":t,"event":e,"data":serde_json::from_str::<Value>(&d).unwrap_or(Value::Null)})).collect::<Vec<_>>();
                Ok(
                    json!({"events":events,"retention":"bounded; sequence gaps indicate pruned history"}),
                )
            }
            "provider.add" => {
                let (_, o) = origin(text(v, "url")?)?;
                let total: i64 = db.query_row(
                    "SELECT COUNT(*) FROM governance_providers WHERE origin!=?1",
                    [&o],
                    |r| r.get(0),
                )?;
                ensure!(
                    total < load_policy(&db)?["maxProviders"].as_i64().unwrap(),
                    "governance_provider_storage_limit"
                );
                ensure!(
                    v["free"] == true && v["termsAllowed"] == true,
                    "provider_free_permission_required"
                );
                let (_, terms_origin) = origin(text(v, "termsUrl")?)?;
                let purpose = text(v, "purpose")?;
                bounded(purpose)?;
                let ttl = v["ttlSeconds"].as_i64().unwrap_or(86400);
                ensure!((60..=604800).contains(&ttl), "invalid_provider_ttl");
                let p = json!({"origin":o,"status":"enabled","free":true,"termsAllowed":true,"termsUrl":v["termsUrl"],"termsOrigin":terms_origin,"purpose":purpose,"checkedAt":now(),"expiresAt":now()+ttl*1000,"verifiedBy":"local_operator_attestation","automaticPaidFallback":false});
                db.execute(
                    "INSERT OR REPLACE INTO governance_providers VALUES(?1,?2)",
                    params![o, p.to_string()],
                )?;
                audit(&db, "provider.add", p.clone())?;
                Ok(p)
            }
            "provider.list" => {
                let mut q = db.prepare("SELECT data FROM governance_providers ORDER BY origin")?;
                let rows = q
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(
                    json!({"providers":rows.into_iter().map(|s|serde_json::from_str::<Value>(&s).unwrap_or(Value::Null)).collect::<Vec<_>>()}),
                )
            }
            "provider.check" => {
                let (_, o) = origin(text(v, "url")?)?;
                let s: String = db.query_row(
                    "SELECT data FROM governance_providers WHERE origin=?1",
                    [&o],
                    |r| r.get(0),
                )?;
                let mut p: Value = serde_json::from_str(&s)?;
                let status = v["statusCode"].as_u64().unwrap_or(200);
                if v["free"] == false || status == 402 || status == 429 {
                    p["status"] = json!("disabled");
                    p["reason"] = json!(if status == 429 {
                        "quota_exhausted"
                    } else {
                        "free_eligibility_lost"
                    });
                }
                p["lastStatusCode"] = json!(status);
                p["lastCheckedAt"] = json!(now());
                db.execute(
                    "UPDATE governance_providers SET data=?2 WHERE origin=?1",
                    params![o, p.to_string()],
                )?;
                audit(&db, "provider.check", p.clone())?;
                Ok(p)
            }
            "inbox.list" => {
                let mut q =
                    db.prepare("SELECT data FROM governance_inbox ORDER BY rowid DESC LIMIT 200")?;
                let rows = q
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(
                    json!({"requests":rows.into_iter().map(|s|serde_json::from_str::<Value>(&s).unwrap_or(Value::Null)).collect::<Vec<_>>()}),
                )
            }
            "inbox.decide" => {
                let id = text(v, "id")?;
                let decision = text(v, "decision")?;
                ensure!(
                    ["allow", "deny"].contains(&decision),
                    "invalid_inbox_decision"
                );
                let s: String =
                    db.query_row("SELECT data FROM governance_inbox WHERE id=?1", [id], |r| {
                        r.get(0)
                    })?;
                let mut p: Value = serde_json::from_str(&s)?;
                p["decision"] = json!(decision);
                p["decidedAt"] = json!(now());
                db.execute(
                    "UPDATE governance_inbox SET data=?2 WHERE id=?1",
                    params![id, p.to_string()],
                )?;
                audit(&db, "inbox.decided", p.clone())?;
                Ok(json!({"request":p,"executionAuthorized":false,"contactAdded":false}))
            }
            _ => bail!("unknown_governance_operation"),
        }
    }
    pub fn governance_admit(
        &self,
        peer: &str,
        owner: Option<&str>,
        action: &str,
        request_id: &str,
    ) -> Result<Value> {
        self.governance_reserve(peer, owner, action, request_id, None)
    }
    pub(crate) fn governance_storage_check(&self, additional_bytes: usize) -> Result<()> {
        let db = self.0.db.lock().unwrap();
        storage_check(&db, additional_bytes)
    }
    pub fn governance_contact_request(&self, remote: &str) -> Result<Value> {
        let known = self.list("contacts")?.iter().any(|c| c["id"] == remote);
        if known {
            return Ok(json!({"knownContact":true,"executionAuthorized":false}));
        }
        let admitted = self.governance_admit(
            remote,
            None,
            "contact.request",
            &format!("contact-request:{remote}"),
        )?;
        Ok(
            json!({"inboxId":admitted["reservationId"],"status":"pending","wake":false,"execute":false,"contactAdded":false}),
        )
    }
    pub fn governance_provider_check(&self, url: &str) -> Result<Value> {
        let (_, o) = origin(url)?;
        let db = self.0.db.lock().unwrap();
        schema(&db)?;
        let s: String = db
            .query_row(
                "SELECT data FROM governance_providers WHERE origin=?1",
                [&o],
                |r| r.get(0),
            )
            .context("provider_registration_required")?;
        let p: Value = serde_json::from_str(&s)?;
        ensure!(
            p["status"] == "enabled"
                && p["free"] == true
                && p["termsAllowed"] == true
                && p["expiresAt"].as_i64().unwrap_or(0) > now(),
            "provider_free_permission_expired"
        );
        Ok(p)
    }
    fn governance_reserve(
        &self,
        peer: &str,
        owner: Option<&str>,
        action: &str,
        request_id: &str,
        conversation: Option<&str>,
    ) -> Result<Value> {
        let result = self.governance_reserve_inner(peer, owner, action, request_id, conversation);
        if let Err(e) = &result {
            if let Ok(db) = self.0.db.lock() {
                if schema(&db).is_ok() {
                    let _ = audit(
                        &db,
                        "denied",
                        json!({"peer":peer.chars().take(256).collect::<String>(),"action":action.chars().take(64).collect::<String>(),"reason":e.to_string().chars().take(1024).collect::<String>()}),
                    );
                }
            }
        }
        result
    }
    fn governance_reserve_inner(
        &self,
        peer: &str,
        owner: Option<&str>,
        action: &str,
        request_id: &str,
        conversation: Option<&str>,
    ) -> Result<Value> {
        bounded(peer)?;
        bounded(request_id)?;
        bounded(action)?;
        if let Some(s) = owner {
            bounded(s)?;
        }
        if let Some(s) = conversation {
            bounded(s)?;
        }
        ensure!(
            ["message", "execute", "contact.request", "fetch"].contains(&action),
            "invalid_governance_action"
        );
        let mut db = self.0.db.lock().unwrap();
        schema(&db)?;
        let tx = db.transaction()?;
        let p = load_policy(&tx)?;
        // Cancelled futures/crashed adapters cannot retain every slot forever. Calls remain charged.
        tx.execute("UPDATE governance_reservations SET status='expired' WHERE status='reserved' AND created<?1",[now()-300000])?;
        // Only transient network accounting expires. Execution idempotency and conversation counters stay durable.
        tx.execute("DELETE FROM governance_reservations WHERE action IN ('message','fetch') AND status!='reserved' AND created<?1",[now()-120000])?;
        let existing = tx.query_row(
            "SELECT id,peer,action,status FROM governance_reservations WHERE request_id=?1",
            [request_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            },
        );
        if let Ok((id, oldpeer, oldaction, status)) = existing {
            ensure!(
                oldpeer == peer && oldaction == action,
                "governance_request_conflict"
            );
            return Ok(json!({"reservationId":id,"idempotent":true,"status":status}));
        }
        let stored: i64 =
            tx.query_row("SELECT COUNT(*) FROM governance_reservations", [], |r| {
                r.get(0)
            })?;
        ensure!(
            stored < p["maxStoredReservations"].as_i64().unwrap(),
            "governance_reservation_storage_limit"
        );
        if action == "contact.request" {
            let total: i64 =
                tx.query_row("SELECT COUNT(*) FROM governance_inbox", [], |r| r.get(0))?;
            ensure!(
                total < p["maxInbox"].as_i64().unwrap(),
                "governance_inbox_storage_limit"
            );
        }
        let minute = now() - 60000;
        for (sql, param, limit) in [
            (
                "SELECT COUNT(*) FROM governance_reservations WHERE created>=?1",
                "",
                p["globalPerMinute"].as_i64().unwrap(),
            ),
            (
                "SELECT COUNT(*) FROM governance_reservations WHERE created>=?1 AND peer=?2",
                peer,
                p["peerPerMinute"].as_i64().unwrap(),
            ),
            (
                "SELECT COUNT(*) FROM governance_reservations WHERE created>=?1 AND owner=?2",
                owner.unwrap_or(peer),
                p["ownerPerMinute"].as_i64().unwrap(),
            ),
        ] {
            let count: i64 = if param.is_empty() {
                tx.query_row(sql, [minute], |r| r.get(0))?
            } else {
                tx.query_row(sql, params![minute, param], |r| r.get(0))?
            };
            ensure!(count < limit, "governance_rate_limit");
        }
        let active: i64 = tx.query_row(
            "SELECT COUNT(*) FROM governance_reservations WHERE status='reserved'",
            [],
            |r| r.get(0),
        )?;
        ensure!(
            active < p["maxReserved"].as_i64().unwrap(),
            "governance_concurrency_limit"
        );
        if action == "execute" {
            let daily:i64=tx.query_row("SELECT COUNT(*) FROM governance_reservations WHERE action='execute' AND created>=?1",[now()/86400000*86400000],|r|r.get(0))?;
            ensure!(
                daily < p["dailyExecutions"].as_i64().unwrap(),
                "governance_daily_limit"
            );
            if let Some(c) = conversation {
                let turns:i64=tx.query_row("SELECT COUNT(*) FROM governance_reservations WHERE action='execute' AND conversation=?1",[c],|r|r.get(0))?;
                ensure!(
                    turns < p["maxConversationTurns"].as_i64().unwrap(),
                    "governance_turn_limit"
                );
            }
        }
        let id = uid();
        let state = if action == "message" || action == "contact.request" {
            "committed"
        } else {
            "reserved"
        };
        tx.execute(
            "INSERT INTO governance_reservations VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                id,
                request_id,
                peer,
                owner.unwrap_or(peer),
                action,
                conversation.unwrap_or(request_id),
                state,
                now()
            ],
        )?;
        if action == "contact.request" {
            let request = json!({"id":id,"peer":peer,"created":now(),"decision":"pending","wake":false,"execute":false});
            tx.execute(
                "INSERT INTO governance_inbox VALUES(?1,?2)",
                params![id, request.to_string()],
            )?;
        }
        audit(
            &tx,
            "admitted",
            json!({"reservationId":id,"peer":peer,"action":action,"requestId":request_id}),
        )?;
        tx.commit()?;
        Ok(
            json!({"reservationId":id,"status":state,"idempotent":false,"executionAuthorized":false}),
        )
    }
    pub fn governance_complete(&self, reservation: &str, success: bool) -> Result<Value> {
        let db = self.0.db.lock().unwrap();
        schema(&db)?;
        let n = db.execute(
            "UPDATE governance_reservations SET status=?2 WHERE id=?1 AND status='reserved'",
            params![reservation, if success { "committed" } else { "released" }],
        )?;
        ensure!(
            n > 0
                || db.query_row(
                    "SELECT COUNT(*) FROM governance_reservations WHERE id=?1",
                    [reservation],
                    |r| r.get::<_, i64>(0)
                )? > 0,
            "unknown_reservation"
        );
        audit(
            &db,
            "completed",
            json!({"reservationId":reservation,"success":success}),
        )?;
        Ok(json!({"reservationId":reservation,"completed":true,"countRefunded":false}))
    }
    pub(crate) fn governance_charge_download(&self, bytes: usize) -> Result<()> {
        let mut db = self.0.db.lock().unwrap();
        schema(&db)?;
        let tx = db.transaction()?;
        let day = now() / 86400000;
        tx.execute("DELETE FROM governance_download WHERE day<?1", [day - 7])?;
        tx.execute(
            "INSERT OR IGNORE INTO governance_download VALUES(?1,0)",
            [day],
        )?;
        let limit = load_policy(&tx)?["dailyDownloadBytes"].as_i64().unwrap();
        let used: i64 = tx.query_row(
            "SELECT bytes FROM governance_download WHERE day=?1",
            [day],
            |r| r.get(0),
        )?;
        ensure!(bytes <= i64::MAX as usize, "invalid_download_charge");
        ensure!(bytes != 0 || used < limit, "governance_download_budget");
        if bytes as i64 > limit - used {
            // Already-received over-budget chunks exhaust remaining allowance; repeated retries cannot reset it.
            tx.execute(
                "UPDATE governance_download SET bytes=?2 WHERE day=?1",
                params![day, limit],
            )?;
            tx.commit()?;
            bail!("governance_download_budget");
        }
        tx.execute(
            "UPDATE governance_download SET bytes=bytes+?2 WHERE day=?1",
            params![day, bytes as i64],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub async fn governance_http_get(
        &self,
        url: &str,
        etag: Option<&str>,
        last_modified: Option<&str>,
        max_bytes: usize,
    ) -> Result<Value> {
        ensure!(
            (1..=4 * 1024 * 1024).contains(&max_bytes),
            "invalid_http_body_limit"
        );
        let (u, o) = origin(url)?;
        self.governance_provider_check(url)?;
        self.governance_charge_download(0)?;
        let reservation = self.governance_admit(&o, None, "fetch", &uid())?;
        let rid = text(&reservation, "reservationId")?.to_string();
        let result:Result<Value>=async {
            let host=u.host_str().context("invalid_http_host")?.trim_matches(['[',']']).to_string();
            let addresses:Vec<SocketAddr>=timeout(Duration::from_secs(5),tokio::net::lookup_host((host.as_str(),443))).await.context("http_dns_timeout")??.collect();
            ensure!(!addresses.is_empty() && addresses.iter().all(|a|public_ip(a.ip())),"http_ssrf_denied");
            let client=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(15)).connect_timeout(Duration::from_secs(5)).resolve_to_addrs(&host,&addresses).build()?;
            let mut req=client.get(u).header("Accept","application/json, application/atom+xml, application/rss+xml, application/feed+json, application/xml, text/xml;q=0.9");
            if let Some(s)=etag {ensure!(s.len()<=1024,"invalid_http_etag");req=req.header("If-None-Match",s);} if let Some(s)=last_modified {ensure!(s.len()<=1024,"invalid_http_modified");req=req.header("If-Modified-Since",s);}
            let mut response=req.send().await?;let status=response.status().as_u16();
            if status==402 || status==429 {self.governance_request(&json!({"op":"provider.check","url":url,"statusCode":status}))?;bail!("provider_disabled_{status}");}
            ensure!((200..300).contains(&status)||status==304,"http_status_{status}");
            ensure!(response.content_length().unwrap_or(0)<=max_bytes as u64,"http_body_too_large");
            let etag=response.headers().get("etag").and_then(|v|v.to_str().ok()).map(str::to_string);let modified=response.headers().get("last-modified").and_then(|v|v.to_str().ok()).map(str::to_string);
            let mut body=Vec::new();while let Some(chunk)=response.chunk().await? {self.governance_charge_download(chunk.len())?;ensure!(body.len()+chunk.len()<=max_bytes,"http_body_too_large");body.extend_from_slice(&chunk);}
            Ok(json!({"status":status,"body":String::from_utf8(body).context("http_utf8_required")?,"etag":etag,"lastModified":modified,"origin":o,"provenance":"https_source_not_author_signature"}))
        }.await;
        self.governance_complete(&rid, result.is_ok())?;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_special_ip_ranges() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "169.254.169.254",
            "100.64.1.2",
            "192.0.2.2",
            "198.18.1.1",
            "203.0.113.1",
            "::1",
            "::ffff:127.0.0.1",
            "2001:db8::1",
            "2002:7f00:1::",
        ] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}");
        }
        assert!(public_ip("8.8.8.8".parse().unwrap()));
        assert!(public_ip("2606:4700:4700::1111".parse().unwrap()));
    }
    #[test]
    fn policy_and_url_validation() {
        assert!(validate_policy(&json!({"dailyExecutions":0})).is_err());
        assert!(validate_policy(&json!({"allowPaid":true})).is_err());
        assert!(origin("http://example.org").is_err());
        assert!(origin("https://user:secret@example.org").is_err());
        assert!(origin("https://example.org:8443").is_err());
        assert!(origin("https://example.org/path").is_ok());
    }
    #[test]
    fn relay_attestation_requires_exact_purpose_and_fresh_free_permission() {
        let db = Connection::open_in_memory().unwrap();
        schema(&db).unwrap();
        let mut p = json!({"status":"enabled","free":true,"termsAllowed":true,"expiresAt":now()+60000,"purpose":"directory"});
        db.execute(
            "INSERT INTO governance_providers VALUES('https://relay.example.org',?1)",
            [p.to_string()],
        )
        .unwrap();
        assert!(validate_relay_registration(&db, "https://relay.example.org").is_err());
        p["purpose"] = json!("iroh-relay");
        db.execute("UPDATE governance_providers SET data=?1", [p.to_string()])
            .unwrap();
        assert!(validate_relay_registration(&db, "https://relay.example.org").is_ok());
        assert!(
            validate_relay_registration(&db, "https://relay.example.org?token=secret").is_err()
        );
        assert!(validate_relay_registration(&db, "https://user:secret@relay.example.org").is_err());
        p["expiresAt"] = json!(now() - 1);
        db.execute("UPDATE governance_providers SET data=?1", [p.to_string()])
            .unwrap();
        assert!(validate_relay_registration(&db, "https://relay.example.org").is_err());
    }
    #[tokio::test]
    async fn download_budget_and_storage_limits_persist() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            directory: dir.path().into(),
            direct_only: true,
            relay_only: false,
            relay_url: None,
        };
        let core = Core::open(config.clone()).await.unwrap();
        core.governance_request(&json!({"action":"policy.set","policy":{"dailyDownloadBytes":10,"maxStoredReservations":2,"maxAuditRows":3,"maxInbox":1}})).unwrap();
        core.governance_charge_download(6).unwrap();
        core.governance_contact_request("stranger").unwrap();
        assert!(
            core.governance_contact_request("another-stranger")
                .unwrap_err()
                .to_string()
                .contains("inbox_storage")
        );
        let grant = core
            .governance_admit("friend", None, "execute", "one")
            .unwrap();
        core.governance_complete(grant["reservationId"].as_str().unwrap(), true)
            .unwrap();
        assert!(
            core.governance_admit("friend", None, "execute", "two")
                .unwrap_err()
                .to_string()
                .contains("reservation_storage")
        );
        let events = core
            .governance_request(&json!({"action":"audit.export"}))
            .unwrap();
        assert!(events["events"].as_array().unwrap().len() <= 3);
        core.close().await;
        drop(core);
        let core = Core::open(config).await.unwrap();
        core.governance_charge_download(4).unwrap();
        assert!(core.governance_charge_download(1).is_err());
        assert!(core.governance_charge_download(0).is_err());
        core.close().await;
    }
    #[tokio::test]
    async fn disk_budget_refuses_new_messages_but_keeps_idempotent_history_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            directory: dir.path().into(),
            direct_only: true,
            relay_only: false,
            relay_url: None,
        };
        let core = Core::open(config.clone()).await.unwrap();
        assert!(
            core.insert(
                "existing",
                "session",
                "author",
                "retained",
                "text",
                now(),
                "acked"
            )
            .unwrap()
        );
        core.governance_request(
            &json!({"action":"policy.set","policy":{"maxLocalStorageBytes":1}}),
        )
        .unwrap();
        assert!(
            !core
                .insert(
                    "existing",
                    "session",
                    "author",
                    "retained",
                    "text",
                    now(),
                    "acked"
                )
                .unwrap()
        );
        assert!(
            core.insert(
                "new",
                "session",
                "author",
                "rejected",
                "text",
                now(),
                "acked"
            )
            .unwrap_err()
            .to_string()
            .contains("local_storage_limit")
        );
        core.close().await;
        drop(core);
        let core = Core::open(config).await.unwrap();
        assert!(
            core.insert(
                "after-restart",
                "session",
                "author",
                "rejected",
                "text",
                now(),
                "acked"
            )
            .is_err()
        );
        let history = core.messages(Some("session"), 0, false).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0]["text"], "retained");
        core.close().await;
    }
}
