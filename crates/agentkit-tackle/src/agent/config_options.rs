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

use crate::state::TackleState;
use crate::config::Auth;
use crate::store::{Session, SessionId};
use agent_client_protocol::schema::v1::{Notice, NoticeSeverity};
use agent_client_protocol::schema::v1::{
    SessionConfigId, SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory,
    SessionConfigSelect, SessionConfigSelectOption, SessionConfigValueId,
};

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
    pub options: Vec<SessionConfigOption>,
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
    let actor_name =
        crate::agent::config_options::session_actor(session, state.config().config.defaults.actor.as_deref());
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
    let first_model = model_options
        .first()
        .map(|option| option.value.0.to_string());
    if !model_options.is_empty() {
        // The effective model when unset: the shared resolution chain,
        // ending at the first listed option — never a silent mismatch
        // with what the prompt will actually run.
        let current = resolve_model_for(state, session, first_model.as_deref())
            .map(SessionConfigValueId::new)
            .unwrap_or_else(|| model_options[0].value.clone());
        options.push(
            SessionConfigOption::new(
                SessionConfigId::new(MODEL_SELECTOR_ID),
                "Model".to_owned(),
                SessionConfigKind::Select(SessionConfigSelect::new(current, model_options)),
            )
            .category(SessionConfigOptionCategory::Model),
        );
    }

    // The actor selector: the loaded actors.
    let current = crate::store::session_actor_value_free(session);
    let actor_options: Vec<SessionConfigSelectOption> = state
        .definitions()
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
    notify: &mut (dyn FnMut(SessionUpdateForOptions) + Send),
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
            crate::agent::compaction::run_compaction(
                state,
                session_id,
                &turn.id,
                "default",
                window,
                notices,
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
    use super::resolve_model;

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
