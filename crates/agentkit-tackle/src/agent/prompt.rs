//! One prompt turn: resolve the actor, model, tools, and context, run the turn loop, report usage/compaction/titling, and release the lease.

use crate::agent::config_options;
use crate::agent::config_options::effective_context_window;
use crate::agent::runtime::TurnRuntime;
use crate::agent::sink::TurnSink;
use crate::agent::tool_call::{policy_script_sources, run_model_tool_call, ToolCallContext};
use crate::agent::turn::TurnError;
use crate::agent::{compaction, titling, TurnOutcome, TurnStop};
use crate::content::{prompt_parts_of, timestamp};
use crate::invokables::Registry;
use crate::providers::openai_chat_completions::{provider_for, RigProvider};
use crate::providers::{ChatContent, ChatMessage, ModelToolCall, ToolCallOutcome, ToolDefinition};
use crate::state::{LeaseHeartbeat, TackleState};
use crate::store::{Session, SessionId, Turn, TurnId};
use std::sync::Arc;

/// The resolved inputs the turn loop consumes: the invokable registry, the
/// provider, the tool catalogue, the system prompt, the prompt text, the
/// prior context, and the overflow note.
struct LoopInputs {
    registry: Registry,
    provider: RigProvider,
    tools: Vec<ToolDefinition>,
    system: String,
    prompt: Vec<ChatContent>,
    prior: Vec<ChatMessage>,
    context_length_note: String,
}

/// One model turn of a prompt: resolve the actor, model, tools, and
/// context, run the turn loop (streaming to the client, dispatching tool
/// calls through the permission pipeline), then report usage, compaction,
/// titling, and release the lease. Returns the response the caller sends.
#[allow(clippy::too_many_arguments)] // the turn's resolved inputs, all needed
pub(crate) async fn run_prompt_turn(
    state: Arc<TackleState>,
    runtime: TurnRuntime,
    session_id: SessionId,
    turn: Turn,
    actor_name: String,
    session_row: Option<Session>,
    first_text: String,
    blocks_json: String,
    owner: Arc<String>,
) -> Result<TurnStop, String> {
    // The caller acquired the lease, but the turn runs in this task, so
    // the heartbeat must live here for the turn's whole duration.
    let _lease_heartbeat = LeaseHeartbeat::spawn(
        Arc::clone(&state.db),
        session_id.clone(),
        owner.as_ref().clone(),
    );

    let (model_ref, size) = resolve_model_and_window(&state, session_row.as_ref()).await?;
    let (registry, tools, system) = build_registry_and_system(
        &state.definitions(),
        &actor_name,
        &runtime.pool,
        &runtime.client_tools,
    )
    .await;
    let prompt = expand_prompt(&first_text, &blocks_json, &registry)?;

    let assembled = state
        .db
        .assemble_context(&session_id)
        .await
        .map_err(|err| err.to_string())?;
    let prior = compaction::prior_messages(&assembled, &turn.id);

    let provider =
        provider_for(&model_ref, &state.config().config).map_err(|err| err.to_string())?;
    let inputs = LoopInputs {
        registry,
        provider,
        tools,
        system,
        prompt,
        prior,
        context_length_note: context_overflow_note(&state),
    };
    let max_requests = state.config().config.turns.max_model_requests.unwrap_or(8);

    let outcome = run_turn_loop(
        inputs,
        &state,
        &runtime,
        &session_id,
        &turn.id,
        &actor_name,
        &model_ref,
        size,
        max_requests,
    )
    .await;

    if let Ok(result) = &outcome {
        let _usage = crate::telemetry::usage_span(
            &session_id,
            result.usage.input_tokens,
            result.usage.output_tokens,
        );
    }

    let sink = Arc::clone(&runtime.sink);
    report_session_info(&state, sink.as_ref(), &session_id).await;

    fire_detached_hooks(
        &state,
        &session_id,
        &turn.id,
        &actor_name,
        size,
        runtime.compaction_notices,
        runtime.titling_notices,
    );

    state.db.release_lease(&session_id, &owner).await.ok();

    finish_turn(&state, sink.as_ref(), &session_id, size, outcome).await
}

/// Resolves the model through the shared chain — metadata, actor
/// frontmatter, `[defaults].model`, then the first listed selector option —
/// and the model's context window (0 when unknown).
pub(crate) async fn resolve_model_and_window(
    state: &TackleState,
    session_row: Option<&Session>,
) -> Result<(String, u64), String> {
    // Nothing resolved is a clear configuration error, never a
    // `default/model` sentinel.
    let session = session_row.ok_or_else(|| "no such session".to_owned())?;
    let model_ref = match config_options::resolve_model_for(state, session, None) {
        Some(model_ref) => model_ref,
        None => {
            let listed = config_options::build(state, session).await;
            listed.first_model.ok_or_else(|| {
                "no model configured; set [defaults].model or select a model".to_owned()
            })?
        }
    };
    let size = effective_context_window(state, &model_ref).unwrap_or(0);
    Ok((model_ref, size))
}

