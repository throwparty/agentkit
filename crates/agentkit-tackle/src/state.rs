//! Shared process state: the session store, loaded configuration,
//! definitions, resolved script bodies, the permission grant store, and
//! the pending project layer. Per-connection state (negotiation, the
//! client surface, its terminals, and its MCP pool) lives in the ACP
//! layer so connections never bleed into one another.

use crate::config::Loaded;
use crate::definitions::Definitions;
use crate::mcp::McpPool;
use crate::permissions::{PermissionChoice, PermissionRequest, PermissionRequester};
use crate::store::SessionStore;
use agent_client_protocol::schema::v1::SessionId;
use std::collections::BTreeMap;
use std::sync::Arc;

/// A crashed or stalled connection frees its session after this long.
pub(crate) const LEASE_TTL_SECS: i64 = 60;

/// How often a live turn renews its lease. Strictly below the TTL, so the
/// lease never lapses mid-turn.
const LEASE_HEARTBEAT_SECS: u64 = 20;

/// Renews a session lease for as long as a turn runs, so the
/// one-turn-at-a-time invariant survives turns longer than the TTL.
/// Dropping the guard stops renewal; releasing the lease remains the
/// caller's job.
pub(crate) struct LeaseHeartbeat {
    handle: tokio::task::JoinHandle<()>,
}

impl LeaseHeartbeat {
    /// Spawns renewal of `owner`'s lease on `session` every
    /// [`LEASE_HEARTBEAT_SECS`]. The task stops on its own if the lease is
    /// no longer held (released or stolen).
    pub(crate) fn spawn(
        store: Arc<SessionStore>,
        session: crate::store::SessionId,
        owner: String,
    ) -> Self {
        Self::spawn_with(
            store,
            session,
            owner,
            LEASE_TTL_SECS,
            std::time::Duration::from_secs(LEASE_HEARTBEAT_SECS),
        )
    }

    pub(crate) fn spawn_with(
        store: Arc<SessionStore>,
        session: crate::store::SessionId,
        owner: String,
        ttl_secs: i64,
        period: std::time::Duration,
    ) -> Self {
        let handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(period);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // The first tick fires immediately; consume it so the first
            // renewal lands one period after acquisition.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                match store.heartbeat_lease(&session, &owner, ttl_secs).await {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(err) => tracing::warn!(session = %session, "lease heartbeat failed: {err}"),
                }
            }
        });
        Self { handle }
    }
}

impl Drop for LeaseHeartbeat {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// State shared across the process's handlers: the session store, loaded
/// configuration, and definitions. Per-connection state (negotiation, the
/// client surface, its terminals, and its MCP pool) lives in
/// `ConnectionNegotiation` so HTTP connections never bleed into one
/// another.
///
/// Configuration and definitions are behind a lock because the project
/// layer is applied late: trust consent is resolved at the first session
/// (there is no ACP connection at startup to prompt through), so the
/// ungated project layer waits in `pending_project` until then.
pub struct TackleState {
    pub db: Arc<SessionStore>,
    config: std::sync::RwLock<Arc<Loaded>>,
    definitions: std::sync::RwLock<Arc<Definitions>>,
    /// Script bodies resolved once when the configuration is set (process
    /// start for user and built-in scripts, trust resolution for the
    /// project layer). Scripts are never re-read, so a body that changes
    /// mid-process cannot take effect without a restart and a re-prompt
    /// (loaded once, no hot reload).
    script_sources: std::sync::RwLock<Arc<BTreeMap<String, String>>>,
    /// The in-memory permission grant store: keyed by session
    /// and invokable, gone on close, process exit, and resume after
    /// restart.
    pub grants: tokio::sync::Mutex<crate::permissions::GrantStore>,
    /// The project layer awaiting first-session consent.
    pending_project: tokio::sync::Mutex<Option<PendingProject>>,
}

/// The ungated project layer, held until the first session resolves
/// trust: its config, the full definitions (project entries included,
/// pruned from the active set), the project directory, and the trust
/// record to extend on approval.
pub struct PendingProject {
    pub config: crate::config::Config,
    pub definitions: Definitions,
    pub project_dir: std::path::PathBuf,
    pub identity: String,
    pub trust_store: crate::config::trust::TrustStore,
}

impl TackleState {
    /// Builds the process state from the user layer and built-in defaults
    /// (no project layer yet).
    pub fn new(db: Arc<SessionStore>, config: Loaded, definitions: Definitions) -> Self {
        let script_sources = crate::config::resolve_script_sources(&config);
        Self {
            db,
            config: std::sync::RwLock::new(Arc::new(config)),
            definitions: std::sync::RwLock::new(Arc::new(definitions)),
            script_sources: std::sync::RwLock::new(Arc::new(script_sources)),
            grants: tokio::sync::Mutex::new(crate::permissions::GrantStore::default()),
            pending_project: tokio::sync::Mutex::new(None),
        }
    }

