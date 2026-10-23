//! The OpenAI chat-completions wire format: the rig-backed provider and its construction from configuration.

use super::rig::{to_rig_message, tool_calls_of};
use super::{ModelError, ModelProvider, ModelRequest, ModelResponse, ModelUsage};
use crate::config::Config;
use crate::credentials::{credential_helper_name, resolve_credential};
use rig_core::completion::{CompletionRequest, ToolDefinition};
use rig_core::driver::Model;
use rig_core::message::{AssistantContent, Message, ToolName};
use rig_core::providers::openai;
use rig_core::streaming::{Item, StreamEvent};
use secrecy::ExposeSecret as _;

/// The rig-backed provider: one endpoint, one wire format.
pub struct RigProvider {
    /// The chat-completions model: the chat client (not the
    /// responses-API default) provides this model type.
    model: Model<openai::wire::Chat>,
}

impl RigProvider {
    /// Builds the provider for a configured endpoint (openai-chat-completions
    /// wire format).
    pub fn openai_completions(
        base_url: &str,
        credential: &str,
        model: &str,
    ) -> Result<Self, ModelError> {
        let client = openai::OpenAIConfig::new(credential.to_owned())
            .with_base_url(base_url)
            .client();
        Ok(Self {
            model: client.chat(model),
        })
    }

    fn completion(&self, request: ModelRequest) -> Result<CompletionRequest, ModelError> {
        // The last message is the prompt; everything before is history;
        // the system prompt rides the preamble (a leading System message).
        let mut messages = request.messages.into_iter();
        let prompt = messages
            .next_back()
            .map(|message| to_rig_message(&message))
            .transpose()?
            .unwrap_or_else(|| Message::user(""));
        let history = messages
            .map(|message| to_rig_message(&message))
            .collect::<Result<Vec<_>, _>>()?;
        let mut builder = CompletionRequest::new(prompt)
            .messages(history)
            .preamble(request.system);
        for tool in request.tools {
            let name = ToolName::new(tool.name)
                .map_err(|_| ModelError::Context("empty tool name".to_owned()))?;
            builder = builder.tool(ToolDefinition::new(name, tool.description, tool.parameters));
        }
        Ok(builder)
    }
}

/// Constructs the concrete provider for an endpoint-qualified model from
/// the configuration, resolving credentials through the helper.
pub fn provider_for(model_ref: &str, config: &Config) -> Result<RigProvider, ModelError> {
    let Some((endpoint_name, model)) = model_ref.split_once('/') else {
        return Err(ModelError::UnknownModel {
            model: model_ref.to_owned(),
        });
    };
    let endpoint = config
        .endpoints
        .get(endpoint_name)
        .ok_or_else(|| ModelError::UnknownModel {
            model: model_ref.to_owned(),
        })?;

    // v1 implements the OpenAI chat-completions wire format only.
    let credential = match endpoint.auth {
        crate::config::Auth::None => secrecy::SecretString::new(String::new().into()),
        crate::config::Auth::Helper => {
            let helper = credential_helper_name(config);
            resolve_credential(&helper, endpoint_name).ok_or_else(|| ModelError::Credential {
                identity: endpoint_name.to_owned(),
            })?
        }
    };
    RigProvider::openai_completions(&endpoint.base_url, credential.expose_secret(), model)
}

