//! Session configuration options: the model selector
//! and the actor selector.
//!
//! The model selector discovers models per endpoint (GET /models, both
//! wire formats' list shapes agree on `data[].id`); discovery is
//! primary, queried at session setup, and non-fatal — on failure or an
//! empty result the endpoint's static model list is the fallback and the
//! degraded state surfaces as a notice. The actor selector lists the
//! loaded actors. A mid-session model switch is validated against the new
//! model's context window: an over-window session auto-compacts,
//! announced, with a context note; the switch is effective the following
//! turn.

use crate::config::Auth;
use crate::state::TackleState;
use crate::store::{Session, SessionId};

/// A notice surfaced to the client (degraded discovery, auto-compaction).
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub severity: NoticeSeverity,
    pub message: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeSeverity {
    Info,
    Warning,
}

impl Notice {
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            severity: NoticeSeverity::Info,
            message: message.into(),
            description: None,
        }
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            severity: NoticeSeverity::Warning,
            message: message.into(),
            description: None,
        }
    }

    pub fn description(mut self, description: Option<String>) -> Self {
        self.description = description;
        self
    }
}

/// A session config option (the model or actor selector).
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigOption {
    pub id: String,
    pub name: String,
    pub kind: ConfigOptionKind,
    pub category: ConfigOptionCategory,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConfigOptionKind {
    Select {
        current: String,
        options: Vec<SelectOption>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct SelectOption {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConfigOptionCategory {
    Model,
    Other(String),
}

/// The model selector's config id.
pub const MODEL_SELECTOR_ID: &str = "model";
/// The actor selector's config id.
pub const ACTOR_SELECTOR_ID: &str = "actor";

/// Per-endpoint model discovery: the OpenAI list shape's `data[].id`.
/// Discovery is queried fresh at each session setup — no process-global
/// cache, so a model added to a running endpoint appears immediately and
/// two endpoints never collide. Failures return nothing and surface as
/// degraded discovery.
async fn discover_models(base_url: &str, credential: Option<String>) -> Option<Vec<String>> {
    // The OpenAI list shape: `data[].id` under `/models`.
    let url = format!("{base_url}/models");
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
    Some(ids)
}

/// Notices discovered while building the selectors (degraded discovery).
pub struct Selectors {
    pub options: Vec<ConfigOption>,
    pub notices: Vec<Notice>,
    /// The first listed model option (endpoint-qualified), when the
    /// selector was built — the unset default in the resolution chain.
    pub first_model: Option<String>,
}

/// The pure resolution chain: session metadata (the config-option
/// switch), then the actor's frontmatter override, then
/// `[defaults].model`, then the first listed selector option. Display
/// and prompt resolution share this so they never disagree.
pub fn resolve_model(
    session_model: Option<&str>,
    actor_model: Option<&str>,
    defaults_model: Option<&str>,
    first_listed: Option<&str>,
) -> Option<String> {
    session_model
        .map(str::to_owned)
        .or_else(|| actor_model.map(str::to_owned))
        .or_else(|| defaults_model.map(str::to_owned))
        .or_else(|| first_listed.map(str::to_owned))
}

/// The effective model for a session against the loaded state: the
/// shared chain, with `first_listed` the selector's unset default when
/// the listed options are known.
pub fn resolve_model_for(
    state: &TackleState,
    session: &Session,
    first_listed: Option<&str>,
) -> Option<String> {
    let session_model = crate::store::session_model_free(session);
    let actor_name = crate::agent::config_options::session_actor(
        session,
        state.config().config.defaults.actor.as_deref(),
    );
    let actor_model = state
        .definitions()
        .actors
        .get(&actor_name)
        .and_then(|actor| actor.frontmatter.model.clone());
    resolve_model(
        session_model.as_deref(),
        actor_model.as_deref(),
        state.config().config.defaults.model.as_deref(),
        first_listed,
    )
}

/// Builds the config options for a session: the model selector
/// (discovery primary, static fallback) and the actor selector.
pub async fn build(state: &TackleState, session: &Session) -> Selectors {
    let mut options = Vec::new();
    let mut notices = Vec::new();

    // The model selector: the endpoint-qualified models across the
    // configured endpoints.
    let mut model_options = Vec::new();
    for (name, endpoint) in &state.config().config.endpoints {
        let credential = match endpoint.auth {
            Auth::None => None,
            Auth::Helper => {
                let helper = crate::credentials::credential_helper_name(&state.config().config);
                crate::credentials::resolve_credential(&helper, name).map(|secret| {
                    use secrecy::ExposeSecret as _;
                    secret.expose_secret().to_owned()
                })
            }
        };
        let qualified = |model: &str| format!("{name}/{model}");
        let discovered = discover_models(&endpoint.base_url, credential).await;
        let models = match discovered {
            Some(models) => models,
            None => {
                if !endpoint.models.is_empty() {
                    notices.push(
                        Notice::info(format!(
                            "Endpoint `{name}` model discovery failed; using the configured static list."
                        ))
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
            model_options.push(SelectOption {
                value: reference,
                label: model,
            });
        }
    }
    let first_model = model_options.first().map(|option| option.value.clone());
    if !model_options.is_empty() {
        // The effective model when unset: the shared resolution chain,
        // ending at the first listed option — never a silent mismatch
        // with what the prompt will actually run.
        let current = resolve_model_for(state, session, first_model.as_deref())
            .unwrap_or_else(|| model_options[0].value.clone());
        options.push(ConfigOption {
            id: MODEL_SELECTOR_ID.to_owned(),
            name: "Model".to_owned(),
            kind: ConfigOptionKind::Select {
                current,
                options: model_options,
            },
            category: ConfigOptionCategory::Model,
        });
    }

    // The actor selector: the loaded actors.
    let current = crate::store::session_actor_value_free(session);
    let actor_options: Vec<SelectOption> = state
        .definitions()
        .actors
        .keys()
        .map(|name| SelectOption {
            value: name.clone(),
            label: name.clone(),
        })
        .collect();
    if !actor_options.is_empty() {
        options.push(ConfigOption {
            id: ACTOR_SELECTOR_ID.to_owned(),
            name: "Actor".to_owned(),
            kind: ConfigOptionKind::Select {
                current: current.unwrap_or_else(|| actor_options[0].value.clone()),
                options: actor_options,
            },
            category: ConfigOptionCategory::Other("actor".to_owned()),
        });
    }

    Selectors {
        options,
        notices,
        first_model,
    }
}

/// Applies a model switch: validated against the new model's context
/// window — an over-window session auto-compacts (announced, with a
/// context note) — effective the following turn.
pub async fn apply_model_switch(
    state: &std::sync::Arc<TackleState>,
    access: &dyn crate::scripts::host::SessionAccess,
    session_id: &SessionId,
    model_reference: &str,
    notify: &mut (dyn FnMut(Notice) + Send),
    notices: std::sync::Arc<dyn crate::scripts::host::NoticeSink>,
) -> Result<(), String> {
    // The switch is validated against the new model's window.
    let window = context_window(model_reference).unwrap_or(0);
    if window > 0 {
        // context_usage reports tokens, the same unit as the window.
        let used_tokens = access
            .context_usage(session_id)
            .map_err(|err| err.to_string())?
            .used;
        if used_tokens >= window {
            // Auto-compaction: the compaction scripts run, announced,
            // with a context note. The switch still applies, effective
            // the following turn.
            notify(
                Notice::warning(
                    "Switching to a model whose context window is smaller than the current context; auto-compacting.",
                )
                .description(Some(format!(
                    "The session uses about {used_tokens} tokens; the new window is {window}."
                ))),
            );
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
            crate::agent::compaction::run_compaction(
                state, session_id, &turn.id, "default", window, notices,
            )
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

/// The actor switch: effective the following turn.
pub async fn apply_actor_switch(
    state: &TackleState,
    session_id: &SessionId,
    actor: &str,
) -> Result<(), String> {
    if !state.definitions().actors.contains_key(actor) {
        return Err(format!("unknown actor `{actor}`"));
    }
    state
        .db
        .set_actor(session_id, actor)
        .await
        .map_err(|err| err.to_string())?;
    Ok(())
}

/// The model's context window from the bundled models.dev snapshot;
/// None when the model is unknown (no usage_update is sent then — the
/// RFD's guidance for unknowable window sizes).
pub fn context_window(model_ref: &str) -> Option<u64> {
    let bare = model_ref.split_once('/')?.1;
    let snapshot = agentkit_models::bundled_snapshot_parsed();
    snapshot.models.get(bare)?.context_window.map(u64::from)
}

/// The cost of one request from the bundled models.dev snapshot. The
/// endpoint-qualified reference splits into the provider key and model
/// name; an endpoint that is not a models.dev provider, or a model with
/// no rates, is unknowable and returns None. A zero rate is known.
pub(crate) fn request_cost(model_ref: &str, input_tokens: u64, output_tokens: u64) -> Option<f64> {
    let (provider, model) = model_ref.split_once('/')?;
    let snapshot = agentkit_models::bundled_snapshot_parsed();
    let pricing = snapshot.providers.get(provider)?.models.get(model)?;
    priced(
        input_tokens,
        output_tokens,
        pricing.input_per_mtok,
        pricing.output_per_mtok,
    )
}

fn priced(
    input_tokens: u64,
    output_tokens: u64,
    input_rate: Option<f64>,
    output_rate: Option<f64>,
) -> Option<f64> {
    if input_rate.is_none() && output_rate.is_none() {
        return None;
    }
    let input = input_rate.map_or(0.0, |rate| rate * input_tokens as f64 / 1_000_000.0);
    let output = output_rate.map_or(0.0, |rate| rate * output_tokens as f64 / 1_000_000.0);
    Some(input + output)
}

/// The session's selected actor: created in session metadata, falling
/// back to `[defaults].actor`, then the built-in "default".
pub(crate) fn session_actor(session: &crate::store::Session, fallback: Option<&str>) -> String {
    serde_json::from_str::<serde_json::Value>(&session.metadata)
        .ok()
        .and_then(|meta| meta.get("actor")?.as_str().map(str::to_owned))
        .or_else(|| fallback.map(str::to_owned))
        .unwrap_or_else(|| "default".into())
}

#[cfg(test)]
mod tests {
    use super::{priced, request_cost, resolve_model};

    fn close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-12,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn request_cost_prices_a_bundled_provider_model() {
        // openai/gpt-4o: $2.50/M input, $10/M output.
        close(
            request_cost("openai/gpt-4o", 1_000_000, 500_000).unwrap(),
            7.5,
        );
    }

    #[test]
    fn request_cost_is_unknown_for_an_unlisted_provider() {
        assert_eq!(request_cost("switchboard/gpt-4o", 1, 1), None);
    }

    #[test]
    fn request_cost_is_unknown_for_a_model_missing_from_a_known_provider() {
        assert_eq!(request_cost("openai/not-a-real-model", 1, 1), None);
    }

    #[test]
    fn request_cost_prices_whichever_rates_exist() {
        close(priced(1_000_000, 1_000_000, Some(2.0), None).unwrap(), 2.0);
        close(priced(1_000_000, 1_000_000, None, Some(8.0)).unwrap(), 8.0);
        assert_eq!(priced(1, 1, None, None), None);
    }

    #[test]
    fn request_cost_treats_an_explicit_zero_as_known() {
        close(
            priced(1_000_000, 1_000_000, Some(0.0), Some(0.0)).unwrap(),
            0.0,
        );
    }

    #[test]
    fn resolution_chain_prefers_session_then_actor_then_defaults_then_first() {
        assert_eq!(
            resolve_model(Some("s/m"), Some("a/m"), Some("d/m"), Some("f/m")).as_deref(),
            Some("s/m"),
            "session metadata wins"
        );
        assert_eq!(
            resolve_model(None, Some("a/m"), Some("d/m"), Some("f/m")).as_deref(),
            Some("a/m"),
            "actor frontmatter next"
        );
        assert_eq!(
            resolve_model(None, None, Some("d/m"), Some("f/m")).as_deref(),
            Some("d/m"),
            "[defaults].model next"
        );
        assert_eq!(
            resolve_model(None, None, None, Some("f/m")).as_deref(),
            Some("f/m"),
            "first listed option last"
        );
        assert_eq!(
            resolve_model(None, None, None, None),
            None,
            "nothing listed"
        );
    }
}
