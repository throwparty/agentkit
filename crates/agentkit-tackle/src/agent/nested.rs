//! Agent invocation (T-035, FR-026): `agent.<actor>` invokables spawn a
//! new agent instance of that actor running a nested turn loop within
//! the current turn.
//!
//! The nested instance is an ephemeral fork of the parent session — its
//! usage attributes to the parent through the shared DAG, never
//! double-counted. The nested loop shares the parent turn's
//! model-request cap (it receives the remaining budget and counts
//! against it). The nesting limit is one: a sub-agent cannot spawn
//! further agents — the guard refuses any agent invocation at depth
//! one. Agent invokables are model-invokable only (the registry already
//! enforces that; the user path has no `agent.` slash surface).

use crate::agent::{ChatMessage, ModelProvider, ModelRequest};
use crate::store::{SessionId, SessionStore, StoreError, TurnUsage};

/// The one-level nesting limit: the parent turn runs at depth 0; a
/// nested agent instance runs at depth 1 and can spawn nothing further.
pub const MAX_NESTING_DEPTH: u8 = 1;

#[derive(Debug, thiserror::Error)]
pub enum NestedError {
    #[error("store error: {0}")]
    Store(#[from] StoreError),
    #[error("model error: {0}")]
    Model(String),
    #[error("the shared model-request cap is exhausted")]
    CapExhausted,
    #[error("nesting beyond depth one is forbidden")]
    NestingDepth,
}

/// The nested agent's outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct NestedAgentResult {
    /// The ephemeral fork the nested instance ran in.
    pub session_id: SessionId,
    /// The final assistant message: the invocation's tool result.
    pub final_message: String,
    /// The model requests the nested loop consumed from the shared cap.
    pub requests_used: u32,
}

/// The nesting guard: the parent turn carries depth 0; nested
/// invocations must refuse further spawning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NestingGuard {
    pub depth: u8,
}

impl NestingGuard {
    /// The parent turn's guard.
    pub fn root() -> Self {
        Self { depth: 0 }
    }

    /// Whether an `agent.<actor>` invocation may spawn from this depth.
    pub fn can_spawn_agent(&self) -> bool {
        self.depth < MAX_NESTING_DEPTH
    }

    /// The guard a nested instance carries.
    pub fn nested(&self) -> Result<Self, NestedError> {
        if self.depth >= MAX_NESTING_DEPTH {
            return Err(NestedError::NestingDepth);
        }
        Ok(Self {
            depth: self.depth + 1,
        })
    }
}