impl ModelProvider for RigProvider {
    async fn stream_completion(
        &self,
        request: ModelRequest,
        on_text_delta: &mut (dyn FnMut(&str) + Send),
        on_thought_delta: &mut (dyn FnMut(&str) + Send),
    ) -> Result<ModelResponse, ModelError> {
        use futures::StreamExt as _;

        let completion = self.completion(request)?;
        let mut stream = self
            .model
            .stream(completion)
            .map_err(|err| ModelError::Completion(err.to_string()))?;
        let mut text = String::new();

        while let Some(part) = stream.next().await {
            match part {
                Ok(Item::Event(StreamEvent::Text { text: delta, .. })) => {
                    on_text_delta(&delta);
                    text.push_str(&delta);
                }
                Ok(Item::Event(StreamEvent::Reasoning { text: delta, .. })) => {
                    if !delta.is_empty() {
                        on_thought_delta(&delta);
                    }
                }
                Ok(_) => {} // tool calls land with the registry
                Err(err) => {
                    // A failure after deltas flowed: the partial text rides
                    // the error (retrying would duplicate what the client
                    // already saw).
                    return Err(ModelError::StreamFailed {
                        text_len: text.len(),
                        text,
                        error: err.to_string(),
                    });
                }
            }
        }

        let response = match stream.finish().await {
            Ok(response) => response,
            Err(err) => {
                return Err(ModelError::StreamFailed {
                    text_len: text.len(),
                    text,
                    error: err.to_string(),
                });
            }
        };

        // The aggregated choice is authoritative for the final text.
        let final_text = response
            .choice
            .iter()
            .filter_map(|content| match content {
                AssistantContent::Text(text) => Some(text.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");
        if !final_text.is_empty() {
            text = final_text;
        }
        let usage = ModelUsage {
            input_tokens: response.usage.input_tokens.unwrap_or(0),
            output_tokens: response.usage.output_tokens.unwrap_or(0),
        };
        Ok(ModelResponse {
            text,
            tool_calls: tool_calls_of(&response.choice),
            usage,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Auth, EndpointConfig, WireFormat};
    use crate::credentials::PATH_LOCK;
    use crate::providers::ChatMessage;
    use crate::store::SessionStore;
    use std::io::Write as _;

    fn endpoint_config(base_url: &str, auth: Auth) -> EndpointConfig {
        EndpointConfig {
            base_url: base_url.to_owned(),
            wire_format: WireFormat::OpenaiChatCompletions,
            auth,
            models: Vec::new(),
        }
    }

    fn config_with_endpoint(base_url: &str, auth: Auth) -> Config {
        let mut config = Config::default();
        config
            .endpoints
            .insert("test".to_owned(), endpoint_config(base_url, auth));
        config
    }

    fn write_fake_helper(dir: &std::path::Path, body: &str) {
        let path = dir.join("agentkit-credential-test");
        let mut file = std::fs::File::create(&path).unwrap();
        // The real helpers reject an invocation without the component
        // argument; this fake does too, so a regression cannot pass.
        writeln!(
            file,
            "#!/bin/sh\n\
             [ \"$#\" -eq 3 ] && [ \"$1\" = get ] && [ \"$2\" = tackle ] || exit 3\n\
             echo '{body}'"
        )
        .unwrap();
        file.flush().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    /// A minimal OpenAI-compatible SSE body: a role chunk, one content
    /// chunk, a terminal chunk carrying usage, and the [DONE] sentinel.
    fn sse_body(text: &str, input: u64, output: u64) -> String {
        let chunk = |delta: serde_json::Value| {
            serde_json::json!({
                "id": "c1",
                "object": "chat.completion.chunk",
                "created": 0,
                "model": "m",
                "choices": [{"index": 0, "delta": delta}],
            })
        };
        let terminal = serde_json::json!({
            "id": "c1",
            "object": "chat.completion.chunk",
            "created": 0,
            "model": "m",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": input, "completion_tokens": output, "total_tokens": input + output},
        });
        [
            format!("data: {}", chunk(serde_json::json!({"role": "assistant"}))),
            String::new(),
            format!("data: {}", chunk(serde_json::json!({"content": text}))),
            String::new(),
            format!("data: {terminal}"),
            String::new(),
            "data: [DONE]".to_owned(),
            String::new(),
        ]
        .join("\n")
    }

    #[test]
    fn unknown_model_ref_is_rejected() {
        let config = config_with_endpoint("http://localhost", Auth::None);
        let Err(err) = provider_for("missing/model", &config) else {
            panic!("expected an unknown-model rejection");
        };
        assert!(err.to_string().contains("not configured"), "{err}");
    }

    #[tokio::test]
    async fn completion_sends_model_system_and_prompt() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/chat/completions"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_raw(
                sse_body("Hello there", 10, 4).into_bytes(),
                "text/event-stream",
            ))
            .mount(&server)
            .await;

        let config = config_with_endpoint(&server.uri(), Auth::None);
        let provider = provider_for("test/test-model", &config).unwrap();
        let response = provider
            .stream_completion(
                ModelRequest {
                    // The endpoint-qualified address, as the loop passes it:
                    // the provider owns the bare wire name.
                    model: "test/test-model".into(),
                    system: "You are testing.".into(),
                    messages: vec![
                        ChatMessage::user("prior"),
                        ChatMessage::assistant("prior answer"),
                        ChatMessage::user("say hello"),
                    ],
                    tools: Vec::new(),
                },
                &mut |_: &str| {},
                &mut |_: &str| {},
            )
            .await
            .unwrap();

        assert_eq!(response.text, "Hello there");
        assert_eq!(response.usage.input_tokens, 10);
        assert_eq!(response.usage.output_tokens, 4);

        let received = &server.received_requests().await.unwrap()[0];
        let body: serde_json::Value = serde_json::from_slice(&received.body).unwrap();
        assert_eq!(
            body["model"], "test-model",
            "the endpoint-qualified address never rides the wire"
        );
        // The system preamble rides the request (rig owns the wire shape —
        // content blocks for the system role in this version).
        assert!(
            body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| message["role"] == "system"),
            "system message missing from the wire: {body}"
        );
        let user_texts: Vec<String> = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "user")
            .map(|message| {
                message["content"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_default()
            })
            .collect();
        assert_eq!(user_texts, ["prior", "say hello"]);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn completion_sends_ordered_text_and_image_content() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/chat/completions"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_raw(
                sse_body("described", 10, 4).into_bytes(),
                "text/event-stream",
            ))
            .mount(&server)
            .await;

        let config = config_with_endpoint(&server.uri(), Auth::None);
        let provider = provider_for("test/test-model", &config).unwrap();
        provider
            .stream_completion(
                ModelRequest {
                    model: "test/test-model".into(),
                    system: String::new(),
                    messages: vec![ChatMessage::with_content(
                        crate::providers::ChatRole::User,
                        vec![
                            crate::providers::ChatContent::text("describe"),
                            crate::providers::ChatContent::Image {
                                url: "data:image/png;base64,AAAA".into(),
                            },
                            crate::providers::ChatContent::text("thanks"),
                        ],
                    )],
                    tools: Vec::new(),
                },
                &mut |_: &str| {},
                &mut |_: &str| {},
            )
            .await
            .unwrap();

        let received = &server.received_requests().await.unwrap()[0];
        let body: serde_json::Value = serde_json::from_slice(&received.body).unwrap();
        let user = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["role"] == "user")
            .unwrap();
        assert_eq!(
            user["content"],
            serde_json::json!([
                { "type": "text", "text": "describe" },
                {
                    "type": "image_url",
                    "image_url": {
                        "url": "data:image/png;base64,AAAA",
                        "detail": "auto"
                    }
                },
                { "type": "text", "text": "thanks" }
            ])
        );
    }

    #[tokio::test]
    async fn helper_auth_resolves_the_credential() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_helper(dir.path(), "{\"access_token\": \"secret\"}");

        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_raw(sse_body("ok", 1, 1).into_bytes(), "text/event-stream"),
            )
            .mount(&server)
            .await;

        let mut config = config_with_endpoint(&server.uri(), Auth::Helper);
        config.credential_helper = Some("test".into());
        // The credential resolves during provider_for, so the lock (and
        // the PATH mutation) need not be held across any await.
        let provider = {
            let _guard = PATH_LOCK.lock().unwrap();
            let path_var = std::env::var("PATH").unwrap();
            std::env::set_var("PATH", format!("{}:{}", dir.path().display(), path_var));
            let provider = provider_for("test/model", &config);
            std::env::set_var("PATH", path_var);
            provider.unwrap()
        };
        let response = provider
            .stream_completion(
                ModelRequest {
                    model: "model".into(),
                    system: String::new(),
                    messages: vec![ChatMessage::user("hi")],
                    tools: Vec::new(),
                },
                &mut |_: &str| {},
                &mut |_: &str| {},
            )
            .await
            .unwrap();
        assert_eq!(response.text, "ok");

        let received = &server.received_requests().await.unwrap()[0];
        let header = received
            .headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert!(header.contains("secret"), "authorization header: {header}");
    }

    #[tokio::test]
    async fn streaming_relays_deltas_and_aggregates() {
        let server = wiremock::MockServer::start().await;
        // The real OpenAI streaming shape: SSE data lines, delta fragments,
        // a terminal chunk with usage, and the [DONE] sentinel.
        let sse = [
            "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"}}]}",
            "",
            "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hel\"}}]}",
            "",
            "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"lo\"}}]}",
            "",
            "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2,\"total_tokens\":7}}",
            "",
            "data: [DONE]",
            "",
        ]
        .join("\n");
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_raw(sse.into_bytes(), "text/event-stream"),
            )
            .mount(&server)
            .await;

        let config = config_with_endpoint(&server.uri(), Auth::None);
        let provider = provider_for("test/streaming-model", &config).unwrap();

        let mut deltas = Vec::new();
        let response = provider
            .stream_completion(
                ModelRequest {
                    model: "streaming-model".into(),
                    system: String::new(),
                    messages: vec![ChatMessage::user("say hi")],
                    tools: Vec::new(),
                },
                &mut |delta: &str| deltas.push(delta.to_owned()),
                &mut |_: &str| {},
            )
            .await
            .unwrap();

        assert_eq!(deltas.join(""), "Hello", "every delta was relayed live");
        assert_eq!(response.text, "Hello", "the aggregated text matches");
        assert_eq!(response.usage.input_tokens, 5);
        assert_eq!(response.usage.output_tokens, 2);
    }

    #[tokio::test]
    async fn reasoning_deltas_stream_as_thoughts_and_stay_out_of_the_answer() {
        let server = wiremock::MockServer::start().await;
        // A reasoning model streams `reasoning_content` deltas alongside
        // the answer (the DeepSeek/Qwen shape rig normalizes).
        let sse = [
            "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"}}]}",
            "",
            "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"think \"}}]}",
            "",
            "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"more\"}}]}",
            "",
            "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Answer\"}}]}",
            "",
            "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2,\"total_tokens\":7}}",
            "",
            "data: [DONE]",
            "",
        ]
        .join("\n");
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_raw(sse.into_bytes(), "text/event-stream"),
            )
            .mount(&server)
            .await;

        let config = config_with_endpoint(&server.uri(), Auth::None);
        let provider = provider_for("test/streaming-model", &config).unwrap();

        let mut thoughts = Vec::new();
        let mut deltas = Vec::new();
        let response = provider
            .stream_completion(
                ModelRequest {
                    model: "streaming-model".into(),
                    system: String::new(),
                    messages: vec![ChatMessage::user("think")],
                    tools: Vec::new(),
                },
                &mut |delta: &str| deltas.push(delta.to_owned()),
                &mut |delta: &str| thoughts.push(delta.to_owned()),
            )
            .await
            .unwrap();

        assert_eq!(
            thoughts.join(""),
            "think more",
            "reasoning streams as thoughts"
        );
        assert_eq!(
            deltas.join(""),
            "Answer",
            "only answer text is a message delta"
        );
        assert_eq!(response.text, "Answer", "thoughts never enter the answer");
    }

    #[tokio::test]
    async fn turn_loop_persists_assistant_response_and_usage() {
        let server = wiremock::MockServer::start().await;
        // The turn loop streams: the mock must answer with SSE.
        let sse = [
            "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"}}]}",
            "",
            "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"the answer\"}}]}",
            "",
            "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":3,\"total_tokens\":12}}",
            "",
            "data: [DONE]",
            "",
        ]
        .join("\n");
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_raw(sse.into_bytes(), "text/event-stream"),
            )
            .mount(&server)
            .await;

        let store = SessionStore::in_memory();
        let session = store
            .create_session(
                crate::store::SessionKind::Interactive,
                "/work",
                None,
                None,
                "{}",
            )
            .await
            .unwrap();
        let turn = store
            .append_turn(
                &session.id,
                None,
                crate::store::TurnKind::Interaction,
                None,
                crate::store::TurnUsage::default(),
            )
            .await
            .unwrap();
        let assistant_id = "msg-assistant-1";

        let config = config_with_endpoint(&server.uri(), Auth::None);
        let provider = provider_for("test/model", &config).unwrap();

        let mut deltas = Vec::new();
        let mut usage_updates = Vec::new();
        let outcome = crate::agent::turn::run_turn(
            &provider,
            &store,
            &session.id,
            &turn.id,
            assistant_id,
            "test/model",
            "system prompt",
            vec![crate::providers::ChatContent::text("the question")],
            Vec::new(),
            Vec::new(),
            8,
            "This model's context is full. Run /compact (the `compaction` script), or start a fresh session.",
            |input: u64, _output: u64, _cost: Option<f64>| usage_updates.push(input),
            |delta: &str| deltas.push(delta.to_owned()),
            |_: &str| {},
            |_attempt: u32, _message: &str| {}, // no retry assertions here
            crate::providers::ignore_tool_call,
        )
        .await
        .unwrap();

        assert_eq!(outcome.stop, crate::agent::TurnStop::EndTurn);
        assert_eq!(usage_updates, [9], "one usage callback per model request");
        assert_eq!(deltas, ["the answer"]);
        let usage = store.session_usage(&session.id).await.unwrap();
        assert_eq!(usage.input_tokens, 9, "the turn's usage delta is persisted");
        assert_eq!(usage.output_tokens, 3);

        // The wire model is the provider's bare name; the loop passes the
        // endpoint-qualified address ("test/model").
        let received = &server.received_requests().await.unwrap()[0];
        let body: serde_json::Value = serde_json::from_slice(&received.body).unwrap();
        assert_eq!(body["model"], "model");
    }

    #[tokio::test]
    async fn retryable_failures_retry_then_succeed() {
        let server = wiremock::MockServer::start().await;
        // Attempt 1: a 429 with a retry-after hint; attempt 2: success.
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/chat/completions"))
            .respond_with(
                wiremock::ResponseTemplate::new(429)
                    .set_body_string("429 too many requests; retry-after: 1"),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(
                serde_json::json!({
                    "id": "chatcmpl-r",
                    "object": "chat.completion",
                    "created": 0,
                    "model": "m",
                    "choices": [{"message": {"role": "assistant", "content": "recovered"}, "finish_reason": "stop"}],
                    "usage": {"prompt_tokens": 2, "completion_tokens": 1, "total_tokens": 3}
                }),
            ))
            .mount(&server)
            .await;

        let config = config_with_endpoint(&server.uri(), Auth::None);
        let provider = provider_for("test/model", &config).unwrap();
        let mut retries = Vec::new();
        let outcome = retry_completion(&provider, "test/model", &mut retries)
            .await
            .unwrap();
        assert_eq!(outcome.stop, crate::agent::TurnStop::EndTurn);
        assert_eq!(retries, [1], "one retry card for the failed attempt");
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn context_length_failures_translate_without_retrying() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(
                wiremock::ResponseTemplate::new(400)
                    .set_body_string("prompt is too long: 300000 tokens > 200000 maximum"),
            )
            .mount(&server)
            .await;

        let store = SessionStore::in_memory();
        let session = store
            .create_session(
                crate::store::SessionKind::Interactive,
                "/work",
                None,
                None,
                "{}",
            )
            .await
            .unwrap();
        let turn = store
            .append_turn(
                &session.id,
                None,
                crate::store::TurnKind::Interaction,
                None,
                crate::store::TurnUsage::default(),
            )
            .await
            .unwrap();

        let config = config_with_endpoint(&server.uri(), Auth::None);
        let provider = provider_for("test/model", &config).unwrap();
        let outcome = crate::agent::turn::run_turn(
            &provider,
            &store,
            &session.id,
            &turn.id,
            "msg-x",
            "test/model",
            "sys",
            vec![crate::providers::ChatContent::text("the question")],
            Vec::new(),
            Vec::new(),
            8,
            "This model's context is full. Run /compact (the `compaction` script), or start a fresh session.",
            |_: u64, _: u64, _: Option<f64>| {},
            |_: &str| {},
            |_: &str| {},
            |_: u32, _: &str| {},
            crate::providers::ignore_tool_call,
        )
        .await
        .unwrap();

        // The turn ENDS with an in-band actionable message; one request only.
        assert_eq!(outcome.stop, crate::agent::TurnStop::ContextLength);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        let usage = store.session_usage(&session.id).await.unwrap();
        let _ = usage;
    }

    #[tokio::test]
    async fn mid_stream_failures_persist_partial_output_without_retrying() {
        let server = wiremock::MockServer::start().await;
        // One good delta, then a malformed line kills the SSE decode.
        let sse = r#"data: {"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{"index":0,"delta":{"content":"partial tex"}}]}

data: {not json}

"#;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_raw(sse.bytes().collect::<Vec<u8>>(), "text/event-stream"),
            )
            .mount(&server)
            .await;

        let store = SessionStore::in_memory();
        let session = store
            .create_session(
                crate::store::SessionKind::Interactive,
                "/work",
                None,
                None,
                "{}",
            )
            .await
            .unwrap();
        let turn = store
            .append_turn(
                &session.id,
                None,
                crate::store::TurnKind::Interaction,
                None,
                crate::store::TurnUsage::default(),
            )
            .await
            .unwrap();

        let config = config_with_endpoint(&server.uri(), Auth::None);
        let provider = provider_for("test/model", &config).unwrap();
        let outcome = crate::agent::turn::run_turn(
            &provider,
            &store,
            &session.id,
            &turn.id,
            "msg-partial",
            "test/model",
            "sys",
            vec![crate::providers::ChatContent::text("go")],
            Vec::new(),
            Vec::new(),
            8,
            "This model's context is full. Run /compact (the `compaction` script), or start a fresh session.",
            |_: u64, _: u64, _: Option<f64>| {},
            |_: &str| {},
            |_: &str| {},
            |_: u32, _: &str| {},
            crate::providers::ignore_tool_call,
        )
        .await
        .unwrap();

        assert_eq!(outcome.stop, crate::agent::TurnStop::EndTurn);
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            1,
            "mid-stream failures never retry"
        );
        let assembled = store.assemble_context(&session.id).await.unwrap();
        let texts: Vec<String> = assembled
            .iter()
            .filter(|a| a.message.role == crate::store::Role::Assistant)
            .map(|a| {
                let blocks: serde_json::Value = serde_json::from_str(&a.message.content).unwrap();
                blocks[0]["text"].as_str().unwrap().to_owned()
            })
            .collect();
        assert_eq!(texts, ["partial tex"], "the partial text is persisted");
    }

    /// Runs one turn, collecting retry cards (the retry seam's shape).
    async fn retry_completion(
        provider: &RigProvider,
        model: &str,
        retries: &mut Vec<u32>,
    ) -> Result<crate::agent::TurnOutcome, crate::agent::turn::TurnError> {
        crate::agent::turn::run_turn(
            provider,
            &SessionStore::in_memory(),
            &"sess-test".to_owned(),
            &"turn-test".to_owned(),
            "msg-r",
            model,
            "sys",
            vec![crate::providers::ChatContent::text("go")],
            Vec::new(),
            Vec::new(),
            8,
            "This model's context is full. Run /compact (the `compaction` script), or start a fresh session.",
            |_: u64, _: u64, _: Option<f64>| {},
            |_: &str| {},
            |_: &str| {},
            |attempt: u32, _: &str| retries.push(attempt),
            crate::providers::ignore_tool_call,
        )
        .await
    }
}
