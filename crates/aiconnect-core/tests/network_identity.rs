use aiconnect_core::{Config, Core};
use serde_json::{Value, json};
use tempfile::TempDir;
async fn peer() -> (TempDir, Core) {
    let dir = TempDir::new().unwrap();
    let core = Core::open(Config {
        directory: dir.path().into(),
        direct_only: true,
        relay_only: false,
        relay_url: None,
    })
    .await
    .unwrap();
    (dir, core)
}
fn request(core: &Core, r: Value) -> Value {
    core.network_identity_request(&r).unwrap()
}
#[tokio::test]
async fn owner_grant_pin_scope_nonce_revocation_persist() {
    let (_a, a) = peer().await;
    let (bdir, b) = peer().await;
    let owner = request(&a, json!({"action":"owner.init"}))["ownerId"].clone();
    assert_ne!(owner, a.id());
    let issued = request(
        &a,
        json!({"action":"owner.grant","agentId":b.id(),"audience":"provider","scopes":["service.invoke"],"maxCalls":2}),
    );
    let import = json!({"action":"owner.import","grant":issued["grant"],"expectedOwner":owner,"trustOwner":true});
    let mut wrong = import.clone();
    wrong["expectedOwner"] = json!(a.id());
    assert!(b.network_identity_request(&wrong).is_err());
    let mut untrusted = import.clone();
    untrusted["trustOwner"] = json!(false);
    assert!(b.network_identity_request(&untrusted).is_err());
    request(&b, import);
    let verify = json!({"action":"owner.verify","grantId":issued["id"],"audience":"provider","scope":"service.invoke"});
    request(&b, verify.clone());
    let mut wrong = verify.clone();
    wrong["audience"] = json!("other");
    assert!(b.network_identity_request(&wrong).is_err());
    wrong = verify.clone();
    wrong["scope"] = json!("shell");
    assert!(b.network_identity_request(&wrong).is_err());
    let mut consume = verify.clone();
    consume["action"] = json!("owner.consume");
    consume["nonce"] = json!("request-1");
    request(&b, consume.clone());
    assert!(b.network_identity_request(&consume).is_err());
    b.close().await;
    drop(b);
    let b = Core::open(Config {
        directory: bdir.path().into(),
        direct_only: true,
        relay_only: false,
        relay_url: None,
    })
    .await
    .unwrap();
    assert!(b.network_identity_request(&consume).is_err());
    consume["nonce"] = json!("request-2");
    request(&b, consume.clone());
    consume["nonce"] = json!("request-3");
    assert!(b.network_identity_request(&consume).is_err());
    let revoked = request(&a, json!({"action":"owner.revoke","grantId":issued["id"]}));
    request(
        &b,
        json!({"action":"owner.revocations","document":revoked["revocation"]}),
    );
    assert!(b.network_identity_request(&verify).is_err());
    // Re-initializing the management identity cannot erase revoked IDs.
    request(&a, json!({"action":"owner.init"}));
    let export = request(&a, json!({"action":"owner.revocation-export"}));
    assert!(
        export["document"]["payload"]["revocations"]
            .as_array()
            .unwrap()
            .contains(&issued["id"])
    );
    a.close().await;
    b.close().await;
}
#[tokio::test]
async fn descriptors_are_private_signed_bounded_metadata() {
    let (_d, c) = peer().await;
    let published = request(
        &c,
        json!({"action":"capability.publish","skills":[{"skillId":"test.echo","description":"untrusted text is metadata","inputSchema":{"type":"object"}}]}),
    );
    assert_eq!(published["descriptor"]["payload"]["visibility"], "private");
    assert_eq!(published["publishedExternally"], false);
    request(&c, json!({"action":"profile.set","visibility":"public"}));
    let public = request(&c, json!({"action":"capability.inspect"}));
    assert_eq!(public["descriptor"]["payload"]["visibility"], "public");
    request(&c, json!({"action":"profile.set","visibility":"private"}));
    let private = request(&c, json!({"action":"capability.inspect"}));
    assert_eq!(private["descriptor"]["payload"]["visibility"], "private");
    assert!(
        private["descriptor"]["payload"]["revision"]
            .as_u64()
            .unwrap()
            > published["descriptor"]["payload"]["revision"]
                .as_u64()
                .unwrap()
    );
    let valid = request(
        &c,
        json!({"action":"capability.validate","descriptor":published["descriptor"]}),
    );
    assert_eq!(valid["executes"], false);
    let with_input = json!({"action":"capability.dry-run","descriptor":published["descriptor"],"skillId":"test.echo","input":{}});
    assert_eq!(request(&c, with_input.clone())["inputValidated"], true);
    let mut bad_input = with_input;
    bad_input["input"] = json!("this is not an object");
    assert!(c.network_identity_request(&bad_input).is_err());
    let mut tampered = published["descriptor"].clone();
    tampered["payload"]["visibility"] = json!("public");
    assert!(
        c.network_identity_request(&json!({"action":"capability.validate","descriptor":tampered}))
            .is_err()
    );
    assert!(c.network_identity_request(&json!({"action":"capability.publish","skills":[{"skillId":"bad","description":"bad","inputSchema":{"$ref":"http://127.0.0.1/secret"}}]})).is_err());
    assert!(c.network_identity_request(&json!({"action":"capability.publish","skills":[{"skillId":"same","description":"one"},{"skillId":"same","description":"two"}]})).is_err());
    assert!(
        c.network_identity_request(&json!({"action":"capability.publish","ttlSeconds":0}))
            .is_err()
    );
    let huge = "x".repeat(70000);
    assert!(
        c.network_identity_request(
            &json!({"action":"capability.publish","skills":[{"skillId":"huge","description":huge}]})
        )
        .is_err()
    );
    c.close().await;
}
#[tokio::test]
async fn pairing_request_is_device_bound_and_one_time() {
    let (_a, a) = peer().await;
    let (_b, b) = peer().await;
    let owner = request(&a, json!({"action":"owner.init"}))["ownerId"].clone();
    let req = request(&b, json!({"action":"owner.pair-request"}));
    let issued = request(
        &a,
        json!({"action":"owner.pair","request":req["request"],"audience":"provider","scopes":["service.invoke"]}),
    );
    let import = json!({"action":"owner.import","grant":issued["grant"],"expectedOwner":owner,"trustOwner":true});
    request(&b, import.clone());
    assert!(b.network_identity_request(&import).is_err());
    let mut forged = req["request"].clone();
    forged["payload"]["nonce"] = json!("forged");
    assert!(a.network_identity_request(&json!({"action":"owner.pair","request":forged,"audience":"provider","scopes":["service.invoke"]})).is_err());
    a.close().await;
    b.close().await;
}
#[tokio::test]
async fn rotation_disables_old_local_root_and_never_exports_private_key() {
    let (_d, c) = peer().await;
    request(&c, json!({"action":"owner.init"}));
    let issued = request(
        &c,
        json!({"action":"owner.grant","agentId":c.id(),"audience":"self","scopes":["service.invoke"]}),
    );
    let verify = json!({"action":"owner.verify","grantId":issued["id"],"audience":"self","scope":"service.invoke"});
    request(&c, verify.clone());
    let rotated = request(&c, json!({"action":"owner.rotate"}));
    assert_ne!(rotated["ownerId"], issued["grant"]["payload"]["ownerId"]);
    assert!(c.network_identity_request(&verify).is_err());
    assert!(!rotated.to_string().contains("privateKey"));
    c.close().await;
}
#[tokio::test]
async fn root_profile_restart_preserves_identity_and_os_protection() {
    let (dir, c) = peer().await;
    let before = request(&c, json!({"action":"owner.init"}));
    let bytes = std::fs::read(dir.path().join("owner-root.key")).unwrap();
    #[cfg(windows)]
    assert!(bytes.starts_with(b"AICONNECT-DPAPI-1\0") && bytes.len() > 32);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(dir.path().join("owner-root.key"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(bytes.len(), 32);
    }
    c.close().await;
    drop(c);
    let c = Core::open(Config {
        directory: dir.path().into(),
        direct_only: true,
        relay_only: false,
        relay_url: None,
    })
    .await
    .unwrap();
    let after = request(&c, json!({"action":"owner.init"}));
    assert_eq!(before["ownerId"], after["ownerId"]);
    let challenge = request(
        &c,
        json!({"action":"owner.domain-challenge","domain":"example.org"}),
    );
    assert_eq!(challenge["published"], false);
    assert_eq!(
        challenge["document"]["payload"]["ownerId"],
        before["ownerId"]
    );
    c.close().await;
}
#[cfg(windows)]
#[tokio::test]
async fn dpapi_backup_restore_is_pinned_encrypted_and_refuses_overwrite() {
    let (dir, c) = peer().await;
    let initialized = request(&c, json!({"action":"owner.init"}));
    let backup = request(&c, json!({"action":"owner.backup"}));
    assert_eq!(backup["containsPlaintextKey"], false);
    assert_eq!(backup["portableAcrossMachines"], false);
    let restore = json!({"action":"owner.restore","expectedOwner":initialized["ownerId"],"backup":backup["backup"]});
    assert!(c.network_identity_request(&restore).is_err());
    std::fs::remove_file(dir.path().join("owner-root.key")).unwrap();
    let mut wrong = restore.clone();
    wrong["expectedOwner"] = json!(c.id());
    assert!(c.network_identity_request(&wrong).is_err());
    wrong = restore.clone();
    wrong["backup"]["payload"]["ciphertext"] = json!("invalid");
    assert!(c.network_identity_request(&wrong).is_err());
    assert_eq!(request(&c, restore.clone())["restored"], true);
    assert!(c.network_identity_request(&restore).is_err());
    assert_eq!(
        request(&c, json!({"action":"owner.init"}))["ownerId"],
        initialized["ownerId"]
    );
    c.close().await;
}
