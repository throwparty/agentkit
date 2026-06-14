pub mod middleware;
pub mod routes;

use std::sync::Arc;
use crate::config::SwitchboardConfig;
use crate::models::db::ModelDb;
use crate::provider::registry::ProviderRegistry;
use crate::session::memory::MemorySessionManager;

pub async fn start(config: SwitchboardConfig) -> Result<(), Box<dyn std::error::Error>> {
    let registry = ProviderRegistry::new(&config.providers);
    let model_db = ModelDb::new(config.models, &config.providers);
    let session_manager = MemorySessionManager::new();
    let credential_helper = config.credential_helper.unwrap_or_else(|| "keychain".to_string());

    let state = Arc::new(routes::AppState {
        registry,
        model_db,
        session_manager,
        credential_helper,
    });

    let app = routes::build_router(state);

    let bind_addr = "127.0.0.1:3812";
    tracing::info!("listening on {bind_addr}");

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
