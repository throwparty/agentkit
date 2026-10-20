//! The session-lifecycle handlers: `session/new`, `list`, `close`,
//! `delete`, `load`, `resume`, `set_config_option`, and `fork`.

use super::capabilities::UnstableFeature;
use super::commands::{available_commands_update, connect_forwarded_servers, mcp_status_notice};
use super::negotiation::ConnectionNegotiation;
use super::notices::connection_notices;
use super::permission::AcpPermissionRequester;
use super::replay::replay_updates;
use super::sink::usage_update;
use super::support::{send_notices, Cx};
use super::{wire_option, wire_options};
use crate::agent::fork::{self, fork_access};
use crate::agent::seed as first_run;
use crate::agent::{config_options, script_loop};
use crate::content::timestamp;
use crate::state::{store_session_id, wire_session_id, TackleState};
use agent_client_protocol::schema::v1::{
    CloseSessionResponse, ConfigOptionUpdate, DeleteSessionResponse, ForkSessionResponse,
    ListSessionsResponse, LoadSessionRequest, LoadSessionResponse, Meta, NewSessionRequest,
    NewSessionResponse, ReleaseTerminalRequest, ResumeSessionRequest, ResumeSessionResponse,
    SessionConfigOptionValue, SessionInfo, SessionNotification, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, TerminalId,
};
use agent_client_protocol::{Error, Responder};
use std::sync::Arc;

/// Sends exactly one usage snapshot for a live session: the effective
/// model's window, the assembled-context estimate, and the cumulative cost
/// when any request in the session was priced. Unknown windows send
/// nothing.
async fn send_usage_snapshot(
    state: &TackleState,
    cx: &Cx,
    wire_session: &agent_client_protocol::schema::v1::SessionId,
) -> Result<(), Error> {
    let session_id = store_session_id(wire_session);
    let session = state
        .db
        .get_session(&session_id)
        .await
        .map_err(|err| Error::internal_error().data(err.to_string()))?
        .ok_or_else(|| Error::internal_error().data("no such session".to_owned()))?;
    let size = config_options::resolve_model_for(state, &session, None)
        .and_then(|model_ref| config_options::effective_context_window(state, &model_ref))
        .unwrap_or(0);
    if size == 0 {
        return Ok(());
    }
    let assembled = state
        .db
        .assemble_context(&session_id)
        .await
        .map_err(|err| Error::internal_error().data(err.to_string()))?;
    let used_bytes: u64 = assembled
        .iter()
        .map(|item| item.message.content.len() as u64)
        .sum();
    let used = crate::scripts::host::estimate_tokens(used_bytes);
    let session_usage = state
        .db
        .session_usage(&session_id)
        .await
        .map_err(|err| Error::internal_error().data(err.to_string()))?;
    let cost = (session_usage.cost_usd > 0.0).then_some(session_usage.cost_usd);
    cx.send_notification(SessionNotification::new(
        wire_session.clone(),
        SessionUpdate::UsageUpdate(usage_update(used, size, cost)),
    ))
    .map_err(|err| Error::internal_error().data(err.to_string()))
}

