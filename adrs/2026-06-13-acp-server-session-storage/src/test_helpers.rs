use std::collections::VecDeque;

use crate::store::{SessionStore, StoreError};
use crate::types::{PromptTurn, Session};

pub fn test_session(id: &str) -> Session {
    Session {
        id: id.to_string(),
        head_prompt_turn_id: None,
        cwd: "/home/user".to_string(),
        title: "Test Session".to_string(),
        mode: None,
        prompt_turns: VecDeque::new(),
        prompt_turn_count: 0,
        forked_from_session_id: None,
        fork_point_turn_id: None,
        created_at: 1000,
        updated_at: 1000,
        active: true,
        transport: "stdio".to_string(),
    }
}

pub async fn run_session_tests<S: SessionStore>(store: &S) {
    session_create_and_get(store).await;
    session_create_duplicate(store).await;
    session_list(store).await;
    session_get_missing(store).await;
    session_close(store).await;
    session_close_missing(store).await;
    session_set_mode(store).await;
    session_set_head(store).await;
    session_clear(store).await;
}

async fn session_create_and_get<S: SessionStore>(store: &S) {
    let session = test_session("sess-1");
    store.create_session(session.clone()).await.unwrap();
    let got = store.get_session("sess-1").await.unwrap();
    assert_eq!(got.id, session.id);
    assert_eq!(got.cwd, session.cwd);
    assert_eq!(got.title, session.title);
    assert_eq!(got.active, session.active);
}

async fn session_create_duplicate<S: SessionStore>(store: &S) {
    let dup = test_session("sess-1");
    let err = store.create_session(dup).await.unwrap_err();
    assert!(
        matches!(&err, StoreError::AlreadyExists { entity, id } if *entity == "session" && *id == "sess-1"),
        "expected AlreadyExists for session sess-1, got {err}"
    );
}

async fn session_list<S: SessionStore>(store: &S) {
    let s2 = test_session("sess-list-2");
    let s3 = test_session("sess-list-3");
    store.create_session(s2).await.unwrap();
    store.create_session(s3).await.unwrap();
    let sessions = store.list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 3);
    assert!(sessions.iter().any(|s| s.id == "sess-1"));
    assert!(sessions.iter().any(|s| s.id == "sess-list-2"));
    assert!(sessions.iter().any(|s| s.id == "sess-list-3"));
}

async fn session_get_missing<S: SessionStore>(store: &S) {
    let err = store.get_session("nonexistent").await.unwrap_err();
    assert!(
        matches!(&err, StoreError::NotFound { entity, id } if *entity == "session" && *id == "nonexistent"),
        "expected NotFound for session nonexistent, got {err}"
    );
}

async fn session_close<S: SessionStore>(store: &S) {
    let s = test_session("sess-close");
    store.create_session(s).await.unwrap();
    store.close_session("sess-close").await.unwrap();
    let got = store.get_session("sess-close").await.unwrap();
    assert!(!got.active);
    assert!(got.updated_at > got.created_at);
}

async fn session_close_missing<S: SessionStore>(store: &S) {
    let err = store.close_session("nonexistent-close").await.unwrap_err();
    assert!(
        matches!(&err, StoreError::NotFound { entity, id } if *entity == "session" && *id == "nonexistent-close"),
        "expected NotFound, got {err}"
    );
}

async fn session_set_mode<S: SessionStore>(store: &S) {
    let s = test_session("sess-mode");
    store.create_session(s).await.unwrap();
    store
        .set_session_mode("sess-mode", "ask".to_string())
        .await
        .unwrap();
    let got = store.get_session("sess-mode").await.unwrap();
    assert_eq!(got.mode, Some("ask".to_string()));
    assert!(got.updated_at > got.created_at);
}

async fn session_set_head<S: SessionStore>(store: &S) {
    let s = test_session("sess-head");
    store.create_session(s).await.unwrap();
    store
        .set_session_head("sess-head", "turn-1")
        .await
        .unwrap();
    let got = store.get_session("sess-head").await.unwrap();
    assert_eq!(got.head_prompt_turn_id, Some("turn-1".to_string()));
    assert!(got.updated_at > got.created_at);
}

async fn session_clear<S: SessionStore>(store: &S) {
    store.clear().await.unwrap();
    let sessions = store.list_sessions().await.unwrap();
    assert!(sessions.is_empty());
}