/// Builds the invokable registry, the model's tool catalogue, and the
/// system prompt (the persona body with the invokable listing appended).
pub(crate) async fn build_registry_and_system(
    definitions: &crate::definitions::Definitions,
    actor_name: &str,
    pool: &Arc<tokio::sync::Mutex<crate::mcp::McpPool>>,
    client_tools: &[crate::invokables::client_tools::ClientTool],
) -> (Registry, Vec<ToolDefinition>, String) {
    let mcp_tokens = {
        let pool = pool.lock().await;
        pool.list_tools().await
    };
    let registry = Registry::build_with_client_tools(definitions, &mcp_tokens, client_tools)
        .unwrap_or_default();
    let tools = registry.model_tools();
    let persona = definitions
        .actors
        .get(actor_name)
        .and_then(|actor| definitions.personas.get(&actor.frontmatter.persona));

    let mut system = persona
        .map(|persona| persona.body.clone())
        .unwrap_or_default();
    if !tools.is_empty() {
        system.push_str("\n\n");
        system.push_str(&registry.manifest());
    }
    (registry, tools, system)
}

/// Expands a user-typed `/name` prompt into the model prompt; the stored
/// message stays literal. A failed expansion falls back to the literal
/// prompt. Images survive expansion, appended after the command's text.
fn expand_prompt(
    first_text: &str,
    blocks_json: &str,
    registry: &Registry,
) -> Result<Vec<ChatContent>, String> {
    let parts = prompt_parts_of(blocks_json)?;
    let Some(rest) = first_text.strip_prefix('/') else {
        return Ok(parts);
    };
    let mut words = rest.splitn(2, char::is_whitespace);
    let name = words.next().unwrap_or("");
    let args = words.next().unwrap_or("").trim();
    let Ok(expanded) = registry.expand_user(name, args) else {
        return Ok(parts);
    };
    let mut prompt = vec![ChatContent::text(expanded)];
    prompt.extend(
        parts
            .into_iter()
            .filter(|part| matches!(part, ChatContent::Image { .. })),
    );
    Ok(prompt)
}

/// The context-length failure note: actionable, naming the configured
/// compaction script (or its absence).
fn context_overflow_note(state: &TackleState) -> String {
    compaction::compaction_disabled_note(
        state
            .config()
            .config
            .scripts
            .iter()
            .find(|(_, script)| {
                script.enabled
                    && script
                        .events
                        .iter()
                        .any(|event| event == "compaction_requested")
            })
            .map(|(name, _)| name.as_str()),
    )
}