/// `session/new`: create the session, resolve project trust, seed the first
/// turn, build the selectors, and (after the response) connect forwarded
/// servers and advertise commands.
pub(super) async fn handle_new_session(
    state: Arc<TackleState>,
    negotiation: Arc<ConnectionNegotiation>,
    cx: Cx,
    request: NewSessionRequest,
    responder: Responder<NewSessionResponse>,
) -> Result<(), Error> {
    // Default actor selection: [defaults].actor, else the built-in "default".
    let actor = state
        .config()
        .config
        .defaults
        .actor
        .clone()
        .unwrap_or_else(|| "default".into());
    let metadata = serde_json::json!({ "actor": actor }).to_string();
    let session = state
        .db
        .create_session(
            crate::store::SessionKind::Interactive,
            &request.cwd.to_string_lossy(),
            None,
            None,
            &metadata,
        )
        .await
        .map_err(|err| Error::internal_error().data(err.to_string()))?;
    // MCP connections kick off asynchronously; session creation never blocks
    // on them. Client-forwarded servers (session/new's mcp_servers) register
    // and connect after the response, below.
    //
    // The project layer awaits trust: resolve consent on this connection (the
    // first session does) before the seed and selectors read the effective
    // configuration.
    {
        let trust_session = wire_session_id(&session.id);
        let requester = AcpPermissionRequester {
            cx: cx.clone(),
            session_id: trust_session.clone(),
        };
        state
            .resolve_pending_project(
                Some(negotiation.mcp_pool()),
                &requester,
                &trust_session.to_string(),
            )
            .await;
    }
    // The first-session seed: harness-authored static content documenting the
    // loaded configuration and the command syntaxes, on its own seed turn.
    let seed = first_run::first_run_seed(&state);
    let seed_turn = state
        .db
        .append_turn(
            &session.id,
            None,
            crate::store::TurnKind::Seed,
            None,
            crate::store::TurnUsage::default(),
        )
        .await
        .map_err(|err| Error::internal_error().data(err.to_string()))?;
    state
        .db
        .append_message(
            &seed_turn.id,
            crate::store::Role::User,
            &serde_json::json!([{ "type": "text", "text": seed }]).to_string(),
            None,
            None,
            None,
            None,
        )
        .await
        .map_err(|err| Error::internal_error().data(err.to_string()))?;
    // The selectors ride the response; degraded discovery surfaces as notices.
    let mut selectors = config_options::build(&state, &session).await;
    send_notices(
        &cx,
        &negotiation,
        &wire_session_id(&session.id),
        &selectors.notices,
    );
    // Seed the effective model into session metadata when unset: display and
    // the first prompt agree without an explicit client switch.
    if crate::store::session_model_free(&session).is_none() {
        if let Some(model) =
            config_options::resolve_model_for(&state, &session, selectors.first_model.as_deref())
        {
            state
                .db
                .set_model(&session.id, &model)
                .await
                .map_err(|err| Error::internal_error().data(err.to_string()))?;
            // Refresh so the response's current matches the seeded metadata
            // (the chain is pure; rebuild picks the same effective model).
            if let Ok(Some(fresh)) = state.db.get_session(&session.id).await {
                selectors = config_options::build(&state, &fresh).await;
            }
        }
    }
    let wire_id = wire_session_id(&session.id);
    let response =
        NewSessionResponse::new(wire_id.clone()).config_options(wire_options(&selectors.options));
    // Zed drops session updates that arrive before the session/new response
    // registers the session (zed-industries/zed#60199) — advertise after.
    responder.respond(response)?;
    // Client-forwarded servers register and connect after the response, so
    // session/new returns immediately while their tools are in the
    // advertisement that follows and their status is known before the first
    // turn.
    connect_forwarded_servers(&negotiation, &request.mcp_servers).await;
    if let Some(notice) = mcp_status_notice(&negotiation).await {
        if negotiation.supports(UnstableFeature::SessionNotices) {
            let _ = cx.send_notification(SessionNotification::new(
                wire_id.clone(),
                SessionUpdate::Notice(notice),
            ));
        }
    }
    let commands = available_commands_update(&state, &negotiation).await;
    cx.send_notification(SessionNotification::new(wire_id.clone(), commands))?;
    send_usage_snapshot(&state, &cx, &wire_id).await
}

/// `session/list`: the stored sessions, filtered by the request's cwd and the
/// configured ephemeral policy.
pub(super) async fn handle_list_sessions(
    state: Arc<TackleState>,
    request: agent_client_protocol::schema::v1::ListSessionsRequest,
    responder: Responder<ListSessionsResponse>,
) -> Result<(), Error> {
    let cwd = request
        .cwd
        .as_ref()
        .map(|cwd| cwd.to_string_lossy().to_string());
    let filter = crate::store::ListFilter {
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
            if let Some(owner) = &session.owner {
                let mut meta = Meta::new();
                meta.insert("owner".into(), owner.clone().into());
                info = info.meta(meta);
            }
            info
        })
        .collect();
    responder.respond(ListSessionsResponse::new(infos))
}

