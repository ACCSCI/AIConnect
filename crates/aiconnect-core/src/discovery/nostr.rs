//! NIP-01/78 metadata only; WSS DNS is pinned before TLS to prevent rebinding.
use super::*;
use ::nostr::prelude::{Event, EventBuilder, FinalizeEvent, Keys, Kind, Tag, Timestamp};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{Message, protocol::WebSocketConfig},
};
type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

fn relay_urls(input: &str) -> Result<(reqwest::Url, String)> {
    ensure!(input.len() <= 2048, "invalid_relay_url");
    let url = reqwest::Url::parse(input)?;
    ensure!(
        url.scheme() == "wss"
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
            && url.host_str().is_some()
            && url.port_or_known_default() == Some(443),
        "relay_secure_public_wss_required"
    );
    let mut https = url.clone();
    https
        .set_scheme("https")
        .map_err(|_| anyhow::anyhow!("invalid_relay_url"))?;
    Ok((url, https.to_string()))
}
fn verify_announcement(event: &Value) -> Result<Value> {
    ensure!(
        serde_json::to_vec(event)?.len() <= 65536,
        "nostr_event_too_large"
    );
    let event: Event = serde_json::from_value(event.clone())?;
    event.verify()?;
    ensure!(event.kind == Kind::Custom(30078), "nostr_wrong_kind");
    let tags = event.tags.iter().map(|t| t.as_slice()).collect::<Vec<_>>();
    ensure!(
        tags.iter()
            .any(|t| t.len() >= 2 && t[0] == "d" && t[1] == "aiconnect:descriptor"),
        "nostr_wrong_identifier"
    );
    ensure!(
        tags.iter().any(|t| t.len() >= 2 && t[0] == "expiration"),
        "nostr_expiration_required"
    );
    ensure!(
        !event.is_expired() && event.created_at.as_secs() <= (now() / 1000 + 30) as u64,
        "nostr_event_expired_or_future"
    );
    let content: Value = serde_json::from_str(&event.content)?;
    let descriptor = crate::network_identity::verify_native_descriptor(&content["descriptor"])?;
    ensure!(
        descriptor["visibility"] == "public",
        "nostr_private_descriptor_denied"
    );
    let binding = &content["binding"];
    let payload = &binding["payload"];
    ensure!(
        payload["kind"] == "aiconnect.nostr-binding" && payload["schemaVersion"] == 1,
        "invalid_nostr_binding"
    );
    ensure!(
        payload["agentId"] == descriptor["agentId"]
            && payload["nostrPublicKey"] == event.pubkey.to_string(),
        "nostr_binding_identity_mismatch"
    );
    ensure!(
        payload["expiresAt"].as_i64().is_some_and(|t| t > now())
            && payload["issuedAt"]
                .as_i64()
                .is_some_and(|t| t <= now() + 30000),
        "nostr_binding_expired"
    );
    let id = EndpointId::from_str(text(payload, "agentId")?)?;
    let signature: [u8; 64] = B64
        .decode(text(binding, "signature")?)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid_binding_signature"))?;
    id.verify(
        &serde_jcs::to_vec(payload)?,
        &iroh::Signature::from_bytes(&signature),
    )?;
    Ok(content["descriptor"].clone())
}

impl Core {
    fn nostr_key(&self) -> Result<Keys> {
        let db_file: String = self.0.db.lock().unwrap().query_row(
            "SELECT file FROM pragma_database_list WHERE name='main'",
            [],
            |r| r.get(0),
        )?;
        let path = PathBuf::from(db_file)
            .parent()
            .context("invalid_profile_path")?
            .join("nostr.key");
        match std::fs::read_to_string(&path) {
            Ok(value) => Ok(Keys::parse(value.trim())?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                use std::io::Write;
                let keys = Keys::generate();
                let mut options = std::fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut file = options.open(path)?;
                file.write_all(keys.secret_key().to_secret_hex().as_bytes())?;
                file.sync_all()?;
                Ok(keys)
            }
            Err(error) => Err(error.into()),
        }
    }
    fn nostr_announcement(&self, confirm: bool) -> Result<Value> {
        ensure!(confirm, "explicit_publication_confirmation_required");
        let descriptor = self.load("settings", "nativeDescriptor")?;
        let payload = crate::network_identity::verify_native_descriptor(&descriptor)?;
        ensure!(
            payload["visibility"] == "public",
            "nostr_private_descriptor_denied"
        );
        let keys = self.nostr_key()?;
        let binding_payload = json!({"schemaVersion":1,"kind":"aiconnect.nostr-binding","agentId":self.id(),"nostrPublicKey":keys.public_key().to_string(),"issuedAt":now(),"expiresAt":payload["expiresAt"]});
        let sig = self
            .0
            .endpoint
            .secret_key()
            .sign(&serde_jcs::to_vec(&binding_payload)?);
        let binding = json!({"payload":binding_payload,"signature":B64.encode(sig.to_bytes())});
        let content = json!({"descriptor":descriptor,"binding":binding});
        let expiry = payload["expiresAt"].as_i64().context("invalid_expiry")? / 1000;
        let event = EventBuilder::new(Kind::Custom(30078), content.to_string())
            .tags([
                Tag::identifier("aiconnect:descriptor"),
                Tag::expiration(Timestamp::from_secs(expiry as u64)),
            ])
            .finalize(&keys)?;
        let event = serde_json::to_value(event)?;
        verify_announcement(&event)?;
        Ok(event)
    }

