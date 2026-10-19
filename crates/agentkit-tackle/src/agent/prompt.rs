//! One prompt turn: resolve the actor, model, tools, and context, run the turn loop, report usage/compaction/titling, and release the lease.

use crate::agent::config_options;
use crate::agent::config_options::context_window;
use crate::agent::tool_call::{ToolCallContext, policy_script_sources, run_model_tool_call};
use crate::agent::{compaction, titling};
use crate::content::{prompt_text_of, timestamp};
use crate::state::{LeaseHeartbeat, TackleState};
use std::sync::Arc;

/// One model turn of a prompt: resolve the actor, model, tools, and
/// context, run the turn loop (streaming to the client, dispatching tool
/// calls through the permission pipeline), then report usage, compaction,
/// titling, and release the lease. Returns the response the caller sends.
#[allow(clippy::too_many_arguments)] // the turn's resolved inputs, all needed
pub(crate) async fn run_prompt_turn(
    state: Arc<TackleState>,
    runtime: crate::agent::runtime::TurnRuntime,
    session_id: crate::store::SessionId,
    turn: crate::store::Turn,
    actor_name: String,
    session_row: Option<crate::store::Session>,
    first_text: String,
    blocks_json: String,
    owner: Arc<String>,
) -> Result<crate::agent::TurnStop, String> {
    let crate::agent::runtime::TurnRuntime {
        pool,
        sink,
        requester,
        executor,
        tracker,
        compaction_notices,
        titling_notices,
        client_tools,
    } = runtime;
    let state_for_prompt = state;
    let owner_for_prompt = owner;
    // The caller acquired the lease, but the turn runs in this task, so
    // the heartbeat must live here for the turn's whole duration.
    let _lease_heartbeat = LeaseHeartbeat::spawn(
        Arc::clone(&state_for_prompt.db),
        session_id.clone(),
        owner_for_prompt.as_ref().clone(),
    );
    let definitions = state_for_prompt.definitions();
    let actor = definitions.actors.get(&actor_name);
    let persona = actor.and_then(|a| definitions.personas.get(&a.frontmatter.persona));
    let mcp_tokens = {
        let pool = pool.lock().await;
        pool.list_tools().await
    };
    let registry = crate::invokables::Registry::build_with_client_tools(
        &definitions,
        &mcp_tokens,
        &client_tools,
    )
    .unwrap_or_default();
    let tools = registry.model_tools();
    // The invokable listing rides the system prompt.
    let mut system = persona.map(|p| p.body.clone()).unwrap_or_default();
    if !tools.is_empty() {
        system.push_str("\n\n");
        system.push_str(&registry.manifest());
    }
    // The model: the shared resolution chain — metadata,
    // actor frontmatter, [defaults].model, then the first
    // listed selector option. Nothing left is a clear
    // configuration error, never a `default/model` sentinel.
    let session_for_model = session_row
        .as_ref()
        .ok_or_else(|| "no such session".to_owned())?;
    let model_ref = {
        if let Some(model_ref) =
            config_options::resolve_model_for(&state_for_prompt, session_for_model, None)
        {
            model_ref
        } else {
            let listed = config_options::build(&state_for_prompt, session_for_model).await;
            listed.first_model.ok_or_else(|| {
                "no model configured; set [defaults].model or select a model".to_owned()
            })?
        }
    };
    let size = context_window(&model_ref).unwrap_or(0);

    // Assemble the prior context (the prompt turn included —
    // its user message is the prompt; the loop re-sends it as
    // the final message).
    let assembled = state_for_prompt
        .db
        .assemble_context(&session_id)
        .await
        .map_err(|err| err.to_string())?;
    let prior: Vec<crate::providers::ChatMessage> =
        compaction::prior_messages(&assembled, &turn.id);
    // A user-typed `/name` prompt expands into the
    // model prompt (the stored message stays literal).
    let prompt_text = match first_text.strip_prefix('/') {
        Some(rest) => {
            let mut parts = rest.splitn(2, char::is_whitespace);
            let name = parts.next().unwrap_or("");
            let args = parts.next().unwrap_or("").trim();
            registry
                .expand_user(name, args)
                .unwrap_or_else(|_| prompt_text_of(&blocks_json))
        }
        None => prompt_text_of(&blocks_json),
    };

    let provider = crate::providers::openai_chat_completions::provider_for(
        &model_ref,
        &state_for_prompt.config().config,
    )
    .map_err(|err| err.to_string())?;
    // The context-length failure note: actionable, naming
    // the configured compaction script (or its absence).
    let context_length_note = compaction::compaction_disabled_note(
        state_for_prompt
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
    );
    let assistant_id = uuid::Uuid::new_v4().to_string();
    // Thought chunks are their own logical message, distinct from the
    // answer: one stable id per turn groups the reasoning stream.
    let thought_id = format!("{assistant_id}-thought");
    // The turn span: parent of the tool-call and usage
    // spans the loop's paths create.
    let turn_span = crate::telemetry::turn_span(&session_id, &actor_name, &model_ref);
    let _turn_guard = turn_span.enter();
    let sink_for_usage = sink.clone();
    let sink_for_text = sink.clone();
    let sink_for_thoughts = sink.clone();
    let sink_for_retry = sink.clone();
    let sink_session = session_id.clone();

    let outcome = crate::agent::turn::run_turn(
        &provider,
        &state_for_prompt.db,
        &session_id,
        &turn.id,
        &assistant_id.to_string(),
        &model_ref,
        &system,
        &prompt_text,
        prior,
        tools,
        state_for_prompt
            .config()
            .config
            .turns
            .max_model_requests
            .unwrap_or(8),
        &context_length_note,
        // usage cadence: a usage_update per model request,
        // skipped when the window is unknown (RFD guidance).
        {
            let sink = sink_for_usage;
            let session = sink_session.clone();
            move |input: u64, _output: u64, _cost: f64| {
                if size == 0 {
                    return;
                }
                sink.usage(&session, input, size);
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
            let state = state_for_prompt.clone();
            let pool = pool.clone();
            let tracker = tracker.clone();
            let sink = sink.clone();
            let requester = requester.clone();
            let executor = executor.clone();
            let session = session_id.clone();
            let actor = actor_name.clone();
            let sources = policy_script_sources(&state_for_prompt);
            let client_tools = client_tools.clone();
            move |call: crate::providers::ModelToolCall| -> std::pin::Pin<
                Box<dyn std::future::Future<Output = crate::providers::ToolCallOutcome> + Send>,
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
    .await;

    if let Ok(result) = &outcome {
        let _usage = crate::telemetry::usage_span(
            &session_id,
            result.usage.input_tokens,
            result.usage.output_tokens,
        );
    }

    // updatedAt each turn: the client-visible last-activity
    // stamp, before the response.
    let updated = state_for_prompt
        .db
        .get_session(&session_id)
        .await
        .ok()
        .flatten()
        .map(|session| timestamp(session.updated_at))
        .unwrap_or_default();
    sink.session_info(&session_id, &updated);

    // Automatic compaction: the post_turn event fires after
    // every model turn, detached so a slow summarisation
    // never blocks the response.
    compaction::fire_post_turn_detached(
        state_for_prompt.clone(),
        session_id.clone(),
        turn.id.clone(),
        actor_name.clone(),
        size,
        compaction_notices,
    );

    // Titling: detached, silent, only for untitled sessions.
    titling::fire_title_trigger_detached(
        state_for_prompt.clone(),
        session_id.clone(),
        size,
        titling_notices,
    );

    state_for_prompt
        .db
        .release_lease(&session_id, &owner_for_prompt)
        .await
        .ok();

    match outcome {
        Ok(result) => {
            // A context overflow ends the turn with the note
            // streamed; refresh the usage indicator from the
            // local estimate so it reflects the oversized
            // context the failing request carried. The provider
            // reported no usage for that request, so without
            // this the indicator sits at the last successful
            // request's size and the growth is invisible.
            if result.stop == crate::agent::TurnStop::ContextLength && size > 0 {
                let used_bytes: u64 = state_for_prompt
                    .db
                    .assemble_context(&session_id)
                    .await
                    .map(|assembled| {
                        assembled
                            .iter()
                            .map(|item| item.message.content.len() as u64)
                            .sum()
                    })
                    .unwrap_or(0);
                let used = crate::scripts::host::estimate_tokens(used_bytes);
                sink.usage(&session_id, used, size);
            }
            Ok(result.stop)
        }
        Err(err) => Err(err.to_string()),
    }
}
