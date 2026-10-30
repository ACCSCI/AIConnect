//! Versioned signed descriptors and device delegations. Root keys stay on the
//! management device; imported delegations contain no private key.
use super::*;
use std::io::Write;

const MAX_DOCUMENT: usize = 65536;
const MAX_TTL: i64 = 7 * 24 * 3600 * 1000;

#[cfg(windows)]
fn protect_root(bytes: &[u8], encrypt: bool) -> Result<Vec<u8>> {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{CRYPT_INTEGER_BLOB, CryptProtectData, CryptUnprotectData},
    };
    let mut input = bytes.to_vec();
    let blob = CRYPT_INTEGER_BLOB {
        cbData: input.len().try_into()?,
        pbData: input.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // UI_FORBIDDEN; deliberately omit LOCAL_MACHINE so another Windows account
    // cannot decrypt using only a copied file. All pointer lengths are bounded.
    let success = unsafe {
        if encrypt {
            CryptProtectData(
                &blob,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                1,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &blob,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                1,
                &mut output,
            )
        }
    };
    input.fill(0);
    ensure!(
        success != 0,
        "owner_root_dpapi_failed: {}",
        std::io::Error::last_os_error()
    );
    ensure!(!output.pbData.is_null(), "owner_root_dpapi_empty");
    // SAFETY: successful DPAPI returns LocalAlloc-owned buffer of cbData bytes.
    let result = unsafe {
        let slice = std::slice::from_raw_parts_mut(output.pbData, output.cbData as usize);
        let result = slice.to_vec();
        slice.fill(0);
        LocalFree(output.pbData.cast());
        result
    };
    Ok(result)
}

