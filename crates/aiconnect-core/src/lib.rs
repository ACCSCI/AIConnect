//! Local-first authenticated agent messaging. No remote text is executed by this crate.
mod channels;
mod discovery;
mod governance;
mod groups;
mod network_identity;
mod services;
mod workflow;
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use iroh::{Endpoint, EndpointId, RelayMode, SecretKey, endpoint::presets};
use iroh_tickets::endpoint::EndpointTicket;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    str::FromStr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use tokio::{task::JoinHandle, time::timeout};

const ALPN: &[u8] = b"aiconnect/rust/1";
const MAX_PACKET: usize = 65536;
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
fn uid() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn permanent(error: &anyhow::Error) -> bool {
    let s = error.to_string();
    [
        "permission_denied",
        "contact_blocked",
        "session_ended",
        "session_expired",
        "temporary_text_only",
        "task_expired",
        "peer_not_allowed",
        "invalid_",
        "message_id_conflict",
        "workflow_",
        "group_",
    ]
    .iter()
    .any(|p| s.contains(p))
}
fn text<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v[k].as_str().with_context(|| format!("missing_{k}"))
}

#[derive(Clone, Debug)]
pub struct Config {
    pub directory: PathBuf,
    pub direct_only: bool,
    pub relay_only: bool,
    pub relay_url: Option<String>,
}
#[derive(Clone)]
pub struct Core(Arc<Inner>);
struct Inner {
    endpoint: Endpoint,
    db: Mutex<Connection>,
    receive_guard: Mutex<()>,
    direct_only: bool,
    configured_relay: Option<String>,
    relay_disabled: AtomicBool,
    closed: AtomicBool,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    inbound: Arc<tokio::sync::Semaphore>,
    group_delivery_slots: Arc<tokio::sync::Semaphore>,
    group_retry_slots: Arc<tokio::sync::Semaphore>,
    group_inflight: Mutex<std::collections::HashSet<(String, String)>>,
    last_path: Mutex<Value>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    id: String,
    label: String,
    host: String,
    capability: String,
    expires: i64,
    role: String,
    status: String,
    peer_id: Option<String>,
    peer_ticket: Option<String>,
    temporary: bool,
}
impl Session {
    fn public(&self) -> Value {
        json!({"id":self.id,"label":self.label,"host":self.host,"expires":self.expires,"role":self.role,"status":self.status,"peerId":self.peer_id,"temporary":self.temporary})
    }
}

