//! The v2 session-lifecycle handlers: `session/new`, `list`, `close`,
//! `delete`, and `set_config_option`, served from the same store and
//! ownership leases as v1.

use crate::agent::config_options::{
    self, ConfigOption, ConfigOptionCategory, ConfigOptionKind, SelectOption,
};
use crate::acp::common::session as common_session;
use crate::agent::fork::{self, fork_access};
use crate::agent::script_loop;
use crate::content::timestamp;
use crate::scripts::host::NoticeSink;
use crate::state::TackleState;
use crate::store::ListFilter;
use agent_client_protocol::role::acp::Client;
use agent_client_protocol::schema::v2::{
    AbsolutePath, AvailableCommandsUpdate, CloseSessionRequest, CloseSessionResponse,
    ContentBlock, Cost, DeleteSessionRequest, DeleteSessionResponse, ForkSessionRequest,
    ForkSessionResponse, IdleStateUpdate, ListSessionsRequest, ListSessionsResponse, MessageId,
    NewSessionRequest, NewSessionResponse, Notice, NoticeSeverity, ReplayFrom, ResumeSessionRequest,
    ResumeSessionResponse,
    ConfigOptionUpdate, SessionConfigId, SessionConfigOption, SessionConfigOptionCategory,
    SessionConfigOptionValue,
    SessionConfigSelectOption, SessionConfigValueId, SessionInfo, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, StateUpdate,
    UpdateSessionNotification, UsageUpdate, UserMessage,
};
use agent_client_protocol::{Error, Responder, V2ConnectionTo};
use std::sync::Arc;

use super::{store_session_id, wire_session_id};


/// Sends exactly one usage snapshot for a live session: the effective model's
/// window, the assembled-context estimate, and the cumulative cost when known.
async fn send_usage_snapshot(
    state: &TackleState,
    cx: &V2ConnectionTo<Client>,
    wire_session: &agent_client_protocol::schema::v2::SessionId,
    store_session: &crate::store::SessionId,
) -> Result<(), Error> {
    let session = state
        .db
        .get_session(store_session)
        .await
        .map_err(|err| Error::internal_error().data(err.to_string()))?
        .ok_or_else(|| Error::internal_error().data("no such session".to_owned()))?;
    let size = config_options::resolve_model_for(state, &session, None)
        .and_then(|model| config_options::effective_context_window(state, &model))
        .unwrap_or(0);
    if size == 0 {
        return Ok(());
    }
    let assembled = state
        .db
        .assemble_context(store_session)
        .await
        .map_err(|err| Error::internal_error().data(err.to_string()))?;
    let used_bytes: u64 = assembled
        .iter()
        .map(|item| item.message.content.len() as u64)
        .sum();
    let used = crate::scripts::host::estimate_tokens(used_bytes);
    let usage = state
        .db
        .session_usage(store_session)
        .await
        .map_err(|err| Error::internal_error().data(err.to_string()))?;
    let cost = (usage.cost_usd > 0.0).then_some(usage.cost_usd);
    let update = UsageUpdate::new(used, size);
    let update = match cost {
        Some(amount) => update.cost(Cost::new(amount, "USD")),
        None => update,
    };
    let _ = cx.send_notification(UpdateSessionNotification::new(
        wire_session.clone(),
        SessionUpdate::UsageUpdate(update),
    ));
    Ok(())
}

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

/// `session/new`: create the session in the shared store, seed the first turn,
/// and ride the selectors on the response.
#[tracing::instrument(skip_all, fields(version = "v2", cwd = %request.cwd.0.display()))]
pub(super) async fn handle_new_session(
    state: Arc<TackleState>,
    negotiation: Arc<crate::acp::v2::negotiation::ConnectionNegotiation>,
    cx: V2ConnectionTo<Client>,
    request: NewSessionRequest,
    responder: Responder<NewSessionResponse>,
) -> Result<(), Error> {
    let additional: Vec<String> = request
        .additional_directories
        .iter()
        .map(|dir| dir.0.to_string_lossy().to_string())
        .collect();
    let session = common_session::create_interactive(
        &state,
        &request.cwd.0.to_string_lossy(),
        Some(serde_json::json!({ "additionalDirectories": additional })),
    )
    .await
    .map_err(|err| Error::internal_error().data(err))?;

    // The project layer awaits trust: resolve consent on this connection (the
    // first session does) before the seed and selectors read the effective
    // configuration.
    let requester = super::permission::AcpPermissionRequester {
        cx: cx.clone(),
        session_id: wire_session_id(&session.id),
    };
    common_session::resolve_project_trust(
        Arc::clone(&state),
        negotiation.mcp_pool(),
        requester,
        wire_session_id(&session.id).to_string(),
    )
    .await;

    let seed_message = common_session::seed_first_turn(&state, &session)
        .await
        .map_err(|err| Error::internal_error().data(err))?;
    let selectors = common_session::build_selectors(&state, &session)
        .await
        .map_err(|err| Error::internal_error().data(err))?;
    for notice in &selectors.notices {
        cx.send_notification(UpdateSessionNotification::new(
            wire_session_id(&session.id),
            SessionUpdate::Notice(wire_notice(notice)),
        ))?;
    }

    responder.respond(
        NewSessionResponse::new(wire_session_id(&session.id))
            .config_options(wire_options(&selectors.options))
            .available_commands(super::commands::available_commands(&state, &negotiation).await),
    )?;
    // The seed is stored as a user message; send it after the response (Zed
    // drops updates that precede session/new) so a fresh session shows it
    // without waiting for a replay.
    let seed_blocks: Vec<ContentBlock> =
        serde_json::from_str(&seed_message.content).unwrap_or_default();
    cx.send_notification(UpdateSessionNotification::new(
        wire_session_id(&session.id),
        SessionUpdate::UserMessage(
            UserMessage::new(MessageId::new(seed_message.id)).content(seed_blocks),
        ),
    ))
    .map_err(|err| Error::internal_error().data(err.to_string()))?;
    super::mcp::connect_forwarded_servers(&negotiation, &request.mcp_servers).await;
    // Forwarded servers joined after the response; refresh the command list
    // so their `!mcp.*` entries appear.
    let commands = super::commands::available_commands(&state, &negotiation).await;
    cx.send_notification(UpdateSessionNotification::new(
        wire_session_id(&session.id),
        SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(commands)),
    ))
    .map_err(|err| Error::internal_error().data(err.to_string()))?;
    if let Some(text) = super::commands::mcp_status_notice(&negotiation).await {
        let _ = cx.send_notification(UpdateSessionNotification::new(
            wire_session_id(&session.id),
            SessionUpdate::Notice(Notice::new(NoticeSeverity::Warning, text)),
        ));
    }
    send_usage_snapshot(
        &state,
        &cx,
        &wire_session_id(&session.id),
        &session.id,
    )
    .await?;
    Ok(())
}

