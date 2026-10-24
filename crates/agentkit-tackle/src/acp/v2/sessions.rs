//! The v2 session-lifecycle handlers: `session/new`, `list`, `close`,
//! `delete`, and `set_config_option`, served from the same store and
//! ownership leases as v1.

use crate::agent::config_options::{
    self, ConfigOption, ConfigOptionCategory, ConfigOptionKind, SelectOption,
};
use crate::agent::fork::{self, fork_access};
use crate::agent::{script_loop, seed as first_run};
use crate::content::timestamp;
use crate::scripts::host::NoticeSink;
use crate::state::TackleState;
use crate::store::{ListFilter, Role, SessionId, SessionKind, TurnKind, TurnUsage};
use agent_client_protocol::role::acp::Client;
use agent_client_protocol::schema::v2::{
    AbsolutePath, CloseSessionRequest, CloseSessionResponse, DeleteSessionRequest,
    DeleteSessionResponse, ForkSessionRequest, ForkSessionResponse, IdleStateUpdate,
    ListSessionsRequest, ListSessionsResponse,
    NewSessionRequest, NewSessionResponse, ReplayFrom, ResumeSessionRequest, ResumeSessionResponse,
    SessionConfigId, SessionConfigOption, SessionConfigOptionCategory, SessionConfigOptionValue,
    SessionConfigSelectOption, SessionConfigValueId, SessionInfo, SessionUpdate, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, StateUpdate, UpdateSessionNotification,
};
use agent_client_protocol::{Error, Responder, V2ConnectionTo};
use std::sync::Arc;

use super::{store_session_id, wire_session_id};

/// Maps a domain config option onto the v2 wire type (object-valued kind with
/// a `type` discriminator and a `configId`).
fn wire_option(option: &ConfigOption) -> SessionConfigOption {
    let ConfigOptionKind::Select { current, options } = &option.kind;
    let choices: Vec<SessionConfigSelectOption> = options
        .iter()
        .map(|SelectOption { value, label }| {
            SessionConfigSelectOption::new(
                SessionConfigValueId::new(value.clone()),
                label.clone(),
            )
        })
        .collect();
    SessionConfigOption::select(
        SessionConfigId::new(option.id.clone()),
        option.name.clone(),
        SessionConfigValueId::new(current.clone()),
        choices,
    )
    .category(match &option.category {
        ConfigOptionCategory::Model => SessionConfigOptionCategory::Model,
        ConfigOptionCategory::Other(name) => SessionConfigOptionCategory::Other(name.clone()),
    })
}

fn wire_options(options: &[ConfigOption]) -> Vec<SessionConfigOption> {
    options.iter().map(wire_option).collect()
}

/// A no-op notice sink: v2 notices are capability-free and land with the
/// notice work, so the config-switch path does not emit one yet.
struct NullNotices;

impl NoticeSink for NullNotices {
    fn script_model_call(&self, _session: &SessionId) {}
}

/// `session/new`: create the session in the shared store, seed the first turn,
/// and ride the selectors on the response.
pub(super) async fn handle_new_session(
    state: Arc<TackleState>,
    negotiation: Arc<crate::acp::v2::negotiation::ConnectionNegotiation>,
    request: NewSessionRequest,
    responder: Responder<NewSessionResponse>,
) -> Result<(), Error> {
    let actor = state
        .config()
        .config
        .defaults
        .actor
        .clone()
        .unwrap_or_else(|| "default".into());
    let additional: Vec<String> = request
        .additional_directories
        .iter()
        .map(|dir| dir.0.to_string_lossy().to_string())
        .collect();
    let metadata = serde_json::json!({
        "actor": actor,
        "additionalDirectories": additional,
    })
    .to_string();
    let session = state
        .db
        .create_session(
            SessionKind::Interactive,
            &request.cwd.0.to_string_lossy(),
            None,
            None,
            &metadata,
        )
        .await
        .map_err(|err| Error::internal_error().data(err.to_string()))?;

    let seed = first_run::first_run_seed(&state);
    let seed_turn = state
        .db
        .append_turn(
            &session.id,
            None,
            TurnKind::Seed,
            None,
            TurnUsage::default(),
        )
        .await
        .map_err(|err| Error::internal_error().data(err.to_string()))?;
    state
        .db
        .append_message(
            &seed_turn.id,
            Role::User,
            &serde_json::json!([{ "type": "text", "text": seed }]).to_string(),
            None,
            None,
            None,
            None,
        )
        .await
        .map_err(|err| Error::internal_error().data(err.to_string()))?;

    let mut selectors = config_options::build(&state, &session).await;
    if crate::store::session_model_free(&session).is_none() {
        if let Some(model) =
            config_options::resolve_model_for(&state, &session, selectors.first_model.as_deref())
        {
            state
                .db
                .set_model(&session.id, &model)
                .await
                .map_err(|err| Error::internal_error().data(err.to_string()))?;
            if let Ok(Some(fresh)) = state.db.get_session(&session.id).await {
                selectors = config_options::build(&state, &fresh).await;
            }
        }
    }

    responder.respond(
        NewSessionResponse::new(wire_session_id(&session.id))
            .config_options(wire_options(&selectors.options))
            .available_commands(super::commands::available_commands(&state)),
    )?;
    super::mcp::connect_forwarded_servers(&negotiation, &request.mcp_servers).await;
    Ok(())
}

