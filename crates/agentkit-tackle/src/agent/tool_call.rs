//! Model tool-call execution: announcing a call, running the permission pipeline, executing it, and reporting the result.

use crate::agent::config_options;
use crate::state::TackleState;
use std::sync::Arc;

/// The registered `pre_tool_use` policy script sources: every
/// enabled registration declaring the event, resolved from its layer
/// (built-in, user, or a trusted project layer).
pub(crate) fn policy_script_sources(state: &TackleState) -> Vec<String> {
    let config = state.config();
    let sources = state.script_sources();
    config
        .config
        .scripts
        .iter()
        .filter(|(_, script)| {
            script.enabled && script.events.iter().any(|event| event == "pre_tool_use")
        })
        .filter_map(|(name, _)| sources.get(name).cloned())
        .collect()
}

/// The per-call context for a model tool call: the registry, shared
/// state, live connection, session, actor, policy sources, the
/// client-capability tools currently available, and the connection whose
/// terminals the call may create or finish.
pub(crate) struct ToolCallContext<'a> {
    pub(crate) registry: &'a crate::invokables::Registry,
    pub(crate) state: &'a Arc<TackleState>,
    pub(crate) pool: &'a Arc<tokio::sync::Mutex<crate::mcp::McpPool>>,
    pub(crate) tracker: &'a dyn crate::invokables::client_tools::TerminalTracker,
    pub(crate) sink: &'a dyn crate::agent::sink::TurnSink,
    pub(crate) requester: &'a dyn crate::permissions::PermissionRequester,
    pub(crate) executor: &'a dyn crate::invokables::client_tools::ClientToolExecutor,
    pub(crate) session_id: &'a crate::store::SessionId,
    pub(crate) actor: &'a str,
    pub(crate) policy_sources: &'a [String],
    pub(crate) client_tools: &'a [crate::invokables::client_tools::ClientTool],
}

/// Runs one model tool call: announce it, run the permission pipeline,
/// execute on allow, report the terminal status, and return the result
/// for the next model request.
pub(crate) async fn run_model_tool_call(
    ctx: ToolCallContext<'_>,
    call: crate::providers::ModelToolCall,
) -> crate::providers::ToolCallOutcome {
    let ToolCallContext {
        registry,
        state,
        pool,
        tracker,
        sink,
        requester,
        executor,
        session_id,
        actor,
        policy_sources,
        client_tools,
    } = ctx;
    let arguments: serde_json::Value =
        serde_json::from_str(&call.arguments).unwrap_or(serde_json::Value::Null);

    sink.tool_call(
        session_id,
        crate::agent::sink::ToolCall {
            id: call.id.clone(),
            title: crate::invokables::titles::tool_call_title(&call.name, &arguments),
            name: call.name.clone(),
            kind: crate::invokables::titles::tool_kind(&call.name),
            status: crate::agent::sink::ToolCallStatus::InProgress,
            arguments: arguments.clone(),
        },
    );

    let policy = crate::scripts::PolicyScripts::load(policy_sources);
    let (content, is_error) = match crate::invokables::kind_of(&call.name) {
        None => (format!("unknown invokable `{}`", call.name), true),
        Some(kind) => {
            let verdict = {
                let mut grants = state.grants.lock().await;
                let assessment = crate::permissions::Assessment {
                    session_id: session_id.to_string(),
                    actor: actor.to_owned(),
                    invokable: call.name.clone(),
                    kind: kind.as_str(),
                    arguments: arguments.clone(),
                    interactive: true,
                    tool_call_id: Some(call.id.clone()),
                };
                let mut pipeline =
                    crate::permissions::PermissionPipeline::new(&mut grants, &policy, requester);
                pipeline.assess(&assessment).await
            };
            match verdict {
                crate::permissions::Verdict::Allow => match kind {
                    crate::invokables::Kind::Client => {
                        if !client_tools.iter().any(|tool| tool.name == call.name) {
                            (
                                format!(
                                    "client tool `{}` is not available on this connection",
                                    call.name
                                ),
                                true,
                            )
                        } else {
                            match executor.execute(session_id, &call.name, &arguments).await {
                                Ok(executed) => {
                                    tracker.track(session_id, executed.created, executed.finished);
                                    (executed.content, false)
                                }
                                Err(err) => (err, true),
                            }
                        }
                    }
                    crate::invokables::Kind::Agent => {
                        // Spawn a nested agent instance of the target
                        // actor and return its final message as the tool result.
                        match run_agent_invocation(state, session_id, &call.name, &arguments).await
                        {
                            Ok(text) => (text, false),
                            Err(err) => (err, true),
                        }
                    }
                    crate::invokables::Kind::Mcp | crate::invokables::Kind::Prompt => {
                        let pool = pool.lock().await;
                        match registry.execute_model(&pool, &call.name, &arguments).await {
                            Ok(text) => (text, false),
                            Err(err) => (err, true),
                        }
                    }
                },
                crate::permissions::Verdict::Deny { reason } => {
                    (format!("Permission denied: {reason}"), true)
                }
            }
        }
    };

    sink.tool_call_update(
        session_id,
        crate::agent::sink::ToolCallUpdate {
            id: call.id.clone(),
            status: if is_error {
                crate::agent::sink::ToolCallStatus::Failed
            } else {
                crate::agent::sink::ToolCallStatus::Completed
            },
            content: content.clone(),
        },
    );

    crate::providers::ToolCallOutcome { content, is_error }
}

/// Runs an `agent.<actor>` invocation: an ephemeral fork of the
/// parent session runs one nested agent instance of the target actor and
/// its final message becomes the tool result.
async fn run_agent_invocation(
    state: &Arc<TackleState>,
    parent_session: &crate::store::SessionId,
    invokable: &str,
    arguments: &serde_json::Value,
) -> Result<String, String> {
    let target = invokable.strip_prefix("agent.").unwrap_or(invokable);
    let parent = parent_session.clone();
    let definitions = state.definitions();
    let actor = definitions
        .actors
        .get(target)
        .ok_or_else(|| format!("unknown actor `{target}`"))?;
    let persona = definitions
        .personas
        .get(&actor.frontmatter.persona)
        .map(|persona| persona.body.clone())
        .unwrap_or_default();
    let prompt = arguments
        .get("prompt")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned();
    if prompt.is_empty() {
        return Err("agent invocations require a `prompt` argument".to_owned());
    }
    let session = state
        .db
        .get_session(&parent)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| "no such session".to_owned())?;
    let model_ref = actor
        .frontmatter
        .model
        .clone()
        .or_else(|| config_options::resolve_model_for(state, &session, None))
        .ok_or_else(|| "no model configured for the nested agent".to_owned())?;
    let provider =
        crate::providers::openai_chat_completions::provider_for(&model_ref, &state.config().config, None)
            .map_err(|err| err.to_string())?;
    let result = crate::agent::nested::run_nested_agent(
        &provider,
        &state.db,
        &parent,
        target,
        &persona,
        &model_ref,
        &prompt,
        state.config().config.turns.max_model_requests.unwrap_or(8),
    )
    .await
    .map_err(|err| err.to_string())?;
    Ok(result.final_message)
}