/// `session/close`: close the session, drop its permission grants, and release
/// the terminals the harness created for it (Q8 lifecycle).
pub(super) async fn handle_close_session(
    state: Arc<TackleState>,
    negotiation: Arc<ConnectionNegotiation>,
    cx: Cx,
    request: agent_client_protocol::schema::v1::CloseSessionRequest,
    responder: Responder<CloseSessionResponse>,
) -> Result<(), Error> {
    state
        .db
        .close_session(&store_session_id(&request.session_id))
        .await
        .map_err(|err| Error::internal_error().data(err.to_string()))?;
    // The session's active life is over: its permission grants go with it
    // (they never outlive their session).
    state
        .grants
        .lock()
        .await
        .clear_session(&request.session_id.to_string());
    // Release the terminals the harness created for this session: connection-
    // scoped and ephemeral, so best-effort and silent on failure.
    let terminals: Vec<String> = {
        let mut map = negotiation.terminals.lock().expect("negotiation poisoned");
        map.remove(&store_session_id(&request.session_id))
            .unwrap_or_default()
            .into_iter()
            .collect()
    };
    for terminal_id in terminals {
        let release =
            ReleaseTerminalRequest::new(request.session_id.clone(), TerminalId::new(terminal_id));
        let _ = cx.send_request(release).block_task().await;
    }
    responder.respond(CloseSessionResponse::new())
}

