use std::collections::VecDeque;

use crate::store::{SessionStore, StoreError};
use crate::types::{Message, PromptTurn, Session};

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

pub fn test_message(
    id: &str,
    prompt_turn_id: &str,
    role: &str,
    content: &str,
    position: usize,
) -> Message {
    Message {
        id: id.to_string(),
        prompt_turn_id: prompt_turn_id.to_string(),
        role: role.to_string(),
        content: content.to_string(),
        position,
        created_at: 1000 + position as u64,
    }
}

pub async fn run_message_tests<S: SessionStore>(store: &S) {
    message_append(store).await;
    message_append_missing_turn(store).await;
    message_get_by_turn(store).await;
    message_position_order(store).await;
    message_multiple_turns(store).await;
}

async fn message_append<S: SessionStore>(store: &S) {
    let s = test_session("msg-sess-1");
    store.create_session(s).await.unwrap();
    let turn = test_prompt_turn("msg-turn-1", "msg-sess-1", None, 0);
    store.append_prompt_turn(turn).await.unwrap();
    let msg = test_message("msg-1", "msg-turn-1", "user", "hello", 0);
    store.append_message(msg).await.unwrap();
}

async fn message_append_missing_turn<S: SessionStore>(store: &S) {
    let msg = test_message("msg-missing", "nonexistent-turn", "user", "test", 0);
    let err = store.append_message(msg).await.unwrap_err();
    assert!(
        matches!(&err, StoreError::NotFound { entity, id } if *entity == "prompt_turn" && *id == "nonexistent-turn"),
        "expected NotFound, got {err}"
    );
}

async fn message_get_by_turn<S: SessionStore>(store: &S) {
    let s = test_session("msg-sess-5");
    store.create_session(s).await.unwrap();
    let turn = test_prompt_turn("msg-turn-5", "msg-sess-5", None, 0);
    store.append_prompt_turn(turn).await.unwrap();
    for i in 0..3 {
        let msg = test_message(&format!("msg-5-{i}"), "msg-turn-5", "user", "content", i);
        store.append_message(msg).await.unwrap();
    }
    let msgs = store.get_messages_for_turn("msg-turn-5").await.unwrap();
    assert_eq!(msgs.len(), 3);
    for (i, m) in msgs.iter().enumerate() {
        assert_eq!(m.position, i);
    }
}

async fn message_position_order<S: SessionStore>(store: &S) {
    let s = test_session("msg-sess-3");
    store.create_session(s).await.unwrap();
    let turn = test_prompt_turn("msg-turn-3", "msg-sess-3", None, 0);
    store.append_prompt_turn(turn).await.unwrap();
    let orders = [("msg-a", 2), ("msg-b", 0), ("msg-c", 1)];
    for (id, pos) in orders {
        let msg = test_message(id, "msg-turn-3", "user", "content", pos);
        store.append_message(msg).await.unwrap();
    }
    let msgs = store.get_messages_for_turn("msg-turn-3").await.unwrap();
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[0].id, "msg-b");
    assert_eq!(msgs[1].id, "msg-c");
    assert_eq!(msgs[2].id, "msg-a");
}

async fn message_multiple_turns<S: SessionStore>(store: &S) {
    let s = test_session("msg-sess-4");
    store.create_session(s).await.unwrap();
    let turn_a = test_prompt_turn("msg-turn-4a", "msg-sess-4", None, 0);
    let turn_b = test_prompt_turn("msg-turn-4b", "msg-sess-4", None, 1);
    store.append_prompt_turn(turn_a).await.unwrap();
    store.append_prompt_turn(turn_b).await.unwrap();
    let msg_a = test_message("msg-4a", "msg-turn-4a", "user", "turn a", 0);
    let msg_b = test_message("msg-4b", "msg-turn-4b", "assistant", "turn b", 0);
    store.append_message(msg_a).await.unwrap();
    store.append_message(msg_b).await.unwrap();
    let msgs_a = store.get_messages_for_turn("msg-turn-4a").await.unwrap();
    assert_eq!(msgs_a.len(), 1);
    assert_eq!(msgs_a[0].content, "turn a");
    let msgs_b = store.get_messages_for_turn("msg-turn-4b").await.unwrap();
    assert_eq!(msgs_b.len(), 1);
    assert_eq!(msgs_b[0].content, "turn b");
}

