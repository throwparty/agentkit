//! acp_v1 unit tests: capability capture, per-connection state, project consent, lease heartbeat, and script loading.

use super::commands::editor_command;
use super::*;
use crate::mcp::McpPool;
use crate::state::LeaseHeartbeat;
use std::sync::Arc;

fn client_meta(pairs: &[&str]) -> Option<agent_client_protocol::schema::v1::Meta> {
    let meta: agent_client_protocol::schema::v1::Meta = pairs
        .iter()
        .map(|key| ((*key).to_owned(), serde_json::json!({})))
        .collect();
    Some(meta)
}

#[test]
fn client_advertised_features_are_captured() {
    let meta = client_meta(&["unstable_session_fork", "unstable_session_notices"]);
    let supported = UnstableFeature::advertised_in(&meta);
    assert!(supported.contains(&UnstableFeature::Fork));
    assert!(supported.contains(&UnstableFeature::SessionNotices));
    assert!(!supported.contains(&UnstableFeature::SessionCompaction));
}

#[test]
fn absent_meta_advertises_nothing() {
    assert!(UnstableFeature::advertised_in(&None).is_empty());
    assert!(UnstableFeature::advertised_in(&client_meta(&[])).is_empty());
}

#[test]
fn editor_toggles_are_per_connection() {
    let on = client_tools::EditorToggles {
        fs: true,
        terminal: true,
    };
    let a = ConnectionNegotiation::new(on, McpPool::new());
    let b = ConnectionNegotiation::new(on, McpPool::new());

    assert_eq!(
        editor_command(&a, "/editor terminal false").as_deref(),
        Some("Editor terminal tools: disabled")
    );
    assert!(!a.editor_toggles().terminal, "a toggled off");
    assert!(b.editor_toggles().terminal, "b is untouched");
}

#[tokio::test]
async fn mcp_pools_are_per_connection() {
    let on = client_tools::EditorToggles {
        fs: true,
        terminal: true,
    };
    let a = ConnectionNegotiation::new(on, McpPool::new());
    let b = ConnectionNegotiation::new(on, McpPool::new());

    a.mcp_pool().lock().await.register(
        "only-a",
        crate::config::McpServerConfig::Http {
            url: "http://example.invalid".to_owned(),
        },
    );

    assert_eq!(a.mcp_pool().lock().await.server_names(), vec!["only-a"]);
    assert!(
        b.mcp_pool().lock().await.server_names().is_empty(),
        "a connection's servers never appear on another"
    );
}

struct ChoiceStub(crate::permissions::PermissionChoice);

impl crate::permissions::PermissionRequester for ChoiceStub {
    fn request(
        &self,
        _request: crate::permissions::PermissionRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = crate::permissions::PermissionChoice> + Send + '_>,
    > {
        let choice = self.0;
        Box::pin(async move { choice })
    }
}

/// A state with a pending project layer: one MCP server and one
/// project persona, neither yet trusted.
async fn pending_project_state(dir: &std::path::Path) -> Arc<TackleState> {
    let project = dir.join(".agentkit/tackle");
    std::fs::create_dir_all(project.join("personas")).unwrap();
    std::fs::write(
        project.join("config.toml"),
        "[mcp_servers.proj]\ntransport = \"http\"\nurl = \"https://example.invalid/mcp\"\n",
    )
    .unwrap();
    std::fs::write(
        project.join("personas").join("proj-persona.md"),
        "+++\ndescription = \"project persona\"\n+++\nPROJECT BODY\n",
    )
    .unwrap();

    let user_dir = dir.join("cfg");
    let full = crate::definitions::discover(&user_dir, Some(&project)).unwrap();
    let mut active = full.clone();
    active.retain_project(&project, &std::collections::BTreeSet::new());
    let (loaded, project_config) =
        crate::config::load_layered_deferred(&user_dir, Some(&project)).unwrap();

    let db = Arc::new(crate::store::SessionStore::in_memory());
    let state = Arc::new(TackleState::new(db, loaded, active));
    let trust_store = crate::config::trust::TrustStore::load(dir.join("trust.toml")).unwrap();
    state
        .set_pending_project(PendingProject {
            config: project_config.unwrap_or_default(),
            definitions: full,
            project_dir: project,
            identity: "test-remote".to_owned(),
            trust_store,
        })
        .await;
    state
}

#[tokio::test]
async fn project_layer_applies_only_after_consent() {
    let dir = tempfile::tempdir().unwrap();
    let state = pending_project_state(dir.path()).await;
    // Before consent: neither project entry is active.
    assert!(!state.config().config.mcp_servers.contains_key("proj"));
    assert!(!state.definitions().personas.contains_key("proj-persona"));

    state
        .resolve_pending_project(
            None,
            &ChoiceStub(crate::permissions::PermissionChoice::AllowOnce),
            "sess_test",
        )
        .await;

    assert!(state.config().config.mcp_servers.contains_key("proj"));
    assert!(state.definitions().personas.contains_key("proj-persona"));
}

#[tokio::test]
async fn a_denied_project_layer_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let state = pending_project_state(dir.path()).await;
    state
        .resolve_pending_project(
            None,
            &ChoiceStub(crate::permissions::PermissionChoice::RejectOnce),
            "sess_test",
        )
        .await;

    assert!(!state.config().config.mcp_servers.contains_key("proj"));
    assert!(!state.definitions().personas.contains_key("proj-persona"));
}

#[tokio::test]
async fn lease_heartbeat_keeps_a_long_turn_owned() {
    let db = Arc::new(crate::store::SessionStore::in_memory());
    let session = db
        .create_session(
            crate::store::SessionKind::Interactive,
            "/work",
            None,
            None,
            "{}",
        )
        .await
        .unwrap();
    // A one-second TTL would lapse within the sleep below; the
    // heartbeat must keep the lease held by owner-a.
    assert!(db.acquire_lease(&session.id, "owner-a", 1).await.unwrap());
    let heartbeat = LeaseHeartbeat::spawn_with(
        Arc::clone(&db),
        session.id.clone(),
        "owner-a".to_owned(),
        1,
        std::time::Duration::from_millis(200),
    );

    tokio::time::sleep(std::time::Duration::from_millis(1300)).await;

    assert_eq!(
        db.lease_holder(&session.id).await.unwrap(),
        Some("owner-a".to_owned())
    );
    assert!(
        !db.acquire_lease(&session.id, "owner-b", 60).await.unwrap(),
        "a live heartbeat must keep the session owned"
    );
    drop(heartbeat);
}

#[test]
fn script_bodies_are_loaded_once() {
    let dir = tempfile::tempdir().unwrap();
    let user = dir.path().join("cfg");
    std::fs::create_dir_all(user.join("scripts")).unwrap();
    std::fs::write(user.join("scripts/deny.rhai"), "one").unwrap();
    std::fs::write(
        user.join("config.toml"),
        "[scripts.deny]\nevents = [\"pre_tool_use\"]\nfile = \"deny.rhai\"\n",
    )
    .unwrap();
    let loaded = crate::config::load_layered(&user, None, None).unwrap();
    let state = TackleState::new(
        Arc::new(crate::store::SessionStore::in_memory()),
        loaded,
        crate::definitions::Definitions::default().with_builtins(),
    );
    assert_eq!(
        state.script_sources().get("deny").map(String::as_str),
        Some("one")
    );

    // A mid-process change to the file is not picked up: the body was
    // loaded once (no hot reload).
    std::fs::write(user.join("scripts/deny.rhai"), "two").unwrap();
    assert_eq!(
        state.script_sources().get("deny").map(String::as_str),
        Some("one"),
        "scripts are not re-read after load"
    );
}