/// `session/delete`: delete the session and drop its permission grants.
pub(super) async fn handle_delete_session(
    state: Arc<TackleState>,
    request: agent_client_protocol::schema::v1::DeleteSessionRequest,
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

/// `session/load`: replay the full user-visible history, re-advertise commands
/// and selectors, and emit exactly one usage snapshot so a reopened client
/// shows the session's context state.
pub(super) async fn handle_load_session(
    state: Arc<TackleState>,
    negotiation: Arc<ConnectionNegotiation>,
    cx: Cx,
    request: LoadSessionRequest,
    responder: Responder<LoadSessionResponse>,
) -> Result<(), Error> {
    // A forwarded server's tools must be addressable before the
    // re-advertisement below lists them.
    connect_forwarded_servers(&negotiation, &request.mcp_servers).await;
    // Replay: full user-visible history in DAG order. Usage snapshot emission
    // lands with the model metadata.
    let assembled = state
        .db
        .assemble_context(&store_session_id(&request.session_id))
        .await
        .map_err(|err| Error::internal_error().data(err.to_string()))?;
    for replay in replay_updates(&request.session_id, &assembled) {
        cx.send_notification(replay)
            .map_err(|err| Error::internal_error().data(err.to_string()))?;
    }
    // Re-advertise on load: older Zed builds rebuild the thread UI from
    // retained state and treat commands as transient
    // (zed-industries/zed#53209), and the pre-response window drops updates
    // outright. Selectors ride the response so a reopened session can switch
    // models before the next prompt.
    let session = state
        .db
        .get_session(&store_session_id(&request.session_id))
        .await
        .map_err(|err| Error::internal_error().data(err.to_string()))?
        .ok_or_else(|| Error::internal_error().data("no such session".to_owned()))?;
    let selectors = config_options::build(&state, &session).await;
    let commands = available_commands_update(&state, &negotiation).await;
    responder
        .respond(LoadSessionResponse::new().config_options(wire_options(&selectors.options)))?;
    send_notices(&cx, &negotiation, &request.session_id, &selectors.notices);
    cx.send_notification(SessionNotification::new(
        request.session_id.clone(),
        commands,
    ))?;
    // The usage snapshot follows the response and replay, so a reopened
    // client shows the session's context state.
    send_usage_snapshot(&state, &cx, &request.session_id).await
}

/// `session/resume`: reattach WITHOUT replay; config selectors ride the
/// response like session/new.
pub(super) async fn handle_resume_session(
    state: Arc<TackleState>,
    negotiation: Arc<ConnectionNegotiation>,
    cx: Cx,
    request: ResumeSessionRequest,
    responder: Responder<ResumeSessionResponse>,
) -> Result<(), Error> {
    connect_forwarded_servers(&negotiation, &request.mcp_servers).await;
    let session = state
        .db
        .get_session(&store_session_id(&request.session_id))
        .await
        .map_err(|err| Error::internal_error().data(err.to_string()))?
        .ok_or_else(|| Error::internal_error().data("no such session".to_owned()))?;
    let selectors = config_options::build(&state, &session).await;
    responder
        .respond(ResumeSessionResponse::new().config_options(wire_options(&selectors.options)))?;
    send_notices(&cx, &negotiation, &request.session_id, &selectors.notices);
    send_usage_snapshot(&state, &cx, &request.session_id).await
}

/// `session/set_config_option`: apply a model or actor switch and return the
/// refreshed selectors; the switch is effective the following turn.
pub(super) async fn handle_set_config(
    state: Arc<TackleState>,
    negotiation: Arc<ConnectionNegotiation>,
    cx: Cx,
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
        SessionConfigOptionValue::ValueId { value } => {
            let value = value.to_string();
            match request.config_id.0.as_ref() {
                config_options::MODEL_SELECTOR_ID => {
                    let notices = connection_notices(&cx, &negotiation, "model switch");
                    let access = script_loop::access(&state, 0, Arc::clone(&notices));
                    let mut updates: Vec<config_options::Notice> = Vec::new();
                    let mut notify = |notice: config_options::Notice| {
                        updates.push(notice);
                    };
                    let result = config_options::apply_model_switch(
                        &state,
                        access.as_ref(),
                        &session_id,
                        &value,
                        &mut notify,
                        notices,
                    )
                    .await;
                    send_notices(&cx, &negotiation, &request.session_id, &updates);
                    result
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

    // The refreshed selectors; the switch is effective the following turn.
    let refreshed = config_options::build(&state, &session).await;
    match applied {
        Ok(()) => {
            let _ = cx.send_notification(SessionNotification::new(
                request.session_id.clone(),
                SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(
                    refreshed.options.iter().map(wire_option).collect(),
                )),
            ));
            responder.respond(SetSessionConfigOptionResponse::new(
                refreshed.options.iter().map(wire_option).collect(),
            ))
        }
        Err(message) => responder.respond_with_error(
            Error::internal_error().data(serde_json::json!({ "error": message })),
        ),
    }
}

/// `session/fork`: create the fork, fire the `session_forked` scripts, and
/// return the fork's id with selectors. No session updates flow here — the
/// client attaches (`session/load`) and updates follow.
pub(super) async fn handle_fork_session(
    state: Arc<TackleState>,
    negotiation: Arc<ConnectionNegotiation>,
    cx: Cx,
    request: agent_client_protocol::schema::v1::ForkSessionRequest,
    responder: Responder<ForkSessionResponse>,
) -> Result<(), Error> {
    let source = store_session_id(&request.session_id);
    let access = fork_access(&state, connection_notices(&cx, &negotiation, "fork"));
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
    // Selectors on the fork response: the client attaching can switch before
    // the first prompt.
    let selectors = config_options::build(&state, &fork).await;
    send_notices(
        &cx,
        &negotiation,
        &wire_session_id(&fork.id),
        &selectors.notices,
    );
    responder.respond(
        ForkSessionResponse::new(wire_session_id(&fork.id))
            .config_options(wire_options(&selectors.options)),
    )
}
