use aiconnect_core::{Config, Core};
use serde_json::{Value, json};
use tempfile::TempDir;
async fn peer() -> (TempDir, Core) {
    let dir = TempDir::new().unwrap();
    let c = Core::open(Config {
        directory: dir.path().into(),
        direct_only: true,
        relay_only: false,
        relay_url: None,
    })
    .await
    .unwrap();
    (dir, c)
}
async fn create(a: &Core) -> String {
    a.dispatch(json!({"action":"groups.create","name":"Agent group","ownerOnline":true}))
        .await
        .unwrap()["group"]["groupId"]
        .as_str()
        .unwrap()
        .into()
}
async fn invite(a: &Core, b: &Core, gid: &str) -> String {
    a.dispatch(json!({"action":"groups.invite","groupId":gid,"peer":b.id()}))
        .await
        .unwrap()["invite"]
        .as_str()
        .unwrap()
        .into()
}
async fn join(b: &Core, ticket: &str) {
    b.dispatch(json!({"action":"groups.join","invite":ticket}))
        .await
        .unwrap();
}
async fn send(c: &Core, gid: &str, body: &str) -> Value {
    c.dispatch(json!({"action":"groups.send","groupId":gid,"text":body}))
        .await
        .unwrap()
}
async fn messages(c: &Core, gid: &str) -> Vec<Value> {
    c.dispatch(json!({"action":"groups.messages","groupId":gid}))
        .await
        .unwrap()["messages"]
        .as_array()
        .unwrap()
        .clone()
}