/// `session/list`: the stored sessions, filtered by the request's cwd and the
/// configured ephemeral policy.
#[tracing::instrument(skip_all, fields(version = "v2"))]
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
#[tracing::instrument(skip_all, fields(version = "v2", session = %request.session_id))]
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
        .clear_session(&store_session_id(&request.session_id));
    responder.respond(CloseSessionResponse::new())
}

/// `session/delete`: delete the session and drop its permission grants.
#[tracing::instrument(skip_all, fields(version = "v2", session = %request.session_id))]
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
        .clear_session(&store_session_id(&request.session_id));
    responder.respond(DeleteSessionResponse::new())
}

/// `session/resume`: reattach to a stored session. With `replayFrom: start`,
/// the full user-visible history replays before the response; an unsupported
/// cursor is rejected with `-32602`. A current-state snapshot follows any
/// replay and precedes the response in every case.
#[tracing::instrument(skip_all, fields(version = "v2", session = %request.session_id))]
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
            .available_commands(super::commands::available_commands(&state, &negotiation).await),
    )?;
    send_usage_snapshot(
        &state,
        &cx,
        &request.session_id,
        &store_session_id(&request.session_id),
    )
    .await
}

/// `session/fork`: create a seeded fork, fire the `session_forked` scripts,
/// and return the fork's id with selectors.
#[tracing::instrument(skip_all, fields(version = "v2", session = %request.session_id))]
pub(super) async fn handle_fork_session(
    state: Arc<TackleState>,
    negotiation: Arc<crate::acp::v2::negotiation::ConnectionNegotiation>,
    cx: V2ConnectionTo<Client>,
    request: ForkSessionRequest,
    responder: Responder<ForkSessionResponse>,
) -> Result<(), Error> {
    let source = store_session_id(&request.session_id);
    let access = fork_access(
        &state,
        Arc::new(super::notices::ConnectionNotices {
            cx: cx.clone(),
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
    let _ = fork::insert_fork_seed(&state.db, &fork.id, &source).await;
    let selectors = config_options::build(&state, &fork).await;
    responder.respond(
        ForkSessionResponse::new(wire_session_id(&fork.id))
            .config_options(wire_options(&selectors.options))
            .available_commands(super::commands::available_commands(&state, &negotiation).await),
    )?;
    send_usage_snapshot(
        &state,
        &cx,
        &wire_session_id(&fork.id),
        &fork.id,
    )
    .await
}

/// `session/set_config_option`: apply a model or actor switch and return the
/// refreshed selectors; the switch is effective the following turn.
#[tracing::instrument(skip_all, fields(version = "v2", session = %request.session_id))]
pub(super) async fn handle_set_config(
    state: Arc<TackleState>,
    cx: V2ConnectionTo<Client>,
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

    let mut updates: Vec<config_options::Notice> = Vec::new();
    let applied = match &request.value {
        SessionConfigOptionValue::Id { value } => {
            let value = value.to_string();
            match request.config_id.0.as_ref() {
                config_options::MODEL_SELECTOR_ID => {
                    let notices: Arc<dyn NoticeSink> =
                        Arc::new(super::notices::ConnectionNotices {
                            cx: cx.clone(),
                            source: "config switch",
                        });
                    let access = script_loop::access(&state, 0, Arc::clone(&notices));
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
        Ok(()) => {
            for notice in &updates {
                let _ = cx.send_notification(UpdateSessionNotification::new(
                    request.session_id.clone(),
                    SessionUpdate::Notice(wire_notice(notice)),
                ));
            }
            let options = wire_options(&refreshed.options);
            let _ = cx.send_notification(UpdateSessionNotification::new(
                request.session_id.clone(),
                SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(options.clone())),
            ));
            responder.respond(SetSessionConfigOptionResponse::new(options))
        }
        Err(message) => responder.respond_with_error(
            Error::internal_error().data(serde_json::json!({ "error": message })),
        ),
    }
}

/// Maps a domain config-switch notice onto the v2 wire type.
fn wire_notice(notice: &config_options::Notice) -> Notice {
    let mut wire = Notice::new(
        match notice.severity {
            config_options::NoticeSeverity::Info => NoticeSeverity::Info,
            config_options::NoticeSeverity::Warning => NoticeSeverity::Warning,
        },
        notice.message.clone(),
    );
    if let Some(description) = &notice.description {
        wire = wire.description(Some(description.clone()));
    }
    wire
}
