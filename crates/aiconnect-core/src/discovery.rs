//! Opt-in discovery. Untrusted advertisements never create contacts or execution grants.
use super::*;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;
mod nostr;

fn metadata_matches(payload: &Value, field: &str, filter: &str) -> bool {
    fn matches(value: &Value, filter: &str) -> bool {
        match value {
            Value::String(s) => s.eq_ignore_ascii_case(filter),
            Value::Array(a) => a.iter().any(|v| matches(v, filter)),
            _ => false,
        }
    }
    let alternate = if field == "language" {
        "languages"
    } else {
        "operatingSystems"
    };
    matches(&payload[field], filter)
        || matches(&payload[alternate], filter)
        || payload["skills"].as_array().is_some_and(|skills| {
            skills
                .iter()
                .any(|skill| matches(&skill[field], filter) || matches(&skill[alternate], filter))
        })
}

const SERVICE: &str = "_aiconnect._udp.local.";
const MAX_DIRECTORY: usize = 512 * 1024;
const MAX_DESCRIPTORS: usize = 128;
type LanMap = Mutex<HashMap<String, (ServiceDaemon, String, String)>>;
static LAN: OnceLock<LanMap> = OnceLock::new();
fn lan_map() -> &'static LanMap {
    LAN.get_or_init(|| Mutex::new(HashMap::new()))
}

impl Core {
    fn discovery_schema(&self) -> Result<()> {
        self.0.db.lock().unwrap().execute_batch(
            "CREATE TABLE IF NOT EXISTS discovery_sources(id TEXT PRIMARY KEY,data TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS discovery_cache(agent_id TEXT NOT NULL,source_id TEXT NOT NULL,descriptor TEXT NOT NULL,verified_at INTEGER NOT NULL,PRIMARY KEY(agent_id,source_id));
             CREATE VIRTUAL TABLE IF NOT EXISTS discovery_fts USING fts5(agent_id UNINDEXED,source_id UNINDEXED,content, tokenize='unicode61');"
        )?;
        Ok(())
    }

    /// Terminate only this identity's explicit LAN advertisement.
    pub(crate) fn discovery_shutdown(&self) {
        if let Some((daemon, fullname, _)) = lan_map().lock().unwrap().remove(&self.id()) {
            let _ = daemon.unregister(&fullname);
            let _ = daemon.shutdown();
        }
    }

