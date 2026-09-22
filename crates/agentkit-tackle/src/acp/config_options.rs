//! Session configuration options (T-031, FR-027): the model selector
//! and the actor selector.
//!
//! The model selector discovers models per endpoint (GET /models, both
//! wire formats' list shapes agree on `data[].id`); discovery is
//! primary, queried at session setup, cached per process, and
//! non-fatal — on failure or an empty result the endpoint's static
//! model list is the fallback and the degraded state surfaces as a
//! notice. The actor selector lists the loaded actors. A mid-session
//! model switch is validated against the new model's context window:
//! an over-window session auto-compacts, announced, with a context
//! note; the switch is effective the following turn.

use crate::acp::TackleState;
use crate::config::{Auth, WireFormat};
use crate::store::{Session, SessionId};
use agent_client_protocol::schema::v1::{Notice, NoticeSeverity};
use agent_client_protocol::schema::v1::{
    SessionConfigId, SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory,
    SessionConfigSelect, SessionConfigSelectOption, SessionConfigValueId,
};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// The model selector's config id.
pub const MODEL_SELECTOR_ID: &str = "model";
/// The actor selector's config id.
pub const ACTOR_SELECTOR_ID: &str = "actor";

/// The process-local discovery cache: endpoint name -> discovered model
/// ids.
fn discovery_cache() -> &'static Mutex<HashMap<String, Vec<String>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Vec<String>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Discover a model id with a context window for window validation.
pub fn context_window(model_ref: &str) -> Option<u64> {
    crate::acp::context_window(model_ref)
}

/// Per-endpoint model discovery: the OpenAI list shape's `data[].id`
/// (Anthropic's Models API paginates the same shape). Results cache per
/// process; failures return nothing and surface as degraded discovery.
async fn discover_models(
    endpoint_name: &str,
    base_url: &str,
    wire_format: WireFormat,
    credential: Option<String>,
) -> Option<Vec<String>> {
    if let Some(cached) = discovery_cache()
        .lock()
        .unwrap()
        .get(endpoint_name)
        .cloned()
    {
        return Some(cached);
    }
    let url = match wire_format {
        WireFormat::OpenaiChatCompletions | WireFormat::OpenaiResponses => {
            format!("{base_url}/models")
        }
        // Anthropic's Models API lives under /v1.
        WireFormat::AnthropicMessages => format!("{base_url}/v1/models"),
    };
    let request = reqwest::Client::new().get(url);
    let request = match &credential {
        Some(credential) => request.bearer_auth(credential),
        None => request,
    };
    let response = request.send().await.ok()?.error_for_status().ok()?;
    let body: serde_json::Value = response.json().await.ok()?;
    let ids: Vec<String> = body["data"]
        .as_array()?
        .iter()
        .filter_map(|model| model["id"].as_str().map(str::to_owned))
        .collect();
    if ids.is_empty() {
        return None;
    }
    discovery_cache()
        .lock()
        .unwrap()
        .insert(endpoint_name.to_owned(), ids.clone());
    Some(ids)
}

/// Notices discovered while building the selectors (degraded discovery).
pub struct Selectors {
    pub options: Vec<SessionConfigOption>,
    pub notices: Vec<Notice>,
}