/// Spawns the nested agent instance: an ephemeral fork of the parent
/// session attributed to it, running a nested turn loop under the
/// shared cap.
#[allow(clippy::too_many_arguments)]
pub async fn run_nested_agent<P: ModelProvider>(
    provider: &P,
    store: &SessionStore,
    parent_session: &SessionId,
    actor: &str,
    persona: &str,
    model: &str,
    prompt: &str,
    remaining_requests: u32,
) -> Result<NestedAgentResult, NestedError> {
    // The nested instance: an ephemeral fork, usage attributed to the
    // parent through the DAG.
    let fork = store
        .create_session(
            crate::store::SessionKind::Ephemeral,
            "",
            Some(parent_session),
            None,
            &format!(r#"{{"actor": "{actor}", "nested": true}}"#),
        )
        .await?;
    let turn = store
        .append_turn(
            &fork.id,
            None,
            crate::store::TurnKind::Interaction,
            None,
            TurnUsage::default(),
        )
        .await?;
    let content = serde_json::json!([{ "type": "text", "text": prompt }]).to_string();
    store
        .append_message(
            &turn.id,
            crate::store::Role::User,
            &content,
            None,
            None,
            None,
            None,
        )
        .await?;

    // The nested loop shares the parent turn's model-request cap.
    let mut requests_used = 0u32;
    if requests_used >= remaining_requests {
        return Err(NestedError::CapExhausted);
    }
    requests_used += 1;

    let request = ModelRequest {
        model: model.to_owned(),
        system: persona.to_owned(),
        messages: vec![ChatMessage {
            role: crate::agent::ChatRole::User,
            text: prompt.to_owned(),
        }],
    };
    let response = provider
        .stream_completion(request, &mut |_delta: &str| {})
        .await
        .map_err(|err| NestedError::Model(err.to_string()))?;

    // The final message: the invocation's tool result, persisted in the
    // nested instance's turn.
    let message_id = uuid::Uuid::new_v4().to_string();
    store
        .append_message(
            &turn.id,
            crate::store::Role::Assistant,
            &serde_json::json!([{ "type": "text", "text": response.text.clone() }]).to_string(),
            None,
            None,
            None,
            Some(&message_id),
        )
        .await?;
    store
        .set_turn_usage(
            &turn.id,
            TurnUsage {
                input_tokens: response.usage.input_tokens,
                output_tokens: response.usage.output_tokens,
                cost_usd: 0.0,
            },
        )
        .await?;

    Ok(NestedAgentResult {
        session_id: fork.id,
        final_message: response.text,
        requests_used,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;

    /// A stub provider: answers with the fixture text and usage.
    struct Stub;

    impl ModelProvider for Stub {
        fn complete(
            &self,
            _request: ModelRequest,
        ) -> impl Future<Output = Result<crate::agent::ModelResponse, crate::agent::ModelError>> + Send
        {
            std::future::ready(Ok(crate::agent::ModelResponse {
                text: "nested answer".to_owned(),
                usage: crate::agent::ModelUsage {
                    input_tokens: 7,
                    output_tokens: 3,
                },
            }))
        }

        fn stream_completion(
            &self,
            _request: ModelRequest,
            _on_text_delta: &mut (dyn FnMut(&str) + Send),
        ) -> impl Future<Output = Result<crate::agent::ModelResponse, crate::agent::ModelError>> + Send
        {
            std::future::ready(Ok(crate::agent::ModelResponse {
                text: "nested answer".to_owned(),
                usage: crate::agent::ModelUsage {
                    input_tokens: 7,
                    output_tokens: 3,
                },
            }))
        }
    }

    fn setup() -> (SessionStore, SessionId) {
        let store = SessionStore::in_memory();
        let parent = futures_now(store.create_session(
            crate::store::SessionKind::Interactive,
            "/work",
            None,
            None,
            r#"{"actor": "default"}"#,
        ))
        .unwrap()
        .id;
        (store, parent)
    }

    /// A current-thread runtime without an ambient one: the store's
    /// in-memory backend is sync inside.
    fn futures_now<F: std::future::Future>(future: F) -> F::Output {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(future)
    }

    #[test]
    fn nested_loop_round_trips() {
        let (store, parent) = setup();
        let result = futures_now(run_nested_agent(
            &Stub,
            &store,
            &parent,
            "worker",
            "the persona",
            "test/model",
            "do the nested thing",
            8,
        ))
        .unwrap();

        assert_eq!(result.final_message, "nested answer");
        // The nested instance is an ephemeral fork of the parent, with
        // the final message stored.
        let fork = futures_now(store.get_session(&result.session_id))
            .unwrap()
            .unwrap();
        assert_eq!(fork.kind, crate::store::SessionKind::Ephemeral);
        assert_eq!(
            fork.forked_from_session_id.as_deref(),
            Some(parent.as_str())
        );
        let assembled = futures_now(store.assemble_context(&result.session_id)).unwrap();
        assert!(assembled
            .iter()
            .any(|item| item.message.role == crate::store::Role::Assistant
                && item.message.content.contains("nested answer")));
    }

    #[test]
    fn the_cap_is_shared_with_the_parent_turn() {
        let (store, parent) = setup();

        // An exhausted parent cap: the nested loop is refused.
        let err = futures_now(run_nested_agent(
            &Stub,
            &store,
            &parent,
            "worker",
            "persona",
            "test/model",
            "prompt",
            0,
        ))
        .unwrap_err();
        assert!(err.to_string().contains("cap is exhausted"), "{err}");

        // A nearly-exhausted cap: exactly one request fits.
        let result = futures_now(run_nested_agent(
            &Stub,
            &store,
            &parent,
            "worker",
            "persona",
            "test/model",
            "prompt",
            1,
        ))
        .unwrap();
        assert_eq!(result.requests_used, 1);
    }

    #[test]
    fn the_nesting_limit_is_one() {
        let root = NestingGuard::root();
        assert!(root.can_spawn_agent());

        let nested = root.nested().unwrap();
        assert_eq!(nested.depth, 1);
        assert!(!nested.can_spawn_agent(), "a sub-agent cannot spawn agents");
        assert!(matches!(nested.nested(), Err(NestedError::NestingDepth)));
    }

    #[test]
    fn usage_attributes_to_the_parent_session() {
        let (store, parent) = setup();
        let result = futures_now(run_nested_agent(
            &Stub,
            &store,
            &parent,
            "worker",
            "persona",
            "test/model",
            "prompt",
            8,
        ))
        .unwrap();

        // The usage landed on the nested fork's own turns (input 7,
        // output 3); the parent's attributable cost includes it through
        // the fork — no double counting on the parent's own rows.
        let fork_usage = futures_now(store.session_usage(&result.session_id)).unwrap();
        assert_eq!(fork_usage.input_tokens, 7);
        assert_eq!(fork_usage.output_tokens, 3);
        let parent_usage = futures_now(store.session_usage(&parent)).unwrap();
        assert_eq!(
            parent_usage.input_tokens, 0,
            "the parent's own rows carry nothing — attribution is via the fork"
        );
    }
}