    fn discovery_import(&self, source: &str, directory: &Value, trusted: bool) -> Result<Value> {
        ensure!(
            source.len() <= 200 && !source.is_empty(),
            "invalid_source_id"
        );
        ensure!(
            serde_json::to_vec(directory)?.len() <= MAX_DIRECTORY,
            "directory_too_large"
        );
        ensure!(
            directory["schemaVersion"] == 1,
            "unsupported_directory_version"
        );
        let descriptors = directory["descriptors"]
            .as_array()
            .context("missing_descriptors")?;
        ensure!(
            descriptors.len() <= MAX_DESCRIPTORS,
            "directory_too_many_descriptors"
        );
        ensure!(
            self.list("discovery_sources")?.len() < 128
                || self.load("discovery_sources", source).is_ok(),
            "directory_source_limit"
        );
        let mut verified = Vec::new();
        let mut rejected = Vec::new();
        for envelope in descriptors {
            match crate::network_identity::verify_native_descriptor(envelope) {
                Ok(payload) => {
                    if payload["visibility"] != "public" && !source.starts_with("local:") {
                        rejected.push(json!({"reason":"directory_descriptor_not_public"}));
                        continue;
                    }
                    verified.push((envelope.clone(), payload));
                }
                Err(error) => rejected.push(json!({"reason":error.to_string()})),
            }
        }
        let mut db = self.0.db.lock().unwrap();
        let tx = db.transaction()?;
        let mut cache_count: i64 =
            tx.query_row("SELECT COUNT(*) FROM discovery_cache", [], |r| r.get(0))?;
        let mut imported = 0;
        for (envelope, payload) in verified {
            let id = text(&payload, "agentId")?;
            let revision = payload["revision"].as_u64().context("invalid_revision")?;
            let old: Option<String> = tx
                .query_row(
                    "SELECT descriptor FROM discovery_cache WHERE agent_id=? AND source_id=?",
                    params![id, source],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(old) = old {
                let previous: Value = serde_json::from_str(&old)?;
                let previous_revision = previous["payload"]["revision"].as_u64().unwrap_or(0);
                if revision < previous_revision
                    || (revision == previous_revision && previous != envelope)
                {
                    rejected.push(json!({"agentId":id,"reason":"descriptor_revision_conflict"}));
                    continue;
                }
            } else {
                ensure!(cache_count < 4096, "discovery_cache_limit");
                cache_count += 1;
            }
            tx.execute("INSERT INTO discovery_cache(agent_id,source_id,descriptor,verified_at) VALUES(?,?,?,?) ON CONFLICT(agent_id,source_id) DO UPDATE SET descriptor=excluded.descriptor,verified_at=excluded.verified_at", params![id,source,envelope.to_string(),now()])?;
            tx.execute(
                "DELETE FROM discovery_fts WHERE agent_id=? AND source_id=?",
                params![id, source],
            )?;
            tx.execute(
                "INSERT INTO discovery_fts(agent_id,source_id,content) VALUES(?,?,?)",
                params![id, source, payload.to_string()],
            )?;
            imported += 1;
        }
        tx.commit()?;
        drop(db);
        if self.load("discovery_sources", source).is_err() {
            self.put(
                "discovery_sources",
                source,
                &json!({"id":source,"kind":"import","trusted":trusted,"createdAt":now()}),
            )?;
        }
        Ok(
            json!({"imported":imported,"rejected":rejected,"sourceId":source,"executionAuthorized":false}),
        )
    }

    async fn directory_sync_one(&self, id: &str) -> Result<Value> {
        let mut source = self.load("discovery_sources", id)?;
        ensure!(source["kind"] == "https", "source_not_https_directory");
        let response = timeout(
            Duration::from_secs(10),
            self.governance_http_get(
                text(&source, "url")?,
                source["etag"].as_str(),
                source["lastModified"].as_str(),
                MAX_DIRECTORY,
            ),
        )
        .await
        .context("directory_sync_timeout")
        .and_then(|r| r);
        match response {
            Ok(response) => {
                let result = if response["status"] == 304 {
                    json!({"notModified":true,"imported":0})
                } else {
                    let directory: Value = serde_json::from_str(text(&response, "body")?)?;
                    self.discovery_import(id, &directory, source["trusted"] == true)?
                };
                source["lastChecked"] = json!(now());
                source["lastError"] = Value::Null;
                for field in ["etag", "lastModified"] {
                    if !response[field].is_null() {
                        source[field] = response[field].clone();
                    }
                }
                self.put("discovery_sources", id, &source)?;
                Ok(json!({"sourceId":id,"result":result}))
            }
            Err(error) => {
                source["lastError"] = json!(error.to_string());
                source["lastAttempt"] = json!(now());
                self.put("discovery_sources", id, &source)?;
                Err(error)
            }
        }
    }

    fn discover_cached(&self, request: &Value) -> Result<Value> {
        let scope = request["scope"].as_str().unwrap_or("all");
        ensure!(
            ["all", "mine", "trusted", "public"].contains(&scope),
            "invalid_discovery_scope"
        );
        let limit = request["limit"].as_u64().unwrap_or(50).clamp(1, 128) as usize;
        let query = request["query"].as_str().unwrap_or("");
        ensure!(query.len() <= 200, "discovery_query_too_long");
        let sources = self.list("discovery_sources")?;
        let source_map: HashMap<String, Value> = sources
            .into_iter()
            .filter_map(|v| Some((v["id"].as_str()?.to_owned(), v)))
            .collect();
        let fts_matches: std::collections::HashSet<String> = if query.is_empty() {
            Default::default()
        } else {
            let expression = format!("\"{}\"", query.replace('"', "\"\""));
            let db = self.0.db.lock().unwrap();
            let mut statement = db.prepare(
                "SELECT agent_id FROM discovery_fts WHERE discovery_fts MATCH ? LIMIT 4096",
            )?;
            statement
                .query_map([expression], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<_, _>>()?
        };
        let rows: Vec<(String, String, String, i64)> = {
            let db = self.0.db.lock().unwrap();
            let mut statement = db.prepare("SELECT agent_id,source_id,descriptor,verified_at FROM discovery_cache ORDER BY verified_at DESC LIMIT 4096")?;
            statement
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut merged: BTreeMap<String, Value> = BTreeMap::new();
        let mut invalid = Vec::new();
        for (id, source, raw, verified_at) in rows {
            let envelope: Value = serde_json::from_str(&raw)?;
            let payload = match crate::network_identity::verify_native_descriptor_signature(
                &envelope,
            ) {
                Ok(payload) => payload,
                Err(error) => {
                    if request["includeExpired"] == true {
                        invalid.push(json!({"agentId":id,"sourceId":source,"verifiedAtImport":verified_at,"validNow":false,"reason":error.to_string()}));
                    }
                    continue;
                }
            };
            let expired = payload["expiresAt"].as_i64().is_none_or(|t| t <= now());
            let contact_trusted = self
                .load("contacts", &id)
                .is_ok_and(|v| v["blocked"] != true);
            if self
                .load("contacts", &id)
                .is_ok_and(|v| v["blocked"] == true)
            {
                continue;
            }
            let trusted = contact_trusted
                || source_map
                    .get(&source)
                    .is_some_and(|v| v["trusted"] == true);
            let verified_owner = self.descriptor_owner(&envelope).ok().flatten();
            let provenance = json!({"sourceId":source,"verifiedAt":verified_at,"lastChecked":source_map.get(&source).map(|v|v["lastChecked"].clone()),"sourceError":source_map.get(&source).map(|v|v["lastError"].clone())});
            if let Some(existing) = merged.get_mut(&id) {
                existing["sources"].as_array_mut().unwrap().push(provenance);
                let current_revision = existing["descriptor"]["payload"]["revision"]
                    .as_u64()
                    .unwrap_or(0);
                let revision = payload["revision"].as_u64().unwrap_or(0);
                if revision > current_revision {
                    existing["descriptor"] = envelope;
                    existing["expired"] = json!(expired);
                    existing["validNow"] = json!(!expired);
                    existing["verifiedOwnerId"] = json!(verified_owner);
                    existing["conflict"] = json!(false);
                } else if revision == current_revision && existing["descriptor"] != envelope {
                    existing["conflict"] = json!(true);
                }
                if trusted {
                    existing["trustedSource"] = json!(true);
                }
            } else {
                merged.insert(id.clone(),json!({"agentId":id,"descriptor":envelope,"sources":[provenance],"signatureValid":true,"expired":expired,"validNow":!expired,"verifiedOwnerId":verified_owner,"trustedSource":trusted,"executionAuthorized":false,"conflict":false}));
            }
        }
        // Filter only after selecting the newest revision. Stale mirrors cannot
        // resurrect capabilities removed in a more recent signed descriptor.
        let filtered: Vec<Value> = merged
            .into_values()
            .filter(|entry| {
                let payload = &entry["descriptor"]["payload"];
                let id = entry["agentId"].as_str().unwrap_or("");
                if entry["expired"] == true && request["includeExpired"] != true {
                    return false;
                }
                if (scope == "mine" && id != self.id() && entry["verifiedOwnerId"].is_null())
                    || (scope == "trusted" && entry["trustedSource"] != true)
                    || (scope == "public" && payload["visibility"] != "public")
                {
                    return false;
                }
                if !query.is_empty() {
                    let substring = payload
                        .to_string()
                        .to_lowercase()
                        .contains(&query.to_lowercase());
                    if !substring || (!fts_matches.contains(id) && query.is_ascii()) {
                        return false;
                    }
                }
                if let Some(capability) = request["capability"].as_str() {
                    if !payload["skills"]
                        .as_array()
                        .is_some_and(|s| s.iter().any(|v| v["skillId"] == capability))
                    {
                        return false;
                    }
                }
                !["language", "os"].iter().any(|field| {
                    request[*field]
                        .as_str()
                        .is_some_and(|filter| !metadata_matches(payload, field, filter))
                })
            })
            .collect();
        let total = filtered.len();
        let results: Vec<Value> = filtered.into_iter().take(limit).collect();
        invalid.truncate(limit);
        Ok(
            json!({"results":results,"invalidOrExpired":invalid,"total":total,"hasMore":total>limit,"fromCache":true,"globalCoverage":false,"scopeMine":"self_or_fresh_proof_from_locally_trusted_owner_root","search":"FTS5 unicode61; bounded Chinese substring fallback"}),
        )
    }

    async fn discovery_lan(&self, request: &Value) -> Result<Value> {
        match request["mode"].as_str().unwrap_or("status") {
            "status" => Ok(
                json!({"advertising":lan_map().lock().unwrap().contains_key(&self.id()),"default":"private","automaticConnect":false}),
            ),
            "stop" => {
                self.discovery_shutdown();
                Ok(json!({"advertising":false}))
            }
            "advertise" => {
                let ttl = request["ttlSeconds"].as_u64().unwrap_or(60).clamp(5, 300);
                self.discovery_shutdown();
                let daemon = ServiceDaemon::new()?;
                let id = self.id();
                let name = format!("aic-{}", &id[..16]);
                let host = format!("{name}.local.");
                let port = self
                    .0
                    .endpoint
                    .bound_sockets()
                    .first()
                    .map(|a| a.port())
                    .unwrap_or(1);
                let mut props = HashMap::from([
                    ("agentId".to_owned(), id.clone()),
                    ("protocol".to_owned(), "aiconnect/rust/1".to_owned()),
                    ("proof".to_owned(), "unverified-presence".to_owned()),
                ]);
                let mut descriptor_included = false;
                if let Ok(envelope) = self.load("settings", "nativeDescriptor") {
                    if let Ok(payload) =
                        crate::network_identity::verify_native_descriptor(&envelope)
                    {
                        if matches!(payload["visibility"].as_str(), Some("lan" | "public")) {
                            let encoded = B64.encode(serde_json::to_vec(&envelope)?);
                            // Bounded TXT chunks. Large capability catalogs stay on
                            // explicit directories rather than oversized LAN packets.
                            if encoded.len() <= 6000 {
                                props.insert(
                                    "descriptorParts".to_owned(),
                                    encoded.len().div_ceil(200).to_string(),
                                );
                                for (index, chunk) in encoded.as_bytes().chunks(200).enumerate() {
                                    props.insert(
                                        format!("descriptor{index}"),
                                        String::from_utf8(chunk.to_vec())?,
                                    );
                                }
                                descriptor_included = true;
                            }
                        }
                    }
                }
                let service =
                    ServiceInfo::new(SERVICE, &name, &host, "", port, props)?.enable_addr_auto();
                let fullname = service.get_fullname().to_owned();
                daemon.register(service)?;
                let lease = uid();
                lan_map()
                    .lock()
                    .unwrap()
                    .insert(id.clone(), (daemon, fullname, lease.clone()));
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(ttl)).await;
                    let mut map = lan_map().lock().unwrap();
                    if map.get(&id).is_some_and(|(_, _, token)| token == &lease) {
                        if let Some((daemon, fullname, _)) = map.remove(&id) {
                            let _ = daemon.unregister(&fullname);
                            let _ = daemon.shutdown();
                        }
                    }
                });
                Ok(
                    json!({"advertising":true,"expiresAt":now()+(ttl as i64)*1000,"proof":"presence_only_unless_descriptor_verifies","automaticConnect":false,"signedDescriptorIncluded":descriptor_included,"metadata":["agentId","protocol","optional_explicit_lan_or_public_descriptor"]}),
                )
            }
            "browse" => {
                let seconds = request["seconds"].as_u64().unwrap_or(3).clamp(1, 10);
                let daemon = ServiceDaemon::new()?;
                let receiver = daemon.browse(SERVICE)?;
                let mut found = BTreeMap::new();
                let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
                while found.len() < 128 {
                    let event = match tokio::time::timeout_at(deadline, receiver.recv_async()).await
                    {
                        Ok(Ok(event)) => event,
                        _ => break,
                    };
                    if let ServiceEvent::ServiceResolved(service) = event {
                        if let Some(id) = service.get_property_val_str("agentId") {
                            if EndpointId::from_str(id).is_err() || id == self.id() {
                                continue;
                            }
                            let known = self
                                .load("contacts", id)
                                .is_ok_and(|v| v["blocked"] != true);
                            let mut result = json!({"agentId":id,"knownContact":known,"ownerVerified":false,"identityVerified":false,"addresses":service.get_addresses().iter().map(|a|a.to_string()).collect::<Vec<_>>(),"port":service.get_port(),"source":"lan-mdns","automaticConnect":false});
                            if let Some(parts) = service
                                .get_property_val_str("descriptorParts")
                                .and_then(|s| s.parse::<usize>().ok())
                                .filter(|n| (1..=30).contains(n))
                            {
                                let joined: Option<String> = (0..parts)
                                    .map(|i| {
                                        service
                                            .get_property_val_str(&format!("descriptor{i}"))
                                            .filter(|s| s.len() <= 200)
                                    })
                                    .collect::<Option<Vec<_>>>()
                                    .map(|s| s.concat());
                                if let Some(joined) = joined {
                                    let verified = (|| -> Result<Value> {
                                        ensure!(joined.len() <= 6000, "lan_descriptor_too_large");
                                        let envelope: Value =
                                            serde_json::from_slice(&B64.decode(joined)?)?;
                                        let payload =
                                            crate::network_identity::verify_native_descriptor(
                                                &envelope,
                                            )?;
                                        ensure!(
                                            payload["agentId"] == id
                                                && payload["visibility"] != "private",
                                            "lan_descriptor_mismatch"
                                        );
                                        self.discovery_import(
                                            &format!("local:lan:{id}"),
                                            &json!({"schemaVersion":1,"descriptors":[envelope]}),
                                            false,
                                        )?;
                                        Ok(envelope)
                                    })();
                                    match verified {
                                        Ok(envelope) => {
                                            let owner =
                                                self.descriptor_owner(&envelope).ok().flatten();
                                            result["identityVerified"] = json!(true);
                                            result["ownerVerified"] = json!(owner.is_some());
                                            result["verifiedOwnerId"] = json!(owner);
                                            result["descriptorCached"] = json!(true);
                                        }
                                        Err(error) => {
                                            result["descriptorError"] = json!(error.to_string());
                                        }
                                    }
                                }
                            }
                            found.insert(id.to_owned(), result);
                        }
                    }
                }
                let _ = daemon.stop_browse(SERVICE);
                let _ = daemon.shutdown();
                Ok(
                    json!({"results":found.into_values().collect::<Vec<_>>(),"proof":"untrusted_lan_presence","automaticConnect":false,"requiresSignedPairing":true}),
                )
            }
            _ => bail!("invalid_lan_mode"),
        }
    }

