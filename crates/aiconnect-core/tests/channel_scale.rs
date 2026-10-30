use aiconnect_core::{Config, Core};
use serde_json::json;
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

#[tokio::test]
async fn public_channel_has_no_member_registry_and_supports_1000_independent_subscriptions() {
    let (_a, a) = peer().await;
    let (_b, b) = peer().await;
    let created = a
        .channel_request(
            &json!({"action":"channels.create","visibility":"public","name":"public updates"}),
        )
        .await
        .unwrap();
    assert!(created["channel"]["subscriberLimit"].is_null());
    assert_eq!(created["channel"]["publisherFanout"], false);
    let id = created["channelId"].as_str().unwrap();
    a.channel_request(&json!({"action":"channels.publish","channelId":id,"text":"One signed event for every subscriber"})).await.unwrap();
    let export = a
        .channel_request(&json!({"action":"channels.export","channelId":id}))
        .await
        .unwrap();
    let publisher = a.id();
    a.close().await;
    drop(a);
    for n in 0..1000 {
        let source = format!("reader-{n}");
        b.channel_request(&json!({"action":"channels.subscribe","sourceId":source,"url":"https://example.com/channel.json","expectedPublisher":publisher,"mirrors":["https://mirror.example.com/channel.json"]})).await.unwrap();
        let imported=b.channel_request(&json!({"action":"channels.import","sourceId":source,"body":export["files"]["channel.json"]})).await.unwrap();
        assert_eq!(imported["inserted"], 1);
        assert_eq!(imported["executed"], false);
    }
    let subs = b
        .channel_request(&json!({"action":"channels.subscriptions"}))
        .await
        .unwrap();
    assert_eq!(subs["subscriptions"].as_array().unwrap().len(), 1000);
    let dedup=b.channel_request(&json!({"action":"channels.import","sourceId":"reader-999","body":export["files"]["channel.json"]})).await.unwrap();
    assert_eq!(dedup["inserted"], 0);
    b.close().await;
}

#[tokio::test]
async fn mirrors_require_pinned_identity_and_do_not_turn_public_channels_into_groups() {
    let (_a, a) = peer().await;
    assert!(a.channel_request(&json!({"action":"channels.create","name":"public","visibility":"public","maxMembers":64})).await.is_err());
    assert!(a.channel_request(&json!({"action":"channels.subscribe","url":"https://example.com/channel.json","mirrors":["https://mirror.example.com/channel.json"]})).await.is_err());
    assert!(a.channel_request(&json!({"action":"channels.subscribe","url":"https://example.com/channel.json","expectedPublisher":a.id(),"mirrors":["http://example.com/channel.json"]})).await.is_err());
    let private = a
        .channel_request(&json!({"action":"channels.create","name":"private"}))
        .await
        .unwrap();
    assert_eq!(private["group"]["maxMembers"], 64);
    a.close().await;
}
