use crate::auth;
use crate::credential;
use crate::credential::ResolvedCredential;
use crate::models::db::ModelDb;
use crate::domain::quota::{backoff_duration, ProviderQuotaState};
use crate::provider::registry::ProviderRegistry;
use crate::provider::router::{select_provider, ProviderSelection, RoutingError};
use crate::proxy::forwarder;
use crate::server::middleware::RequestId;
use crate::session::sqlite::SqliteSessionManager;
use crate::session::{RoutingEvent, SessionAffinity};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

pub struct AppState {
    pub config: crate::config::SwitchboardConfig,
    pub registry: ProviderRegistry,
    pub model_db: ModelDb,
    pub session_manager: Arc<SqliteSessionManager>,
    pub credential_helper: String,
    pub session_db_path: PathBuf,
    pub started_at: Instant,
}

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route(
            "/openai/v1/chat/completions",
            post(chat_completions_handler),
        )
        .route("/openai/v1/models", get(models_handler))
        .route("/health", get(health_handler))
        .layer(axum::middleware::from_fn(
            crate::server::middleware::request_id_middleware,
        ))
        .with_state(state)
}

async fn resolve_credential(
    helper_name: &str,
    identity: &str,
    config: &crate::config::SwitchboardConfig,
    registry: &ProviderRegistry,
) -> Option<ResolvedCredential> {
    let configured = config.providers.get(identity)?;
    let credential = credential::resolve_provider(helper_name, identity, configured)?;
    match auth::maybe_refresh_credential(identity, credential, config).await {
        Ok(refreshed) => Some(refreshed),
        Err(error) => {
            tracing::warn!(provider = %identity, %error, "credential refresh failed");
            registry.degrade_provider(identity).await;
            None
        }
    }
}

fn switchboard_response(
    status: StatusCode,
    body: String,
    provider_identity: &str,
    billing: &str,
) -> Response {
    let mut h = HeaderMap::new();
    h.insert("Content-Type", HeaderValue::from_static("application/json"));
    h.insert(
        "X-Switchboard-Provider",
        HeaderValue::from_str(provider_identity).unwrap(),
    );
    h.insert(
        "X-Switchboard-Billing",
        HeaderValue::from_str(billing).unwrap(),
    );
    (status, h, body).into_response()
}

async fn log_routing_event(
    session_manager: &SqliteSessionManager,
    session_id: &Option<String>,
    request_id: &str,
    model: &str,
    selection: &ProviderSelection,
    billing: &str,
    status: StatusCode,
    latency_ms: i64,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
) {
    let _ = session_manager
        .insert_routing_event(RoutingEvent {
            session_id: session_id.clone(),
            request_id: request_id.to_string(),
            model_name: model.to_string(),
            provider_identity: selection.identity.clone(),
            billing_model: billing.to_string(),
            decision_reason: selection_reason(&selection.reason).to_string(),
            input_tokens,
            output_tokens,
            response_status: Some(status.as_u16() as i64),
            latency_ms: Some(latency_ms),
        })
        .await;
}