impl Core {
    pub async fn open(config: Config) -> Result<Self> {
        ensure!(
            !(config.direct_only && config.relay_only),
            "incompatible_transport_modes"
        );
        ensure!(
            !config.direct_only || config.relay_url.is_none(),
            "direct_only_conflicts_with_custom_relay"
        );
        std::fs::create_dir_all(&config.directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&config.directory, std::fs::Permissions::from_mode(0o700))?;
        }
        let db = Connection::open(config.directory.join("local.sqlite"))?;
        let schema_version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
        ensure!(
            matches!(schema_version, 0 | 300),
            "incompatible_profile_use_new_data_directory"
        );
        let message_table: i64 = db.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='messages'",
            [],
            |row| row.get(0),
        )?;
        if message_table > 0 {
            let mut query = db.prepare("PRAGMA table_info(messages)")?;
            let columns = query
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            ensure!(
                columns.iter().any(|s| s == "sequence") && columns.iter().any(|s| s == "kind"),
                "incompatible_profile_use_new_data_directory"
            );
        }
        let key_path = config.directory.join("identity.key");
        let key = match std::fs::read(&key_path) {
            Ok(b) => SecretKey::from_bytes(
                &b.try_into()
                    .map_err(|_| anyhow::anyhow!("invalid_identity_key"))?,
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let k = SecretKey::generate();
                use std::io::Write;
                let mut options = std::fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut f = options.open(&key_path)?;
                f.write_all(&k.to_bytes())?;
                f.sync_all()?;
                k
            }
            Err(e) => return Err(e.into()),
        };
        db.execute_batch("PRAGMA journal_mode=WAL;PRAGMA synchronous=FULL;PRAGMA busy_timeout=3000;CREATE TABLE IF NOT EXISTS settings(id TEXT PRIMARY KEY,data TEXT NOT NULL);CREATE TABLE IF NOT EXISTS sessions(id TEXT PRIMARY KEY,data TEXT NOT NULL);CREATE TABLE IF NOT EXISTS contacts(id TEXT PRIMARY KEY,data TEXT NOT NULL);CREATE TABLE IF NOT EXISTS messages(sequence INTEGER PRIMARY KEY AUTOINCREMENT,id TEXT UNIQUE NOT NULL,session_id TEXT NOT NULL,sender TEXT NOT NULL,text TEXT NOT NULL,kind TEXT NOT NULL,created INTEGER NOT NULL,status TEXT NOT NULL);CREATE INDEX IF NOT EXISTS messages_session ON messages(session_id,sequence);")?;
        db.pragma_update(None, "user_version", 300)?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS workflow_tasks(id TEXT PRIMARY KEY,data TEXT NOT NULL);CREATE TABLE IF NOT EXISTS chat_groups(id TEXT PRIMARY KEY,data TEXT NOT NULL);CREATE TABLE IF NOT EXISTS group_invites(id TEXT PRIMARY KEY,data TEXT NOT NULL);CREATE TABLE IF NOT EXISTS group_messages(sequence INTEGER PRIMARY KEY AUTOINCREMENT,id TEXT UNIQUE NOT NULL,group_id TEXT NOT NULL,sender TEXT NOT NULL,text TEXT NOT NULL,created INTEGER NOT NULL);CREATE TABLE IF NOT EXISTS group_outbox(message_id TEXT NOT NULL,recipient TEXT NOT NULL,status TEXT NOT NULL,error TEXT,PRIMARY KEY(message_id,recipient));",
        )?;
        // Additive queue scheduling migration; retain all existing history and delivery states.
        let columns = {
            let mut q = db.prepare("PRAGMA table_info(group_outbox)")?;
            q.query_map([], |r| r.get::<_, String>(1))?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        for column in ["attempts", "last_attempt", "retry_after"] {
            if !columns.iter().any(|c| c == column) {
                db.execute_batch(&format!(
                    "ALTER TABLE group_outbox ADD COLUMN {column} INTEGER NOT NULL DEFAULT 0"
                ))?;
            }
        }
        db.execute_batch("CREATE TABLE IF NOT EXISTS group_snapshots(reference TEXT PRIMARY KEY,group_id TEXT NOT NULL,data TEXT NOT NULL);CREATE TABLE IF NOT EXISTS group_records(sequence INTEGER PRIMARY KEY AUTOINCREMENT,id TEXT UNIQUE NOT NULL,group_id TEXT NOT NULL,sender TEXT NOT NULL,author_sequence INTEGER NOT NULL,reference TEXT UNIQUE NOT NULL,state_ref TEXT NOT NULL,epoch INTEGER NOT NULL,data TEXT NOT NULL,UNIQUE(group_id,sender,author_sequence));CREATE INDEX IF NOT EXISTS group_records_group ON group_records(group_id,sequence);CREATE TABLE IF NOT EXISTS group_replica_cursors(group_id TEXT NOT NULL,peer TEXT NOT NULL,cursor INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(group_id,peer));CREATE TABLE IF NOT EXISTS group_peer_cards(group_id TEXT NOT NULL,peer TEXT NOT NULL,data TEXT NOT NULL,PRIMARY KEY(group_id,peer));")?;
        if let Some(url) = config.relay_url.as_deref() {
            governance::validate_relay_registration(&db, url)?;
        }
        let builder = if config.direct_only {
            Endpoint::builder(presets::Minimal)
        } else {
            Endpoint::builder(presets::N0)
        };
        let mut builder = builder.secret_key(key).alpns(vec![ALPN.to_vec()]);
        let configured_relay = config.relay_url.clone();
        if let Some(url) = config.relay_url {
            builder = builder.relay_mode(RelayMode::custom([url.parse()?]));
        }
        let port_path = config.directory.join("endpoint-port");
        if let Ok(port) = std::fs::read_to_string(&port_path) {
            let port: u16 = port.trim().parse().context("invalid_endpoint_port")?;
            builder = builder.bind_addr(std::net::SocketAddrV4::new(
                std::net::Ipv4Addr::UNSPECIFIED,
                port,
            ))?;
        }
        // Explicit local-test mode avoids opening a listener on LAN/public
        // interfaces (and repeated Windows Firewall prompts for test binaries).
        // Normal application and relay behavior is unchanged.
        if config.direct_only && std::env::var("AICONNECT_TEST_LOOPBACK").as_deref() == Ok("1") {
            let port = std::fs::read_to_string(&port_path)
                .ok()
                .and_then(|p| p.trim().parse::<u16>().ok())
                .unwrap_or(0);
            builder = builder
                .clear_ip_transports()
                .bind_addr(std::net::SocketAddrV4::new(
                    std::net::Ipv4Addr::LOCALHOST,
                    port,
                ))?;
        }
        if config.relay_only {
            builder = builder.clear_ip_transports();
        }
        let endpoint = builder.bind().await?;
        if let Some(addr) = endpoint.bound_sockets().iter().find(|a| a.is_ipv4()) {
            std::fs::write(&port_path, addr.port().to_string())?;
        }
        let core = Self(Arc::new(Inner {
            endpoint,
            db: Mutex::new(db),
            receive_guard: Mutex::new(()),
            direct_only: config.direct_only,
            configured_relay,
            relay_disabled: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            tasks: Mutex::new(vec![]),
            inbound: Arc::new(tokio::sync::Semaphore::new(32)),
            group_delivery_slots: Arc::new(tokio::sync::Semaphore::new(4)),
            group_retry_slots: Arc::new(tokio::sync::Semaphore::new(1)),
            group_inflight: Mutex::new(std::collections::HashSet::new()),
            last_path: Mutex::new(Value::Null),
        }));
        let weak = Arc::downgrade(&core.0);
        let endpoint = core.0.endpoint.clone();
        let accept = tokio::spawn(async move {
            while let Some(incoming) = endpoint.accept().await {
                let Some(inner) = weak.upgrade() else { break };
                let c = Core(inner);
                // Apply bounded backpressure rather than refusing legitimate bursts.
                // Only one accepted incoming waits here; active handlers remain capped at 32.
                let Ok(permit) = c.0.inbound.clone().acquire_owned().await else {
                    break;
                };
                tokio::spawn(async move {
                    let _permit = permit;
                    let _ = timeout(Duration::from_secs(15), async {
                        let conn = incoming.await?;
                        c.handle(conn).await
                    })
                    .await;
                });
            }
        });
        let weak = Arc::downgrade(&core.0);
        let retry = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let Some(inner) = weak.upgrade() else { break };
                if inner.closed.load(Ordering::Relaxed) {
                    break;
                }
                let _ = Core(inner).retry_messages().await;
            }
        });
        let weak = Arc::downgrade(&core.0);
        let replication = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let Some(inner) = weak.upgrade() else { break };
                if inner.closed.load(Ordering::Relaxed) {
                    break;
                }
                let _ = Core(inner).group_replication_tick().await;
            }
        });
        let weak = Arc::downgrade(&core.0);
        let relay_policy = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let Some(inner) = weak.upgrade() else { break };
                if inner.closed.load(Ordering::Relaxed) {
                    break;
                }
                let _ = Core(inner).check_transport_policy().await;
            }
        });
        let weak = Arc::downgrade(&core.0);
        let channels = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let Some(inner) = weak.upgrade() else { break };
                if inner.closed.load(Ordering::Relaxed) {
                    break;
                }
                let _ = Core(inner).channel_tick().await;
            }
        });
        let weak = Arc::downgrade(&core.0);
        let group_retry = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(250)).await;
                let Some(inner) = weak.upgrade() else { break };
                if inner.closed.load(Ordering::Relaxed) {
                    break;
                }
                let _ = Core(inner).group_retry(false).await;
            }
        });
        core.0.tasks.lock().unwrap().extend([
            accept,
            retry,
            group_retry,
            replication,
            channels,
            relay_policy,
        ]);
        Ok(core)
    }
    pub fn id(&self) -> String {
        self.0.endpoint.id().to_string()
    }
    pub async fn close(&self) {
        if self.0.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.discovery_shutdown();
        let tasks = self.0.tasks.lock().unwrap().drain(..).collect::<Vec<_>>();
        for task in &tasks {
            task.abort();
        }
        for task in tasks {
            let _ = task.await;
        }
        self.0.endpoint.close().await;
    }
    async fn check_transport_policy(&self) -> Result<()> {
        if let Some(url) = &self.0.configured_relay {
            let valid = {
                let db = self.0.db.lock().unwrap();
                governance::validate_relay_registration(&db, url)
            };
            if valid.is_err() && !self.0.relay_disabled.swap(true, Ordering::SeqCst) {
                if let Ok(relay) = url.parse() {
                    self.0.endpoint.remove_relay(&relay).await;
                }
            }
            ensure!(
                !self.0.relay_disabled.load(Ordering::SeqCst),
                "relay_free_permission_lost_restart_direct_or_reauthorize"
            );
            valid?;
        }
        Ok(())
    }
    async fn ticket(&self) -> String {
        if !self.0.direct_only {
            let _ = timeout(Duration::from_secs(6), self.0.endpoint.online()).await;
        }
        EndpointTicket::new(self.0.endpoint.addr()).to_string()
    }
    fn load(&self, table: &str, id: &str) -> Result<Value> {
        let db = self.0.db.lock().unwrap();
        let r = db.query_row(&format!("SELECT data FROM {table} WHERE id=?"), [id], |r| {
            r.get::<_, String>(0)
        });
        match r {
            Ok(s) => Ok(serde_json::from_str(&s)?),
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                bail!("{}_not_found", table.trim_end_matches('s'))
            }
            Err(e) => Err(e.into()),
        }
    }
    fn list(&self, table: &str) -> Result<Vec<Value>> {
        let db = self.0.db.lock().unwrap();
        let mut q = db.prepare(&format!("SELECT data FROM {table} ORDER BY rowid"))?;
        let rows = q.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }
    fn put(&self, table: &str, id: &str, value: &Value) -> Result<()> {
        self.0.db.lock().unwrap().execute(&format!("INSERT INTO {table}(id,data) VALUES(?,?) ON CONFLICT(id) DO UPDATE SET data=excluded.data"),params![id,value.to_string()])?;
        Ok(())
    }
    fn session(&self, id: &str) -> Result<Session> {
        Ok(serde_json::from_value(self.load("sessions", id)?)?)
    }
    fn save_session(&self, s: &Session) -> Result<()> {
        self.put("sessions", &s.id, &serde_json::to_value(s)?)?;
        if matches!(s.status.as_str(), "ended" | "expired") {
            self.cancel_pending(&s.id, false)?;
        }
        Ok(())
    }
    fn cancel_pending(&self, session_id: &str, tasks_only: bool) -> Result<()> {
        self.0.db.lock().unwrap().execute(
            "UPDATE messages SET status='rejected' WHERE session_id=? AND status='pending' AND (?=0 OR kind='task')",
            params![session_id, tasks_only as i32],
        )?;
        Ok(())
    }
    fn active(&self, id: &str) -> Result<Session> {
        let mut s = self.session(id)?;
        if s.expires > 0 && s.expires <= now() {
            s.status = "expired".into();
            self.save_session(&s)?;
            bail!("session_expired")
        }
        ensure!(
            matches!(s.status.as_str(), "open" | "waiting"),
            "session_ended"
        );
        if let Some(peer) = &s.peer_id {
            if !s.temporary {
                self.load("contacts", peer)?;
            }
            self.allowed(peer, "chat")?;
        }
        Ok(s)
    }
    fn allowed(&self, id: &str, permission: &str) -> Result<()> {
        if let Ok(c) = self.load("contacts", id) {
            ensure!(c["blocked"] != true, "contact_blocked");
            ensure!(
                c["policy"][permission]
                    .as_bool()
                    .unwrap_or(permission == "chat"),
                "permission_denied"
            );
        } else {
            ensure!(permission == "chat", "permission_denied")
        }
        Ok(())
    }
    fn signed(&self, body: Value) -> Result<Value> {
        let raw = serde_json::to_vec(&body)?;
        let signature = self.0.endpoint.secret_key().sign(&raw);
        Ok(json!({"body":B64.encode(&raw),"signature":B64.encode(signature.to_bytes())}))
    }
    fn verify(envelope: &Value) -> Result<Value> {
        Self::verify_bounded(envelope, 24000)
    }
    fn verify_bounded(envelope: &Value, limit: usize) -> Result<Value> {
        let raw = B64.decode(text(envelope, "body")?)?;
        ensure!(raw.len() < limit, "invalid_signed_payload");
        let body: Value = serde_json::from_slice(&raw)?;
        let id = EndpointId::from_str(text(&body, "id")?)?;
        let sig_bytes: [u8; 64] = B64
            .decode(text(envelope, "signature")?)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid_signature"))?;
        id.verify(&raw, &iroh::Signature::from_bytes(&sig_bytes))?;
        Ok(body)
    }
    pub async fn dispatch(&self, r: Value) -> Result<Value> {
        let action = text(&r, "action")?;
        if action == "owner.domain-verify" {
            return self.network_domain_verify(&r).await;
        }
        if ["owner.", "profile.", "capability."]
            .iter()
            .any(|p| action.starts_with(p))
        {
            if action == "capability.publish" {
                self.governance_storage_check(serde_json::to_vec(&r)?.len())?;
            }
            return self.network_identity_request(&r);
        }
        if action == "discover"
            || ["directory.", "discovery."]
                .iter()
                .any(|p| action.starts_with(p))
        {
            return self.discovery_request(&r).await;
        }
        if action.starts_with("channels.") {
            return self.channel_request(&r).await;
        }
        if action.starts_with("service.") {
            return self.service_request(&r).await;
        }
        if action == "governance.http.get" {
            return self
                .governance_http_get(
                    text(&r, "url")?,
                    r["etag"].as_str(),
                    r["lastModified"].as_str(),
                    r["maxBytes"].as_u64().unwrap_or(262144).min(1048576) as usize,
                )
                .await;
        }
        if ["policy.", "budget.", "audit.", "provider.", "governance."]
            .iter()
            .any(|p| action.starts_with(p))
            || matches!(action, "inbox.list" | "inbox.decide")
        {
            let result = self.governance_request(&r);
            if action == "provider.check" {
                let _ = self.check_transport_policy().await;
            }
            return result;
        }
        if r["action"]
            .as_str()
            .is_some_and(|a| a.starts_with("groups."))
        {
            return self.group_action(r).await;
        }
        if r["action"]
            .as_str()
            .is_some_and(|a| a.starts_with("tasks."))
        {
            return self.workflow_action(r).await;
        }
        match text(&r, "action")? {
            "network.diagnostics" => Ok(json!({
                "directOnly":self.0.direct_only,
                "boundSockets":self.0.endpoint.bound_sockets().iter().map(|a| a.to_string()).collect::<Vec<_>>(),
                "lastObservedPath":self.0.last_path.lock().unwrap().clone(),
                "freeProductionRelayVerified":false,"automaticPaidFallback":false,
                "configuredRelayDisabled":self.0.relay_disabled.load(Ordering::SeqCst),
                "reachability":"not_proven_by_local_diagnostics"
            })),
            "network.probe" => {
                let peer = text(&r, "peer")?;
                let contact = self.load("contacts", peer)?;
                self.allowed(peer, "chat")?;
                let response = timeout(
                    Duration::from_secs(5),
                    self.rpc(
                        text(&contact, "ticket")?,
                        json!({"kind":"network.ping"}),
                        peer,
                    ),
                )
                .await
                .context("network_probe_timeout")??;
                Ok(
                    json!({"peer":peer,"reachable":true,"response":response,"path":self.0.last_path.lock().unwrap().clone(),"scope":"this_peer_at_this_time_not_global_nat_coverage"}),
                )
            }
            "status" => Ok(
                json!({"id":self.id(),"version":env!("CARGO_PKG_VERSION"),"directOnly":self.0.direct_only,"lastPath":self.0.last_path.lock().unwrap().clone()}),
            ),
            "identity" => {
                let mut profile = self
                    .load("settings", "profile")
                    .unwrap_or(json!({"name":"Agent","owner":""}));
                for k in ["name", "owner"] {
                    if let Some(value) = r[k].as_str() {
                        ensure!(value.chars().count() <= 80, "invalid_profile");
                        profile[k] = json!(value);
                    }
                }
                self.put("settings", "profile", &profile)?;
                self.signed(json!({"v":1,"kind":"contact","id":self.id(),"name":profile["name"],"owner":profile["owner"],"ownerVerified":false,"ticket":self.ticket().await}))
            }
            "contacts.list" => Ok(json!({"contacts":self.list("contacts")?})),
            "contacts.request" => {
                let card = Self::verify(&r["card"]).context("invalid_contact_signature")?;
                ensure!(card["v"] == 1 && card["kind"] == "contact", "invalid_card");
                let peer = text(&card, "id")?;
                ensure!(peer != self.id(), "cannot_request_self");
                self.allowed(peer, "chat")?;
                self.rpc(
                    text(&card, "ticket")?,
                    json!({"kind":"contact.request"}),
                    peer,
                )
                .await
            }
            "contacts.import" => {
                let _serial = self.0.receive_guard.lock().unwrap();
                let body = Self::verify(&r["card"]).context("invalid_contact_signature")?;
                ensure!(body["v"] == 1 && body["kind"] == "contact", "invalid_card");
                let id = text(&body, "id")?;
                ensure!(id != self.id(), "cannot_import_self");
                let ticket = EndpointTicket::from_str(text(&body, "ticket")?)?;
                ensure!(
                    ticket.endpoint_addr().id.to_string() == id,
                    "card_identity_mismatch"
                );
                let old = self.load("contacts", id).ok();
                let c = json!({"id":id,"name":r["name"].as_str().or(body["name"].as_str()).unwrap_or("Agent"),"owner":body["owner"],"ownerVerified":false,"group":r["group"].as_str().unwrap_or(""),"card":r["card"],"ticket":body["ticket"],"blocked":old.as_ref().map(|v|v["blocked"].clone()).unwrap_or(json!(false)),"policy":old.as_ref().map(|v|v["policy"].clone()).unwrap_or(json!({"chat":true,"wake":false,"task":false}))});
                self.put("contacts", id, &c)?;
                Ok(c)
            }
            "contacts.export" => Ok(self.load("contacts", text(&r, "id")?)?["card"].clone()),
            "contacts.remove" => {
                let _serial = self.0.receive_guard.lock().unwrap();
                let id = text(&r, "id")?;
                for v in self.list("sessions")? {
                    let mut s: Session = serde_json::from_value(v)?;
                    if s.peer_id.as_deref() == Some(id) {
                        s.status = "ended".into();
                        self.save_session(&s)?;
                    }
                }
                self.0
                    .db
                    .lock()
                    .unwrap()
                    .execute("DELETE FROM contacts WHERE id=?", [id])?;
                Ok(json!({"removed":true}))
            }
            "contacts.block" | "contacts.policy" | "contacts.group" => {
                let _serial = self.0.receive_guard.lock().unwrap();
                let id = text(&r, "id")?;
                let mut c = self.load("contacts", id)?;
                match r["action"].as_str().unwrap() {
                    "contacts.block" => {
                        c["blocked"] = json!(r["blocked"].as_bool().unwrap_or(true))
                    }
                    "contacts.group" => c["group"] = json!(text(&r, "group")?),
                    _ => {
                        for p in ["chat", "wake", "task"] {
                            if let Some(v) = r[p].as_bool() {
                                c["policy"][p] = json!(v);
                            }
                        }
                    }
                }
                self.put("contacts", id, &c)?;
                if c["blocked"] == true || c["policy"]["chat"] == false {
                    for value in self.list("sessions")? {
                        let session: Session = serde_json::from_value(value)?;
                        if session.peer_id.as_deref() == Some(id) {
                            self.cancel_pending(&session.id, false)?;
                        }
                    }
                }
                Ok(c)
            }
            "temp.create" => {
                let ttl = r["ttlSeconds"].as_u64().unwrap_or(1800);
                ensure!((1..=86400).contains(&ttl), "invalid_ttl");
                let label = r["label"].as_str().unwrap_or("Temporary");
                ensure!(label.chars().count() <= 80, "invalid_label");
                let s = Session {
                    id: uid(),
                    label: label.into(),
                    host: self.id(),
                    capability: B64.encode(rand::random::<[u8; 32]>()),
                    expires: now() + ttl as i64 * 1000,
                    role: "host".into(),
                    status: "waiting".into(),
                    peer_id: None,
                    peer_ticket: None,
                    temporary: true,
                };
                let envelope=self.signed(json!({"id":self.id(),"v":1,"kind":"temporary","sessionId":s.id,"label":s.label,"capability":s.capability,"expires":s.expires,"ticket":self.ticket().await}))?;
                let invite = B64.encode(serde_json::to_vec(&envelope)?);
                self.save_session(&s)?;
                Ok(json!({"session":s.public(),"invite":invite}))
            }
            "temp.join" => {
                let invite = text(&r, "invite")?;
                ensure!(invite.len() <= 16000, "invalid_invite");
                let envelope: Value = serde_json::from_slice(&B64.decode(invite)?)?;
                let b = Self::verify(&envelope).context("invalid_invite_signature")?;
                ensure!(b["v"] == 1 && b["kind"] == "temporary", "invalid_invite");
                let expires = b["expires"].as_i64().context("invalid_expiry")?;
                ensure!(expires > now(), "invite_expired");
                let peer = text(&b, "id")?;
                ensure!(peer != self.id(), "cannot_join_self");
                self.allowed(peer, "chat")?;
                let id = text(&b, "sessionId")?;
                if let Ok(old) = self.session(id) {
                    ensure!(old.status == "open", "session_ended");
                }
                let ticket = text(&b, "ticket")?;
                let response=self.rpc(ticket,json!({"kind":"join","sessionId":id,"capability":b["capability"],"ticket":self.ticket().await}),peer).await?;
                ensure!(response["status"] == "open", "join_rejected");
                let s = Session {
                    id: id.into(),
                    label: b["label"].as_str().unwrap_or("Temporary").into(),
                    host: peer.into(),
                    capability: text(&b, "capability")?.into(),
                    expires,
                    role: "guest".into(),
                    status: "open".into(),
                    peer_id: Some(peer.into()),
                    peer_ticket: Some(ticket.into()),
                    temporary: true,
                };
                self.save_session(&s)?;
                Ok(json!({"session":s.public()}))
            }
            "connect" => {
                let peer = text(&r, "id")?;
                self.allowed(peer, "chat")?;
                let c = self.load("contacts", peer)?;
                let id = uid();
                let capability = B64.encode(rand::random::<[u8; 32]>());
                let s = Session {
                    id: id.clone(),
                    label: c["name"].as_str().unwrap_or("Contact").into(),
                    host: self.id(),
                    capability: capability.clone(),
                    expires: 0,
                    role: "host".into(),
                    status: "open".into(),
                    peer_id: Some(peer.into()),
                    peer_ticket: Some(text(&c, "ticket")?.into()),
                    temporary: false,
                };
                self.rpc(text(&c,"ticket")?,json!({"kind":"contact.connect","sessionId":id,"capability":capability,"ticket":self.ticket().await}),peer).await?;
                {
                    let _serial = self.0.receive_guard.lock().unwrap();
                    self.load("contacts", peer)?;
                    self.allowed(peer, "chat")?;
                    self.save_session(&s)?;
                }
                Ok(json!({"session":s.public()}))
            }
            "sessions" => {
                let mut sessions = vec![];
                for v in self.list("sessions")? {
                    let mut s: Session = serde_json::from_value(v)?;
                    if s.expires > 0
                        && s.expires <= now()
                        && matches!(s.status.as_str(), "waiting" | "open")
                    {
                        s.status = "expired".into();
                        self.save_session(&s)?;
                    }
                    sessions.push(s.public());
                }
                Ok(json!({"sessions":sessions}))
            }
            "send" => self.send(&r).await,
            "messages" => {
                let after = r["after"].as_i64().unwrap_or(0);
                Self::page(
                    self.messages(Some(text(&r, "sessionId")?), after, false)?,
                    after,
                )
            }
            "inbox" => {
                let after = r["after"].as_i64().unwrap_or(0);
                Self::page(self.messages(None, after, true)?, after)
            }
            "inbox.cursor" => {
                let cursor: i64 = self.0.db.lock().unwrap().query_row(
                    "SELECT COALESCE(MAX(sequence),0) FROM messages WHERE sender<>?",
                    [self.id()],
                    |row| row.get(0),
                )?;
                Ok(json!({"cursor":cursor}))
            }
            "session.close" => {
                let s = {
                    let _serial = self.0.receive_guard.lock().unwrap();
                    let mut s = self.session(text(&r, "sessionId")?)?;
                    s.status = "ended".into();
                    self.save_session(&s)?;
                    s
                };
                if let (Some(ticket), Some(peer)) = (&s.peer_ticket, &s.peer_id) {
                    let _ = self
                        .rpc(
                            ticket,
                            json!({"kind":"end","sessionId":s.id,"capability":s.capability}),
                            peer,
                        )
                        .await;
                }
                Ok(json!({"ended":true}))
            }
            "task.authorize" => {
                let sid = text(&r, "sessionId")?;
                let peer = text(&r, "sender")?;
                let s = self.active(sid)?;
                let matching = !s.temporary && s.peer_id.as_deref() == Some(peer);
                let wake = matching && self.allowed(peer, "wake").is_ok();
                let task = matching && self.allowed(peer, "task").is_ok();
                Ok(
                    json!({"allowed":wake&&task,"wake":wake,"task":task,"expiresAt":if s.expires>0 {Some(s.expires)}else{None}}),
                )
            }
            "retry" => {
                self.retry().await?;
                Ok(json!({"retried":true}))
            }
            _ => bail!("unknown_action"),
        }
    }
    fn page(rows: Vec<Value>, after: i64) -> Result<Value> {
        let total = rows.len();
        let mut page = vec![];
        let mut bytes = 0;
        for row in rows {
            let size = serde_json::to_vec(&row)?.len();
            if page.len() >= 32 || (!page.is_empty() && bytes + size > 400000) {
                break;
            }
            bytes += size;
            page.push(row);
        }
        let next = page
            .last()
            .and_then(|row| row["sequence"].as_i64())
            .unwrap_or(after);
        Ok(json!({"hasMore":page.len()<total,"nextAfter":next,"messages":page}))
    }
    fn insert(
        &self,
        id: &str,
        sid: &str,
        sender: &str,
        text: &str,
        kind: &str,
        created: i64,
        status: &str,
    ) -> Result<bool> {
        let mut db = self.0.db.lock().unwrap();
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let old = tx.query_row(
            "SELECT session_id,sender,text,kind FROM messages WHERE id=?",
            [id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            },
        );
        let inserted = match old {
            Ok(old) => {
                ensure!(
                    old == (sid.into(), sender.into(), text.into(), kind.into()),
                    "message_id_conflict"
                );
                false
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                governance::storage_check(&tx, text.len())?;
                if kind == "task_event" {
                    workflow::apply(&tx, &self.id(), id, sid, sender, text)?;
                }
                tx.execute("INSERT INTO messages(id,session_id,sender,text,kind,created,status) VALUES(?,?,?,?,?,?,?)",params![id,sid,sender,text,kind,created,status])?;
                true
            }
            Err(e) => return Err(e.into()),
        };
        tx.commit()?;
        Ok(inserted)
    }
    fn messages(&self, sid: Option<&str>, after: i64, incoming: bool) -> Result<Vec<Value>> {
        let mut rows = {
            let db = self.0.db.lock().unwrap();
            let mut q=db.prepare("SELECT sequence,id,session_id,sender,text,kind,created,status FROM messages WHERE sequence>? AND (? IS NULL OR session_id=?) AND (?=0 OR sender<>?) ORDER BY sequence")?;
            q.query_map(params![after,sid,sid,incoming as i32,self.id()],|r|Ok(json!({"sequence":r.get::<_,i64>(0)?,"id":r.get::<_,String>(1)?,"sessionId":r.get::<_,String>(2)?,"sender":r.get::<_,String>(3)?,"text":r.get::<_,String>(4)?,"kind":r.get::<_,String>(5)?,"created":r.get::<_,i64>(6)?,"status":r.get::<_,String>(7)?})))?.collect::<std::result::Result<Vec<_>,_>>()?
        };
        for v in &mut rows {
            let peer = v["sender"].as_str().unwrap_or("");
            let active = self.active(v["sessionId"].as_str().unwrap_or("")).ok();
            let persistent = active.as_ref().map(|s| !s.temporary).unwrap_or(false);
            let wake = persistent && self.allowed(peer, "wake").is_ok();
            let task = persistent && self.allowed(peer, "task").is_ok();
            v["wakeAllowed"] = json!(wake);
            v["taskAllowed"] = json!(task);
            if v["kind"] == "task" {
                v["expiresAt"] = json!(v["created"].as_i64().unwrap_or(0) + 60000);
            }
        }
        Ok(rows)
    }
    async fn send(&self, r: &Value) -> Result<Value> {
        let s = self.active(text(r, "sessionId")?)?;
        ensure!(s.status == "open", "waiting_for_peer");
        let body = text(r, "text")?;
        ensure!(
            !body.trim().is_empty() && body.len() <= 16000,
            "invalid_message"
        );
        let kind = r["kind"].as_str().unwrap_or("text");
        ensure!(
            matches!(kind, "text" | "task" | "task_event"),
            "invalid_message_kind"
        );
        if kind != "text" {
            ensure!(!s.temporary, "temporary_text_only");
        }
        let id = r["id"].as_str().map(String::from).unwrap_or_else(uid);
        ensure!(!id.is_empty() && id.len() <= 80, "invalid_message_id");
        self.insert(&id, &s.id, &self.id(), body, kind, now(), "pending")?;
        let msg = self
            .messages(Some(&s.id), 0, false)?
            .into_iter()
            .find(|m| m["id"] == id)
            .context("message_not_found")?;
        if msg["status"] == "acked" {
            return Ok(json!({"id":id,"status":"acked","duplicate":true}));
        }
        ensure!(msg["status"] != "rejected", "message_rejected");
        match self.deliver(&s, &msg).await {
            Ok(()) => Ok(json!({"id":id,"status":"acked"})),
            Err(e) => {
                if permanent(&e) {
                    self.0
                        .db
                        .lock()
                        .unwrap()
                        .execute("UPDATE messages SET status='rejected' WHERE id=?", [&id])?;
                    return Err(e);
                }
                Ok(json!({"id":id,"status":"pending","error":e.to_string()}))
            }
        }
    }
    async fn deliver(&self, s: &Session, msg: &Value) -> Result<()> {
        self.active(&s.id)?;
        let peer = s.peer_id.as_ref().context("waiting_for_peer")?;
        let response=self.rpc(s.peer_ticket.as_ref().context("waiting_for_peer")?,json!({"kind":"message","sessionId":s.id,"capability":s.capability,"id":msg["id"],"text":msg["text"],"messageKind":msg["kind"],"created":msg["created"]}),peer).await?;
        ensure!(
            response["ack"] == msg["id"] && response["persisted"] == true,
            "invalid_ack"
        );
        self.0.db.lock().unwrap().execute(
            "UPDATE messages SET status='acked' WHERE id=?",
            [text(msg, "id")?],
        )?;
        Ok(())
    }
    async fn retry(&self) -> Result<()> {
        self.retry_messages().await?;
        self.group_retry(true).await
    }
    async fn retry_messages(&self) -> Result<()> {
        let pending = self
            .messages(None, 0, false)?
            .into_iter()
            .filter(|m| m["status"] == "pending" && m["sender"] == self.id())
            .collect::<Vec<_>>();
        for msg in pending {
            if let Ok(s) = self.active(text(&msg, "sessionId")?) {
                if let Err(e) = self.deliver(&s, &msg).await {
                    if permanent(&e) {
                        self.0.db.lock().unwrap().execute(
                            "UPDATE messages SET status='rejected' WHERE id=?",
                            [text(&msg, "id")?],
                        )?;
                    }
                }
            }
        }
        Ok(())
    }
    async fn rpc(&self, ticket: &str, packet: Value, expected: &str) -> Result<Value> {
        self.check_transport_policy().await?;
        let addr = EndpointTicket::from_str(ticket)?.endpoint_addr().clone();
        ensure!(addr.id.to_string() == expected, "peer_identity_mismatch");
        let conn = timeout(Duration::from_secs(8), self.0.endpoint.connect(addr, ALPN))
            .await
            .context("peer_unreachable")??;
        ensure!(
            conn.remote_id().to_string() == expected,
            "peer_identity_mismatch"
        );
        if !self.0.direct_only {
            let until = tokio::time::Instant::now() + Duration::from_millis(300);
            while tokio::time::Instant::now() < until
                && !conn.paths().iter().any(|p| p.is_selected() && p.is_ip())
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
        let result = timeout(Duration::from_secs(8), async {
            let (mut send, mut recv) = conn.open_bi().await?;
            send.write_all(&serde_json::to_vec(&packet)?).await?;
            send.finish()?;
            let bytes = recv.read_to_end(MAX_PACKET).await?;
            let response: Value = serde_json::from_slice(&bytes)?;
            if let Some(err) = response["error"].as_str() {
                bail!("{err}")
            }
            Ok::<Value, anyhow::Error>(response)
        })
        .await
        .context("ack_timeout")?;
        let paths = conn.paths();
        *self.0.last_path.lock().unwrap() = json!({"peerId":expected,"direct":paths.iter().any(|p|p.is_selected()&&p.is_ip()),"relay":paths.iter().any(|p|p.is_selected()&&p.is_relay())});
        conn.close(0u32.into(), b"done");
        result
    }
    async fn handle(&self, conn: iroh::endpoint::Connection) -> Result<()> {
        let remote = conn.remote_id().to_string();
        let (mut send, mut recv) = conn.accept_bi().await?;
        let packet: Value = serde_json::from_slice(&recv.read_to_end(MAX_PACKET).await?)?;
        let verified_owner = self.verified_peer_owner(&remote);
        let admission = self.governance_admit(
            &remote,
            verified_owner.as_deref(),
            "message",
            &format!("inbound:{}:{}", remote, uid()),
        );
        let result = if let Err(error) = admission {
            Err(error)
        } else if packet["kind"] == "contact.request" {
            self.governance_contact_request(&remote)
        } else if packet["kind"] == "network.ping" {
            self.load("contacts", &remote)
                .and_then(|_| self.allowed(&remote, "chat"))
                .map(|_| json!({"pong":true,"agentId":self.id(),"at":now()}))
        } else if packet["kind"]
            .as_str()
            .is_some_and(|k| k.starts_with("group."))
        {
            self.group_receive(&remote, packet).await
        } else {
            self.receive(&remote, packet)
        };
        let response = match result {
            Ok(v) => v,
            Err(e) => json!({"error":e.to_string()}),
        };
        send.write_all(&serde_json::to_vec(&response)?).await?;
        send.finish()?;
        let _ = timeout(Duration::from_secs(2), send.stopped()).await;
        Ok(())
    }
    fn receive(&self, remote: &str, p: Value) -> Result<Value> {
        let _serial = self.0.receive_guard.lock().unwrap();
        self.allowed(remote, "chat")?;
        let kind = text(&p, "kind")?;
        let sid = text(&p, "sessionId")?;
        ensure!(!sid.is_empty() && sid.len() <= 80, "invalid_session_id");
        if kind == "contact.connect" {
            self.load("contacts", remote)?;
            let ticket = text(&p, "ticket")?;
            ensure!(
                EndpointTicket::from_str(ticket)?
                    .endpoint_addr()
                    .id
                    .to_string()
                    == remote,
                "peer_identity_mismatch"
            );
            ensure!(self.session(sid).is_err(), "session_id_conflict");
            let cap = text(&p, "capability")?;
            ensure!(cap.len() == 43, "invalid_capability");
            let s = Session {
                id: sid.into(),
                label: "Contact".into(),
                host: remote.into(),
                capability: cap.into(),
                expires: 0,
                role: "guest".into(),
                status: "open".into(),
                peer_id: Some(remote.into()),
                peer_ticket: Some(ticket.into()),
                temporary: false,
            };
            self.save_session(&s)?;
            return Ok(json!({"status":"open"}));
        }
        let mut s = self.active(sid)?;
        let cap = text(&p, "capability")?;
        ensure!(
            bool::from(s.capability.as_bytes().ct_eq(cap.as_bytes())),
            "invalid_capability"
        );
        if kind == "join" {
            ensure!(s.temporary && s.role == "host", "invalid_join");
            ensure!(
                s.peer_id.as_deref().map(|id| id == remote).unwrap_or(true),
                "invite_already_used"
            );
            let ticket = text(&p, "ticket")?;
            ensure!(
                EndpointTicket::from_str(ticket)?
                    .endpoint_addr()
                    .id
                    .to_string()
                    == remote,
                "peer_identity_mismatch"
            );
            s.peer_id = Some(remote.into());
            s.peer_ticket = Some(ticket.into());
            s.status = "open".into();
            self.save_session(&s)?;
            return Ok(json!({"sessionId":sid,"status":"open","expires":s.expires}));
        }
        ensure!(
            s.status == "open" && s.peer_id.as_deref() == Some(remote),
            "peer_not_allowed"
        );
        match kind {
            "message" => {
                let id = text(&p, "id")?;
                let body = text(&p, "text")?;
                let mk = p["messageKind"].as_str().unwrap_or("text");
                ensure!(
                    !id.is_empty()
                        && id.len() <= 80
                        && !body.trim().is_empty()
                        && body.len() <= 16000,
                    "invalid_message"
                );
                ensure!(
                    matches!(mk, "text" | "task" | "task_event"),
                    "invalid_message_kind"
                );
                if mk != "text" {
                    ensure!(!s.temporary, "temporary_text_only");
                }
                if mk == "task" {
                    self.allowed(remote, "task")?;
                }
                let created = p["created"].as_i64().context("invalid_timestamp")?;
                if mk == "task" {
                    ensure!(
                        created <= now() + 30000 && created + 60000 > now(),
                        "task_expired"
                    );
                }
                self.insert(id, sid, remote, body, mk, created, "received")?;
                Ok(json!({"ack":id,"persisted":true}))
            }
            "end" => {
                s.status = "ended".into();
                self.save_session(&s)?;
                Ok(json!({"ended":true}))
            }
            _ => bail!("unsupported_message_kind"),
        }
    }
}