fn bounded(v: &Value, depth: usize) -> Result<()> {
    ensure!(depth < 20, "document_too_deep");
    match v {
        Value::Object(m) => {
            ensure!(m.len() <= 128, "document_too_wide");
            for (k, x) in m {
                ensure!(
                    k != "$ref" && k != "$dynamicRef",
                    "remote_schema_references_disabled"
                );
                bounded(x, depth + 1)?;
            }
        }
        Value::Array(a) => {
            ensure!(a.len() <= 128, "document_too_wide");
            for x in a {
                bounded(x, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn canonical(v: &Value) -> Result<Vec<u8>> {
    ensure!(
        serde_json::to_vec(v)?.len() <= MAX_DOCUMENT,
        "document_too_large"
    );
    bounded(v, 0)?;
    Ok(serde_jcs::to_vec(v)?)
}
fn sign_document(key: &SecretKey, payload: Value) -> Result<Value> {
    let bytes = canonical(&payload)?;
    Ok(json!({"payload":payload,"signature":B64.encode(key.sign(&bytes).to_bytes())}))
}
fn verify_document(envelope: &Value, key_field: &str) -> Result<Value> {
    ensure!(
        serde_json::to_vec(envelope)?.len() <= MAX_DOCUMENT,
        "document_too_large"
    );
    let payload = &envelope["payload"];
    let key = EndpointId::from_str(text(payload, key_field)?)?;
    let sig: [u8; 64] = B64
        .decode(text(envelope, "signature")?)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid_signature"))?;
    key.verify(&canonical(payload)?, &iroh::Signature::from_bytes(&sig))?;
    ensure!(
        payload["schemaVersion"] == 1,
        "unsupported_document_version"
    );
    Ok(payload.clone())
}
fn time_window(v: &Value) -> Result<()> {
    let start = v["issuedAt"].as_i64().context("invalid_issued_at")?;
    let end = v["expiresAt"].as_i64().context("invalid_expires_at")?;
    ensure!(
        start <= now() + 30_000
            && end > now()
            && end > start
            && end.checked_sub(start).is_some_and(|n| n <= MAX_TTL),
        "document_expired_or_invalid_window"
    );
    Ok(())
}
fn schema_supported(schema: &Value) -> Result<()> {
    bounded(schema, 0)?;
    ensure!(schema.is_object() || schema.is_boolean(), "invalid_schema");
    compile_schema(schema)?;
    Ok(())
}
fn compile_schema(schema: &Value) -> Result<jsonschema::Validator> {
    jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .with_pattern_options(
            jsonschema::PatternOptions::fancy_regex()
                .backtrack_limit(10000)
                .size_limit(1048576)
                .dfa_size_limit(1048576),
        )
        .build(schema)
        .map_err(|e| anyhow::anyhow!("invalid_json_schema: {e}"))
}
fn domain_url(domain: &str) -> Result<String> {
    ensure!(
        !domain.is_empty()
            && domain.len() <= 253
            && domain
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-'),
        "invalid_domain"
    );
    ensure!(
        !domain.starts_with('.') && !domain.ends_with('.') && domain.contains('.'),
        "invalid_domain"
    );
    ensure!(
        domain.parse::<std::net::IpAddr>().is_err()
            && domain.split('.').all(|label| !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')),
        "invalid_domain"
    );
    let url = reqwest::Url::parse(&format!(
        "https://{domain}/.well-known/aiconnect-owner.json"
    ))?;
    ensure!(
        url.host_str()
            .is_some_and(|h| h.eq_ignore_ascii_case(domain)),
        "invalid_domain"
    );
    Ok(url.to_string())
}
fn verify_domain_document(document: &Value, domain: &str, expected_owner: &str) -> Result<Value> {
    let p = verify_document(document, "ownerId")?;
    time_window(&p)?;
    ensure!(
        p["kind"] == "aiconnect.domain-proof",
        "invalid_domain_proof"
    );
    ensure!(p["ownerId"] == expected_owner, "owner_pin_mismatch");
    ensure!(
        p["domain"]
            .as_str()
            .is_some_and(|s| s.eq_ignore_ascii_case(domain)),
        "domain_proof_mismatch"
    );
    Ok(p)
}
/// Validate metadata only. Never fetch schema references or run capability text.
pub(crate) fn verify_native_descriptor(envelope: &Value) -> Result<Value> {
    let p = verify_native_descriptor_signature(envelope)?;
    time_window(&p)?;
    Ok(p)
}
/// Structural/signature verification for stale discovery displays. Callers must
/// label expiration and must not treat this as fresh execution evidence.
pub(crate) fn verify_native_descriptor_signature(envelope: &Value) -> Result<Value> {
    let p = verify_document(envelope, "agentId")?;
    ensure!(
        p["kind"] == "aiconnect.descriptor",
        "invalid_descriptor_kind"
    );
    ensure!(
        matches!(p["visibility"].as_str(), Some("private" | "lan" | "public")),
        "invalid_visibility"
    );
    ensure!(
        p["revision"].as_u64().is_some_and(|x| x > 0),
        "invalid_revision"
    );
    let skills = p["skills"].as_array().context("invalid_skills")?;
    ensure!(skills.len() <= 64, "too_many_skills");
    let mut seen = std::collections::HashSet::new();
    for skill in skills {
        let id = text(skill, "skillId")?;
        ensure!(
            !id.is_empty() && id.len() <= 128 && seen.insert(id),
            "invalid_or_duplicate_skill_id"
        );
        ensure!(
            skill["description"]
                .as_str()
                .is_some_and(|s| s.len() <= 4096),
            "invalid_skill_description"
        );
        for field in ["inputSchema", "outputSchema"] {
            if let Some(s) = skill.get(field) {
                schema_supported(s)?;
            }
        }
    }
    let transports = p["transports"].as_array().context("invalid_transports")?;
    ensure!(transports.len() <= 8, "invalid_transports");
    // A descriptor never fabricates an HTTP A2A endpoint for a native-only node.
    for t in transports {
        ensure!(
            t["protocol"] == "iroh",
            "native_descriptor_transport_not_enabled"
        );
    }
    Ok(p)
}

impl Core {
    /// Owner bucket for ingress/runtime quotas. Never infer it from a profile
    /// label or directory trust flag. Conflicting newest descriptors fail closed.
    pub fn verified_peer_owner(&self, peer: &str) -> Option<String> {
        if self
            .load("contacts", peer)
            .is_ok_and(|v| v["blocked"] == true)
        {
            return None;
        }
        let values: Vec<Value> = {
            let db = self.0.db.lock().ok()?;
            let mut q=db.prepare("SELECT descriptor FROM discovery_cache WHERE agent_id=? ORDER BY json_extract(descriptor,'$.payload.revision') DESC LIMIT 128").ok()?;
            let raw = q
                .query_map([peer], |r| r.get::<_, String>(0))
                .ok()?
                .collect::<std::result::Result<Vec<_>, _>>()
                .ok()?;
            raw.into_iter()
                .map(|s| serde_json::from_str(&s))
                .collect::<std::result::Result<_, _>>()
                .ok()?
        };
        let first = values.first()?;
        let revision = first["payload"]["revision"].as_u64()?;
        if first["payload"]["agentId"] != peer
            || values
                .iter()
                .any(|v| v["payload"]["revision"].as_u64() == Some(revision) && v != first)
        {
            return None;
        }
        self.descriptor_owner(first).ok().flatten()
    }
    /// Returns a verified common-owner claim only when the local root pin and
    /// revocation view are fresh. Descriptor text/owner labels never establish it.
    pub(crate) fn descriptor_owner(&self, envelope: &Value) -> Result<Option<String>> {
        let p = verify_native_descriptor(envelope)?;
        let Some(proof) = p.get("ownerProof").filter(|p| !p.is_null()) else {
            return Ok(None);
        };
        let grant = verify_document(proof, "ownerId")?;
        time_window(&grant)?;
        ensure!(
            grant["kind"] == "aiconnect.delegation"
                && grant["agentId"] == p["agentId"]
                && grant["audience"] == "discovery",
            "invalid_owner_discovery_proof"
        );
        ensure!(
            grant["scopes"]
                .as_array()
                .is_some_and(|xs| xs.iter().any(|s| s == "discover")),
            "owner_discovery_scope_denied"
        );
        let owner = text(&grant, "ownerId")?;
        let root = self.load("network_roots", owner)?;
        ensure!(
            root["trusted"] == true && root["validUntil"].as_i64().is_some_and(|n| n > now()),
            "owner_discovery_trust_stale"
        );
        ensure!(
            !root["revocations"]
                .as_array()
                .is_some_and(|xs| xs.contains(&grant["id"])),
            "delegation_revoked"
        );
        Ok(Some(owner.to_string()))
    }
    pub async fn network_domain_verify(&self, r: &Value) -> Result<Value> {
        let domain = text(r, "domain")?;
        let url = domain_url(domain)?;
        let expected = text(r, "expectedOwner")?;
        EndpointId::from_str(expected)?;
        let response = self
            .governance_http_get(&url, None, None, MAX_DOCUMENT)
            .await?;
        let document: Value = serde_json::from_str(text(&response, "body")?)?;
        let p = verify_domain_document(&document, domain, expected)?;
        let evidence = json!({"evidenceType":"domain_control","subjectKey":expected,"sourceURL":url,"verifiedAt":now(),"validUntil":p["expiresAt"],"verificationMethod":"HTTPS well-known + pinned Ed25519 signature","legalIdentityVerified":false,"qualityVerified":false,"ownerTrustChanged":false,"document":document});
        self.put(
            "settings",
            &format!("domainEvidence:{}", domain.to_ascii_lowercase()),
            &evidence,
        )?;
        Ok(json!({"evidence":evidence,"ownerTrustChanged":false}))
    }
    fn identity_tables(&self) -> Result<()> {
        self.0.db.lock().unwrap().execute_batch("CREATE TABLE IF NOT EXISTS network_grants(id TEXT PRIMARY KEY,data TEXT NOT NULL);CREATE TABLE IF NOT EXISTS network_roots(id TEXT PRIMARY KEY,data TEXT NOT NULL);CREATE TABLE IF NOT EXISTS network_grant_uses(grant_id TEXT NOT NULL,nonce TEXT NOT NULL,created INTEGER NOT NULL,PRIMARY KEY(grant_id,nonce));CREATE TABLE IF NOT EXISTS network_pairing(id TEXT PRIMARY KEY,data TEXT NOT NULL);")?;
        Ok(())
    }
    fn root_path(&self) -> Result<PathBuf> {
        let file: String = self.0.db.lock().unwrap().query_row(
            "SELECT file FROM pragma_database_list WHERE name='main'",
            [],
            |r| r.get(0),
        )?;
        Ok(PathBuf::from(file)
            .parent()
            .context("invalid_profile_path")?
            .join("owner-root.key"))
    }
    fn owner_key(&self) -> Result<SecretKey> {
        let mut stored = std::fs::read(self.root_path()?)
            .context("owner_root_not_initialized_on_this_device")?;
        #[cfg(windows)]
        let legacy = stored.len() == 32;
        #[cfg(windows)]
        if !legacy {
            let prefix = b"AICONNECT-DPAPI-1\0";
            ensure!(stored.starts_with(prefix), "invalid_owner_root_key_format");
            stored = protect_root(&stored[prefix.len()..], false)?;
        }
        let mut bytes: [u8; 32] = stored
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid_owner_root_key"))?;
        let key = SecretKey::from_bytes(&bytes);
        bytes.fill(0);
        stored.fill(0);
        #[cfg(windows)]
        if legacy {
            self.save_root_key(&key, false)?;
        }
        Ok(key)
    }
    fn save_root_key(&self, key: &SecretKey, create: bool) -> Result<()> {
        let target = self.root_path()?;
        let path = if create {
            target.clone()
        } else {
            target.with_file_name(format!("owner-root.{}.tmp", uid()))
        };
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut secret = key.to_bytes();
        #[cfg(windows)]
        let mut data = {
            let mut out = b"AICONNECT-DPAPI-1\0".to_vec();
            out.extend_from_slice(&protect_root(&secret, true)?);
            out
        };
        #[cfg(not(windows))]
        let mut data = secret.to_vec();
        secret.fill(0);
        let mut file = opts.open(&path)?;
        file.write_all(&data)?;
        file.sync_all()?;
        data.fill(0);
        drop(file);
        if !create {
            std::fs::rename(&path, &target)?;
        }
        Ok(())
    }
    fn root_record(&self, key: &SecretKey) -> Result<()> {
        let id = key.public().to_string();
        let mut record = self
            .load("network_roots", &id)
            .unwrap_or(json!({"ownerId":id,"trusted":true,"local":true,"revocations":[]}));
        record["checkedAt"] = json!(now());
        record["validUntil"] = json!(now() + MAX_TTL);
        self.put("network_roots", &id, &record)
    }
    pub(crate) fn validate_service_grant(
        &self,
        grant_id: &str,
        audience: &str,
        scope: &str,
    ) -> Result<Value> {
        self.identity_tables()?;
        let record = self.load("network_grants", grant_id)?;
        ensure!(record["revoked"] != true, "delegation_revoked");
        let p = verify_document(&record["envelope"], "ownerId")?;
        ensure!(
            p["kind"] == "aiconnect.delegation",
            "invalid_delegation_kind"
        );
        time_window(&p)?;
        ensure!(
            p["maxCalls"]
                .as_i64()
                .is_some_and(|n| (1..=10000).contains(&n)),
            "invalid_max_calls"
        );
        ensure!(p["agentId"] == self.id(), "delegation_wrong_device");
        ensure!(p["audience"] == audience, "delegation_wrong_audience");
        ensure!(
            p["scopes"]
                .as_array()
                .is_some_and(|xs| xs.iter().any(|v| v.as_str() == Some(scope))),
            "delegation_scope_denied"
        );
        let root = self.load("network_roots", text(&p, "ownerId")?)?;
        ensure!(root["trusted"] == true, "owner_not_trusted");
        ensure!(
            root["validUntil"].as_i64().is_some_and(|n| n > now()),
            "revocation_status_stale"
        );
        ensure!(
            !root["revocations"]
                .as_array()
                .is_some_and(|xs| xs.iter().any(|x| x == grant_id)),
            "delegation_revoked"
        );
        Ok(p)
    }
    pub(crate) fn consume_service_grant(
        &self,
        grant_id: &str,
        audience: &str,
        scope: &str,
        nonce: &str,
    ) -> Result<Value> {
        ensure!(!nonce.is_empty() && nonce.len() <= 128, "invalid_nonce");
        let grant = self.validate_service_grant(grant_id, audience, scope)?;
        let mut db = self.0.db.lock().unwrap();
        let tx = db.transaction()?;
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM network_grant_uses WHERE grant_id=?",
            [grant_id],
            |r| r.get(0),
        )?;
        ensure!(
            count < grant["maxCalls"].as_i64().unwrap_or(1),
            "delegation_call_limit"
        );
        tx.execute(
            "INSERT INTO network_grant_uses(grant_id,nonce,created) VALUES(?,?,?)",
            params![grant_id, nonce, now()],
        )
        .context("delegation_nonce_replay")?;
        tx.commit()?;
        Ok(grant)
    }
    pub fn network_identity_request(&self, r: &Value) -> Result<Value> {
        let _serial = self.0.receive_guard.lock().unwrap();
        self.identity_tables()?;
        let action = text(r, "action")?;
        let result = match action {
            "owner.init" => {
                let path = self.root_path()?;
                if !path.exists() {
                    self.save_root_key(&SecretKey::generate(), true)?;
                }
                let key = self.owner_key()?;
                self.root_record(&key)?;
                let protection = if cfg!(windows) {
                    "Windows DPAPI current user + profile ACL; same-user programs remain trusted"
                } else {
                    "Unix mode 0600 + profile directory ACL; not OS credential vault"
                };
                Ok(
                    json!({"ownerId":key.public().to_string(),"agentId":self.id(),"rootStoredOnThisDevice":true,"keyProtection":protection,"privateKeyExported":false,"backupNote":"No plaintext backup created. DPAPI recovery normally requires the same Windows credentials and machine; lost keys require explicit re-pairing."}),
                )
            }
            "owner.inspect" => Ok(
                json!({"roots":self.list("network_roots")?,"grants":self.list("network_grants")?,"rootStoredOnThisDevice":self.root_path()?.exists()}),
            ),
            "owner.backup" => {
                #[cfg(not(windows))]
                {
                    bail!("owner_backup_not_supported_on_this_platform_no_plaintext_export");
                }
                #[cfg(windows)]
                {
                    let key = self.owner_key()?;
                    let bytes = std::fs::read(self.root_path()?)?;
                    ensure!(
                        bytes.starts_with(b"AICONNECT-DPAPI-1\0"),
                        "owner_backup_requires_dpapi"
                    );
                    let root = self.load("network_roots", &key.public().to_string())?;
                    let backup = sign_document(
                        &key,
                        json!({"schemaVersion":1,"kind":"aiconnect.owner-backup","ownerId":key.public().to_string(),"protection":"windows-dpapi-current-user","ciphertext":B64.encode(bytes),"createdAt":now(),"revocations":root["revocations"]}),
                    )?;
                    Ok(
                        json!({"backup":backup,"containsPlaintextKey":false,"portableAcrossMachines":false,"scope":"Normally requires the same Windows user credentials and machine; not an independent disaster-recovery backup. Contacts and history are not included."}),
                    )
                }
            }
            "owner.restore" => {
                #[cfg(not(windows))]
                {
                    bail!("owner_restore_not_supported_on_this_platform_no_plaintext_import");
                }
                #[cfg(windows)]
                {
                    ensure!(
                        !self.root_path()?.exists(),
                        "owner_root_exists_restore_refuses_overwrite"
                    );
                    let p = verify_document(&r["backup"], "ownerId")?;
                    ensure!(
                        p["kind"] == "aiconnect.owner-backup"
                            && p["protection"] == "windows-dpapi-current-user",
                        "invalid_owner_backup"
                    );
                    let expected = text(r, "expectedOwner")?;
                    ensure!(p["ownerId"] == expected, "owner_pin_mismatch");
                    if let Ok(old) = self.load("network_roots", expected) {
                        ensure!(
                            old["rotatedTo"].is_null(),
                            "owner_backup_root_already_rotated"
                        );
                    }
                    let bytes = B64.decode(text(&p, "ciphertext")?)?;
                    let prefix = b"AICONNECT-DPAPI-1\0";
                    ensure!(bytes.starts_with(prefix), "owner_backup_requires_dpapi");
                    let mut plaintext = protect_root(&bytes[prefix.len()..], false)?;
                    let mut seed: [u8; 32] = plaintext
                        .as_slice()
                        .try_into()
                        .map_err(|_| anyhow::anyhow!("invalid_owner_backup_key"))?;
                    let key = SecretKey::from_bytes(&seed);
                    seed.fill(0);
                    plaintext.fill(0);
                    ensure!(
                        key.public().to_string() == expected,
                        "owner_backup_key_mismatch"
                    );
                    self.save_root_key(&key, true)?;
                    if self.load("network_roots", expected).is_err() {
                        self.put("network_roots",expected,&json!({"ownerId":expected,"trusted":true,"local":true,"revocations":p["revocations"],"restoredAt":now(),"backupCreatedAt":p["createdAt"]}))?;
                    }
                    self.root_record(&key)?;
                    Ok(
                        json!({"ownerId":expected,"restored":true,"overwritten":false,"contactsAndHistoryRestored":false,"revocationState":"Existing local revocations retained; a backup cannot prove later remote revocations absent from this device."}),
                    )
                }
            }
            "owner.domain-challenge" => {
                let domain = text(r, "domain")?.to_ascii_lowercase();
                let url = domain_url(&domain)?;
                let key = self.owner_key()?;
                let document = sign_document(
                    &key,
                    json!({"schemaVersion":1,"kind":"aiconnect.domain-proof","ownerId":key.public().to_string(),"domain":domain,"nonce":uid(),"issuedAt":now(),"expiresAt":now()+86400000}),
                )?;
                Ok(
                    json!({"document":document,"publishURL":url,"published":false,"instructions":"Publish this JSON at the well-known URL using your static host. Then verify by HTTPS; no deployment was performed."}),
                )
            }
            "owner.domain-inspect" => {
                let domain = text(r, "domain")?.to_ascii_lowercase();
                domain_url(&domain)?;
                Ok(
                    json!({"evidence":self.load("settings",&format!("domainEvidence:{domain}")).ok(),"ownerTrustChanged":false}),
                )
            }
            "owner.pair-request" => {
                let db = self.0.db.lock().unwrap();
                db.execute(
                    "DELETE FROM network_pairing WHERE json_extract(data,'$.expiresAt')<=?",
                    [now()],
                )?;
                let count: i64 =
                    db.query_row("SELECT COUNT(*) FROM network_pairing", [], |r| r.get(0))?;
                ensure!(count < 128, "pairing_pending_limit");
                drop(db);
                let nonce = uid();
                let envelope = sign_document(
                    self.0.endpoint.secret_key(),
                    json!({"schemaVersion":1,"kind":"aiconnect.pair-request","agentId":self.id(),"nonce":nonce,"issuedAt":now(),"expiresAt":now()+300000}),
                )?;
                self.put(
                    "network_pairing",
                    &nonce,
                    &json!({"used":false,"expiresAt":now()+300000,"request":envelope}),
                )?;
                Ok(json!({"request":envelope,"nonce":nonce}))
            }
            "owner.grant" | "owner.pair" => {
                let key = self.owner_key()?;
                let pair = if r["action"] == "owner.pair" {
                    let p = verify_document(&r["request"], "agentId")?;
                    time_window(&p)?;
                    ensure!(
                        p["kind"] == "aiconnect.pair-request",
                        "invalid_pair_request"
                    );
                    Some(p)
                } else {
                    None
                };
                let agent = if let Some(p) = &pair {
                    text(p, "agentId")?
                } else {
                    text(r, "agentId")?
                };
                EndpointId::from_str(agent)?;
                let audience = text(r, "audience")?;
                ensure!(
                    !audience.is_empty() && audience.len() <= 256,
                    "invalid_audience"
                );
                let scopes = r["scopes"].as_array().context("missing_scopes")?;
                ensure!(
                    !scopes.is_empty()
                        && scopes.len() <= 32
                        && scopes.iter().all(|x| x
                            .as_str()
                            .is_some_and(|s| !s.is_empty() && s.len() <= 128 && s != "*")),
                    "invalid_scopes"
                );
                let ttl = r["ttlSeconds"].as_i64().unwrap_or(3600);
                ensure!((1..=604800).contains(&ttl), "invalid_ttl");
                let max = r["maxCalls"].as_i64().unwrap_or(1);
                ensure!((1..=10000).contains(&max), "invalid_max_calls");
                let root = self.load("network_roots", &key.public().to_string())?;
                let id = uid();
                let envelope = sign_document(
                    &key,
                    json!({"schemaVersion":1,"kind":"aiconnect.delegation","id":id,"ownerId":key.public().to_string(),"agentId":agent,"audience":audience,"scopes":scopes,"issuedAt":now(),"expiresAt":now()+ttl*1000,"maxCalls":max,"pairNonce":pair.as_ref().and_then(|p|p["nonce"].as_str()),"revocationView":{"issuedAt":now(),"expiresAt":now()+300000,"revocations":root["revocations"]}}),
                )?;
                self.put(
                    "network_grants",
                    &id,
                    &json!({"id":id,"envelope":envelope,"revoked":false,"issuedLocally":true}),
                )?;
                Ok(json!({"grant":envelope,"id":id,"containsPrivateKey":false}))
            }
            "owner.import" => {
                // Explicit expectedOwner pin is required; possession of a public
                // document cannot silently assign the machine to a new owner.
                let envelope = &r["grant"];
                let p = verify_document(envelope, "ownerId")?;
                time_window(&p)?;
                ensure!(
                    p["kind"] == "aiconnect.delegation" && p["agentId"] == self.id(),
                    "invalid_delegation_target"
                );
                let owner = text(&p, "ownerId")?;
                ensure!(owner == text(r, "expectedOwner")?, "owner_pin_mismatch");
                ensure!(r["trustOwner"] == true, "explicit_owner_trust_required");
                let id = text(&p, "id")?;
                ensure!(!id.is_empty() && id.len() <= 128, "invalid_delegation_id");
                ensure!(
                    p["audience"]
                        .as_str()
                        .is_some_and(|s| !s.is_empty() && s.len() <= 256),
                    "invalid_audience"
                );
                ensure!(
                    p["maxCalls"]
                        .as_i64()
                        .is_some_and(|n| (1..=10000).contains(&n)),
                    "invalid_max_calls"
                );
                ensure!(
                    p["scopes"].as_array().is_some_and(|xs| !xs.is_empty()
                        && xs.len() <= 32
                        && xs.iter().all(|x| x
                            .as_str()
                            .is_some_and(|s| !s.is_empty() && s.len() <= 128 && s != "*"))),
                    "invalid_scopes"
                );
                if let Ok(old) = self.load("network_grants", id) {
                    ensure!(old["envelope"] == *envelope, "delegation_id_conflict");
                    ensure!(old["revoked"] != true, "delegation_revoked");
                }
                let view = &p["revocationView"];
                let view_start = view["issuedAt"]
                    .as_i64()
                    .context("missing_revocation_view")?;
                let view_end = view["expiresAt"]
                    .as_i64()
                    .context("missing_revocation_view")?;
                ensure!(
                    view_start <= now() + 30000
                        && view_end > view_start
                        && view_end
                            .checked_sub(view_start)
                            .is_some_and(|n| n <= 300000),
                    "invalid_revocation_view"
                );
                let incoming = view["revocations"]
                    .as_array()
                    .context("invalid_revocation_view")?;
                ensure!(!incoming.contains(&p["id"]), "delegation_revoked");
                if let Some(nonce) = p["pairNonce"].as_str() {
                    let mut pending = self.load("network_pairing", nonce)?;
                    ensure!(
                        pending["used"] != true
                            && pending["expiresAt"].as_i64().is_some_and(|n| n > now()),
                        "pairing_used_or_expired"
                    );
                    pending["used"] = json!(true);
                    self.put("network_pairing", nonce, &pending)?;
                }
                if self.load("network_roots", owner).is_err() {
                    self.put("network_roots",owner,&json!({"ownerId":owner,"trusted":true,"local":false,"checkedAt":now(),"validUntil":view_end,"revocations":incoming,"lastRevocationIssuedAt":view_start}))?;
                }
                self.put(
                    "network_grants",
                    id,
                    &json!({"id":id,"envelope":envelope,"revoked":false,"issuedLocally":false}),
                )?;
                Ok(
                    json!({"id":id,"ownerId":owner,"requiresFreshRevocationAfterSeconds":300,"privateKeyImported":false}),
                )
            }
            "owner.verify" => {
                let p = self.validate_service_grant(
                    text(r, "grantId")?,
                    text(r, "audience")?,
                    text(r, "scope")?,
                )?;
                Ok(json!({"valid":true,"grant":p}))
            }
            "owner.consume" => {
                let p = self.consume_service_grant(
                    text(r, "grantId")?,
                    text(r, "audience")?,
                    text(r, "scope")?,
                    text(r, "nonce")?,
                )?;
                Ok(json!({"valid":true,"grant":p}))
            }
            "owner.revoke" => {
                let id = text(r, "grantId")?;
                let key = self.owner_key()?;
                let mut record = self.load("network_grants", id)?;
                ensure!(
                    record["envelope"]["payload"]["ownerId"] == key.public().to_string(),
                    "owner_permission_denied"
                );
                let owner = key.public().to_string();
                let mut root = self.load("network_roots", &owner)?;
                let revocations = root["revocations"]
                    .as_array_mut()
                    .context("invalid_revocations")?;
                if !revocations.contains(&json!(id)) {
                    revocations.push(json!(id));
                }
                let signed = sign_document(
                    &key,
                    json!({"schemaVersion":1,"kind":"aiconnect.revocations","ownerId":owner,"revocations":root["revocations"],"issuedAt":now(),"expiresAt":now()+300000}),
                )?;
                record["revoked"] = json!(true);
                self.put("network_grants", id, &record)?;
                self.put("network_roots", &owner, &root)?;
                Ok(json!({"revoked":id,"revocation":signed}))
            }
            "owner.revocations" => {
                let envelope = &r["document"];
                let p = verify_document(envelope, "ownerId")?;
                time_window(&p)?;
                ensure!(
                    p["kind"] == "aiconnect.revocations",
                    "invalid_revocation_document"
                );
                let owner = text(&p, "ownerId")?;
                let mut root = self.load("network_roots", owner)?;
                let issued = p["issuedAt"].as_i64().context("invalid_issued_at")?;
                ensure!(
                    issued >= root["lastRevocationIssuedAt"].as_i64().unwrap_or(0),
                    "revocation_rollback"
                );
                let previous = root["revocations"]
                    .as_array()
                    .context("invalid_revocations")?;
                let incoming = p["revocations"].as_array().context("invalid_revocations")?;
                ensure!(
                    previous.iter().all(|x| incoming.contains(x)),
                    "revocation_rollback"
                );
                root["revocations"] = p["revocations"].clone();
                root["validUntil"] = p["expiresAt"].clone();
                root["checkedAt"] = json!(now());
                root["lastRevocationIssuedAt"] = json!(issued);
                self.put("network_roots", owner, &root)?;
                Ok(json!({"updated":true,"ownerId":owner}))
            }
            "owner.revocation-export" => {
                let key = self.owner_key()?;
                let owner = key.public().to_string();
                let root = self.load("network_roots", &owner)?;
                Ok(
                    json!({"document":sign_document(&key,json!({"schemaVersion":1,"kind":"aiconnect.revocations","ownerId":owner,"revocations":root["revocations"],"issuedAt":now(),"expiresAt":now()+300000}))?}),
                )
            }
            "owner.rotate" => {
                // Rotation deliberately does not copy existing delegations to the
                // new root. Agents must import the signed transition explicitly.
                let old = self.owner_key()?;
                let new = SecretKey::generate();
                let transition = sign_document(
                    &old,
                    json!({"schemaVersion":1,"kind":"aiconnect.owner-rotation","ownerId":old.public().to_string(),"newOwnerId":new.public().to_string(),"issuedAt":now(),"expiresAt":now()+300000}),
                )?;
                let old_id = old.public().to_string();
                let mut record = self.load("network_roots", &old_id)?;
                record["trusted"] = json!(false);
                record["rotatedTo"] = json!(new.public().to_string());
                self.put("network_roots", &old_id, &record)?;
                self.save_root_key(&new, false)?;
                self.root_record(&new)?;
                Ok(
                    json!({"ownerId":new.public().to_string(),"transition":transition,"oldDelegationsUsable":false}),
                )
            }
            "owner.rotation-import" => {
                let p = verify_document(&r["transition"], "ownerId")?;
                time_window(&p)?;
                ensure!(
                    p["kind"] == "aiconnect.owner-rotation",
                    "invalid_rotation_document"
                );
                let old = text(&p, "ownerId")?;
                let new = text(&p, "newOwnerId")?;
                EndpointId::from_str(new)?;
                ensure!(old != new, "invalid_rotation_target");
                let mut root = self.load("network_roots", old)?;
                ensure!(root["trusted"] == true, "rotation_root_not_trusted");
                root["trusted"] = json!(false);
                root["rotatedTo"] = json!(new);
                self.put("network_roots", old, &root)?;
                self.put("network_roots",new,&json!({"ownerId":new,"trusted":true,"local":false,"checkedAt":now(),"validUntil":now()+300000,"revocations":[],"rotatedFrom":old}))?;
                Ok(json!({"ownerId":new,"oldDelegationsUsable":false,"requiresNewDelegation":true}))
            }
            "profile.set" => {
                let visibility = r["visibility"].as_str().unwrap_or("private");
                ensure!(
                    matches!(visibility, "private" | "lan" | "public"),
                    "invalid_visibility"
                );
                let mut profile = self
                    .load("settings", "profile")
                    .unwrap_or(json!({"name":"Agent"}));
                profile["visibility"] = json!(visibility);
                let updated = if let Ok(existing) = self.load("settings", "nativeDescriptor") {
                    let mut p = verify_native_descriptor_signature(&existing)?;
                    p["visibility"] = json!(visibility);
                    p["revision"] = json!(
                        p["revision"]
                            .as_u64()
                            .context("invalid_revision")?
                            .checked_add(1)
                            .context("descriptor_revision_exhausted")?
                    );
                    // Keep the original expiry; changing visibility does not
                    // silently renew capabilities or owner evidence.
                    let envelope = sign_document(self.0.endpoint.secret_key(), p)?;
                    self.put("settings", "nativeDescriptor", &envelope)?;
                    true
                } else {
                    false
                };
                self.put("settings", "profile", &profile)?;
                Ok(
                    json!({"profile":profile,"currentDescriptorUpdated":updated,"descriptorRepublishRequired":true,"previousPublicCopiesMayPersist":true}),
                )
            }
            "capability.publish" => {
                let profile = self
                    .load("settings", "profile")
                    .unwrap_or(json!({"visibility":"private"}));
                let visibility = r["visibility"]
                    .as_str()
                    .or(profile["visibility"].as_str())
                    .unwrap_or("private");
                let ttl = r["ttlSeconds"].as_i64().unwrap_or(3600);
                ensure!((1..=604800).contains(&ttl), "invalid_ttl");
                let revision = self
                    .load("settings", "nativeDescriptor")
                    .ok()
                    .and_then(|x| x["payload"]["revision"].as_u64())
                    .unwrap_or(0)
                    .checked_add(1)
                    .context("descriptor_revision_exhausted")?;
                let owner_proof = if let Some(grant_id) = r["ownerGrantId"].as_str() {
                    self.validate_service_grant(grant_id, "discovery", "discover")?;
                    Some(self.load("network_grants", grant_id)?["envelope"].clone())
                } else {
                    None
                };
                let envelope = sign_document(
                    self.0.endpoint.secret_key(),
                    json!({"schemaVersion":1,"kind":"aiconnect.descriptor","agentId":self.id(),"ownerProof":owner_proof,"name":profile["name"],"visibility":visibility,"issuedAt":now(),"expiresAt":now()+ttl*1000,"revision":revision,"skills":r.get("skills").cloned().unwrap_or(json!([])),"transports":[{"protocol":"iroh","alpn":"aiconnect/rust/1","endpointId":self.id()}]}),
                )?;
                verify_native_descriptor(&envelope)?;
                self.put("settings", "nativeDescriptor", &envelope)?;
                Ok(
                    json!({"descriptor":envelope,"publishedExternally":false,"claimVerification":"self-declared"}),
                )
            }
            "capability.inspect" => Ok(
                json!({"descriptor":self.load("settings","nativeDescriptor").ok(),"executes":false}),
            ),
            "capability.validate" | "capability.dry-run" => {
                let descriptor = r
                    .get("descriptor")
                    .cloned()
                    .or_else(|| self.load("settings", "nativeDescriptor").ok())
                    .context("descriptor_not_found")?;
                let p = verify_native_descriptor(&descriptor)?;
                let mut input_validated = false;
                if let Some(input) = r.get("input") {
                    canonical(input)?;
                    let skill_id = text(r, "skillId")?;
                    let skill = p["skills"]
                        .as_array()
                        .context("invalid_skills")?
                        .iter()
                        .find(|s| s["skillId"] == skill_id)
                        .context("skill_not_found")?;
                    let schema = skill
                        .get("inputSchema")
                        .context("input_schema_not_declared")?;
                    let validator = compile_schema(schema)?;
                    validator
                        .validate(input)
                        .map_err(|e| anyhow::anyhow!("input_schema_violation: {e}"))?;
                    input_validated = true;
                }
                Ok(
                    json!({"valid":true,"payload":p,"inputValidated":input_validated,"executes":false,"callsPaidServices":false,"schemaSupport":"JSON Schema 2020-12; remote references disabled"}),
                )
            }
            _ => bail!("unknown_identity_action"),
        };
        if let Ok(value) = &result {
            if matches!(
                action,
                "owner.init"
                    | "owner.grant"
                    | "owner.pair"
                    | "owner.import"
                    | "owner.consume"
                    | "owner.revoke"
                    | "owner.revocations"
                    | "owner.rotate"
                    | "owner.rotation-import"
                    | "owner.backup"
                    | "owner.restore"
                    | "profile.set"
                    | "capability.publish"
            ) {
                let owner = value
                    .get("ownerId")
                    .cloned()
                    .or_else(|| {
                        value
                            .get("grant")
                            .and_then(|v| v.get("payload"))
                            .and_then(|v| v.get("ownerId"))
                            .cloned()
                    })
                    .unwrap_or(Value::Null);
                self.governance_audit_event(action,json!({"agentId":self.id(),"ownerId":owner,"grantId":value.get("id").or_else(||value.get("revoked")),"success":true}))?;
            }
        }
        result
    }
}

#[cfg(test)]
mod canonical_tests {
    use super::*;
    #[test]
    fn node_generated_jcs_ed25519_vector_matches_rust() {
        let vector: Value =
            serde_json::from_str(include_str!("../tests/fixtures/jcs-ed25519-vector.json"))
                .unwrap();
        let bytes = canonical(&vector["payload"]).unwrap();
        assert_eq!(
            String::from_utf8(bytes.clone()).unwrap(),
            vector["canonical"].as_str().unwrap()
        );
        let seed: [u8; 32] = hex::decode(vector["seedHex"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        let key = SecretKey::from_bytes(&seed);
        assert_eq!(
            key.public().to_string(),
            vector["publicKey"].as_str().unwrap()
        );
        assert_eq!(
            B64.encode(key.sign(&bytes).to_bytes()),
            vector["signature"].as_str().unwrap()
        );
        verify_document(
            &json!({"payload":vector["payload"],"signature":vector["signature"]}),
            "agentId",
        )
        .unwrap();
    }
    #[test]
    fn domain_evidence_is_bound_to_domain_and_owner_not_legal_identity() {
        let key = SecretKey::from_bytes(&[9; 32]);
        let p = json!({"schemaVersion":1,"kind":"aiconnect.domain-proof","ownerId":key.public().to_string(),"domain":"example.org","nonce":"challenge","issuedAt":now(),"expiresAt":now()+300000});
        let doc = sign_document(&key, p).unwrap();
        verify_domain_document(&doc, "example.org", &key.public().to_string()).unwrap();
        assert!(verify_domain_document(&doc, "attacker.org", &key.public().to_string()).is_err());
        assert!(
            verify_domain_document(
                &doc,
                "example.org",
                &SecretKey::from_bytes(&[8; 32]).public().to_string()
            )
            .is_err()
        );
        for domain in [
            "https://example.org",
            "example.org/a",
            "127.0.0.1:443",
            "user@example.org",
            "example.org?x=1",
        ] {
            assert!(domain_url(domain).is_err());
        }
        let expired=sign_document(&key,json!({"schemaVersion":1,"kind":"aiconnect.domain-proof","ownerId":key.public().to_string(),"domain":"example.org","issuedAt":now()-2000,"expiresAt":now()-1000})).unwrap();
        assert!(
            verify_domain_document(&expired, "example.org", &key.public().to_string()).is_err()
        );
    }
    #[test]
    fn malicious_time_windows_do_not_overflow() {
        for (start, end) in [
            (i64::MIN, i64::MAX),
            (i64::MIN, now() + 1000),
            (now(), i64::MAX),
            (now() + 1000, now()),
        ] {
            assert!(time_window(&json!({"issuedAt":start,"expiresAt":end})).is_err());
        }
    }
}
