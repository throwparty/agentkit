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
    let registry = ProviderRegistry::new(&config.providers, &credential_helper)
        .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
    let model_db = if let Some(path) = models_db_path {
        ModelDb::from_snapshot_path(&path, config.models.clone(), &config.providers)?
    } else {
        ModelDb::new(config.models.clone(), &config.providers)
    };
    let session_db_path = config
        .session_db_path
        .clone()
        .unwrap_or_else(default_session_db_path);
    let session_pool = db::create_pool_from_path(&session_db_path).await?;
    model_db.sync_to_db(&session_pool).await?;
    let session_manager = Arc::new(SqliteSessionManager::new(session_pool));

    let mutations = crate::proxy::mutation::compile_provider_mutations(
        &Arc::new(agentkit_script::ScriptEngine::new()),
        &config.providers,
    )
    .map_err(|error| -> Box<dyn std::error::Error> { error.into() })?;

    let state = Arc::new(routes::AppState {
        config,
        registry,
        model_db,
        session_manager,
        credential_helper,
        session_db_path,
        started_at: std::time::Instant::now(),
        mutations,
    });

    let app = routes::build_router(state);

    let bind_addr = format!("{bind}:{port}");
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    let actual_addr = listener.local_addr()?;
    tracing::info!(address = %actual_addr, "listening");
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(name)
    }

    #[test]
    fn mutations_are_compiled_at_startup_from_the_loaded_config() {
        let config = crate::config::loader::load_config(&fixture("mutations.toml")).unwrap();
        let engine = Arc::new(agentkit_script::ScriptEngine::new());

        let compiled =
            crate::proxy::mutation::compile_provider_mutations(&engine, &config.providers).unwrap();

        let set = compiled.get("test_provider").unwrap();
        let mut headers = axum::http::HeaderMap::new();
        let mut body = Vec::new();
        let provider = config.providers.get("test_provider").unwrap();
        let context = crate::proxy::mutation::MutationContext {
            provider_identity: provider.identity.clone(),
            surface: provider.api_surface.clone(),
            model: "some-model".to_string(),
            session_id: None,
        };

        set.apply(&context, &mut headers, &mut body).unwrap();

        assert_eq!(headers.get("x-fixture-provider").unwrap(), "test_provider");
    }

    #[test]
    fn a_broken_mutation_script_aborts_startup() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("broken.rhai");
        std::fs::write(&script, "fn mutate(request) { request }").unwrap();

        let mut config = crate::config::loader::load_config(&fixture("minimal.toml")).unwrap();
        config
            .providers
            .get_mut("test_provider")
            .unwrap()
            .mutations = vec![script.join("missing.rhai")];

        let engine = Arc::new(agentkit_script::ScriptEngine::new());
        let error = crate::proxy::mutation::compile_provider_mutations(&engine, &config.providers)
            .unwrap_err();

        assert!(
            error.to_string().contains("test_provider"),
            "the provider identity should be reported: {error}"
        );
    }
}