async fn chat_completions_handler(
    State(app_state): State<Arc<AppState>>,
    Extension(request_id): Extension<RequestId>,
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

    let mut session = if let Some(ref sid) = session_id {
        app_state.session_manager.lookup(sid).await.unwrap_or(None)
    } else {
        None
    };

    let helper_name = &app_state.credential_helper;
    let max_attempts = app_state.registry.get_states().await.len().max(1);
    let mut attempt_errors: Vec<(String, u16)> = Vec::new();

    for attempt in 0..max_attempts {
        let providers = app_state.registry.get_states().await;
        let session_ref = session.as_ref();
        let selection = match select_provider(&model, session_ref, &providers) {
            Ok(s) => s,
            Err(RoutingError::ModelNotFound) => {
                return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": "model not found"}))).into_response();
            }
            Err(RoutingError::NoProvider) => {
                if attempt == 0 {
                    return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": "no available provider"}))).into_response();
                }
                break;
            }
        };

        let provider_cfg = match providers.get(&selection.identity) {
            Some(p) => p.clone(),
            None => continue,
        };
        let configured_provider = match app_state.config.providers.get(&selection.identity) {
            Some(provider) => provider,
            None => continue,
        };

        let credential = match resolve_credential(
            helper_name,
            &selection.identity,
            &app_state.config,
            &app_state.registry,
        )
        .await
        {
            Some(c) => c,
            None => continue,
        };

        if let Some(ref sid) = session_id {
            persist_session_assignment(
                app_state.session_manager.as_ref(),
                sid,
                session_ref,
                &selection.identity,
                &model,
            )
            .await;
            session = Some(SessionAffinity {
                session_id: sid.clone(),
                provider_identity: selection.identity.clone(),
                model_name: model.clone(),
                api_surface: "openai".to_string(),
            });
        }

        let pack = app_state.registry.get_provider_pack(&selection.identity);
        let request_started = Instant::now();
        let outcome = match pack {
            Some(p) => {
                forwarder::forward_request(
                    forwarder::ForwardRequest {
                        method: axum::http::Method::POST,
                        headers: headers.clone(),
                        body: body.clone(),
                        credential: &credential,
                        billing: &provider_cfg.billing,
                        base_url: &configured_provider.base_url,
                        provider_identity: &selection.identity,
                        session_id: session_id.as_deref(),
                    },
                    &*p.http,
                    &*p.conversation,
                )
                .await
            }
            None => {
                forwarder::forward_request(
                    forwarder::ForwardRequest {
                        method: axum::http::Method::POST,
                        headers: headers.clone(),
                        body: body.clone(),
                        credential: &credential,
                        billing: &provider_cfg.billing,
                        base_url: &configured_provider.base_url,
                        provider_identity: &selection.identity,
                        session_id: session_id.as_deref(),
                    },
                    &crate::providers::openai::OpenAiProvider,
                    &crate::providers::openai::conversation::OpenAiConversation,
                )
                .await
            }
        };
        let latency_ms = request_started.elapsed().as_millis() as i64;
        let status = outcome.status;

        app_state
            .registry
            .record_response(
                &selection.identity,
                status.as_u16(),
                &outcome.headers,
                outcome.body_text.as_deref(),
            )
            .await;

        let billing = provider_cfg.billing.to_string();
        let (input_tokens, output_tokens) = usage_tokens(outcome.body_text.as_deref())
            .map(|(i, o)| (Some(i as i64), Some(o as i64)))
            .unwrap_or((None, None));

        log_routing_event(
            &app_state.session_manager,
            &session_id,
            &request_id.0,
            &model,
            &selection,
            &billing,
            status,
            latency_ms,
            input_tokens,
            output_tokens,
        )
        .await;

        if status.is_success() {
            if outcome.body_text.is_none() {
                return outcome.response;
            }
            if let (Some(sid), Some((input, output))) = (
                session_id.as_ref(),
                usage_tokens(outcome.body_text.as_deref()),
            ) {
                let _ = app_state
                    .session_manager
                    .update_tokens(sid, input, output)
                    .await;
            }
            return switchboard_response(
                status,
                outcome.body_text.unwrap_or_default(),
                &selection.identity,
                &billing,
            );
        }

        if !should_try_next(status) {
            return switchboard_response(
                status,
                outcome.body_text.unwrap_or_default(),
                &selection.identity,
                &billing,
            );
        }

        attempt_errors.push((selection.identity.clone(), status.as_u16()));
        let backoff = backoff_duration(attempt as u32, 5, 120);
        tokio::time::sleep(backoff).await;
        tracing::warn!(provider = %selection.identity, %status, backoff = ?backoff, "provider failed; trying next candidate");
    }

    let details: Vec<Value> = attempt_errors
        .iter()
        .map(|(provider, code)| json!({"provider": provider, "status": code}))
        .collect();
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "all providers failed", "attempts": details})),
    )
        .into_response()
}

