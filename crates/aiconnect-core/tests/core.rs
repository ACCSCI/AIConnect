use aiconnect_core::{Config, Core};
use serde_json::json;
use tempfile::TempDir;
#[tokio::test]
async fn direct_only_rejects_custom_relay_before_initializing_identity() {
    let dir = TempDir::new().unwrap();
    let result = Core::open(Config {
        directory: dir.path().into(),
        direct_only: true,
        relay_only: false,
        relay_url: Some("https://example.com".into()),
    })
    .await;
    assert!(
        result
            .err()
            .unwrap()
            .to_string()
            .contains("direct_only_conflicts")
    );
    assert!(!dir.path().join("identity.key").exists());
}
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

#[tokio::test]
async fn temporary_p2p_ack_dedup_revoke() {
    let (_a, a) = peer().await;
    let (_b, b) = peer().await;
    let (_c, c) = peer().await;
    let invite = a
        .dispatch(json!({"action":"temp.create","ttlSeconds":60}))
        .await
        .unwrap();
    let session = b
        .dispatch(json!({"action":"temp.join","invite":invite["invite"]}))
        .await
        .unwrap();
    let sid = &session["session"]["id"];
    assert!(
        c.dispatch(json!({"action":"temp.join","invite":invite["invite"]}))
            .await
            .unwrap_err()
            .to_string()
            .contains("invite_already_used")
    );
    let msg = json!({"action":"send","sessionId":sid,"text":"hello","id":"stable-id"});
    assert_eq!(b.dispatch(msg.clone()).await.unwrap()["status"], "acked");
    b.dispatch(msg).await.unwrap();
    assert_eq!(
        a.dispatch(json!({"action":"messages","sessionId":sid}))
            .await
            .unwrap()["messages"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        b.dispatch(json!({"action":"status"})).await.unwrap()["lastPath"]["direct"],
        true
    );
    assert!(
        b.dispatch(json!({"action":"send","sessionId":sid,"text":"task","kind":"task"}))
            .await
            .is_err()
    );
    a.dispatch(json!({"action":"session.close","sessionId":sid}))
        .await
        .unwrap();
    assert!(
        b.dispatch(json!({"action":"send","sessionId":sid,"text":"late"}))
            .await
            .is_err()
    );
    a.close().await;
    b.close().await;
    c.close().await;
}

#[tokio::test]
async fn contacts_permissions_block_and_profile_persist() {
    let (da, a) = peer().await;
    let (_db, b) = peer().await;
    let ac = a
        .dispatch(json!({"action":"identity","name":"Alice"}))
        .await
        .unwrap();
    let bc = b
        .dispatch(json!({"action":"identity","name":"Bob"}))
        .await
        .unwrap();
    a.dispatch(json!({"action":"contacts.import","card":bc}))
        .await
        .unwrap();
    b.dispatch(json!({"action":"contacts.import","card":ac}))
        .await
        .unwrap();
    let s = a
        .dispatch(json!({"action":"connect","id":b.id()}))
        .await
        .unwrap();
    let sid = &s["session"]["id"];
    assert!(
        a.dispatch(
            json!({"action":"send","sessionId":sid,"text":"task","kind":"task","id":"denied"})
        )
        .await
        .is_err()
    );
    assert_eq!(
        a.dispatch(json!({"action":"messages","sessionId":sid}))
            .await
            .unwrap()["messages"][0]["status"],
        "rejected"
    );
    b.dispatch(json!({"action":"contacts.policy","id":a.id(),"wake":true,"task":true}))
        .await
        .unwrap();
    a.dispatch(json!({"action":"send","sessionId":sid,"text":"allowed task","kind":"task"}))
        .await
        .unwrap();
    let inbox = b.dispatch(json!({"action":"inbox"})).await.unwrap();
    assert_eq!(inbox["messages"][0]["taskAllowed"], true);
    // Model an already persisted outbox item from a transient outage. Explicit block
    // must cancel it, so a later unblock cannot revive the old request.
    rusqlite::Connection::open(_db.path().join("local.sqlite")).unwrap().execute(
        "INSERT INTO messages(id,session_id,sender,text,kind,created,status) VALUES(?,?,?,?,?,?,?)",
        rusqlite::params!["queued-before-block",sid.as_str().unwrap(),b.id(),"old queued text","text",0,"pending"],
    ).unwrap();
    b.dispatch(json!({"action":"contacts.block","id":a.id(),"blocked":true}))
        .await
        .unwrap();
    let history = b
        .dispatch(json!({"action":"messages","sessionId":sid}))
        .await
        .unwrap();
    assert_eq!(
        history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["id"] == "queued-before-block")
            .unwrap()["status"],
        "rejected"
    );
    assert_eq!(
        b.dispatch(json!({"action":"inbox"})).await.unwrap()["messages"][0]["taskAllowed"],
        false
    );
    assert!(
        a.dispatch(json!({"action":"send","sessionId":sid,"text":"blocked"}))
            .await
            .is_err()
    );
    b.dispatch(json!({"action":"session.close","sessionId":sid}))
        .await
        .unwrap();
    let id = a.id();
    a.close().await;
    drop(a);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let reopened = Core::open(Config {
        directory: da.path().into(),
        direct_only: true,
        relay_only: false,
        relay_url: None,
    })
    .await
    .unwrap();
    assert_eq!(reopened.id(), id);
    let card = reopened
        .dispatch(json!({"action":"identity"}))
        .await
        .unwrap();
    use base64::Engine;
    let body: serde_json::Value = serde_json::from_slice(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(card["body"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["name"], "Alice");
    reopened.close().await;
    b.close().await;
}

#[tokio::test]
async fn large_history_has_bounded_pages_and_constant_size_tail_cursor() {
    let (dir, core) = peer().await;
    let mut db = rusqlite::Connection::open(dir.path().join("local.sqlite")).unwrap();
    let tx = db.transaction().unwrap();
    let payload = "x".repeat(16000);
    for index in 0..100 {
        tx.execute("INSERT INTO messages(id,session_id,sender,text,kind,created,status) VALUES(?,?,?,?,?,?,?)",rusqlite::params![format!("bulk-{index}"),"bulk-session","external-peer",payload,"text",0,"received"]).unwrap();
    }
    tx.commit().unwrap();
    assert_eq!(
        core.dispatch(json!({"action":"inbox.cursor"}))
            .await
            .unwrap()["cursor"],
        100
    );
    let mut after = 0;
    let mut received = 0;
    loop {
        let page = core
            .dispatch(json!({"action":"inbox","after":after}))
            .await
            .unwrap();
        assert!(serde_json::to_vec(&page).unwrap().len() < 450000);
        received += page["messages"].as_array().unwrap().len();
        after = page["nextAfter"].as_i64().unwrap();
        if page["hasMore"] == false {
            break;
        }
    }
    assert_eq!(received, 100);
    assert_eq!(after, 100);
    core.close().await;
}

#[tokio::test]
async fn rejects_legacy_node_schema_without_modifying_it() {
    let dir = TempDir::new().unwrap();
    let db = rusqlite::Connection::open(dir.path().join("local.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE messages(id TEXT PRIMARY KEY,session_id TEXT,sender TEXT,text TEXT,created INTEGER,status TEXT);INSERT INTO messages VALUES('old','session','peer','keep this',0,'received');").unwrap();
    let result = Core::open(Config {
        directory: dir.path().into(),
        direct_only: true,
        relay_only: false,
        relay_url: None,
    })
    .await;
    assert!(
        result
            .err()
            .unwrap()
            .to_string()
            .contains("incompatible_profile_use_new_data_directory")
    );
    assert!(!dir.path().join("identity.key").exists());
    assert_eq!(
        db.query_row("SELECT text FROM messages WHERE id='old'", [], |row| row
            .get::<_, String>(
            0
        ))
        .unwrap(),
        "keep this"
    );
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name='settings'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}
