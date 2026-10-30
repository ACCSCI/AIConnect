use aiconnect_core::{Config, Core};
use serde_json::{Value, json};
use tempfile::TempDir;

async fn peer(d: &TempDir) -> Core {
    Core::open(Config {
        directory: d.path().into(),
        direct_only: true,
        relay_only: false,
        relay_url: None,
    })
    .await
    .unwrap()
}
async fn setup(c: &Core) -> Value {
    c.dispatch(json!({"action":"owner.init"})).await.unwrap();
    c.dispatch(json!({"action":"provider.add","url":"https://example.com/a2a","free":true,"termsAllowed":true,"termsUrl":"https://example.com/terms","purpose":"local fixture; no HTTP requests"})).await.unwrap();
    let grant=c.dispatch(json!({"action":"owner.grant","agentId":c.id(),"audience":"https://example.com/a2a","scopes":["service:fixture.echo"],"maxCalls":10,"ttlSeconds":120})).await.unwrap();
    json!({"action":"service.invoke","transport":"a2a","provider":"https://example.com/a2a","audience":"https://example.com/a2a","capability":"fixture.echo","input":{"text":"hello"},"declaredPrice":{"amount":0,"currency":"USD"},"grantId":grant["id"],"idempotencyKey":"fixture-job","ttlSeconds":120})
}

#[tokio::test]
async fn durable_claim_delivery_review_and_idempotency() {
    let d = TempDir::new().unwrap();
    let c = peer(&d).await;
    let req = setup(&c).await;
    let first = c.dispatch(req.clone()).await.unwrap();
    let id = first["job"]["jobId"].clone();
    assert_eq!(first["job"]["state"], "queued");
    assert_eq!(first["executionStarted"], false);
    let retry = c.dispatch(req.clone()).await.unwrap();
    assert_eq!(retry["job"]["jobId"], id);
    assert_eq!(retry["idempotent"], true);
    let mut changed = req.clone();
    changed["input"] = json!({"text":"changed"});
    assert!(c.dispatch(changed).await.is_err());
    assert!(
        c.dispatch(json!({"action":"service.update","jobId":id,"providerStatus":"completed"}))
            .await
            .is_err()
    );
    c.dispatch(json!({"action":"service.dispatch","jobId":id}))
        .await
        .unwrap();
    assert!(
        c.dispatch(json!({"action":"service.dispatch","jobId":id}))
            .await
            .is_err()
    );
    let completed=c.dispatch(json!({"action":"service.update","jobId":id,"providerStatus":"completed","text":"answer","artifacts":[{"name":"result.txt","sha256":"a".repeat(64),"size":6}]})).await.unwrap();
    assert_eq!(completed["job"]["state"], "delivered");
    assert_eq!(completed["job"]["localReviewStatus"], "awaiting_review");
    c.dispatch(json!({"action":"service.snapshot","jobId":id,"externalTask":{"id":"remote1","status":{"state":"completed"}}})).await.unwrap();
    c.close().await;
    drop(c);
    let c = peer(&d).await;
    let persisted = c
        .dispatch(json!({"action":"service.status","jobId":id}))
        .await
        .unwrap();
    assert_eq!(persisted["job"]["state"], "delivered");
    let reviewed = c
        .dispatch(json!({"action":"service.review","jobId":id,"approve":true}))
        .await
        .unwrap();
    assert_eq!(reviewed["job"]["state"], "completed");
    assert!(
        c.dispatch(json!({"action":"service.update","jobId":id,"providerStatus":"working"}))
            .await
            .is_err()
    );
    c.close().await;
}

#[tokio::test]
async fn fee_schema_grant_cancel_and_revocation_fail_closed() {
    let d = TempDir::new().unwrap();
    let c = peer(&d).await;
    let req = setup(&c).await;
    for price in [Value::Null, json!({"amount":1,"currency":"USD"})] {
        let mut r = req.clone();
        r["declaredPrice"] = price;
        assert!(c.dispatch(r).await.is_err());
    }
    let mut r = req.clone();
    r["inputSchema"] = json!({"type":"object","required":["missing"]});
    assert!(c.dispatch(r).await.is_err());
    let mut r = req.clone();
    r["inputSchema"] = json!({"$ref":"https://example.com/schema"});
    assert!(c.dispatch(r).await.is_err());
    let mut r = req.clone();
    r["grantId"] = json!("unknown");
    assert!(c.dispatch(r).await.is_err());
    let first = c.dispatch(req.clone()).await.unwrap();
    let id = first["job"]["jobId"].clone();
    let cancel = c
        .dispatch(json!({"action":"service.cancel","jobId":id}))
        .await
        .unwrap();
    assert_eq!(cancel["guaranteedRollback"], false);
    assert_eq!(cancel["job"]["cancelRequested"], true);
    assert!(
        c.dispatch(json!({"action":"service.dispatch","jobId":id}))
            .await
            .is_err()
    );
    let mut r = req.clone();
    r["idempotencyKey"] = json!("second");
    let job = c.dispatch(r).await.unwrap();
    c.dispatch(json!({"action":"owner.revoke","grantId":req["grantId"]}))
        .await
        .unwrap();
    assert!(
        c.dispatch(json!({"action":"service.dispatch","jobId":job["job"]["jobId"]}))
            .await
            .is_err()
    );
    c.close().await;
}

