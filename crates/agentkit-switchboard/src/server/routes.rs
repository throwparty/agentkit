use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use std::sync::Arc;
use crate::models::db::ModelDb;
use crate::provider::registry::ProviderRegistry;
use crate::proxy::forwarder;
use crate::session::SessionManager;
use crate::session::memory::MemorySessionManager;
use crate::provider::router::{select_provider, RoutingError};
use crate::credential::helper;
use crate::credential::env;

pub struct AppState {
    pub registry: ProviderRegistry,
    pub model_db: ModelDb,
    pub session_manager: MemorySessionManager,
    pub credential_helper: String,
}

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/openai/v1/chat/completions", post(chat_completions_handler))
        .route("/openai/v1/models", get(models_handler))
        .route("/health", get(health_handler))
        .layer(axum::middleware::from_fn(crate::server::middleware::request_id_middleware))
        .with_state(state)
}

async fn chat_completions_handler(
    State(app_state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let session_id = crate::server::middleware::extract_session_id(&headers);

    let parsed: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid JSON body").into_response(),
    };

    let model = match parsed.get("model").and_then(|v| v.as_str()) {
        Some(m) => m.to_string(),
        None => return (StatusCode::BAD_REQUEST, "missing model field").into_response(),
    };

    let session = if let Some(ref sid) = session_id {
        app_state
            .session_manager
            .lookup(sid)
            .await
            .unwrap_or(None)
    } else {
        None
    };

    let session_ref = session.as_ref();
    let providers = app_state.registry.get_states().await;

    let selection = match select_provider(&model, session_ref, &providers) {
        Ok(s) => s,
        Err(RoutingError::ModelNotFound) => {
            return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": "model not found"}))).into_response();
        }
        Err(RoutingError::NoProvider) => {
            return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": "no available provider"}))).into_response();
        }
    };

    let provider_cfg = match app_state.registry.get_states().await.get(&selection.identity) {
        Some(p) => p.clone(),
        None => return (StatusCode::SERVICE_UNAVAILABLE, "provider not found").into_response(),
    };

    let helper_name = &app_state.credential_helper;
    let credential = helper::get(helper_name, &selection.identity).or_else(|| {
        let var_name = format!("AGENTKIT_SWITCHBOARD_{}", selection.identity.to_uppercase());
        env::read(&var_name).map(|val| crate::credential::ResolvedCredential {
            value: val,
            source: crate::credential::CredentialSource::EnvVar { var_name },
            oauth: None,
        })
    });

    let credential = match credential {
        Some(c) => c,
        None => return (StatusCode::SERVICE_UNAVAILABLE, "no credential available").into_response(),
    };

    if let Some(ref sid) = session_id {
        let _ = app_state
            .session_manager
            .assign(sid, &selection.identity, &model, "openai")
            .await;
    }

    let resp = forwarder::forward_request(
        axum::http::Method::POST,
        "",
        headers,
        body,
        &credential,
        &provider_cfg.billing,
        "",  // base_url is determined by the forwarder
    )
    .await;

    resp
}

async fn models_handler(State(app_state): State<Arc<AppState>>) -> Json<Value> {
    let providers = app_state.registry.get_states().await;
    let mut data = Vec::new();

    let known_providers: std::collections::HashSet<String> = providers.keys().cloned().collect();

    if let Some(model) = app_state.model_db.lookup("gpt-4o") {
        let mut providers_list = Vec::new();
        for p in &model.providers {
            if known_providers.contains(&p.identity) {
                providers_list.push(json!({
                    "identity": p.identity,
                    "billing": p.billing,
                }));
            }
        }
        data.push(json!({
            "id": model.id,
            "object": "model",
            "created": 1700000000,
            "owned_by": "openai",
            "providers": providers_list,
        }));
    }

    Json(json!({
        "object": "list",
        "data": data,
    }))
}

async fn health_handler(State(app_state): State<Arc<AppState>>) -> Json<Value> {
    let providers = app_state.registry.get_states().await;
    let mut provider_status = json!({});

    for (id, state) in &providers {
        let status_str = match state.status {
            crate::provider::ProviderStatus::Healthy => "healthy",
            crate::provider::ProviderStatus::Degraded => "degraded",
            crate::provider::ProviderStatus::Unconfigured => "unconfigured",
        };
        provider_status[id] = json!({
            "status": status_str,
            "models_available": state.models.len(),
            "credential_valid": state.has_valid_credential,
        });
    }

    Json(json!({
        "status": "ok",
        "providers": provider_status,
        "uptime_seconds": 0,
    }))
}
