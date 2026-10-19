//! The completion plumbing behind script-issued model turns.
//!
//! A behaviour script can fork a session, `send_prompt` it, and
//! `await_completion` the answer (titling and compaction both do). This
//! module supplies the [`PromptDispatcher`] that runs that prompt through
//! the model loop and the [`CompletionSink`] the awaiting script polls,
//! so the completion a script receives is the model's real answer.

use crate::acp::TackleState;
use crate::acp::config_options;
use crate::providers::ModelProvider;
use crate::scripts::host::{
    Completion, CompletionSink, HostError, NoticeSink, PromptDispatcher, SessionAccess,
    StoreAccess, block_on,
};
use crate::store::SessionId;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// The shared completion registry: the sink the access awaits from and
/// the dispatcher publishes into.
#[derive(Default)]
pub struct SharedCompletions {
    map: Mutex<HashMap<SessionId, Completion>>,
}

impl SharedCompletions {
    pub fn new() -> Self {
        Self {
            map: Mutex::new(HashMap::new()),
        }
    }
}

impl CompletionSink for SharedCompletions {
    fn publish(&self, session: &SessionId, completion: Completion) {
        self.map.lock().unwrap().insert(session.clone(), completion);
    }

    fn take(&self, session: &SessionId) -> Option<Completion> {
        self.map.lock().unwrap().remove(session)
    }
}

/// The dispatcher that runs script-issued prompts through the model loop,
/// publishes the real completion into the shared sink, and surfaces the
/// call to the client through the notice sink.
pub struct ModelLoopDispatcher {
    state: Arc<TackleState>,
    sink: Arc<SharedCompletions>,
    notices: Arc<dyn NoticeSink>,
}

impl PromptDispatcher for ModelLoopDispatcher {
    fn dispatch(&self, session: &SessionId, text: &str) -> Result<(), HostError> {
        self.notices.script_model_call(session);
        let completion = block_on(run_script_turn(&self.state, session, text))?;
        self.sink.publish(session, completion);
        Ok(())
    }
}

/// Builds a [`SessionAccess`] whose script-issued prompts run real model
/// turns, whose completions the script can await, and whose model calls
/// are surfaced through `notices`.
pub fn access(
    state: &Arc<TackleState>,
    window_size: u64,
    notices: Arc<dyn NoticeSink>,
) -> Arc<dyn SessionAccess> {
    let sink = Arc::new(SharedCompletions::new());
    let dispatcher = Arc::new(ModelLoopDispatcher {
        state: Arc::clone(state),
        sink: Arc::clone(&sink),
        notices,
    });
    Arc::new(StoreAccess::new(
        Arc::clone(&state.db),
        dispatcher,
        sink,
        window_size,
    ))
}