/// Builds the config options for a session: the model selector
/// (discovery primary, static fallback) and the actor selector.
pub async fn build(state: &TackleState, session: &Session) -> Selectors {
    let mut options = Vec::new();
    let mut notices = Vec::new();

    // The model selector: the endpoint-qualified models across the
    // configured endpoints.
    let mut model_options = Vec::new();
    for (name, endpoint) in &state.config.config.endpoints {
        let credential = match endpoint.auth {
            Auth::None => None,
            Auth::Helper => state
                .config
                .config
                .credential_helper
                .as_deref()
                .and_then(|helper| crate::agent::provider::resolve_credential(helper, name))
                .map(|secret| {
                    use secrecy::ExposeSecret as _;
                    secret.expose_secret().to_owned()
                }),
        };
        let qualified = |model: &str| format!("{name}/{model}");
        let discovered =
            discover_models(name, &endpoint.base_url, endpoint.wire_format, credential).await;
        let models = match discovered {
            Some(models) => models,
            None => {
                if !endpoint.models.is_empty() {
                    notices.push(
                        Notice::new(
                            NoticeSeverity::Info,
                            format!("Endpoint `{name}` model discovery failed; using the configured static list."),
                        )
                        .description(Some(
                            "Model discovery is primary; the static models list is the fallback."
                                .to_owned(),
                        )),
                    );
                }
                endpoint.models.clone()
            }
        };
        for model in models {
            let reference = qualified(&model);
            model_options.push(SessionConfigSelectOption::new(
                SessionConfigValueId::new(reference.clone()),
                model.clone(),
            ));
        }
    }
    if !model_options.is_empty() {
        let current = crate::store::session_model_free(session);
        options.push(
            SessionConfigOption::new(
                SessionConfigId::new(MODEL_SELECTOR_ID),
                "Model".to_owned(),
                SessionConfigKind::Select(SessionConfigSelect::new(
                    // The switch's default when unset: the first
                    // listed model.
                    current
                        .map(SessionConfigValueId::new)
                        .unwrap_or_else(|| model_options[0].value.clone()),
                    model_options,
                )),
            )
            .category(SessionConfigOptionCategory::Model),
        );
    }

    // The actor selector: the loaded actors.
    let current = crate::store::session_actor_value_free(session);
    let actor_options: Vec<SessionConfigSelectOption> = state
        .definitions
        .actors
        .keys()
        .map(|name| {
            SessionConfigSelectOption::new(SessionConfigValueId::new(name.clone()), name.clone())
        })
        .collect();
    if !actor_options.is_empty() {
        options.push(
            SessionConfigOption::new(
                SessionConfigId::new(ACTOR_SELECTOR_ID),
                "Actor".to_owned(),
                SessionConfigKind::Select(SessionConfigSelect::new(
                    current
                        .map(SessionConfigValueId::new)
                        .unwrap_or_else(|| actor_options[0].value.clone()),
                    actor_options,
                )),
            )
            .category(SessionConfigOptionCategory::Other("actor".to_owned())),
        );
    }

    Selectors {
        options,
        notices,
    }
}

/// Applies a model switch: validated against the new model's context
/// window — an over-window session auto-compacts (announced, with a
/// context note) — effective the following turn.
pub async fn apply_model_switch(
    state: &TackleState,
    access: &dyn crate::scripts::host::SessionAccess,
    session_id: &SessionId,
    model_reference: &str,
    notify: &mut (dyn FnMut(SessionUpdateForOptions) + Send),
) -> Result<(), String> {
    // The switch is validated against the new model's window.
    let window = context_window(model_reference).unwrap_or(0);
    if window > 0 {
        let used = access
            .context_usage(session_id)
            .map_err(|err| err.to_string())?
            .used;
        let used_tokens = used / 4;
        if used_tokens * 4 >= window {
            // Auto-compaction: the compaction scripts run, announced,
            // with a context note. The switch still applies, effective
            // the following turn.
            notify(SessionUpdateForOptions::Notice(
                Notice::new(
                    NoticeSeverity::Warning,
                    "Switching to a model whose context window is smaller than the current context; auto-compacting.",
                )
                .description(Some(format!(
                    "The session uses about {used_tokens} tokens; the new window is {window}."
                ))),
            ));
            let _ = (&used_tokens, &window);
            let turn = state
                .db
                .append_turn(
                    session_id,
                    None,
                    crate::store::TurnKind::Interaction,
                    None,
                    Default::default(),
                )
                .await
                .map_err(|err| err.to_string())?;
            crate::acp::compaction::run_compaction(state, session_id, &turn.id, "default", window)
                .await;
        }
    }
    state
        .db
        .set_model(session_id, model_reference)
        .await
        .map_err(|err| err.to_string())?;
    Ok(())
}

/// The update kinds the switch path sends (kept opaque so the handler
/// maps onto the SDK types).
pub enum SessionUpdateForOptions {
    Notice(Notice),
}

/// The actor switch: effective the following turn.
pub async fn apply_actor_switch(
    state: &TackleState,
    session_id: &SessionId,
    actor: &str,
) -> Result<(), String> {
    if !state.definitions.actors.contains_key(actor) {
        return Err(format!("unknown actor `{actor}`"));
    }
    state
        .db
        .set_actor(session_id, actor)
        .await
        .map_err(|err| err.to_string())?;
    Ok(())
}
