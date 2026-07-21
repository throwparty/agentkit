pub mod middleware;
pub mod routes;

use crate::config::SwitchboardConfig;
use crate::db;
use crate::models::db::ModelDb;
use crate::provider::registry::ProviderRegistry;
use crate::session::sqlite::SqliteSessionManager;
use agentkit_path::data_dir;
use std::path::PathBuf;
use std::sync::Arc;

pub async fn start(
    config: SwitchboardConfig,
    bind: &str,
    port: u16,
    models_db_path: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let credential_helper = config
        .credential_helper
        .clone()
        .unwrap_or_else(|| "keychain".to_string());
    let registry = ProviderRegistry::new(&config.providers, &credential_helper);
    let model_db = if let Some(path) = models_db_path {
        ModelDb::from_snapshot_path(&path, config.models.clone(), &config.providers)?
    } else {
        ModelDb::new(config.models.clone(), &config.providers)
    };
    let session_db_path = config
        .session_db_path
        .clone()
        .unwrap_or_else(default_session_db_path);
    let session_db_url = format!("sqlite://{}", session_db_path.display());
    let session_pool = db::create_pool(&session_db_url).await?;
    model_db.sync_to_db(&session_pool).await?;
    let session_manager = Arc::new(SqliteSessionManager::new(session_pool));

    let state = Arc::new(routes::AppState {
        config,
        registry,
        model_db,
        session_manager,
        credential_helper,
        session_db_path,
        started_at: std::time::Instant::now(),
    });

    let app = routes::build_router(state);

    let bind_addr = format!("{bind}:{port}");
    tracing::info!("listening on {bind_addr}");

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

fn default_session_db_path() -> std::path::PathBuf {
    data_dir("switchboard").join("sessions.db")
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::warn!(%error, "failed to install Ctrl-C handler");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => {
                tracing::warn!(%error, "failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