/// Runs one script-issued prompt to completion: resolve the session's
/// actor, persona, and model, send the assembled conversation plus the
/// prompt as a single model request, and persist the answer. The
/// summariser actor is tool-less, so no tool loop is needed.
async fn run_script_turn(
    state: &Arc<TackleState>,
    session: &SessionId,
    prompt: &str,
) -> Result<Completion, HostError> {
    let session_row = state
        .db
        .get_session(session)
        .await?
        .ok_or(HostError::Invalid("no such session"))?;
    let actor_name = crate::store::session_actor_value_free(&session_row)
        .unwrap_or_else(|| "default".to_owned());
    let definitions = state.definitions();
    let persona = definitions
        .actors
        .get(&actor_name)
        .and_then(|actor| definitions.personas.get(&actor.frontmatter.persona))
        .map(|persona| persona.body.clone())
        .unwrap_or_default();
    let model_ref = match config_options::resolve_model_for(state, &session_row, None) {
        Some(model) => model,
        None => config_options::build(state, &session_row)
            .await
            .first_model
            .ok_or(HostError::Invalid(
                "no model configured for the script turn",
            ))?,
    };
    let provider = crate::providers::openai_chat_completions::provider_for(&model_ref, &state.config().config)
        .map_err(|err| HostError::Model(err.to_string()))?;

    // The prompt's own turn carries the instruction; the rest of the
    // chain (the fork's lineage) is the context to summarise or title.
    let turn_id = session_row
        .head_turn_id
        .clone()
        .ok_or(HostError::Invalid("script turn has no head"))?;
    let assembled = state.db.assemble_context(session).await?;
    let mut messages = crate::acp::compaction::prior_messages(&assembled, &turn_id);
    messages.push(crate::providers::ChatMessage {
        role: crate::providers::ChatRole::User,
        text: prompt.to_owned(),
        ..Default::default()
    });

    let response = provider
        .stream_completion(
            crate::providers::ModelRequest {
                model: model_ref,
                system: persona,
                messages,
                tools: Vec::new(),
            },
            &mut |_delta: &str| {},
            &mut |_delta: &str| {},
        )
        .await
        .map_err(|err| HostError::Model(err.to_string()))?;

    let content = serde_json::json!([{ "type": "text", "text": response.text }]).to_string();
    state
        .db
        .append_message(
            &turn_id,
            crate::store::Role::Assistant,
            &content,
            None,
            None,
            None,
            None,
        )
        .await?;
    state
        .db
        .set_turn_usage(
            &turn_id,
            crate::store::TurnUsage {
                input_tokens: response.usage.input_tokens,
                output_tokens: response.usage.output_tokens,
                cost_usd: 0.0,
            },
        )
        .await?;

    Ok(Completion {
        stop_reason: "end_turn".to_owned(),
        final_message: response.text,
        input_tokens: response.usage.input_tokens,
        output_tokens: response.usage.output_tokens,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Auth, Config, EndpointConfig, Loaded, WireFormat};
    use crate::loader::Definitions;
    use crate::store::{SessionKind, SessionStore, TurnKind, TurnUsage};

    fn sse_body(text: &str) -> String {
        [
            "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"}}]}".to_owned(),
            String::new(),
            format!("data: {{\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"m\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{text}\"}}}}]}}"),
            String::new(),
            "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":3,\"total_tokens\":10}}".to_owned(),
            String::new(),
            "data: [DONE]".to_owned(),
            String::new(),
        ]
        .join("\n")
    }

    fn state_with_endpoint(base_url: &str) -> Arc<TackleState> {
        let mut config = Config::default();
        config.endpoints.insert(
            "test".to_owned(),
            EndpointConfig {
                base_url: base_url.to_owned(),
                wire_format: WireFormat::OpenaiChatCompletions,
                auth: Auth::None,
                models: Vec::new(),
            },
        );
        config.defaults.model = Some("test/model".to_owned());
        Arc::new(TackleState::new(
            Arc::new(SessionStore::in_memory()),
            Loaded {
                config,
                user_layer: None,
                project_layer: None,
            },
            Definitions::default().with_builtins(),
        ))
    }

    #[tokio::test]
    async fn a_script_turn_returns_the_model_answer() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_raw(
                sse_body("A Six Word Title").into_bytes(),
                "text/event-stream",
            ))
            .mount(&server)
            .await;
        let state = state_with_endpoint(&server.uri());

        // An ephemeral summariser fork with its prompt turn, as the host
        // API leaves it after send_prompt.
        let fork = state
            .db
            .create_session(
                SessionKind::Ephemeral,
                "/work",
                None,
                None,
                r#"{"actor": "summariser"}"#,
            )
            .await
            .unwrap();
        let turn = state
            .db
            .append_turn(
                &fork.id,
                None,
                TurnKind::Interaction,
                None,
                TurnUsage::default(),
            )
            .await
            .unwrap();
        state
            .db
            .append_message(
                &turn.id,
                crate::store::Role::User,
                &serde_json::json!([{ "type": "text", "text": "Write a title." }]).to_string(),
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        let completion = run_script_turn(&state, &fork.id, "Write a title.")
            .await
            .unwrap();
        assert_eq!(completion.stop_reason, "end_turn");
        assert_eq!(completion.final_message, "A Six Word Title");
        assert_eq!(completion.input_tokens, 7);
        assert_eq!(completion.output_tokens, 3);

        // The answer is persisted in the prompt turn.
        let usage = state.db.session_usage(&fork.id).await.unwrap();
        assert_eq!(usage.input_tokens, 7);
        assert_eq!(usage.output_tokens, 3);
    }

    /// A notice sink that records every script-driven model call.
    struct Recording(Mutex<Vec<SessionId>>);

    impl NoticeSink for Recording {
        fn script_model_call(&self, session: &SessionId) {
            self.0.lock().unwrap().push(session.clone());
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn script_model_calls_are_surfaced_to_the_notice_sink() {
        // No reachable endpoint: the model call fails, but the notice
        // fires before it.
        let state = state_with_endpoint("http://127.0.0.1:1");
        let session = state
            .db
            .create_session(
                SessionKind::Interactive,
                "/work",
                None,
                None,
                r#"{"actor": "default"}"#,
            )
            .await
            .unwrap();

        let recording = Arc::new(Recording(Mutex::new(Vec::new())));
        let access = access(&state, 0, recording.clone());
        let _ = access.send_prompt(&session.id, "summarise this");

        assert_eq!(
            recording.0.lock().unwrap().as_slice(),
            std::slice::from_ref(&session.id),
            "a script-issued prompt surfaces one model-call notice"
        );
    }
}
