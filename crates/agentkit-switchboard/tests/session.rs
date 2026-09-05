use agentkit_switchboard::session::sqlite::SqliteSessionManager;
use agentkit_switchboard::session::RoutingEvent;
use sqlx::SqlitePool;

async fn test_session_manager(sm: &SqliteSessionManager) {
    let lookup = sm.lookup("sess_1").await.unwrap();
    assert!(lookup.is_none(), "unknown session should return None");

    sm.assign("sess_1", "provider_a", "gpt-4o", "openai")
        .await
        .unwrap();

    let lookup = sm.lookup("sess_1").await.unwrap();
    assert!(lookup.is_some(), "assigned session should be found");
    let sa = lookup.unwrap();
    assert_eq!(sa.provider_identity, "provider_a");
    assert_eq!(sa.model_name, "gpt-4o");

    sm.update_tokens("sess_1", 100, 50).await.unwrap();

    sm.increment_switch("sess_1", "provider_b").await.unwrap();
    let lookup = sm.lookup("sess_1").await.unwrap().unwrap();
    assert_eq!(lookup.provider_identity, "provider_b");
}

async fn make_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::migrate!("src/db/migrations")
        .run(&pool)
        .await
        .unwrap();
    pool
}

#[tokio::test]
async fn session_sqlite_impl() {
    let pool = make_pool().await;
    let sm = SqliteSessionManager::new(pool);
    test_session_manager(&sm).await;
}

#[tokio::test]
async fn session_sqlite_assign_upsert() {
    let pool = make_pool().await;
    let sm = SqliteSessionManager::new(pool);

    sm.assign("sess_x", "provider_a", "gpt-4o", "openai")
        .await
        .unwrap();
    sm.assign("sess_x", "provider_b", "gpt-4o", "openai")
        .await
        .unwrap();

    let lookup = sm.lookup("sess_x").await.unwrap().unwrap();
    assert_eq!(lookup.provider_identity, "provider_b");
}

#[tokio::test]
async fn session_sqlite_tokens() {
    let pool = make_pool().await;
    let sm = SqliteSessionManager::new(pool);

    sm.assign("sess_t", "provider_a", "gpt-4o", "openai")
        .await
        .unwrap();
    sm.update_tokens("sess_t", 100, 50).await.unwrap();
    sm.update_tokens("sess_t", 200, 75).await.unwrap();
}

#[tokio::test]
async fn session_routing_event() {
    let pool = make_pool().await;
    let sm = SqliteSessionManager::new(pool);

    let event = RoutingEvent {
        session_id: Some("sess_e".into()),
        request_id: "req_1".into(),
        model_name: "gpt-4o".into(),
        provider_identity: "provider_a".into(),
        billing_model: "pay_as_you_go".into(),
        decision_reason: "cost".into(),
        input_tokens: Some(100),
        output_tokens: Some(50),
        response_status: Some(200),
        latency_ms: Some(42),
    };
    sm.insert_routing_event(event).await.unwrap();
}