// ---------------------------------------------------------------------------
// Context assembly
// ---------------------------------------------------------------------------

pub async fn run_context_tests<S: SessionStore>(store: &S) {
    context_basic(store).await;
    context_max_turns(store).await;
    context_empty_session(store).await;
    context_missing_session(store).await;
}

async fn context_basic<S: SessionStore>(store: &S) {
    store.create_session(test_session("ctx-sess-1")).await.unwrap();
    let t0 = test_prompt_turn("ctx-turn-0", "ctx-sess-1", None, 0);
    let t1 = test_prompt_turn("ctx-turn-1", "ctx-sess-1", None, 1);
    store.append_prompt_turn(t0).await.unwrap();
    store.append_prompt_turn(t1).await.unwrap();

    store.append_message(test_message("ctx-msg-0", "ctx-turn-0", "user", "hi", 0)).await.unwrap();
    store.append_message(test_message("ctx-msg-1", "ctx-turn-0", "assistant", "hello", 1)).await.unwrap();
    store.append_message(test_message("ctx-msg-2", "ctx-turn-1", "user", "bye", 0)).await.unwrap();
    store.append_message(test_message("ctx-msg-3", "ctx-turn-1", "assistant", "goodbye", 1)).await.unwrap();

    let ctx = store.get_context("ctx-sess-1", None).await.unwrap();
    assert_eq!(ctx.len(), 4);
    assert_eq!(ctx[0].content, "hi");
    assert_eq!(ctx[1].content, "hello");
    assert_eq!(ctx[2].content, "bye");
    assert_eq!(ctx[3].content, "goodbye");
}

async fn context_max_turns<S: SessionStore>(store: &S) {
    store.create_session(test_session("ctx-sess-2")).await.unwrap();
    for i in 0..3 {
        let t = test_prompt_turn(&format!("ctx-t2-{i}"), "ctx-sess-2", None, i);
        store.append_prompt_turn(t).await.unwrap();
        let m = test_message(&format!("ctx-m2-{i}"), &format!("ctx-t2-{i}"), "user", &format!("msg-{i}"), 0);
        store.append_message(m).await.unwrap();
    }

    let ctx = store.get_context("ctx-sess-2", Some(2)).await.unwrap();
    assert_eq!(ctx.len(), 2, "expected 2 messages with max_turns=2");
    assert_eq!(ctx[0].content, "msg-1");
    assert_eq!(ctx[1].content, "msg-2");
}

async fn context_empty_session<S: SessionStore>(store: &S) {
    store.create_session(test_session("ctx-sess-3")).await.unwrap();
    let ctx = store.get_context("ctx-sess-3", None).await.unwrap();
    assert!(ctx.is_empty());
}

async fn context_missing_session<S: SessionStore>(store: &S) {
    let err = store.get_context("ctx-sess-nonexistent", None).await.unwrap_err();
    assert!(
        matches!(&err, StoreError::NotFound { entity, id } if *entity == "session" && *id == "ctx-sess-nonexistent"),
        "expected NotFound, got {err}"
    );
}

// ---------------------------------------------------------------------------
// Fork
// ---------------------------------------------------------------------------

pub async fn run_fork_tests<S: SessionStore>(store: &S) {
    fork_basic(store).await;
    fork_at_first_turn(store).await;
    fork_missing_session(store).await;
    fork_missing_turn(store).await;
}