pub fn test_prompt_turn(
    id: &str,
    session_id: &str,
    parent_id: Option<&str>,
    position: usize,
) -> PromptTurn {
    PromptTurn {
        id: id.to_string(),
        session_id: session_id.to_string(),
        parent_id: parent_id.map(|s| s.to_string()),
        messages: Vec::new(),
        position,
        created_at: 1000 + position as u64,
    }
}

pub async fn run_prompt_turn_tests<S: SessionStore>(store: &S) {
    prompt_turn_append(store).await;
    prompt_turn_append_first(store).await;
    prompt_turn_append_missing_session(store).await;
    prompt_turn_dag_parent(store).await;
    prompt_turn_children(store).await;
    prompt_turn_session_list(store).await;
    prompt_turn_position_increments(store).await;
}

async fn prompt_turn_append<S: SessionStore>(store: &S) {
    let s = test_session("pt-sess-1");
    store.create_session(s).await.unwrap();
    let turn = test_prompt_turn("turn-1", "pt-sess-1", None, 0);
    store.append_prompt_turn(turn).await.unwrap();
}

async fn prompt_turn_append_first<S: SessionStore>(store: &S) {
    let s = test_session("pt-sess-2");
    store.create_session(s).await.unwrap();
    let turn = test_prompt_turn("turn-2", "pt-sess-2", None, 0);
    store.append_prompt_turn(turn).await.unwrap();
}

async fn prompt_turn_append_missing_session<S: SessionStore>(store: &S) {
    let turn = test_prompt_turn("turn-missing", "nonexistent", None, 0);
    let err = store.append_prompt_turn(turn).await.unwrap_err();
    assert!(
        matches!(&err, StoreError::NotFound { entity, id } if *entity == "session" && *id == "nonexistent"),
        "expected NotFound, got {err}"
    );
}

async fn prompt_turn_dag_parent<S: SessionStore>(store: &S) {
    let s = test_session("pt-sess-3");
    store.create_session(s).await.unwrap();
    let turn_a = test_prompt_turn("turn-A", "pt-sess-3", None, 0);
    store.append_prompt_turn(turn_a).await.unwrap();
    let turn_b = test_prompt_turn("turn-B", "pt-sess-3", Some("turn-A"), 1);
    store.append_prompt_turn(turn_b).await.unwrap();

    let children = store.get_prompt_turn_children("turn-A").await.unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].id, "turn-B");
    assert_eq!(children[0].parent_id, Some("turn-A".to_string()));
}

async fn prompt_turn_children<S: SessionStore>(store: &S) {
    let s = test_session("pt-sess-4");
    store.create_session(s).await.unwrap();
    let turn_a = test_prompt_turn("turn-C", "pt-sess-4", None, 0);
    store.append_prompt_turn(turn_a).await.unwrap();
    let turn_b = test_prompt_turn("turn-D", "pt-sess-4", Some("turn-C"), 1);
    store.append_prompt_turn(turn_b).await.unwrap();
    let turn_c = test_prompt_turn("turn-E", "pt-sess-4", Some("turn-C"), 2);
    store.append_prompt_turn(turn_c).await.unwrap();

    let children = store.get_prompt_turn_children("turn-C").await.unwrap();
    assert_eq!(children.len(), 2);
    let child_ids: Vec<&str> = children.iter().map(|t| t.id.as_str()).collect();
    assert!(child_ids.contains(&"turn-D"));
    assert!(child_ids.contains(&"turn-E"));
}

async fn prompt_turn_session_list<S: SessionStore>(store: &S) {
    let s = test_session("pt-sess-5");
    store.create_session(s).await.unwrap();
    let turn_a = test_prompt_turn("turn-F", "pt-sess-5", None, 0);
    store.append_prompt_turn(turn_a).await.unwrap();
    let turn_b = test_prompt_turn("turn-G", "pt-sess-5", Some("turn-F"), 1);
    store.append_prompt_turn(turn_b).await.unwrap();

    let turns = store.get_session_prompt_turns("pt-sess-5").await.unwrap();
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0].position, 0);
    assert_eq!(turns[1].position, 1);
}

async fn prompt_turn_position_increments<S: SessionStore>(store: &S) {
    let s = test_session("pt-sess-6");
    store.create_session(s).await.unwrap();
    for i in 0..3 {
        let turn =
            test_prompt_turn(&format!("turn-pos-{i}"), "pt-sess-6", None, i);
        store.append_prompt_turn(turn).await.unwrap();
    }
    let turns = store.get_session_prompt_turns("pt-sess-6").await.unwrap();
    assert_eq!(turns.len(), 3);
    for (i, t) in turns.iter().enumerate() {
        assert_eq!(t.position, i);
    }
}