    pub async fn discovery_request(&self, request: &Value) -> Result<Value> {
        self.discovery_schema()?;
        if request["action"]
            .as_str()
            .is_some_and(|s| s.starts_with("discovery.relay.") || s.starts_with("discovery.nostr."))
        {
            return self.discovery_nostr_request(request).await;
        }
        match request["action"].as_str().unwrap_or("") {
            "discover" | "discovery.search" => self.discover_cached(request),
            "directory.import" => self.discovery_import(
                request["sourceId"].as_str().unwrap_or("local:import"),
                &request["directory"],
                request["trusted"] == true,
            ),
            "directory.add" => {
                let url = text(request, "url")?;
                ensure!(
                    request["free"] == true && request["termsAllowed"] == true,
                    "free_provider_attestation_required"
                );
                let provider = self.governance_request(&json!({"op":"provider.add","url":url,"free":request["free"],"termsAllowed":request["termsAllowed"],"termsUrl":request["termsUrl"],"purpose":"directory","ttlSeconds":request["ttlSeconds"].as_u64().unwrap_or(86400)}))?;
                let id = request["id"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(uid);
                ensure!(id.len() <= 200 && !id.is_empty(), "invalid_source_id");
                ensure!(
                    self.list("discovery_sources")?.len() < 128
                        || self.load("discovery_sources", &id).is_ok(),
                    "directory_source_limit"
                );
                let source = json!({"id":id,"kind":"https","url":url,"trusted":request["trusted"] == true,"createdAt":now(),"provider":provider});
                self.put("discovery_sources", &id, &source)?;
                Ok(source)
            }
            "directory.list" => Ok(json!({"sources":self.list("discovery_sources")?})),
            "directory.remove" => {
                let id = text(request, "id")?;
                let db = self.0.db.lock().unwrap();
                db.execute("DELETE FROM discovery_sources WHERE id=?", [id])?;
                db.execute("DELETE FROM discovery_cache WHERE source_id=?", [id])?;
                db.execute("DELETE FROM discovery_fts WHERE source_id=?", [id])?;
                Ok(json!({"removed":true}))
            }
            "directory.sync" => {
                if let Some(id) = request["id"].as_str() {
                    return self.directory_sync_one(id).await;
                }
                let mut results = Vec::new();
                // Bounded work per IPC call; clients can sync other sources explicitly.
                for source in self
                    .list("discovery_sources")?
                    .into_iter()
                    .filter(|v| v["kind"] == "https")
                    .take(2)
                {
                    let id = text(&source, "id")?;
                    match self.directory_sync_one(id).await {Ok(v)=>results.push(v),Err(e)=>results.push(json!({"sourceId":id,"error":e.to_string(),"cachedResultsRetained":true}))}
                }
                Ok(json!({"sources":results,"maximumSourcesPerCall":2}))
            }
            "discovery.lan" => self.discovery_lan(request).await,
            _ => bail!("unknown_discovery_action"),
        }
    }
}

use rusqlite::OptionalExtension;