    async fn nostr_connect(&self, url: &str) -> Result<Socket> {
        let (parsed, https) = relay_urls(url)?;
        self.governance_provider_check(&https)?;
        self.governance_charge_download(0)?;
        let host = parsed
            .host_str()
            .context("invalid_relay_host")?
            .trim_matches(['[', ']'])
            .to_owned();
        let addresses: Vec<_> = timeout(
            Duration::from_secs(4),
            tokio::net::lookup_host((host.as_str(), 443)),
        )
        .await
        .context("relay_dns_timeout")??
        .collect();
        ensure!(
            !addresses.is_empty()
                && addresses
                    .iter()
                    .all(|a| crate::governance::public_ip(a.ip())),
            "relay_ssrf_denied"
        );
        let stream = timeout(
            Duration::from_secs(5),
            tokio::net::TcpStream::connect(addresses.as_slice()),
        )
        .await
        .context("relay_connect_timeout")??;
        let config = WebSocketConfig::default()
            .max_message_size(Some(65536))
            .max_frame_size(Some(65536));
        let handshake = timeout(
            Duration::from_secs(6),
            tokio_tungstenite::client_async_tls_with_config(url, stream, Some(config), None),
        )
        .await
        .context("relay_tls_timeout")?;
        match handshake {
            Ok((socket, _)) => Ok(socket),
            Err(error) => {
                if let tokio_tungstenite::tungstenite::Error::Http(response) = &error {
                    let status = response.status().as_u16();
                    if status == 402 || status == 429 {
                        self.governance_request(
                            &json!({"action":"provider.check","url":https,"statusCode":status}),
                        )?;
                    }
                }
                Err(error.into())
            }
        }
    }