async fn fork_basic<S: SessionStore>(store: &S) {
    store.create_session(test_session("fork-src-1")).await.unwrap();
    let t0 = test_prompt_turn("fork-t0", "fork-src-1", None, 0);
    let t1 = test_prompt_turn("fork-t1", "fork-src-1", None, 1);
    let t2 = test_prompt_turn("fork-t2", "fork-src-1", None, 2);
    store.append_prompt_turn(t0).await.unwrap();
    store.append_prompt_turn(t1).await.unwrap();
    store.append_prompt_turn(t2).await.unwrap();

    store.append_message(test_message("fork-m0", "fork-t0", "user", "turn0", 0)).await.unwrap();
    store.append_message(test_message("fork-m1", "fork-t1", "user", "turn1", 0)).await.unwrap();
    store.append_message(test_message("fork-m2", "fork-t2", "user", "turn2", 0)).await.unwrap();

    let fork_sess = Session {
        id: "fork-sess-1".to_string(),
        head_prompt_turn_id: None,
        cwd: "/home/user".to_string(),
        title: "forked".to_string(),
        mode: None,
        prompt_turns: VecDeque::new(),
        prompt_turn_count: 0,
        forked_from_session_id: None,
        fork_point_turn_id: None,
        created_at: 2000,
        updated_at: 2000,
        active: true,
        transport: "stdio".to_string(),
    };

    store.fork_session(fork_sess, "fork-src-1", "fork-t1").await.unwrap();

    let new_sess = store.get_session("fork-sess-1").await.unwrap();
    assert_eq!(new_sess.forked_from_session_id.as_deref(), Some("fork-src-1"));
    assert_eq!(new_sess.fork_point_turn_id.as_deref(), Some("fork-t1"));
    assert!(new_sess.head_prompt_turn_id.is_some());

    let turns = store.get_session_prompt_turns("fork-sess-1").await.unwrap();
    assert_eq!(turns.len(), 2, "fork should copy turns up to fork point");

    let ctx = store.get_context("fork-sess-1", None).await.unwrap();
    assert_eq!(ctx.len(), 2);
    assert_eq!(ctx[0].content, "turn0");
    assert_eq!(ctx[1].content, "turn1");
}

async fn fork_at_first_turn<S: SessionStore>(store: &S) {
    store.create_session(test_session("fork-src-2")).await.unwrap();
    let t0 = test_prompt_turn("fork-t0b", "fork-src-2", None, 0);
    let t1 = test_prompt_turn("fork-t1b", "fork-src-2", None, 1);
    store.append_prompt_turn(t0).await.unwrap();
    store.append_prompt_turn(t1).await.unwrap();
    store.append_message(test_message("fork-m0b", "fork-t0b", "user", "only", 0)).await.unwrap();

    let fork_sess = Session {
        id: "fork-sess-2".to_string(),
        head_prompt_turn_id: None,
        cwd: "/home/user".to_string(),
        title: "fork-at-first".to_string(),
        mode: None,
        prompt_turns: VecDeque::new(),
        prompt_turn_count: 0,
        forked_from_session_id: None,
        fork_point_turn_id: None,
        created_at: 3000,
        updated_at: 3000,
        active: true,
        transport: "stdio".to_string(),
    };

    store.fork_session(fork_sess, "fork-src-2", "fork-t0b").await.unwrap();
    let turns = store.get_session_prompt_turns("fork-sess-2").await.unwrap();
    assert_eq!(turns.len(), 1);
    let ctx = store.get_context("fork-sess-2", None).await.unwrap();
    assert_eq!(ctx.len(), 1);
    assert_eq!(ctx[0].content, "only");
}

async fn fork_missing_session<S: SessionStore>(store: &S) {
    let fork_sess = Session {
        id: "fork-nonexistent".to_string(),
        head_prompt_turn_id: None,
        cwd: "/".to_string(),
        title: "bad".to_string(),
        mode: None,
        prompt_turns: VecDeque::new(),
        prompt_turn_count: 0,
        forked_from_session_id: None,
        fork_point_turn_id: None,
        created_at: 0,
        updated_at: 0,
        active: true,
        transport: "stdio".to_string(),
    };

    let err = store.fork_session(fork_sess, "no-such-session", "some-turn").await.unwrap_err();
    assert!(
        matches!(&err, StoreError::NotFound { entity, id } if *entity == "session" && *id == "no-such-session"),
        "expected NotFound, got {err}"
    );
}

async fn fork_missing_turn<S: SessionStore>(store: &S) {
    store.create_session(test_session("fork-src-mt")).await.unwrap();
    let fork_sess = Session {
        id: "fork-mt".to_string(),
        head_prompt_turn_id: None,
        cwd: "/".to_string(),
        title: "bad-turn".to_string(),
        mode: None,
        prompt_turns: VecDeque::new(),
        prompt_turn_count: 0,
        forked_from_session_id: None,
        fork_point_turn_id: None,
        created_at: 0,
        updated_at: 0,
        active: true,
        transport: "stdio".to_string(),
    };

    let err = store.fork_session(fork_sess, "fork-src-mt", "no-such-turn").await.unwrap_err();
    assert!(
        matches!(&err, StoreError::NotFound { entity, id } if *entity == "prompt_turn" && *id == "no-such-turn"),
        "expected NotFound, got {err}"
    );
}
