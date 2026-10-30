use aiconnect_core::{Config, Core};
use serde_json::{Value, json};
use tempfile::TempDir;

async fn peer() -> (TempDir, Core) {
    let directory = TempDir::new().unwrap();
    let core = Core::open(Config {
        directory: directory.path().into(),
        direct_only: true,
        relay_only: false,
        relay_url: None,
    })
    .await
    .unwrap();
    (directory, core)
}
async fn descriptor(core: &Core, visibility: &str, ttl: u64) -> Value {
    core.network_identity_request(&json!({"action":"capability.publish","visibility":visibility,"ttlSeconds":ttl,"skills":[{"skillId":"example.inspect","description":"只读配置清单","language":["zh"],"os":["windows"]}]})).unwrap()["descriptor"].clone()
}

#[tokio::test]
async fn signed_sources_deduplicate_filter_persist_and_reject_tamper() {
    let (_da, a) = peer().await;
    let (db, b) = peer().await;
    let signed = descriptor(&a, "public", 3600).await;
    for source in ["mirror-one", "mirror-two"] {
        let r=b.discovery_request(&json!({"action":"directory.import","sourceId":source,"directory":{"schemaVersion":1,"descriptors":[signed]}})).await.unwrap();
        assert_eq!(r["imported"], 1);
    }
    let result=b.discovery_request(&json!({"action":"discover","query":"配置","capability":"example.inspect","language":"zh","os":"windows"})).await.unwrap();
    assert_eq!(result["results"].as_array().unwrap().len(), 1);
    assert_eq!(result["results"][0]["sources"].as_array().unwrap().len(), 2);
    assert_eq!(result["results"][0]["executionAuthorized"], false);
    assert!(
        b.discovery_request(&json!({"action":"discover","os":"linux"}))
            .await
            .unwrap()["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let mut tampered = signed.clone();
    tampered["payload"]["name"] = json!("attacker");
    let rejected=b.discovery_request(&json!({"action":"directory.import","sourceId":"evil","directory":{"schemaVersion":1,"descriptors":[tampered]}})).await.unwrap();
    assert_eq!(rejected["imported"], 0);
    assert_eq!(rejected["rejected"].as_array().unwrap().len(), 1);
    assert_eq!(
        b.discovery_request(&json!({"action":"discover","scope":"mine"}))
            .await
            .unwrap()["results"],
        json!([])
    );
    b.close().await;
    // iroh close drains traffic, but its UDP socket belongs to Endpoint clones
    // and is released only when the last clone is dropped.
    drop(b);
    let reopened = Core::open(Config {
        directory: db.path().into(),
        direct_only: true,
        relay_only: false,
        relay_url: None,
    })
    .await
    .unwrap();
    assert_eq!(
        reopened
            .discovery_request(&json!({"action":"discover"}))
            .await
            .unwrap()["results"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    reopened.close().await;
    a.close().await;
}

#[tokio::test]
async fn private_metadata_expiry_and_free_provider_fail_closed() {
    let (_da, a) = peer().await;
    let (_db, b) = peer().await;
    let private = descriptor(&a, "private", 3600).await;
    let result=b.discovery_request(&json!({"action":"directory.import","sourceId":"public-directory","directory":{"schemaVersion":1,"descriptors":[private]}})).await.unwrap();
    assert_eq!(result["imported"], 0);
    let short = descriptor(&a, "public", 1).await;
    b.discovery_request(
        &json!({"action":"directory.import","directory":{"schemaVersion":1,"descriptors":[short]}}),
    )
    .await
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    assert!(
        b.discovery_request(&json!({"action":"discover"}))
            .await
            .unwrap()["results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let stale = b
        .discovery_request(&json!({"action":"discover","includeExpired":true}))
        .await
        .unwrap();
    assert_eq!(stale["results"][0]["expired"], true);
    assert_eq!(stale["results"][0]["signatureValid"], true);
    assert!(b.discovery_request(&json!({"action":"directory.add","url":"https://example.org/directory.json","free":false,"termsAllowed":true,"termsUrl":"https://example.org/terms"})).await.is_err());
    assert!(b.discovery_request(&json!({"action":"directory.add","url":"http://127.0.0.1","free":true,"termsAllowed":true,"termsUrl":"https://example.org/terms"})).await.is_err());
    assert_eq!(
        b.discovery_request(&json!({"action":"discovery.lan","mode":"status"}))
            .await
            .unwrap()["advertising"],
        false
    );
    a.close().await;
    b.close().await;
}

#[tokio::test]
async fn same_owner_scope_requires_live_signed_proof_and_nostr_binding() {
    let (_da, a) = peer().await;
    let (_db, b) = peer().await;
    let (_dc, c) = peer().await;
    let root = a
        .network_identity_request(&json!({"action":"owner.init"}))
        .unwrap();
    let grant=a.network_identity_request(&json!({"action":"owner.grant","agentId":b.id(),"audience":"discovery","scopes":["discover"],"ttlSeconds":3600})).unwrap();
    b.network_identity_request(&json!({"action":"owner.import","grant":grant["grant"],"expectedOwner":root["ownerId"],"trustOwner":true})).unwrap();
    let signed=b.network_identity_request(&json!({"action":"capability.publish","visibility":"public","ownerGrantId":grant["id"],"skills":[{"skillId":"example.inspect","description":"filesystem win configuration","language":["zh"],"os":["windows"]}]})).unwrap()["descriptor"].clone();
    for receiver in [&a, &c] {
        receiver.discovery_request(&json!({"action":"directory.import","sourceId":"directory","directory":{"schemaVersion":1,"descriptors":[signed]}})).await.unwrap();
    }
    assert_eq!(
        a.discovery_request(&json!({"action":"discover","scope":"mine"}))
            .await
            .unwrap()["results"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        c.discovery_request(&json!({"action":"discover","scope":"mine"}))
            .await
            .unwrap()["results"],
        json!([])
    );
    // Descriptive text containing 'win' cannot satisfy an OS filter.
    assert_eq!(
        a.discovery_request(&json!({"action":"discover","os":"win"}))
            .await
            .unwrap()["results"],
        json!([])
    );
    assert!(
        b.discovery_request(&json!({"action":"discovery.nostr.export"}))
            .await
            .is_err()
    );
    let event = b
        .discovery_request(&json!({"action":"discovery.nostr.export","confirmPublic":true}))
        .await
        .unwrap()["event"]
        .clone();
    let imported = c
        .discovery_request(&json!({"action":"discovery.nostr.import","event":event}))
        .await
        .unwrap();
    assert_eq!(imported["imported"], 1);
    let mut tampered = event.clone();
    tampered["content"] = json!("{}");
    assert!(
        c.discovery_request(&json!({"action":"discovery.nostr.import","event":tampered}))
            .await
            .is_err()
    );
    a.network_identity_request(&json!({"action":"owner.revoke","grantId":grant["id"]}))
        .unwrap();
    assert_eq!(
        a.discovery_request(&json!({"action":"discover","scope":"mine"}))
            .await
            .unwrap()["results"],
        json!([])
    );
    assert!(a.discovery_request(&json!({"action":"discovery.relay.add","url":"ws://127.0.0.1","free":true,"termsAllowed":true,"termsUrl":"https://example.org/terms"})).await.is_err());
    a.close().await;
    b.close().await;
    c.close().await;
}

#[tokio::test]
async fn stale_mirror_cannot_resurrect_removed_capability() {
    let (_da, a) = peer().await;
    let (_db, b) = peer().await;
    let old = descriptor(&a, "public", 3600).await;
    let new = a
        .network_identity_request(
            &json!({"action":"capability.publish","visibility":"public","skills":[]}),
        )
        .unwrap()["descriptor"]
        .clone();
    for (source, signed) in [("stale", old), ("current", new)] {
        b.discovery_request(&json!({"action":"directory.import","sourceId":source,"directory":{"schemaVersion":1,"descriptors":[signed]}})).await.unwrap();
    }
    assert_eq!(
        b.discovery_request(&json!({"action":"discover","capability":"example.inspect"}))
            .await
            .unwrap()["results"],
        json!([])
    );
    assert_eq!(
        b.discovery_request(&json!({"action":"discover","query":"配置"}))
            .await
            .unwrap()["results"],
        json!([])
    );
    a.close().await;
    b.close().await;
}

/// A real multicast probe, kept separate from hermetic tests because host
/// firewall/network policies may deliberately disable mDNS.
#[tokio::test]
#[ignore = "requires host multicast UDP permission; run explicitly for LAN acceptance"]
async fn lan_mdns_real_advertisement_and_browse() {
    let (_da, a) = peer().await;
    let (_db, b) = peer().await;
    let root = a
        .network_identity_request(&json!({"action":"owner.init"}))
        .unwrap();
    let grant=a.network_identity_request(&json!({"action":"owner.grant","agentId":b.id(),"audience":"discovery","scopes":["discover"]})).unwrap();
    b.network_identity_request(&json!({"action":"owner.import","grant":grant["grant"],"expectedOwner":root["ownerId"],"trustOwner":true})).unwrap();
    b.network_identity_request(&json!({"action":"capability.publish","ownerGrantId":grant["id"],"visibility":"lan","skills":[{"skillId":"example.inspect","description":"只读配置清单"}]})).unwrap();
    let announced = b
        .discovery_request(&json!({"action":"discovery.lan","mode":"advertise","ttlSeconds":20}))
        .await
        .unwrap();
    assert_eq!(announced["automaticConnect"], false);
    assert_eq!(announced["signedDescriptorIncluded"], true);
    let browsed = a
        .discovery_request(&json!({"action":"discovery.lan","mode":"browse","seconds":5}))
        .await
        .unwrap();
    let present = browsed["results"].as_array().unwrap().iter().any(|v| {
        v["agentId"] == b.id()
            && v["identityVerified"] == true
            && v["ownerVerified"] == true
            && v["automaticConnect"] == false
    });
    let mine = a
        .discovery_request(&json!({"action":"discover","scope":"mine"}))
        .await
        .unwrap();
    b.discovery_request(&json!({"action":"discovery.lan","mode":"stop"}))
        .await
        .unwrap();
    a.close().await;
    b.close().await;
    assert!(
        present,
        "multicast discovery did not resolve the test advertiser: {browsed}"
    );
    assert!(
        mine["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["agentId"] == b.id())
    );
}