/// Runs the turn loop for the turn's whole duration: streams text and
/// thoughts, reports retries and usage, and dispatches model tool calls
/// through the permission pipeline.
#[allow(clippy::too_many_arguments)] // the resolved inputs and loop collaborators
async fn run_turn_loop(
    inputs: LoopInputs,
    state: &Arc<TackleState>,
    runtime: &TurnRuntime,
    session_id: &SessionId,
    turn_id: &TurnId,
    actor_name: &str,
    model_ref: &str,
    size: u64,
    max_requests: u32,
) -> Result<TurnOutcome, TurnError> {
    let LoopInputs {
        registry,
        provider,
        tools,
        system,
        prompt,
        prior,
        context_length_note,
    } = inputs;
    let assistant_id = uuid::Uuid::new_v4().to_string();
    // Thought chunks are their own logical message, distinct from the
    // answer: one stable id per turn groups the reasoning stream.
    let thought_id = format!("{assistant_id}-thought");
    // The turn span: parent of the tool-call and usage spans the loop's
    // paths create.
    let turn_span = crate::telemetry::turn_span(session_id, actor_name, model_ref);
    let _turn_guard = turn_span.enter();

    let sink = Arc::clone(&runtime.sink);
    let sink_for_usage = Arc::clone(&sink);
    let sink_for_text = Arc::clone(&sink);
    let sink_for_thoughts = Arc::clone(&sink);
    let sink_for_retry = Arc::clone(&sink);
    let sink_session = session_id.clone();

    crate::agent::turn::run_turn(
        &provider,
        &state.db,
        session_id,
        turn_id,
        &assistant_id,
        model_ref,
        &system,
        prompt,
        prior,
        tools,
        max_requests,
        &context_length_note,
        // usage cadence: a usage_update per model request,
        // skipped when the window is unknown (RFD guidance).
        {
            let sink = sink_for_usage;
            let session = sink_session.clone();
            move |input: u64, _output: u64, cost: Option<f64>| {
                if size == 0 {
                    return;
                }
                sink.usage(&session, input, size, cost);
            }
        },
        {
            let sink = sink_for_text;
            let session = sink_session.clone();
            let message_id = assistant_id.clone();
            move |delta: &str| sink.text_delta(&session, &message_id, delta)
        },
        // Reasoning streams as thought chunks, a separate logical message
        // from the answer (its own messageId); thoughts are advisory and
        // are never stored.
        {
            let sink = sink_for_thoughts;
            let session = sink_session.clone();
            let message_id = thought_id.clone();
            move |delta: &str| sink.thought_delta(&session, &message_id, delta)
        },
        // retry attempts surface as status cards.
        {
            let sink = sink_for_retry;
            let session = sink_session.clone();
            move |attempt: u32, message: &str| sink.retry(&session, attempt, message)
        },
        // Model tool calls: announce, run the permission
        // pipeline, execute via the registry, report the result
        // back into the loop.
        {
            let registry = registry.clone();
            let state = Arc::clone(state);
            let pool = Arc::clone(&runtime.pool);
            let tracker = Arc::clone(&runtime.tracker);
            let sink = Arc::clone(&sink);
            let requester = Arc::clone(&runtime.requester);
            let executor = Arc::clone(&runtime.executor);
            let session = session_id.clone();
            let actor = actor_name.to_owned();
            let sources = policy_script_sources(state.as_ref());
            let client_tools = runtime.client_tools.clone();
            move |call: ModelToolCall| -> std::pin::Pin<
                Box<dyn std::future::Future<Output = ToolCallOutcome> + Send>,
            > {
                let registry = registry.clone();
                let state = state.clone();
                let pool = pool.clone();
                let tracker = tracker.clone();
                let sink = sink.clone();
                let requester = requester.clone();
                let executor = executor.clone();
                let session = session.clone();
                let actor = actor.clone();
                let sources = sources.clone();
                let client_tools = client_tools.clone();
                Box::pin(async move {
                    run_model_tool_call(
                        ToolCallContext {
                            registry: &registry,
                            state: &state,
                            pool: &pool,
                            tracker: tracker.as_ref(),
                            sink: sink.as_ref(),
                            requester: requester.as_ref(),
                            executor: executor.as_ref(),
                            session_id: &session,
                            actor: &actor,
                            policy_sources: &sources,
                            client_tools: &client_tools,
                        },
                        call,
                    )
                    .await
                })
            }
        },
    )
    .await
}

/// Sends the session's last-activity stamp, refreshed each turn.
async fn report_session_info(state: &TackleState, sink: &dyn TurnSink, session_id: &SessionId) {
    let updated = state
        .db
        .get_session(session_id)
        .await
        .ok()
        .flatten()
        .map(|session| timestamp(session.updated_at))
        .unwrap_or_default();
    sink.session_info(session_id, &updated);
}

/// Fires the detached post-turn work: automatic compaction and session
/// titling. Both run detached so a slow summarisation never blocks the
/// response.
fn fire_detached_hooks(
    state: &Arc<TackleState>,
    session_id: &SessionId,
    turn_id: &TurnId,
    actor_name: &str,
    size: u64,
    compaction_notices: Arc<dyn crate::scripts::host::NoticeSink>,
    titling_notices: Arc<dyn crate::scripts::host::NoticeSink>,
) {
    compaction::fire_post_turn_detached(
        Arc::clone(state),
        session_id.clone(),
        turn_id.clone(),
        actor_name.to_owned(),
        size,
        compaction_notices,
    );
    titling::fire_title_trigger_detached(
        Arc::clone(state),
        session_id.clone(),
        size,
        titling_notices,
    );
}

/// Maps the loop's outcome to the caller's result, refreshing the usage
/// indicator from the local estimate after a context overflow so the
/// oversized context is visible.
async fn finish_turn(
    state: &TackleState,
    sink: &dyn TurnSink,
    session_id: &SessionId,
    size: u64,
    outcome: Result<TurnOutcome, TurnError>,
) -> Result<TurnStop, String> {
    let result = outcome.map_err(|err| err.to_string())?;

    // A context overflow ends the turn with the note streamed; refresh the
    // usage indicator from the local estimate so it reflects the
    // oversized context the failing request carried. The provider reported
    // no usage for that request, so without this the indicator sits at the
    // last successful request's size and the growth is invisible.
    if result.stop == TurnStop::ContextLength && size > 0 {
        let used_bytes: u64 = state
            .db
            .assemble_context(session_id)
            .await
            .map(|assembled| {
                assembled
                    .iter()
                    .map(|item| item.message.content.len() as u64)
                    .sum()
            })
            .unwrap_or(0);
        let used = crate::scripts::host::estimate_tokens(used_bytes);
        sink.usage(session_id, used, size, None);
    }

    Ok(result.stop)
}