#[tokio::test]
async fn offline_recipient_queue_survives_sender_restart_and_delivers_once() {
    let (_da, a) = peer().await;
    let (db, b) = peer().await;
    let (dc, c) = peer().await;
    let gid = create(&a).await;
    join(&b, &invite(&a, &b, &gid).await).await;
    join(&c, &invite(&a, &c, &gid).await).await;
    c.close().await;
    drop(c);
    let result = send(&b, &gid, "durable offline delivery").await;
    assert!(
        result["deliveries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["status"] == "pending")
    );
    assert_eq!(messages(&a, &gid).await.len(), 1);
    b.close().await;
    drop(b);
    let reopen = |dir: &TempDir| Config {
        directory: dir.path().into(),
        direct_only: true,
        relay_only: false,
        relay_url: None,
    };
    let c = Core::open(reopen(&dc)).await.unwrap();
    c.dispatch(json!({"action":"groups.sync","groupId":gid}))
        .await
        .unwrap();
    assert!(messages(&c, &gid).await.is_empty());
    let b = Core::open(reopen(&db)).await.unwrap();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(45);
    while messages(&c, &gid).await.is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "automatic retry did not recover after restart"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    b.dispatch(json!({"action":"retry"})).await.unwrap();
    assert_eq!(messages(&a, &gid).await.len(), 1);
    assert_eq!(messages(&c, &gid).await.len(), 1);
    for node in [&a, &b, &c] {
        node.close().await;
    }
}

#[tokio::test]
async fn group_roles_join_muting_removal_and_dissolution_enforced_by_owner() {
    let (_da, a) = peer().await;
    let (_db, b) = peer().await;
    let (_dc, c) = peer().await;
    let (_dd, d) = peer().await;
    let gid = create(&a).await;
    let ib = invite(&a, &b, &gid).await;
    assert!(
        d.dispatch(json!({"action":"groups.join","invite":ib}))
            .await
            .is_err()
    );
    join(&b, &ib).await;
    let ic = invite(&a, &c, &gid).await;
    join(&c, &ic).await;
    let id = uuid::Uuid::new_v4().to_string();
    let request = json!({"action":"groups.send","groupId":gid,"text":"B to everyone","id":id});
    let delivered = b.dispatch(request.clone()).await.unwrap();
    assert!(
        delivered["deliveries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|x| x["status"] == "acked")
    );
    b.dispatch(request).await.unwrap();
    for p in [&a, &b, &c] {
        let rows = messages(p, &gid).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["sender"], b.id());
        assert_eq!(rows[0]["executionAllowed"], false);
    }
    assert!(
        b.dispatch(json!({"action":"groups.invite","groupId":gid,"peer":d.id()}))
            .await
            .is_err()
    );
    a.dispatch(json!({"action":"groups.role","groupId":gid,"peer":b.id(),"role":"admin"}))
        .await
        .unwrap();
    join(&b, &ib).await;
    assert_eq!(
        b.dispatch(json!({"action":"groups.show","groupId":gid}))
            .await
            .unwrap()["group"]["members"][b.id()]["role"],
        "admin"
    );
    let idd = invite(&b, &d, &gid).await;
    a.dispatch(json!({"action":"groups.role","groupId":gid,"peer":b.id(),"role":"member"}))
        .await
        .unwrap();
    assert!(
        d.dispatch(json!({"action":"groups.join","invite":idd}))
            .await
            .is_err()
    );
    a.dispatch(json!({"action":"groups.role","groupId":gid,"peer":b.id(),"role":"admin"}))
        .await
        .unwrap();
    assert!(
        d.dispatch(json!({"action":"groups.join","invite":idd}))
            .await
            .is_err()
    );
    let idd = invite(&b, &d, &gid).await;
    join(&d, &idd).await;
    assert!(
        b.dispatch(json!({"action":"groups.role","groupId":gid,"peer":c.id(),"role":"admin"}))
            .await
            .is_err()
    );
    assert!(
        b.dispatch(json!({"action":"groups.remove","groupId":gid,"peer":a.id()}))
            .await
            .is_err()
    );
    b.dispatch(json!({"action":"groups.policy","groupId":gid,"peer":c.id(),"canSend":false}))
        .await
        .unwrap();
    assert!(
        c.dispatch(json!({"action":"groups.send","groupId":gid,"text":"muted"}))
            .await
            .is_err()
    );
    b.dispatch(json!({"action":"groups.policy","groupId":gid,"peer":c.id(),"canSend":true}))
        .await
        .unwrap();
    send(&c, &gid, "unmuted").await;
    b.dispatch(json!({"action":"groups.remove","groupId":gid,"peer":c.id()}))
        .await
        .unwrap();
    assert!(
        c.dispatch(json!({"action":"groups.send","groupId":gid,"text":"stale membership"}))
            .await
            .is_err()
    );
    assert!(
        c.dispatch(json!({"action":"groups.join","invite":ic}))
            .await
            .is_err()
    );
    assert!(
        c.dispatch(json!({"action":"groups.sync","groupId":gid}))
            .await
            .is_err()
    );
    // Removed members retain only their already-local history.
    assert_eq!(messages(&c, &gid).await.len(), 2);
    d.dispatch(json!({"action":"groups.leave","groupId":gid}))
        .await
        .unwrap();
    assert!(
        d.dispatch(json!({"action":"groups.join","invite":idd}))
            .await
            .is_err()
    );
    assert!(
        a.dispatch(json!({"action":"groups.leave","groupId":gid}))
            .await
            .is_err()
    );
    assert!(
        b.dispatch(json!({"action":"groups.dissolve","groupId":gid}))
            .await
            .is_err()
    );
    a.dispatch(json!({"action":"groups.dissolve","groupId":gid}))
        .await
        .unwrap();
    assert!(
        b.dispatch(json!({"action":"groups.send","groupId":gid,"text":"after dissolve"}))
            .await
            .is_err()
    );
    for p in [&a, &b, &c, &d] {
        p.close().await;
    }
}

#[tokio::test]
async fn queued_group_message_is_not_revived_by_unmuting_sender() {
    let (_da, a) = peer().await;
    let (_db, b) = peer().await;
    let (dc, c) = peer().await;
    let gid = create(&a).await;
    join(&b, &invite(&a, &b, &gid).await).await;
    join(&c, &invite(&a, &c, &gid).await).await;
    c.close().await;
    drop(c);
    let mid = uuid::Uuid::new_v4().to_string();
    let request =
        json!({"action":"groups.send","groupId":gid,"text":"queued while C offline","id":mid});
    let pending = b.dispatch(request.clone()).await.unwrap();
    assert!(
        pending["deliveries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["status"] == "pending")
    );
    a.dispatch(json!({"action":"groups.policy","groupId":gid,"peer":b.id(),"canSend":false}))
        .await
        .unwrap();
    let c = Core::open(Config {
        directory: dc.path().into(),
        direct_only: true,
        relay_only: false,
        relay_url: None,
    })
    .await
    .unwrap();
    b.dispatch(json!({"action":"retry"})).await.unwrap();
    a.dispatch(json!({"action":"groups.policy","groupId":gid,"peer":b.id(),"canSend":true}))
        .await
        .unwrap();
    let result = b.dispatch(request).await.unwrap();
    assert!(
        result["deliveries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["peer"] == c.id() && x["status"] == "rejected")
    );
    assert!(messages(&c, &gid).await.is_empty());
    send(&b, &gid, "new authorized message").await;
    assert_eq!(messages(&c, &gid).await.len(), 1);
    for p in [&a, &b, &c] {
        p.close().await;
    }
}

#[tokio::test]
async fn authority_restart_and_expired_invites_fail_closed() {
    let (da, a) = peer().await;
    let (_db, b) = peer().await;
    let (_dc, c) = peer().await;
    let gid = create(&a).await;
    let expired = a
        .dispatch(json!({"action":"groups.invite","groupId":gid,"peer":c.id(),"ttlSeconds":1}))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    assert!(
        c.dispatch(json!({"action":"groups.join","invite":expired["invite"]}))
            .await
            .is_err()
    );
    join(&b, &invite(&a, &b, &gid).await).await;
    send(&b, &gid, "before restart").await;
    a.close().await;
    drop(a);
    assert!(
        b.dispatch(json!({"action":"groups.send","groupId":gid,"text":"owner unavailable"}))
            .await
            .is_err()
    );
    assert_eq!(messages(&b, &gid).await.len(), 1);
    let a = Core::open(Config {
        directory: da.path().into(),
        direct_only: true,
        relay_only: false,
        relay_url: None,
    })
    .await
    .unwrap();
    send(&b, &gid, "after restart").await;
    assert_eq!(messages(&a, &gid).await.len(), 2);
    assert_eq!(
        a.dispatch(json!({"action":"groups.show","groupId":gid}))
            .await
            .unwrap()["group"]["members"][b.id()]["role"],
        "member"
    );
    for p in [&a, &b, &c] {
        p.close().await;
    }
}

#[tokio::test]
async fn configured_limit_is_signed_and_enforced_without_consuming_failed_invite() {
    let (_da, a) = peer().await;
    let (_db, b) = peer().await;
    let (_dc, c) = peer().await;
    assert!(
        a.dispatch(json!({"action":"groups.create","name":"invalid","maxMembers":65}))
            .await
            .is_err()
    );
    let created = a
        .dispatch(json!({"action":"groups.create","name":"two only","maxMembers":2}))
        .await
        .unwrap();
    let gid = created["group"]["groupId"].as_str().unwrap();
    join(&b, &invite(&a, &b, gid).await).await;
    let token = invite(&a, &c, gid).await;
    assert!(
        c.dispatch(json!({"action":"groups.join","invite":token}))
            .await
            .is_err()
    );
    assert_eq!(
        a.dispatch(json!({"action":"groups.show","groupId":gid}))
            .await
            .unwrap()["group"]["members"]
            .as_object()
            .unwrap()
            .len(),
        2
    );
    b.dispatch(json!({"action":"groups.leave","groupId":gid}))
        .await
        .unwrap();
    join(&c, &token).await;
    let joined = c
        .dispatch(json!({"action":"groups.show","groupId":gid}))
        .await
        .unwrap();
    assert_eq!(joined["group"]["maxMembers"], 2);
    assert_eq!(joined["group"]["members"].as_object().unwrap().len(), 2);
    for p in [&a, &b, &c] {
        p.close().await;
    }
}