#[tokio::test]
async fn native_job_uses_signed_task_and_requester_review() {
    let da = TempDir::new().unwrap();
    let db = TempDir::new().unwrap();
    let a = peer(&da).await;
    let b = peer(&db).await;
    let ac = a.dispatch(json!({"action":"identity"})).await.unwrap();
    let bc = b.dispatch(json!({"action":"identity"})).await.unwrap();
    a.dispatch(json!({"action":"contacts.import","card":bc}))
        .await
        .unwrap();
    b.dispatch(json!({"action":"contacts.import","card":ac}))
        .await
        .unwrap();
    let session = a
        .dispatch(json!({"action":"connect","id":b.id()}))
        .await
        .unwrap()["session"]["id"]
        .clone();
    a.dispatch(json!({"action":"owner.init"})).await.unwrap();
    let grant=a.dispatch(json!({"action":"owner.grant","agentId":a.id(),"audience":b.id(),"scopes":["service:fixture.echo"],"maxCalls":2})).await.unwrap();
    let descriptor=b.dispatch(json!({"action":"capability.publish","skills":[{"skillId":"fixture.echo","description":"Echo fixture","inputSchema":{"type":"object","required":["text"],"properties":{"text":{"type":"string"}}},"declaredPrice":{"amount":0,"currency":"USD"}}]})).await.unwrap()["descriptor"].clone();
    let req = json!({"action":"service.invoke","transport":"native","sessionId":session,"audience":b.id(),"capability":"fixture.echo","input":{"text":"hello"},"declaredPrice":{"amount":0,"currency":"USD"},"grantId":grant["id"],"idempotencyKey":"native","descriptor":descriptor});
    let mut no_descriptor = req.clone();
    no_descriptor.as_object_mut().unwrap().remove("descriptor");
    assert!(a.dispatch(no_descriptor).await.is_err());
    let mut bad_input = req.clone();
    bad_input["input"] = json!({"text":1});
    bad_input["inputSchema"] = json!(true);
    assert!(a.dispatch(bad_input).await.is_err());
    let job = a.dispatch(req.clone()).await.unwrap();
    let id = job["job"]["jobId"].clone();
    assert_eq!(job["job"]["state"], "submitted");
    assert_eq!(
        b.dispatch(json!({"action":"tasks.show","taskId":id}))
            .await
            .unwrap()["task"]["state"],
        "proposed"
    );
    b.dispatch(json!({"action":"tasks.accept","taskId":id}))
        .await
        .unwrap();
    b.dispatch(json!({"action":"tasks.deliver","taskId":id,"text":"native result"}))
        .await
        .unwrap();
    let status = a
        .dispatch(json!({"action":"service.status","jobId":id}))
        .await
        .unwrap();
    assert_eq!(status["job"]["state"], "delivered");
    a.dispatch(json!({"action":"service.review","jobId":id,"approve":true}))
        .await
        .unwrap();
    assert_eq!(
        b.dispatch(json!({"action":"tasks.show","taskId":id}))
            .await
            .unwrap()["task"]["state"],
        "completed"
    );
    // Two individually valid signed directories disagree at the same revision.
    // A service call must not pick whichever source SQLite happens to return.
    let mut fork = req["descriptor"].clone();
    fork["payload"]["skills"][0]["description"] = json!("Conflicting description");
    let bytes: [u8; 32] = std::fs::read(db.path().join("identity.key"))
        .unwrap()
        .try_into()
        .unwrap();
    let key = iroh::SecretKey::from_bytes(&bytes);
    use base64::Engine;
    fork["signature"] = json!(
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            key.sign(&serde_jcs::to_vec(&fork["payload"]).unwrap())
                .to_bytes()
        )
    );
    let sql = rusqlite::Connection::open(da.path().join("local.sqlite")).unwrap();
    sql.execute_batch("CREATE TABLE IF NOT EXISTS discovery_cache(agent_id TEXT NOT NULL,source_id TEXT NOT NULL,descriptor TEXT NOT NULL,verified_at INTEGER NOT NULL,PRIMARY KEY(agent_id,source_id));").unwrap();
    for (source, document) in [("first", req["descriptor"].clone()), ("second", fork)] {
        sql.execute(
            "INSERT INTO discovery_cache VALUES(?,?,?,0)",
            rusqlite::params![b.id(), source, document.to_string()],
        )
        .unwrap();
    }
    drop(sql);
    let mut conflict = req;
    conflict["idempotencyKey"] = json!("forked-sources");
    assert!(
        a.dispatch(conflict)
            .await
            .unwrap_err()
            .to_string()
            .contains("service_descriptor_revision_fork")
    );
    a.close().await;
    b.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_invocations_share_one_job_and_one_budget_reservation() {
    let d = TempDir::new().unwrap();
    let c = peer(&d).await;
    let req = setup(&c).await;
    let mut handles = Vec::new();
    for _ in 0..8 {
        let c = c.clone();
        let req = req.clone();
        handles.push(tokio::spawn(async move { c.dispatch(req).await }));
    }
    let mut id = None;
    for handle in handles {
        let value = handle.await.unwrap().unwrap();
        let actual = value["job"]["jobId"].clone();
        if let Some(expected) = &id {
            assert_eq!(expected, &actual);
        } else {
            id = Some(actual);
        }
    }
    let jobs = c.dispatch(json!({"action":"service.list"})).await.unwrap();
    assert_eq!(jobs["jobs"].as_array().unwrap().len(), 1);
    let budget = c
        .dispatch(json!({"action":"budget.inspect"}))
        .await
        .unwrap();
    assert_eq!(budget["dailyExecutions"], 1);
    c.close().await;
}
