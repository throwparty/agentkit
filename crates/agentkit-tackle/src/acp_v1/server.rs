//! The ACP server: the stdio entry point and the per-connection handler builder.

use super::capabilities::UnstableFeature;
use super::client_tools;
use super::commands::{
    available_commands_update, connect_config_servers, connect_forwarded_servers, editor_command,
    mcp_command, mcp_status_notice,
};
use crate::agent::config_options;
use crate::agent::config_options::context_window;
use crate::agent::fork;
use crate::agent::fork::fork_access;
use super::negotiation::ConnectionNegotiation;
use super::notices::connection_notices;
use super::permission::AcpPermissionRequester;
use super::replay::replay_updates;
use crate::content::timestamp;
use crate::agent::seed as first_run;
use crate::agent::{compaction, script_loop};
use crate::agent::config_options::session_actor;
use crate::agent::direct::run_direct_invocation;
use crate::agent::prompt::run_prompt_turn;
use crate::state::{LEASE_TTL_SECS, LeaseHeartbeat, TackleState, store_session_id, wire_session_id};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    CloseSessionResponse, DeleteSessionResponse, InitializeRequest, InitializeResponse,
    ListSessionsResponse, LoadSessionRequest, LoadSessionResponse, NewSessionRequest,
    NewSessionResponse, ResumeSessionRequest, ResumeSessionResponse, SessionInfo,
    SessionNotification, SessionUpdate,
};
use agent_client_protocol::{Agent, Error, Stdio};
use std::sync::Arc;

pub async fn run_stdio(state: Arc<TackleState>) -> agent_client_protocol::Result<()> {
    let editor = client_tools::EditorToggles::from_config(&state.config().config);
    let pool = connect_config_servers(&state).await;
    let negotiation = Arc::new(ConnectionNegotiation::new(editor, pool));
    // Ctrl-C/SIGTERM ends the transport cleanly: the connection is
    // dropped and the caller reports a normal shutdown.
    tokio::select! {
        result = run_agent_over(state, negotiation, Stdio::new()) => result,
        _ = crate::cli::wait_for_shutdown() => Ok(()),
    }
}