    /// The active configuration. A cheap `Arc` clone, so callers never
    /// hold a lock guard across an await.
    pub fn config(&self) -> Arc<Loaded> {
        Arc::clone(&self.config.read().expect("config lock poisoned"))
    }

    /// The resolved script bodies by registration name, loaded once (see
    /// [`Self::script_sources`]). A cheap `Arc` clone.
    pub fn script_sources(&self) -> Arc<BTreeMap<String, String>> {
        Arc::clone(
            &self.script_sources.read().expect("script sources lock poisoned"),
        )
    }

    /// The active definitions (project entries included once trusted).
    pub fn definitions(&self) -> Arc<Definitions> {
        Arc::clone(&self.definitions.read().expect("definitions lock poisoned"))
    }

    /// Records the ungated project layer to resolve at the first session.
    pub async fn set_pending_project(&self, pending: PendingProject) {
        *self.pending_project.lock().await = Some(pending);
    }

    /// Resolves the pending project layer against this connection: prompt
    /// for consent through `session/request_permission`, record it in
    /// `trust.toml`, and apply the approved config, definitions, and MCP
    /// servers. Only the first session resolves it. The connection's MCP
    /// pool is passed in so this stays free of the ACP layer.
    pub(crate) async fn resolve_pending_project<R: PermissionRequester>(
        &self,
        pool: Option<Arc<tokio::sync::Mutex<McpPool>>>,
        requester: &R,
        session_id: &str,
    ) {
        let Some(mut pending) = self.pending_project.lock().await.take() else {
            return;
        };
        let mut entries =
            crate::config::project_trust_entries(&pending.config, &pending.project_dir);
        entries.extend(
            pending
                .definitions
                .project_trust_entries(&pending.project_dir),
        );
        if !entries.is_empty() {
            let needed = pending
                .trust_store
                .consent_needed(&pending.identity, &entries);
            if !needed.is_empty() {
                let content = format!(
                    "Trust project `{}` and load its configuration: {}",
                    pending.identity,
                    needed.iter().cloned().collect::<Vec<_>>().join(", ")
                );
                let choice = requester
                    .request(PermissionRequest {
                        session_id: session_id.to_owned(),
                        invokable: "trust".to_owned(),
                        arguments: serde_json::Value::Null,
                        ask_reason: Some(content),
                        tool_call_id: None,
                    })
                    .await;
                if matches!(
                    choice,
                    PermissionChoice::AllowOnce | PermissionChoice::AllowAlways
                ) {
                    if let Err(err) = pending.trust_store.approve(&pending.identity, &entries) {
                        tracing::error!("{err}");
                    }
                }
            }
        }
        let approved = pending.trust_store.approved(&pending.identity, &entries);
        let gated =
            crate::config::retain_project_entries(pending.config, &pending.project_dir, &approved);
        let mut definitions = pending.definitions;
        definitions.retain_project(&pending.project_dir, &approved);

        let base = self.config();
        let merged = base.config.clone().merge(gated.clone());
        let loaded = Loaded {
            config: merged,
            user_layer: base.user_layer.clone(),
            project_layer: base.project_layer.clone(),
        };
        // Resolve the approved project scripts now, once, alongside the
        // configuration they belong to (loaded once, no hot reload).
        let sources = crate::config::resolve_script_sources(&loaded);
        *self.script_sources.write().expect("script sources lock poisoned") =
            Arc::new(sources);
        *self.config.write().expect("config lock poisoned") = Arc::new(loaded);
        *self.definitions.write().expect("definitions lock poisoned") =
            Arc::new(definitions);

        // Connect the approved project servers into this connection's
        // pool (built from the user layer when the connection opened).
        if let Some(pool) = pool {
            if !gated.mcp_servers.is_empty() {
                let mut pool = pool.lock().await;
                for (name, config) in &gated.mcp_servers {
                    pool.register(name, config.clone());
                }
                for (name, config) in &gated.mcp_servers {
                    pool.connect(name, config).await;
                }
            }
        }
    }
}

/// Prefixes a session id for the wire (the storage ADR's serialization
/// boundary).
pub(crate) fn wire_session_id(id: &crate::store::SessionId) -> SessionId {
    SessionId::new(format!("sess_{id}"))
}

/// Strips the wire prefix back to the bare store id.
pub(crate) fn store_session_id(id: &SessionId) -> String {
    id.to_string()
        .strip_prefix("sess_")
        .unwrap_or(id.to_string().as_str())
        .to_owned()
}