/// `session/list`: the stored sessions, filtered by the request's cwd and the
/// configured ephemeral policy.
pub(super) async fn handle_list_sessions(
    state: Arc<TackleState>,
    request: ListSessionsRequest,
    responder: Responder<ListSessionsResponse>,
) -> Result<(), Error> {
    let cwd = request
        .cwd
        .as_ref()
        .map(|cwd| cwd.0.to_string_lossy().to_string());
    let filter = ListFilter {
        cwd: cwd.as_deref(),
        include_ephemeral: state
            .config()
            .config
            .sessions
            .include_ephemeral
            .unwrap_or(false),
        include_deleted: false,
    };
    let sessions = state
        .db
        .list_sessions(&filter)
        .await
        .map_err(|err| Error::internal_error().data(err.to_string()))?;
    let infos: Vec<SessionInfo> = sessions
        .into_iter()
        .map(|session| {
            let mut info = SessionInfo::new(wire_session_id(&session.id), session.cwd.clone())
                .updated_at(timestamp(session.updated_at));
            if !session.title.is_empty() {
                info = info.title(session.title.clone());
            }
            info.additional_directories = serde_json::from_str::<serde_json::Value>(
                &session.metadata,
            )
            .ok()
            .and_then(|meta| meta.get("additionalDirectories").cloned())
            .and_then(|value| serde_json::from_value::<Vec<AbsolutePath>>(value).ok())
            .unwrap_or_default();
            info
        })
        .collect();
    responder.respond(ListSessionsResponse::new(infos))
}

/// `session/close`: close the session and drop its permission grants.
pub(super) async fn handle_close_session(
    state: Arc<TackleState>,
    negotiation: Arc<crate::acp::v2::negotiation::ConnectionNegotiation>,
    request: CloseSessionRequest,
    responder: Responder<CloseSessionResponse>,
) -> Result<(), Error> {
    let store_session = store_session_id(&request.session_id);
    negotiation.cancel(&store_session);
    negotiation.wait_cancelled(&store_session).await;
    state
        .db
        .close_session(&store_session_id(&request.session_id))
        .await
        .map_err(|err| Error::internal_error().data(err.to_string()))?;
    state
        .grants
        .lock()
        .await
        .clear_session(&request.session_id.to_string());
    responder.respond(CloseSessionResponse::new())
}

/// `session/delete`: delete the session and drop its permission grants.
pub(super) async fn handle_delete_session(
    state: Arc<TackleState>,
    request: DeleteSessionRequest,
    responder: Responder<DeleteSessionResponse>,
) -> Result<(), Error> {
    state
        .db
        .delete_session(&store_session_id(&request.session_id))
        .await
        .map_err(|err| Error::internal_error().data(err.to_string()))?;
    state
        .grants
        .lock()
        .await
        .clear_session(&request.session_id.to_string());
    responder.respond(DeleteSessionResponse::new())
}