pub(crate) async fn run_agent_over<T>(
    state: Arc<TackleState>,
    negotiation: Arc<ConnectionNegotiation>,
    transport: T,
) -> agent_client_protocol::Result<()>
where
    T: agent_client_protocol::ConnectTo<agent_client_protocol::role::acp::Agent>,
{
    let negotiation_for_init = negotiation.clone();
    let state_for_init = state.clone();

    let state_for_new = state.clone();
    let state_for_list = state.clone();
    let negotiation_for_new = negotiation.clone();
    let negotiation_for_config = negotiation.clone();
    let state_for_config = state.clone();
    let state_for_close = state.clone();
    let state_for_delete = state.clone();
    let state_for_load = state.clone();
    let state_for_resume = state.clone();
    let state_for_fork = state.clone();
    let negotiation_for_load = negotiation.clone();
    let negotiation_for_resume = negotiation.clone();
    let negotiation_for_fork = negotiation.clone();
    let negotiation_for_compaction = negotiation.clone();
    let negotiation_for_prompt = negotiation.clone();
    let negotiation_for_close = negotiation.clone();
    // The connection's lease-owner id: turns acquire it for their
    // lifetime; released when the turn responds. Transport-neutral —
    // stdio and HTTP connections both use this handler.
    let owner = Arc::new(format!("conn-{}", uuid::Uuid::new_v4()));
    let owner_for_prompt = owner.clone();
    let state_for_prompt = state.clone();

    let builder = Agent
        .builder()
        .name("tackle")
        .on_receive_request(
            async move |request: InitializeRequest, responder, _cx| {
                // Protocol version negotiation: tackle implements v1, so it
                // responds with v1 regardless of the client's latest; a
                // v1-only client proceeds, a v2-only client disconnects.
                negotiation_for_init.capture(&request.client_capabilities);
                // Missing provider credentials surface as authMethods:
                // the client drives the ACP authenticate method.
                let mut response = InitializeResponse::new(ProtocolVersion::V1)
                    .agent_capabilities(state_for_init.capabilities());
                if first_run::missing_credentials(&state_for_init) {
                    response.auth_methods =
                        vec![agent_client_protocol::schema::v1::AuthMethod::Agent(
                            agent_client_protocol::schema::v1::AuthMethodAgent::new(
                                "credentials",
                                "Set up model credentials",
                            )
                            .description(Some(
                                "Resolve the configured model endpoints' credentials.".to_owned(),
                            )),
                        )];
                }
                responder.respond(response)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: agent_client_protocol::schema::v1::AuthenticateRequest,
                        responder,
                        _cx| {
                // Credentials resolve out-of-band (the `tackle auth`
                // subcommand), so this method only acknowledges the
                // client's login attempt; the next prompt re-resolves and
                // surfaces any remaining failure.
                responder.respond(agent_client_protocol::schema::v1::AuthenticateResponse::new())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: NewSessionRequest, responder, cx| {
                // Default actor selection: [defaults].actor, else the
                // built-in "default".
                let actor = state_for_new
                    .config()
                    .config
                    .defaults
                    .actor
                    .clone()
                    .unwrap_or_else(|| "default".into());
                let metadata = serde_json::json!({ "actor": actor }).to_string();
                let session = state_for_new
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
                // MCP connections kick off asynchronously; session
                // creation never blocks on them. Client-forwarded servers
                // (session/new's mcp_servers) register and connect after
                // the response, below.
                //
                // The project layer awaits trust: resolve consent on this
                // connection (the first session does) before the seed and
                // selectors read the effective configuration.
                {
                    let trust_session = wire_session_id(&session.id);
                    let requester = AcpPermissionRequester {
                        cx: cx.clone(),
                        session_id: trust_session.clone(),
                    };
                    state_for_new
                        .resolve_pending_project(
                            Some(negotiation_for_new.mcp_pool()),
                            &requester,
                            &trust_session.to_string(),
                        )
                        .await;
                }
                // The first-session seed: harness-authored static content
                // documenting the loaded configuration and the command
                // syntaxes, on its own seed turn.
                let seed = first_run::first_run_seed(&state_for_new);
                let seed_turn = state_for_new
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
                state_for_new
                    .db
                    .append_message(
                        &seed_turn.id,
                        crate::store::Role::User,
                        &serde_json::json!([{ "type": "text", "text": seed }])
                            .to_string(),
                        None,
                        None,
                        None,
                        None,
                    )
                    .await
                    .map_err(|err| Error::internal_error().data(err.to_string()))?;
                // The selectors ride the response; degraded discovery
                // surfaces as notices.
                let mut selectors = config_options::build(&state_for_new, &session).await;
                for notice in &selectors.notices {
                    if negotiation_for_new.supports(UnstableFeature::SessionNotices) {
                        let _ = cx.send_notification(SessionNotification::new(
                            wire_session_id(&session.id),
                            SessionUpdate::Notice(notice.clone()),
                        ));
                    }
                }
                // Seed the effective model into session metadata when
                // unset: display and the first prompt agree without an
                // explicit client switch.
                if crate::store::session_model_free(&session).is_none() {
                    if let Some(model) = config_options::resolve_model_for(
                        &state_for_new,
                        &session,
                        selectors.first_model.as_deref(),
                    ) {
                        state_for_new
                            .db
                            .set_model(&session.id, &model)
                            .await
                            .map_err(|err| Error::internal_error().data(err.to_string()))?;
                        // Refresh so the response's current matches the
                        // seeded metadata (the chain is pure; rebuild
                        // picks the same effective model).
                        if let Ok(Some(fresh)) = state_for_new.db.get_session(&session.id).await {
                            selectors = config_options::build(&state_for_new, &fresh).await;
                        }
                    }
                }
                let wire_id = wire_session_id(&session.id);
                let response =
                    NewSessionResponse::new(wire_id.clone()).config_options(selectors.options);
                // Zed drops session updates that arrive before the
                // session/new response registers the session
                // (zed-industries/zed#60199) — advertise after.
                responder.respond(response)?;
                // Client-forwarded servers register and connect after the
                // response, so session/new returns immediately while their
                // tools are in the advertisement that follows and their
                // status is known before the first turn.
                connect_forwarded_servers(&negotiation_for_new, &request.mcp_servers).await;
                if let Some(notice) = mcp_status_notice(&negotiation_for_new).await {
                    if negotiation_for_new.supports(UnstableFeature::SessionNotices) {
                        let _ = cx.send_notification(SessionNotification::new(
                            wire_id.clone(),
                            SessionUpdate::Notice(notice),
                        ));
                    }
                }
                let commands = available_commands_update(&state_for_new, &negotiation_for_new).await;
                cx.send_notification(SessionNotification::new(wire_id, commands))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: crate::agent_client_protocol::schema::v1::ListSessionsRequest,
                        responder,
                        _cx| {
                let cwd = request
                    .cwd
                    .as_ref()
                    .map(|cwd| cwd.to_string_lossy().to_string());
                let filter = crate::store::ListFilter {
                    cwd: cwd.as_deref(),
                    include_ephemeral: state_for_list
                        .config()
                        .config
                        .sessions
                        .include_ephemeral
                        .unwrap_or(false),
                    include_deleted: false,
                };
                let sessions = state_for_list
                    .db
                    .list_sessions(&filter)
                    .await
                    .map_err(|err| Error::internal_error().data(err.to_string()))?;
                let infos: Vec<SessionInfo> = sessions
                    .into_iter()
                    .map(|session| {
                        let mut info =
                            SessionInfo::new(wire_session_id(&session.id), session.cwd.clone())
                                .updated_at(timestamp(session.updated_at));
                        if !session.title.is_empty() {
                            info = info.title(session.title.clone());
                        }
                        if let Some(owner) = &session.owner {
                            let mut meta = agent_client_protocol::schema::v1::Meta::new();
                            meta.insert("owner".into(), owner.clone().into());
                            info = info.meta(meta);
                        }
                        info
                    })
                    .collect();
                responder.respond(ListSessionsResponse::new(infos))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: agent_client_protocol::schema::v1::CloseSessionRequest,
                        responder,
                        cx| {
                state_for_close
                    .db
                    .close_session(&store_session_id(&request.session_id))
                    .await
                    .map_err(|err| Error::internal_error().data(err.to_string()))?;
                // The session's active life is over: its permission
                // grants go with it (they never outlive their session).
                state_for_close
                    .grants
                    .lock()
                    .await
                    .clear_session(&request.session_id.to_string());
                // Release the terminals the harness created for this
                // session (Q8 lifecycle): connection-scoped and
                // ephemeral, so best-effort and silent on failure.
                let terminals: Vec<String> = {
                    let mut map = negotiation_for_close
                        .terminals
                        .lock()
                        .expect("negotiation poisoned");
                    map.remove(&store_session_id(&request.session_id))
                        .unwrap_or_default()
                        .into_iter()
                        .collect()
                };
                for terminal_id in terminals {
                    let release = agent_client_protocol::schema::v1::ReleaseTerminalRequest::new(
                        request.session_id.clone(),
                        agent_client_protocol::schema::v1::TerminalId::new(terminal_id),
                    );
                    let _ = cx.send_request(release).block_task().await;
                }
                responder.respond(CloseSessionResponse::new())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: agent_client_protocol::schema::v1::PromptRequest,
                        responder,
                        cx| {
                use agent_client_protocol::schema::v1::{
                    ContentBlock, PromptResponse, StopReason, UsageUpdate,
                };

                let session_id = store_session_id(&request.session_id);

                // The lease is the one-turn-at-a-time invariant: prompt
                // against an actively-owned session fails precisely.
                if !state_for_prompt
                    .db
                    .acquire_lease(&session_id, &owner_for_prompt, LEASE_TTL_SECS)
                    .await
                    .map_err(|err| Error::internal_error().data(err.to_string()))?
                {
                    let holder = state_for_prompt
                        .db
                        .lease_holder(&session_id)
                        .await
                        .ok()
                        .flatten()
                        .unwrap_or_else(|| "another connection".into());
                    return responder.respond_with_error(Error::internal_error().data(
                        serde_json::json!({
                            "error": "session-actively-owned",
                            "owner": holder,
                        }),
                    ));
                }

                // Keep the lease live for the whole turn, however long it
                // runs; dropping the guard on any return path stops renewal.
                let _lease_heartbeat = LeaseHeartbeat::spawn(
                    Arc::clone(&state_for_prompt.db),
                    session_id.clone(),
                    owner_for_prompt.as_ref().clone(),
                );

                // Persist the prompt turn + the user message (original
                // content blocks, stored as the JSON block array).
                let blocks_json =
                    serde_json::to_string(&request.prompt).unwrap_or_else(|_| "[]".to_owned());
                let head = state_for_prompt
                    .db
                    .get_session(&session_id)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|session| session.head_turn_id);
                let turn = state_for_prompt
                    .db
                    .append_turn(
                        &session_id,
                        head.as_ref(),
                        crate::store::TurnKind::Interaction,
                        None,
                        crate::store::TurnUsage::default(),
                    )
                    .await
                    .map_err(|err| Error::internal_error().data(err.to_string()))?;
                state_for_prompt
                    .db
                    .append_message(
                        &turn.id,
                        crate::store::Role::User,
                        &blocks_json,
                        None,
                        None,
                        None,
                        None,
                    )
                    .await
                    .map_err(|err| Error::internal_error().data(err.to_string()))?;

                // The /fork fallback for v1 clients: exact first-token
                // match on user-typed input. Creates the fork, seeds it,
                // fires the session_forked scripts, and reports the new
                // session id in the parent turn — no model request.
                let first_text = request
                    .prompt
                    .first()
                    .and_then(|block| match block {
                        ContentBlock::Text(text) => Some(text.text.trim().to_owned()),
                        _ => None,
                    })
                    .unwrap_or_default();
                if first_text.split_whitespace().next() == Some("/fork") {
                    let access =
                        fork_access(&state_for_prompt, connection_notices(&cx, &negotiation_for_prompt, "fork"));
                    let fork_result = fork::run_fork(
                        &state_for_prompt,
                        std::sync::Arc::clone(&access),
                        &session_id,
                    )
                    .await;
                    let fork = match fork_result {
                        Ok(fork) => fork,
                        Err(err) => {
                            state_for_prompt
                                .db
                                .release_lease(&session_id, &owner_for_prompt)
                                .await
                                .ok();
                            return responder
                                .respond_with_error(Error::internal_error().data(err.to_string()));
                        }
                    };
                    // Seeds are harness-authored static user-facing
                    // content with their own messageId.
                    let _ =
                        fork::insert_fork_seed(&state_for_prompt.db, &fork.id, &session_id).await;

                    // The parent-turn report: an agent message carrying
                    // the new session id (wire form for the client).
                    let wire_fork_id = wire_session_id(&fork.id).to_string();
                    let report =
                        fork::fork_report_message(&wire_fork_id, fork.fork_point_turn_id.as_ref());
                    let message = state_for_prompt
                        .db
                        .append_message(
                            &turn.id,
                            crate::store::Role::Assistant,
                            &serde_json::json!([{ "type": "text", "text": report.clone() }])
                                .to_string(),
                            None,
                            None,
                            None,
                            None,
                        )
                        .await
                        .map_err(|err| Error::internal_error().data(err.to_string()))?;
                    let chunk =
                        agent_client_protocol::schema::v1::ContentChunk::new(ContentBlock::Text(
                            agent_client_protocol::schema::v1::TextContent::new(report.clone()),
                        ))
                        .message_id(
                            agent_client_protocol::schema::v1::MessageId::new(message.id.clone()),
                        );
                    let _ = cx.send_notification(SessionNotification::new(
                        request.session_id.clone(),
                        agent_client_protocol::schema::v1::SessionUpdate::AgentMessageChunk(chunk),
                    ));

                    // updatedAt each turn — the fork's report turn too.
                    let updated = agent_client_protocol::schema::v1::SessionInfoUpdate::new()
                        .updated_at(
                            state_for_prompt
                                .db
                                .get_session(&session_id)
                                .await
                                .ok()
                                .flatten()
                                .map(|session| timestamp(session.updated_at))
                                .unwrap_or_default(),
                        );
                    let _ = cx.send_notification(SessionNotification::new(
                        request.session_id.clone(),
                        SessionUpdate::SessionInfoUpdate(updated),
                    ));
                    state_for_prompt
                        .db
                        .release_lease(&session_id, &owner_for_prompt)
                        .await
                        .ok();
                    return responder.respond(PromptResponse::new(StopReason::EndTurn));
                }

                // `/!mcp.<server>.<tool> {json}` or
                // `/!client.<surface>.<method> {json}`: the user calls a
                // tool directly. No model request, and no permission
                // prompt — the call is the user's own keystroke, so the
                // pipeline has nothing to ask. The result is reported as
                // a tool call and stored in the turn.
                if let Some(input) = crate::invokables::parse_direct(&first_text) {
                    let call_id = format!("direct-{}", uuid::Uuid::new_v4());
                    let arguments: serde_json::Value =
                        serde_json::from_str(&input.arguments).unwrap_or(serde_json::Value::Null);
                    let announce = agent_client_protocol::schema::v1::ToolCall::new(
                        agent_client_protocol::schema::v1::ToolCallId::new(call_id.clone()),
                        crate::invokables::titles::tool_call_title(&input.invokable, &arguments),
                    )
                    .name(input.invokable.clone())
                    .kind(super::wire_kind(crate::invokables::titles::tool_kind(&input.invokable)))
                    .status(agent_client_protocol::schema::v1::ToolCallStatus::InProgress)
                    .raw_input(Some(arguments.clone()));
                    let _ = cx.send_notification(SessionNotification::new(
                        request.session_id.clone(),
                        agent_client_protocol::schema::v1::SessionUpdate::ToolCall(announce),
                    ));

                    // Direct invocation may issue a request back to the
                    // client (a client-capability tool), and awaiting it
                    // inside the dispatch callback would deadlock the
                    // dispatch loop on its own response. Run the whole
                    // effect on a spawned task, like a model turn, and
                    // respond when it completes.
                    let cx = cx.clone();
                    let state = state_for_prompt.clone();
                    let negotiation = negotiation_for_prompt.clone();
                    let wire_id = request.session_id.clone();
                    let turn_id = turn.id.clone();
                    let owner = owner_for_prompt.clone();
                    let spawn_cx = cx.clone();
                    let runtime = crate::agent::runtime::TurnRuntime {
                        pool: negotiation.mcp_pool(),
                        sink: std::sync::Arc::new(super::ConnectionTurnSink::new(cx.clone())),
                        requester: std::sync::Arc::new(
                            super::permission::AcpPermissionRequester {
                                cx: cx.clone(),
                                session_id: wire_id.clone(),
                            },
                        ),
                        executor: std::sync::Arc::new(client_tools::ConnectionClientTools {
                            cx: cx.clone(),
                        }),
                        tracker: negotiation.clone(),
                        compaction_notices: connection_notices(&cx, &negotiation, "compaction"),
                        titling_notices: connection_notices(&cx, &negotiation, "titling"),
                        client_tools: client_tools::catalog(
                            negotiation.client_caps(),
                            negotiation.editor_toggles(),
                        ),
                    };
                    spawn_cx.spawn(async move {
                        match run_direct_invocation(
                            state,
                            runtime,
                            turn_id,
                            session_id.clone(),
                            owner,
                            call_id,
                            input,
                        )
                        .await
                        {
                            Ok(()) => responder.respond(PromptResponse::new(StopReason::EndTurn)),
                            Err(err) => {
                                responder.respond_with_error(Error::internal_error().data(err))
                            }
                        }
                    })?;
                    return Ok(());
                }

                // Runtime availability commands: `/mcp` and `/editor`
                // are handled before any model request. An unrecognised
                // subcommand yields no reply and falls through to
                // normal processing.
                if first_text == "/mcp"
                    || first_text.starts_with("/mcp ")
                    || first_text == "/editor"
                    || first_text.starts_with("/editor ")
                {
                    let reply = if first_text.starts_with("/editor") {
                        editor_command(&negotiation_for_prompt, &first_text)
                    } else {
                        mcp_command(&negotiation_for_prompt, &first_text).await
                    };
                    if let Some(reply) = reply {
                        let message = state_for_prompt
                            .db
                            .append_message(
                                &turn.id,
                                crate::store::Role::Assistant,
                                &serde_json::json!([{ "type": "text", "text": reply }])
                                    .to_string(),
                                None,
                                None,
                                None,
                                None,
                            )
                            .await
                            .map_err(|err| Error::internal_error().data(err.to_string()))?;
                        let chunk =
                            agent_client_protocol::schema::v1::ContentChunk::new(ContentBlock::Text(
                                agent_client_protocol::schema::v1::TextContent::new(reply),
                            ))
                            .message_id(agent_client_protocol::schema::v1::MessageId::new(
                                message.id.clone(),
                            ));
                        let _ = cx.send_notification(SessionNotification::new(
                            request.session_id.clone(),
                            agent_client_protocol::schema::v1::SessionUpdate::AgentMessageChunk(chunk),
                        ));
                        // The tool surface changed shape, so the command
                        // listing is republished for the session.
                        let commands = available_commands_update(&state_for_prompt, &negotiation_for_prompt).await;
                        let _ = cx.send_notification(SessionNotification::new(
                            request.session_id.clone(),
                            commands,
                        ));
                        state_for_prompt
                            .db
                            .release_lease(&session_id, &owner_for_prompt)
                            .await
                            .ok();
                        return responder.respond(PromptResponse::new(StopReason::EndTurn));
                    }
                }

                // Resolve the actor for this session (compaction's
                // event payload and the model turn both need it).
                let session_row = state_for_prompt
                    .db
                    .get_session(&session_id)
                    .await
                    .ok()
                    .flatten();
                let actor_name = session_row
                    .as_ref()
                    .map(|row| {
                        session_actor(
                            row,
                            state_for_prompt.config().config.defaults.actor.as_deref(),
                        )
                    })
                    .unwrap_or_else(|| "default".into());

                // Compaction-tagged prompts: intercepted — the
                // compaction_requested script event fires instead of a
                // model turn in the main session.
                if compaction::is_compaction_command(&state_for_prompt.definitions(), &first_text) {
                    let size = session_row
                        .as_ref()
                        .and_then(|row| {
                            config_options::resolve_model_for(&state_for_prompt, row, None)
                        })
                        .and_then(|model_ref| context_window(&model_ref))
                        .unwrap_or(0);
                    let (before, after) = compaction::run_compaction(
                        &state_for_prompt,
                        &session_id,
                        &turn.id,
                        &actor_name,
                        size,
                        connection_notices(&cx, &negotiation_for_compaction, "compaction"),
                    )
                    .await;

                    // The in-band announcement: trigger and counts.
                    let note =
                        compaction::announcement(first_text.trim_start_matches('/'), before, after);
                    let message = state_for_prompt
                        .db
                        .append_message(
                            &turn.id,
                            crate::store::Role::Assistant,
                            &serde_json::json!([{ "type": "text", "text": note.clone() }])
                                .to_string(),
                            None,
                            None,
                            None,
                            None,
                        )
                        .await
                        .map_err(|err| Error::internal_error().data(err.to_string()))?;
                    let chunk =
                        agent_client_protocol::schema::v1::ContentChunk::new(ContentBlock::Text(
                            agent_client_protocol::schema::v1::TextContent::new(note.clone()),
                        ))
                        .message_id(
                            agent_client_protocol::schema::v1::MessageId::new(message.id.clone()),
                        );
                    let _ = cx.send_notification(SessionNotification::new(
                        request.session_id.clone(),
                        agent_client_protocol::schema::v1::SessionUpdate::AgentMessageChunk(chunk),
                    ));

                    // The visible usage_update drop.
                    if size > 0 {
                        let _ = cx.send_notification(SessionNotification::new(
                            request.session_id.clone(),
                            SessionUpdate::UsageUpdate(UsageUpdate::new(after, size)),
                        ));
                    }

                    // RFD-shaped compaction updates flow only to
                    // clients advertising the capability.
                    if negotiation_for_compaction.supports(UnstableFeature::SessionCompaction) {
                        let update = agent_client_protocol::schema::v1::CompactionUpdate::new(
                            agent_client_protocol::schema::v1::CompactionId::new(
                                uuid::Uuid::new_v4().to_string(),
                            ),
                            agent_client_protocol::schema::v1::CompactionStatus::Completed,
                        )
                        .summary(vec![
                            agent_client_protocol::schema::v1::ContentBlock::Text(
                                agent_client_protocol::schema::v1::TextContent::new(note.clone()),
                            ),
                        ]);
                        let _ = cx.send_notification(SessionNotification::new(
                            request.session_id.clone(),
                            SessionUpdate::CompactionUpdate(update),
                        ));
                    }

                    // updatedAt each turn — intercepted turns included.
                    let updated = agent_client_protocol::schema::v1::SessionInfoUpdate::new()
                        .updated_at(
                            state_for_prompt
                                .db
                                .get_session(&session_id)
                                .await
                                .ok()
                                .flatten()
                                .map(|session| timestamp(session.updated_at))
                                .unwrap_or_default(),
                        );
                    let _ = cx.send_notification(SessionNotification::new(
                        request.session_id.clone(),
                        SessionUpdate::SessionInfoUpdate(updated),
                    ));

                    state_for_prompt
                        .db
                        .release_lease(&session_id, &owner_for_prompt)
                        .await
                        .ok();
                    return responder.respond(PromptResponse::new(StopReason::EndTurn));
                }

                // The model turn is long and awaits client messages
                // (request_permission); the connection processes messages
                // on one task, so the turn runs spawned, keeping the loop
                // free to route the client's replies.
                let cx = cx.clone();
                let state_for_prompt = state_for_prompt.clone();
                let owner_for_prompt = owner_for_prompt.clone();
                let negotiation_for_turn = negotiation_for_prompt.clone();
                let spawn_cx = cx.clone();
                let runtime = crate::agent::runtime::TurnRuntime {
                    pool: negotiation_for_turn.mcp_pool(),
                    sink: std::sync::Arc::new(super::ConnectionTurnSink::new(cx.clone())),
                    requester: std::sync::Arc::new(super::permission::AcpPermissionRequester {
                        cx: cx.clone(),
                        session_id: request.session_id.clone(),
                    }),
                    executor: std::sync::Arc::new(
                        client_tools::ConnectionClientTools { cx: cx.clone() },
                    ),
                    tracker: negotiation_for_turn.clone(),
                    compaction_notices: connection_notices(
                        &cx,
                        &negotiation_for_turn,
                        "compaction",
                    ),
                    titling_notices: connection_notices(&cx, &negotiation_for_turn, "titling"),
                    client_tools: client_tools::catalog(
                        negotiation_for_turn.client_caps(),
                        negotiation_for_turn.editor_toggles(),
                    ),
                };
                spawn_cx.spawn(async move {
                    match run_prompt_turn(
                        state_for_prompt,
                        runtime,
                        session_id,
                        turn,
                        actor_name,
                        session_row,
                        first_text,
                        blocks_json,
                        owner_for_prompt,
                    )
                    .await
                    {
                        Ok(stop) => responder.respond(PromptResponse::new(match stop {
                            crate::agent::TurnStop::EndTurn
                            | crate::agent::TurnStop::ContextLength => StopReason::EndTurn,
                            crate::agent::TurnStop::MaxTurnRequests => StopReason::MaxTurnRequests,
                        })),
                        Err(err) => responder.respond_with_error(Error::internal_error().data(err)),
                    }
                })?;
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: agent_client_protocol::schema::v1::DeleteSessionRequest,
                        responder,
                        _cx| {
                state_for_delete
                    .db
                    .delete_session(&store_session_id(&request.session_id))
                    .await
                    .map_err(|err| Error::internal_error().data(err.to_string()))?;
                state_for_delete
                    .grants
                    .lock()
                    .await
                    .clear_session(&request.session_id.to_string());
                responder.respond(DeleteSessionResponse::new())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: LoadSessionRequest, responder, cx| {
                // A forwarded server's tools must be addressable before
                // the re-advertisement below lists them.
                connect_forwarded_servers(&negotiation_for_load, &request.mcp_servers).await;
                // Replay: full user-visible history in DAG order. Usage
                // snapshot emission lands with the model metadata.
                let assembled = state_for_load
                    .db
                    .assemble_context(&store_session_id(&request.session_id))
                    .await
                    .map_err(|err| Error::internal_error().data(err.to_string()))?;
                for replay in replay_updates(&request.session_id, &assembled) {
                    cx.send_notification(replay)
                        .map_err(|err| Error::internal_error().data(err.to_string()))?;
                }
                // Re-advertise on load: older Zed builds rebuild the
                // thread UI from retained state and treat commands as
                // transient (zed-industries/zed#53209), and the
                // pre-response window drops updates outright. Selectors
                // ride the response so a reopened session can switch
                // models before the next prompt.
                let session = state_for_load
                    .db
                    .get_session(&store_session_id(&request.session_id))
                    .await
                    .map_err(|err| Error::internal_error().data(err.to_string()))?
                    .ok_or_else(|| Error::internal_error().data("no such session".to_owned()))?;
                let selectors = config_options::build(&state_for_load, &session).await;
                let commands = available_commands_update(&state_for_load, &negotiation_for_load).await;
                // The usage snapshot: exactly one after replay, so a
                // reopened client shows the session's context state.
                let size = config_options::resolve_model_for(&state_for_load, &session, None)
                    .and_then(|model_ref| context_window(&model_ref))
                    .unwrap_or(0);
                if size > 0 {
                    let used_bytes: u64 = assembled
                        .iter()
                        .map(|item| item.message.content.len() as u64)
                        .sum();
                    let used = crate::scripts::host::estimate_tokens(used_bytes);
                    cx.send_notification(SessionNotification::new(
                        request.session_id.clone(),
                        SessionUpdate::UsageUpdate(
                            agent_client_protocol::schema::v1::UsageUpdate::new(used, size),
                        ),
                    ))
                    .map_err(|err| Error::internal_error().data(err.to_string()))?;
                }
                responder.respond(
                    LoadSessionResponse::new().config_options(selectors.options),
                )?;
                for notice in &selectors.notices {
                    if negotiation_for_load.supports(UnstableFeature::SessionNotices) {
                        let _ = cx.send_notification(SessionNotification::new(
                            request.session_id.clone(),
                            SessionUpdate::Notice(notice.clone()),
                        ));
                    }
                }
                cx.send_notification(SessionNotification::new(
                    request.session_id.clone(),
                    commands,
                ))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: ResumeSessionRequest, responder, cx| {
                // Resume reattaches WITHOUT replay; config
                // selectors ride the response like session/new.
                connect_forwarded_servers(&negotiation_for_resume, &request.mcp_servers).await;
                let session = state_for_resume
                    .db
                    .get_session(&store_session_id(&request.session_id))
                    .await
                    .map_err(|err| Error::internal_error().data(err.to_string()))?
                    .ok_or_else(|| Error::internal_error().data("no such session".to_owned()))?;
                let selectors = config_options::build(&state_for_resume, &session).await;
                responder.respond(
                    ResumeSessionResponse::new().config_options(selectors.options),
                )?;
                for notice in &selectors.notices {
                    if negotiation_for_resume.supports(UnstableFeature::SessionNotices) {
                        let _ = cx.send_notification(SessionNotification::new(
                            request.session_id.clone(),
                            SessionUpdate::Notice(notice.clone()),
                        ));
                    }
                }
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: agent_client_protocol::schema::v1::SetSessionConfigOptionRequest,
                        responder,
                        cx| {
                use agent_client_protocol::schema::v1::SessionConfigOptionValue;
                let session_id = store_session_id(&request.session_id);
                let session = state_for_config
                    .db
                    .get_session(&session_id)
                    .await
                    .map_err(|err| Error::internal_error().data(err.to_string()))?
                    .ok_or_else(|| {
                        Error::internal_error().data("no such session".to_owned())
                    })?;

                let applied = match &request.value {
                    SessionConfigOptionValue::ValueId { value } => {
                        let value = value.to_string();
                        match request.config_id.0.as_ref() {
                            config_options::MODEL_SELECTOR_ID => {
                                let notices = connection_notices(
                                    &cx,
                                    &negotiation_for_config,
                                    "model switch",
                                );
                                let access = script_loop::access(
                                    &state_for_config,
                                    0,
                                    std::sync::Arc::clone(&notices),
                                );
                                let mut updates: Vec<agent_client_protocol::schema::v1::Notice> =
                                    Vec::new();
                                let mut notify =
                                    |update: config_options::SessionUpdateForOptions| {
                                        match update {
                                            config_options::SessionUpdateForOptions::Notice(
                                                notice,
                                            ) => {
                                                updates.push(notice);
                                            }
                                        }
                                    };
                                let result = config_options::apply_model_switch(
                                    &state_for_config,
                                    access.as_ref(),
                                    &session_id,
                                    &value,
                                    &mut notify,
                                    notices,
                                )
                                .await;
                                for notice in updates {
                                    if negotiation_for_config.supports(UnstableFeature::SessionNotices) {
                                        let _ = cx.send_notification(SessionNotification::new(
                                            request.session_id.clone(),
                                            SessionUpdate::Notice(notice),
                                        ));
                                    }
                                }
                                result
                            }
                            config_options::ACTOR_SELECTOR_ID => {
                                config_options::apply_actor_switch(
                                    &state_for_config,
                                    &session_id,
                                    &value,
                                )
                                .await
                            }
                            other => Err(format!("unknown config option `{other}`")),
                        }
                    }
                    SessionConfigOptionValue::Boolean { .. } => {
                        Err("boolean options are not configured".to_owned())
                    }
                    other => Err(format!("unsupported config value: {other:?}")),
                };

                // The refreshed selectors; the switch is effective the
                // following turn.
                let refreshed = config_options::build(&state_for_config, &session).await;
                match applied {
                    Ok(()) => {
                        let _ = cx.send_notification(SessionNotification::new(
                            request.session_id.clone(),
                            SessionUpdate::ConfigOptionUpdate(
                                agent_client_protocol::schema::v1::ConfigOptionUpdate::new(
                                    refreshed.options.clone(),
                                ),
                            ),
                        ));
                        responder.respond(
                            agent_client_protocol::schema::v1::SetSessionConfigOptionResponse::new(
                                refreshed.options,
                            ),
                        )
                    }
                    Err(message) => responder.respond_with_error(
                        Error::internal_error().data(serde_json::json!({ "error": message })),
                    ),
                }
            },
            agent_client_protocol::on_receive_request!(),
        );

    // The RFD path for forking: session/fork, served to clients that
    // call it; the fork capability advertisement is itself gated.
    let builder = {
        builder.on_receive_request(
            async move |request: agent_client_protocol::schema::v1::ForkSessionRequest,
                        responder,
                        cx| {
                // No session updates flow here — the client attaches
                // (session/load) and updates follow.
                let source = store_session_id(&request.session_id);
                let access =
                    fork_access(&state_for_fork, connection_notices(&cx, &negotiation_for_fork, "fork"));
                let fork = fork::create_fork(&state_for_fork.db, &source)
                    .await
                    .map_err(|err| Error::internal_error().data(err.to_string()))?;
                fork::fire_session_forked(
                    &state_for_fork,
                    access,
                    &fork.id,
                    &source,
                    fork.fork_point_turn_id.as_ref(),
                );
                // Selectors on the fork response: the client attaching
                // can switch before the first prompt.
                let selectors = config_options::build(&state_for_fork, &fork).await;
                for notice in &selectors.notices {
                    if negotiation_for_fork.supports(UnstableFeature::SessionNotices) {
                        let _ = cx.send_notification(SessionNotification::new(
                            wire_session_id(&fork.id),
                            SessionUpdate::Notice(notice.clone()),
                        ));
                    }
                }
                responder.respond(
                    agent_client_protocol::schema::v1::ForkSessionResponse::new(wire_session_id(
                        &fork.id,
                    ))
                    .config_options(selectors.options),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
    };

    builder.connect_to(transport).await
}