    async fn nostr_exchange(&self, request: &Value) -> Result<Value> {
        let source = self.load("discovery_sources", text(request, "id")?)?;
        ensure!(source["kind"] == "nostr", "source_not_nostr_relay");
        let url = text(&source, "url")?;
        let (_, https) = relay_urls(url)?;
        let reservation = self.governance_admit(&https, None, "fetch", &uid())?;
        let rid = text(&reservation, "reservationId")?.to_owned();
        let result:Result<Value>=timeout(Duration::from_secs(20),async {
            let mut socket=self.nostr_connect(url).await?;
            let publishing=request["action"]=="discovery.nostr.publish";
            let event=if publishing { Some(self.nostr_announcement(request["confirmPublic"]==true)?) }else{None};
            let subscription=uid();
            let command=if let Some(event)=&event {json!(["EVENT",event])}else{json!(["REQ",subscription,{"kinds":[30078],"#d":["aiconnect:descriptor"],"limit":request["limit"].as_u64().unwrap_or(32).clamp(1,64)}])};
            socket.send(Message::Text(command.to_string().into())).await?;
            let mut descriptors=Vec::new();let mut rejected=Vec::new();let mut eose=false;let mut accepted=false;
            for _ in 0..128 {
                let message=timeout(Duration::from_secs(8),socket.next()).await.context("relay_response_timeout")?.context("relay_closed")??;
                self.governance_charge_download(message.len())?;
                if !message.is_text() {continue;}
                let message:Value=serde_json::from_str(message.to_text()?)?;
                match message[0].as_str().unwrap_or("") {
                    "OK" if publishing=>{
                        ensure!(message[1]==event.as_ref().unwrap()["id"],"relay_ack_wrong_event");
                        if message[2]!=true { self.nostr_disable_on_fee(&https,message[3].as_str().unwrap_or(""))?;bail!("relay_rejected_event: {}",message[3]); }
                        accepted=true;break;
                    }
                    "EVENT" if !publishing && message[1]==subscription=>{
                        match verify_announcement(&message[2]) {Ok(descriptor)=>descriptors.push(descriptor),Err(e)=>rejected.push(e.to_string())}
                        if descriptors.len()>=64 {break;}
                    }
                    "EOSE" if message[1]==subscription=>{eose=true;break;}
                    "NOTICE" | "CLOSED"=>{
                        let reason=message.as_array().and_then(|a|a.last()).and_then(Value::as_str).unwrap_or("");
                        self.nostr_disable_on_fee(&https,reason)?;
                        bail!("relay_notice: {reason}");
                    }
                    _=>{},
                }
            }
            let _=socket.close(None).await;
            if publishing {
                ensure!(accepted,"relay_publication_unconfirmed");
                Ok(json!({"published":true,"eventId":event.unwrap()["id"],"relay":url,"content":"public_descriptor_only","deliveryGuarantee":false}))
            }else{
                let imported=self.discovery_import(text(&source,"id")?,&json!({"schemaVersion":1,"descriptors":descriptors}),false)?;
                Ok(json!({"relay":url,"endOfStoredEvents":eose,"import":imported,"rejected":rejected,"protocolVerified":true,"globalCoverage":false}))
            }
        }).await.context("relay_operation_timeout").and_then(|r|r);
        self.governance_complete(&rid, result.is_ok())?;
        result
    }
    fn nostr_disable_on_fee(&self, https: &str, reason: &str) -> Result<()> {
        let reason = reason.to_lowercase();
        if ["payment", "paid", "pay-to", "quota", "rate-limit"]
            .iter()
            .any(|word| reason.contains(word))
        {
            self.governance_request(&json!({"action":"provider.check","url":https,"free":false}))?;
        }
        Ok(())
    }

    pub(super) async fn discovery_nostr_request(&self, request: &Value) -> Result<Value> {
        match request["action"].as_str().unwrap_or("") {
            "discovery.relay.add" => {
                let (url, https) = relay_urls(text(request, "url")?)?;
                ensure!(
                    request["free"] == true && request["termsAllowed"] == true,
                    "free_provider_attestation_required"
                );
                let provider=self.governance_request(&json!({"action":"provider.add","url":https,"free":request["free"],"termsAllowed":request["termsAllowed"],"termsUrl":request["termsUrl"],"purpose":"nostr-public-metadata","ttlSeconds":request["ttlSeconds"].as_u64().unwrap_or(86400)}))?;
                ensure!(
                    self.list("discovery_sources")?.len() < 128,
                    "directory_source_limit"
                );
                let id = uid();
                let source = json!({"id":id,"url":url.to_string(),"kind":"nostr","provider":provider,"irohCompatible":false,"trusted":false});
                self.put("discovery_sources", &id, &source)?;
                Ok(source)
            }
            "discovery.relay.list" => Ok(
                json!({"relays":self.list("discovery_sources")?.into_iter().filter(|s|s["kind"]=="nostr").collect::<Vec<_>>(),"transport":"nostr_metadata_not_iroh_relay"}),
            ),
            "discovery.relay.remove" => {
                let id = text(request, "id")?;
                let mut db = self.0.db.lock().unwrap();
                let tx = db.transaction()?;
                let removed=tx.execute("DELETE FROM discovery_sources WHERE id=? AND json_extract(data,'$.kind')='nostr'",[id])?;
                if removed > 0 {
                    tx.execute("DELETE FROM discovery_cache WHERE source_id=?", [id])?;
                    tx.execute("DELETE FROM discovery_fts WHERE source_id=?", [id])?;
                }
                tx.commit()?;
                Ok(json!({"removed":true}))
            }
            "discovery.relay.probe" | "discovery.nostr.read" | "discovery.nostr.publish" => {
                self.nostr_exchange(request).await
            }
            "discovery.nostr.export" => Ok(
                json!({"event":self.nostr_announcement(request["confirmPublic"]==true)?,"publishedExternally":false}),
            ),
            "discovery.nostr.import" => {
                let descriptor = verify_announcement(&request["event"])?;
                self.discovery_import(
                    request["sourceId"].as_str().unwrap_or("nostr:offline"),
                    &json!({"schemaVersion":1,"descriptors":[descriptor]}),
                    false,
                )
            }
            _ => bail!("unknown_nostr_discovery_action"),
        }
    }
}