/// `session/resume`: reattach to a stored session. With `replayFrom: start`,
/// the full user-visible history replays before the response; an unsupported
/// cursor is rejected with `-32602`. A current-state snapshot follows any
/// replay and precedes the response in every case.
pub(super) async fn handle_resume_session(
    state: Arc<TackleState>,
    negotiation: Arc<crate::acp::v2::negotiation::ConnectionNegotiation>,
    cx: V2ConnectionTo<Client>,
    request: ResumeSessionRequest,
    responder: Responder<ResumeSessionResponse>,
) -> Result<(), Error> {
    super::mcp::connect_forwarded_servers(&negotiation, &request.mcp_servers).await;
    let session_id = store_session_id(&request.session_id);
    let session = state
        .db
        .get_session(&session_id)
        .await
        .map_err(|err| Error::internal_error().data(err.to_string()))?
        .ok_or_else(|| Error::internal_error().data("no such session".to_owned()))?;

    match &request.replay_from {
        Some(ReplayFrom::Start(_)) => {
            let assembled = state
                .db
                .assemble_context(&session_id)
                .await
                .map_err(|err| Error::internal_error().data(err.to_string()))?;
            for update in super::replay::replay_updates(&request.session_id, &assembled) {
                cx.send_notification(update)
                    .map_err(|err| Error::internal_error().data(err.to_string()))?;
            }
        }
        None => {}
        Some(_) => {
            return responder.respond_with_error(
                Error::invalid_params().data("unsupported replay cursor"),
            );
        }
    }

    cx.send_notification(UpdateSessionNotification::new(
        request.session_id.clone(),
        SessionUpdate::StateUpdate(StateUpdate::Idle(IdleStateUpdate::new())),
    ))
    .map_err(|err| Error::internal_error().data(err.to_string()))?;

    let selectors = config_options::build(&state, &session).await;
    responder.respond(
        ResumeSessionResponse::new()
            .config_options(wire_options(&selectors.options))
            .available_commands(super::commands::available_commands(&state)),
    )
}

/// `session/fork`: create a seeded fork, fire the `session_forked` scripts,
/// and return the fork's id with selectors.
pub(super) async fn handle_fork_session(
    state: Arc<TackleState>,
    cx: V2ConnectionTo<Client>,
    request: ForkSessionRequest,
    responder: Responder<ForkSessionResponse>,
) -> Result<(), Error> {
    let source = store_session_id(&request.session_id);
    let access = fork_access(
        &state,
        Arc::new(super::notices::ConnectionNotices {
            cx,
            source: "fork",
        }),
    );
    let fork = fork::create_fork(&state.db, &source)
        .await
        .map_err(|err| Error::internal_error().data(err.to_string()))?;
    fork::fire_session_forked(
        &state,
        access,
        &fork.id,
        &source,
        fork.fork_point_turn_id.as_ref(),
    );
    let selectors = config_options::build(&state, &fork).await;
    responder.respond(
        ForkSessionResponse::new(wire_session_id(&fork.id))
            .config_options(wire_options(&selectors.options))
            .available_commands(super::commands::available_commands(&state)),
    )
}

/// `session/set_config_option`: apply a model or actor switch and return the
/// refreshed selectors; the switch is effective the following turn.
pub(super) async fn handle_set_config(
    state: Arc<TackleState>,
    request: SetSessionConfigOptionRequest,
    responder: Responder<SetSessionConfigOptionResponse>,
) -> Result<(), Error> {
    let session_id = store_session_id(&request.session_id);
    let session = state
        .db
        .get_session(&session_id)
        .await
        .map_err(|err| Error::internal_error().data(err.to_string()))?
        .ok_or_else(|| Error::internal_error().data("no such session".to_owned()))?;

    let applied = match &request.value {
        SessionConfigOptionValue::Id { value } => {
            let value = value.to_string();
            match request.config_id.0.as_ref() {
                config_options::MODEL_SELECTOR_ID => {
                    let notices: Arc<dyn NoticeSink> = Arc::new(NullNotices);
                    let access = script_loop::access(&state, 0, Arc::clone(&notices));
                    let mut updates: Vec<config_options::Notice> = Vec::new();
                    let mut notify = |notice: config_options::Notice| {
                        updates.push(notice);
                    };
                    config_options::apply_model_switch(
                        &state,
                        access.as_ref(),
                        &session_id,
                        &value,
                        &mut notify,
                        notices,
                    )
                    .await
                }
                config_options::ACTOR_SELECTOR_ID => {
                    config_options::apply_actor_switch(&state, &session_id, &value).await
                }
                other => Err(format!("unknown config option `{other}`")),
            }
        }
        SessionConfigOptionValue::Boolean { .. } => {
            Err("boolean options are not configured".to_owned())
        }
        other => Err(format!("unsupported config value: {other:?}")),
    };

    let refreshed_session = if applied.is_ok() {
        state
            .db
            .get_session(&session_id)
            .await
            .ok()
            .flatten()
            .unwrap_or(session)
    } else {
        session
    };
    let refreshed = config_options::build(&state, &refreshed_session).await;
    match applied {
        Ok(()) => responder.respond(SetSessionConfigOptionResponse::new(wire_options(
            &refreshed.options,
        ))),
        Err(message) => responder.respond_with_error(
            Error::internal_error().data(serde_json::json!({ "error": message })),
        ),
    }
}