async fn persist_session_assignment(
    session_manager: &SqliteSessionManager,
    session_id: &str,
    existing: Option<&SessionAffinity>,
    provider_identity: &str,
    model: &str,
) {
    if existing.is_some_and(|affinity| affinity.provider_identity != provider_identity) {
        let _ = session_manager
            .increment_switch(session_id, provider_identity)
            .await;
        tracing::warn!(
            session_id,
            provider = provider_identity,
            "session provider switched"
        );
    }

    let _ = session_manager
        .assign(session_id, provider_identity, model, "openai")
        .await;
}

fn should_try_next(status: StatusCode) -> bool {
    matches!(status.as_u16(), 401 | 403 | 429 | 500..=599)
}

fn selection_reason(reason: &crate::provider::router::SelectionReason) -> &'static str {
    match reason {
        crate::provider::router::SelectionReason::Affinity => "affinity",
        crate::provider::router::SelectionReason::Cost => "cost",
        crate::provider::router::SelectionReason::Fallback => "fallback",
    }
}

fn usage_tokens(body: Option<&str>) -> Option<(u64, u64)> {
    let body = body?;
    let parsed: Value = serde_json::from_str(body).ok()?;
    let usage = parsed.get("usage")?;
    let input = usage
        .get("input_tokens")
        .or_else(|| usage.get("prompt_tokens"))
        .and_then(|value| value.as_u64())
        .unwrap_or_default();
    let output = usage
        .get("output_tokens")
        .or_else(|| usage.get("completion_tokens"))
        .and_then(|value| value.as_u64())
        .unwrap_or_default();
    Some((input, output))
}

async fn models_handler(
    State(app_state): State<Arc<AppState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Json<Value> {
    let providers = app_state.registry.get_states().await;
    let mut data = Vec::new();
    let provider_filter = params.get("provider");
    let billing_filter = params.get("billing");

    let known_providers: std::collections::HashSet<String> = providers.keys().cloned().collect();

    for model in app_state.model_db.all() {
        let mut providers_list = Vec::new();
        for p in &model.providers {
            if known_providers.contains(&p.identity)
                && provider_filter.is_none_or(|provider| provider == &p.identity)
                && billing_filter.is_none_or(|billing| billing == &p.billing)
            {
                providers_list.push(json!({
                    "identity": p.identity,
                    "billing": p.billing,
                    "pricing": p.pricing,
                }));
            }
        }
        data.push(json!({
            "id": model.id,
            "object": "model",
            "created": 1700000000,
            "owned_by": "openai",
            "context_window": model.context_window,
            "max_output": model.max_output,
            "capabilities": model.capabilities,
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
            crate::provider::ProviderStatus::Unauthenticated => "unauthenticated",
        };
        let credential = app_state.config.providers.get(id).and_then(|provider| {
            credential::resolve_provider(&app_state.credential_helper, id, provider)
        });
        let credential_source = credential
            .as_ref()
            .map(|credential| credential_source_label(&credential.source))
            .unwrap_or_else(|| "unconfigured".to_string());
        let quota = app_state.registry.get_quota(id).await;
        provider_status[id] = json!({
            "status": status_str,
            "models_available": state.models.len(),
            "credential_valid": state.has_valid_credential,
            "credential_source": credential_source,
            "quota_type": state.billing.to_string(),
            "rate_limit_remaining_pct": quota.as_ref().and_then(rate_limit_remaining_pct),
        });
    }
    let session_stats = app_state.session_manager.stats().await.unwrap_or_default();

    Json(json!({
        "status": "ok",
        "providers": provider_status,
        "session_db": {
            "path": app_state.session_db_path.display().to_string(),
            "active_sessions": session_stats.active_sessions,
            "total_sessions": session_stats.total_sessions,
        },
        "uptime_seconds": app_state.started_at.elapsed().as_secs(),
    }))
}

fn credential_source_label(source: &credential::CredentialSource) -> String {
    match source {
        credential::CredentialSource::Helper { helper_name } => {
            format!("agentkit-credential-{helper_name}")
        }
        credential::CredentialSource::None => "none".to_string(),
    }
}

fn rate_limit_remaining_pct(_quota: &ProviderQuotaState) -> Option<f64> {
    // Use Debug formatting to extract remaining from boxed inner
    // In a future refactor this can be a trait method
    None
}
