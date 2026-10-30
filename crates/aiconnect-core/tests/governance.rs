use aiconnect_core::{Config, Core};
use serde_json::json;

async fn core(path: &std::path::Path) -> Core {
    Core::open(Config {
        directory: path.into(),
        direct_only: true,
        relay_only: false,
        relay_url: None,
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn budget_survives_restart_and_releases_do_not_reset_turns() {
    let dir = tempfile::tempdir().unwrap();
    let c = core(dir.path()).await;
    c.governance_request(&json!({"action":"policy.set","policy":{"maxConversationTurns":2,"dailyExecutions":3,"maxReserved":1}})).unwrap();
    let a=c.governance_request(&json!({"action":"budget.reserve","peer":"peer","resourceAction":"execute","requestId":"first","conversationId":"chat"})).unwrap();
    assert!(
        c.governance_admit("peer", None, "execute", "blocked")
            .is_err()
    );
    c.governance_complete(a["reservationId"].as_str().unwrap(), false)
        .unwrap();
    let second=c.governance_request(&json!({"action":"budget.reserve","peer":"peer","requestId":"second","conversationId":"chat"})).unwrap();
    c.governance_complete(second["reservationId"].as_str().unwrap(), true)
        .unwrap();
    assert!(c.governance_request(&json!({"action":"budget.reserve","peer":"peer","requestId":"third","conversationId":"chat"})).unwrap_err().to_string().contains("turn_limit"));
    c.close().await;
    drop(c);
    let c = core(dir.path()).await;
    assert_eq!(
        c.governance_request(&json!({"action":"budget.inspect"}))
            .unwrap()["dailyExecutions"],
        2
    );
    assert!(c.governance_request(&json!({"action":"budget.reserve","peer":"peer","requestId":"third","conversationId":"chat"})).is_err());
    let replay = c
        .governance_admit("peer", None, "execute", "first")
        .unwrap();
    assert_eq!(replay["idempotent"], true);
    assert!(
        c.governance_admit("attacker", None, "execute", "first")
            .is_err()
    );
    c.close().await;
}

#[tokio::test]
async fn free_provider_explicit_policy_and_ssrf() {
    let dir = tempfile::tempdir().unwrap();
    let c = core(dir.path()).await;
    assert!(c.governance_request(&json!({"action":"provider.add","url":"https://example.org","free":false,"termsAllowed":true,"termsUrl":"https://example.org/terms","purpose":"directory"})).is_err());
    c.governance_request(&json!({"action":"provider.add","url":"https://example.org","free":true,"termsAllowed":true,"termsUrl":"https://example.org/terms","purpose":"directory"})).unwrap();
    assert!(
        c.governance_provider_check("https://example.org/path")
            .is_ok()
    );
    assert!(
        c.governance_provider_check("https://different.example.org/path")
            .is_err()
    );
    c.governance_request(
        &json!({"action":"provider.check","url":"https://example.org","statusCode":429}),
    )
    .unwrap();
    assert!(
        c.governance_provider_check("https://example.org/path")
            .is_err()
    );
    c.governance_request(&json!({"action":"provider.add","url":"https://127.0.0.1","free":true,"termsAllowed":true,"termsUrl":"https://example.org/terms","purpose":"test_ssrf"})).unwrap();
    assert!(
        c.governance_http_get("https://127.0.0.1/secret", None, None, 1024)
            .await
            .unwrap_err()
            .to_string()
            .contains("ssrf")
    );
    assert!(
        c.governance_http_get("http://example.org", None, None, 1024)
            .await
            .is_err()
    );
    let audit = c
        .governance_request(&json!({"action":"audit.export"}))
        .unwrap();
    assert!(audit["events"].as_array().unwrap().len() >= 3);
    c.close().await;
}

#[tokio::test]
async fn imported_policy_requires_adoption_and_unknown_inbox_never_grants_execution() {
    let dir = tempfile::tempdir().unwrap();
    let c = core(dir.path()).await;
    let proposal=c.governance_request(&json!({"action":"policy.import","policy":{"dailyExecutions":1},"label":"organization"})).unwrap();
    assert_eq!(
        c.governance_request(&json!({"action":"policy.inspect"}))
            .unwrap()["policy"]["dailyExecutions"],
        200
    );
    c.governance_request(&json!({"action":"policy.adopt","proposalId":proposal["proposalId"]}))
        .unwrap();
    assert_eq!(
        c.governance_request(&json!({"action":"policy.inspect"}))
            .unwrap()["policy"]["dailyExecutions"],
        1
    );
    let inbox = c.governance_contact_request("unknown-peer").unwrap();
    assert_eq!(inbox["wake"], false);
    let decision = c
        .governance_request(
            &json!({"action":"inbox.decide","id":inbox["inboxId"],"decision":"allow"}),
        )
        .unwrap();
    assert_eq!(decision["executionAuthorized"], false);
    assert_eq!(decision["contactAdded"], false);
    assert_eq!(
        c.governance_request(&json!({"action":"inbox.list"}))
            .unwrap()["requests"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    c.close().await;
}
