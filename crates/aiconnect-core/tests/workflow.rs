use aiconnect_core::{Config, Core};
use serde_json::{Value, json};
use tempfile::TempDir;

async fn peer() -> (TempDir, Core) {
    let d = TempDir::new().unwrap();
    let c = Core::open(Config {
        directory: d.path().into(),
        direct_only: true,
        relay_only: false,
        relay_url: None,
    })
    .await
    .unwrap();
    (d, c)
}
async fn pair(a: &Core, b: &Core) -> String {
    let ac = a.dispatch(json!({"action":"identity"})).await.unwrap();
    let bc = b.dispatch(json!({"action":"identity"})).await.unwrap();
    a.dispatch(json!({"action":"contacts.import","card":bc}))
        .await
        .unwrap();
    b.dispatch(json!({"action":"contacts.import","card":ac}))
        .await
        .unwrap();
    a.dispatch(json!({"action":"connect","id":b.id()}))
        .await
        .unwrap()["session"]["id"]
        .as_str()
        .unwrap()
        .into()
}
async fn state(c: &Core, id: &Value) -> Value {
    c.dispatch(json!({"action":"tasks.show","taskId":id}))
        .await
        .unwrap()["task"]
        .clone()
}

#[tokio::test]
async fn confirmation_roles_revision_delivery_and_review_are_distinct() {
    let (_da, a) = peer().await;
    let (_db, b) = peer().await;
    let sid = pair(&a, &b).await;
    let proposed=a.dispatch(json!({"action":"tasks.propose","sessionId":sid,"title":"Inventory","text":"List fixture settings","contractId":"future-contract","ttlSeconds":60})).await.unwrap();
    let id = proposed["task"]["taskId"].clone();
    assert_eq!(state(&b, &id).await["state"], "proposed");
    assert_eq!(state(&b, &id).await["contractId"], "future-contract");
    assert_eq!(proposed["delivery"]["status"], "acked");
    assert!(
        a.dispatch(json!({"action":"tasks.accept","taskId":id}))
            .await
            .is_err()
    );
    assert!(
        b.dispatch(json!({"action":"tasks.deliver","taskId":id,"text":"too early"}))
            .await
            .is_err()
    );
    let accept_id = uuid::Uuid::new_v4().to_string();
    b.dispatch(json!({"action":"tasks.accept","taskId":id,"eventId":accept_id}))
        .await
        .unwrap();
    assert_eq!(state(&a, &id).await["state"], "accepted");
    assert!(
        a.dispatch(json!({"action":"tasks.approve","taskId":id,"text":"no delivery"}))
            .await
            .is_err()
    );
    let deliver_id = uuid::Uuid::new_v4().to_string();
    b.dispatch(json!({"action":"tasks.deliver","taskId":id,"text":"v1 inventory","eventId":deliver_id,"sha256":"a".repeat(64)})).await.unwrap();
    assert_eq!(state(&a, &id).await["state"], "delivered");
    assert!(
        b.dispatch(json!({"action":"tasks.approve","taskId":id,"text":"self approve"}))
            .await
            .is_err()
    );
    a.dispatch(json!({"action":"tasks.reject","taskId":id,"text":"Add version"}))
        .await
        .unwrap();
    assert_eq!(state(&b, &id).await["state"], "changes_requested");
    b.dispatch(json!({"action":"tasks.deliver","taskId":id,"text":"v2 inventory"}))
        .await
        .unwrap();
    a.dispatch(json!({"action":"tasks.approve","taskId":id,"text":"Format and contents checked"}))
        .await
        .unwrap();
    assert_eq!(state(&b, &id).await["state"], "completed");
    // Idempotent event replay must not regress a completed task to accepted.
    b.dispatch(json!({"action":"tasks.accept","taskId":id,"eventId":accept_id}))
        .await
        .unwrap();
    assert_eq!(state(&a, &id).await["state"], "completed");
    assert!(
        b.dispatch(
            json!({"action":"tasks.deliver","taskId":id,"text":"changed","eventId":deliver_id})
        )
        .await
        .is_err()
    );
    let history = a
        .dispatch(json!({"action":"tasks.history","taskId":id}))
        .await
        .unwrap();
    assert_eq!(history["messages"].as_array().unwrap().len(), 6);
    assert_eq!(state(&a, &id).await["revision"], 6);
    let inbox = b.dispatch(json!({"action":"inbox"})).await.unwrap();
    assert!(
        inbox["messages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["kind"] == "task_event")
    );
    a.close().await;
    b.close().await;
}

#[tokio::test]
async fn declined_expired_and_temporary_tasks_cannot_progress() {
    let (_da, a) = peer().await;
    let (_db, b) = peer().await;
    let sid = pair(&a, &b).await;
    let p=a.dispatch(json!({"action":"tasks.propose","sessionId":sid,"text":"Optional task","ttlSeconds":60})).await.unwrap();
    let id = &p["task"]["taskId"];
    b.dispatch(json!({"action":"tasks.decline","taskId":id,"text":"Unavailable"}))
        .await
        .unwrap();
    assert_eq!(state(&a, id).await["state"], "declined");
    assert!(
        b.dispatch(json!({"action":"tasks.accept","taskId":id}))
            .await
            .is_err()
    );
    let p = a
        .dispatch(json!({"action":"tasks.propose","sessionId":sid,"text":"Expires","ttlSeconds":1}))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    assert_eq!(state(&b, &p["task"]["taskId"]).await["state"], "expired");
    assert!(
        b.dispatch(json!({"action":"tasks.accept","taskId":p["task"]["taskId"]}))
            .await
            .is_err()
    );
    let invite = a.dispatch(json!({"action":"temp.create"})).await.unwrap();
    let temp = b
        .dispatch(json!({"action":"temp.join","invite":invite["invite"]}))
        .await
        .unwrap();
    assert!(a.dispatch(json!({"action":"tasks.propose","sessionId":temp["session"]["id"],"text":"not permitted"})).await.is_err());
    a.close().await;
    b.close().await;
}

#[tokio::test]
async fn workflow_persists_and_rejects_spoofed_events_atomically() {
    let (da, a) = peer().await;
    let (_db, b) = peer().await;
    let sid = pair(&a, &b).await;
    let fixed = uuid::Uuid::new_v4().to_string();
    let p=a.dispatch(json!({"action":"tasks.propose","sessionId":sid,"text":"Persist this","eventId":fixed,"ttlSeconds":60})).await.unwrap();
    a.dispatch(json!({"action":"tasks.propose","sessionId":sid,"text":"Persist this","eventId":fixed,"ttlSeconds":60})).await.unwrap();
    assert!(a.dispatch(json!({"action":"tasks.propose","sessionId":sid,"text":"Rewritten terms","eventId":fixed,"ttlSeconds":60})).await.is_err());
    let id = p["task"]["taskId"].clone();
    let bad_id = uuid::Uuid::new_v4().to_string();
    let forged = json!({"v":1,"eventId":bad_id,"taskId":id,"sessionId":sid,"actor":a.id(),"created":0,"type":"accepted","revision":2});
    assert!(a.dispatch(json!({"action":"send","sessionId":sid,"id":bad_id,"kind":"task_event","text":forged.to_string()})).await.is_err());
    assert_eq!(state(&a, &id).await["state"], "proposed");
    assert_eq!(
        a.dispatch(json!({"action":"tasks.history","taskId":id}))
            .await
            .unwrap()["messages"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    a.close().await;
    drop(a);
    let restored = Core::open(Config {
        directory: da.path().into(),
        direct_only: true,
        relay_only: false,
        relay_url: None,
    })
    .await
    .unwrap();
    assert_eq!(state(&restored, &id).await["state"], "proposed");
    assert_eq!(state(&restored, &id).await["lastEventDelivery"], "acked");
    restored.close().await;
    b.close().await;
}

#[tokio::test]
async fn proposal_outbox_recovers_without_implicit_acceptance() {
    let (_da, a) = peer().await;
    let (db, b) = peer().await;
    let sid = pair(&a, &b).await;
    b.close().await;
    drop(b);
    let pending=a.dispatch(json!({"action":"tasks.propose","sessionId":sid,"text":"Offline proposal","ttlSeconds":120})).await.unwrap();
    assert_eq!(pending["delivery"]["status"], "pending");
    let id = pending["task"]["taskId"].clone();
    assert_eq!(state(&a, &id).await["state"], "proposed");
    let restored = Core::open(Config {
        directory: db.path().into(),
        direct_only: true,
        relay_only: false,
        relay_url: None,
    })
    .await
    .unwrap();
    a.dispatch(json!({"action":"retry"})).await.unwrap();
    assert_eq!(state(&restored, &id).await["state"], "proposed");
    restored
        .dispatch(json!({"action":"tasks.accept","taskId":id}))
        .await
        .unwrap();
    assert_eq!(state(&a, &id).await["state"], "accepted");
    assert_eq!(
        a.dispatch(json!({"action":"tasks.history","taskId":id}))
            .await
            .unwrap()["messages"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    a.close().await;
    restored.close().await;
}
