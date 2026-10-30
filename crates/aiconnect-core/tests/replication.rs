use aiconnect_core::{Config, Core};
use serde_json::{Value, json};
use tempfile::TempDir;
fn config(dir: &TempDir) -> Config {
    Config {
        directory: dir.path().into(),
        direct_only: true,
        relay_only: false,
        relay_url: None,
    }
}
async fn peer() -> (TempDir, Core) {
    let dir = TempDir::new().unwrap();
    let core = Core::open(config(&dir)).await.unwrap();
    (dir, core)
}
async fn create(a: &Core) -> String {
    a.dispatch(json!({"action":"groups.create","name":"Replicas"}))
        .await
        .unwrap()["group"]["groupId"]
        .as_str()
        .unwrap()
        .into()
}
async fn join(a: &Core, b: &Core, gid: &str) {
    let i = a
        .dispatch(json!({"action":"groups.invite","groupId":gid,"peer":b.id()}))
        .await
        .unwrap();
    b.dispatch(json!({"action":"groups.join","invite":i["invite"]}))
        .await
        .unwrap();
}
async fn send(a: &Core, gid: &str, body: &str) -> Value {
    a.dispatch(json!({"action":"groups.send","groupId":gid,"text":body}))
        .await
        .unwrap()
}
async fn rows(a: &Core, gid: &str) -> Vec<Value> {
    a.dispatch(json!({"action":"groups.messages","groupId":gid}))
        .await
        .unwrap()["messages"]
        .as_array()
        .unwrap()
        .clone()
}
async fn sync(a: &Core, gid: &str) -> Value {
    a.dispatch(json!({"action":"groups.sync","groupId":gid}))
        .await
        .unwrap()
}
async fn wait_rows(a: &Core, gid: &str, count: usize) {
    let until = tokio::time::Instant::now() + std::time::Duration::from_secs(25);
    while rows(a, gid).await.len() < count {
        assert!(
            tokio::time::Instant::now() < until,
            "replica did not converge"
        );
        sync(a, gid).await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn owner_and_original_sender_offline_peer_replica_recovers_history_after_restart() {
    let (_da, a) = peer().await;
    let (_db, b) = peer().await;
    let (_dc, c) = peer().await;
    let (dd, d) = peer().await;
    let gid = create(&a).await;
    for p in [&b, &c, &d] {
        join(&a, p, &gid).await;
    }
    d.close().await;
    drop(d);
    a.close().await;
    drop(a);
    let result = send(&b, &gid, "replicated without owner").await;
    assert_eq!(result["replicated"], true);
    wait_rows(&c, &gid, 1).await;
    assert_eq!(rows(&c, &gid).await.len(), 1, "{result}");
    assert_eq!(rows(&c, &gid).await[0]["signatureVerified"], true);
    b.close().await;
    drop(b);
    let d = Core::open(config(&dd)).await.unwrap();
    let synced = sync(&d, &gid).await;
    wait_rows(&d, &gid, 1).await;
    assert_eq!(
        rows(&d, &gid).await.len(),
        1,
        "history must come from C, not sender or owner: {synced}"
    );
    send(&c, &gid, "C continues while owner and B are offline").await;
    sync(&d, &gid).await;
    sync(&d, &gid).await;
    assert_eq!(rows(&d, &gid).await.len(), 2);
    for row in rows(&d, &gid).await {
        assert_eq!(row["executionAllowed"], false);
        assert_eq!(row["signatureVerified"], true);
    }
    c.close().await;
    d.close().await;
}

#[tokio::test]
async fn membership_updates_block_live_send_and_new_members_cannot_pull_old_history() {
    let (_da, a) = peer().await;
    let (_db, b) = peer().await;
    let (_dc, c) = peer().await;
    let gid = create(&a).await;
    join(&a, &b, &gid).await;
    send(&b, &gid, "before C joined").await;
    join(&a, &c, &gid).await;
    sync(&c, &gid).await;
    assert!(rows(&c, &gid).await.is_empty());
    a.dispatch(json!({"action":"groups.policy","groupId":gid,"peer":b.id(),"canSend":false}))
        .await
        .unwrap();
    assert!(
        b.dispatch(json!({"action":"groups.send","groupId":gid,"text":"muted"}))
            .await
            .is_err()
    );
    let result = send(&a, &gid, "after C joined").await;
    let synced = sync(&c, &gid).await;
    wait_rows(&c, &gid, 1).await;
    assert_eq!(rows(&c, &gid).await.len(), 1, "{result}; {synced}");
    let held = rows(&b, &gid).await.len();
    a.dispatch(json!({"action":"groups.remove","groupId":gid,"peer":b.id()}))
        .await
        .unwrap();
    assert!(
        b.dispatch(json!({"action":"groups.sync","groupId":gid}))
            .await
            .is_err()
    );
    assert_eq!(
        rows(&b, &gid).await.len(),
        held,
        "removal retains already-held history"
    );
    a.dispatch(json!({"action":"groups.dissolve","groupId":gid}))
        .await
        .unwrap();
    assert!(
        c.dispatch(json!({"action":"groups.send","groupId":gid,"text":"dissolved"}))
            .await
            .is_err()
    );
    for p in [&a, &b, &c] {
        p.close().await;
    }
}

#[tokio::test]
async fn legacy_upgrade_is_owner_signed_and_does_not_forge_old_authorship() {
    let (_da, a) = peer().await;
    let (_db, b) = peer().await;
    let gid = a
        .dispatch(json!({"action":"groups.create","name":"Legacy","ownerOnline":true}))
        .await
        .unwrap()["group"]["groupId"]
        .as_str()
        .unwrap()
        .to_string();
    join(&a, &b, &gid).await;
    send(&b, &gid, "legacy history").await;
    assert!(
        b.dispatch(json!({"action":"groups.replicate","groupId":gid}))
            .await
            .is_err()
    );
    a.dispatch(json!({"action":"groups.replicate","groupId":gid}))
        .await
        .unwrap();
    a.close().await;
    drop(a);
    send(&b, &gid, "signed after upgrade").await;
    let history = rows(&b, &gid).await;
    assert_eq!(history.len(), 2);
    assert_eq!(history[0]["authorization"], "legacy_unsigned");
    assert_eq!(history[1]["signatureVerified"], true);
    b.close().await;
}
