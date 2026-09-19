//! The rig-core-backed model provider: named endpoints, credential-helper
//! resolution, and the openai-chat-completions wire format in v1.

use super::{
    ChatMessage, ChatRole, ModelError, ModelProvider, ModelRequest, ModelResponse, ModelUsage,
};
use crate::config::Config;
use rig_core::client::CompletionClient as _;
use rig_core::completion::CompletionModel as _;
use rig_core::message::{AssistantContent, Message};
use rig_core::providers::openai;
use secrecy::ExposeSecret as _;

/// The rig-backed provider: one endpoint, one wire format.
pub struct RigProvider {
    /// The chat-completions model: the completions client (not the
    /// responses-API default) provides this model type.
    model: openai::completion::GenericCompletionModel,
}

impl RigProvider {
    /// Builds the provider for a configured endpoint (openai-chat-completions
    /// wire format).
    pub fn openai_completions(
        base_url: &str,
        credential: &str,
        model: &str,
    ) -> Result<Self, ModelError> {
        let client = openai::CompletionsClient::builder()
            .api_key(credential.to_owned())
            .base_url(base_url)
            .build()
            .map_err(|err| ModelError::Completion(err.to_string()))?;
        Ok(Self {
            model: client.completion_model(model),
        })
    }

    fn completion(
        &self,
        request: ModelRequest,
    ) -> rig_core::completion::CompletionRequestBuilder<openai::completion::CompletionModel> {
        // The last message is the prompt; everything before is history;
        // the system prompt rides the legacy preamble (still supported —
        // the canonical alternative is a leading System message).
        let mut messages = request.messages.into_iter();
        let prompt = messages
            .next_back()
            .map(|message| to_rig_message(&message))
            .unwrap_or_else(|| Message::user(""));
        self.model
            .completion_request(prompt)
            .messages(
                messages
                    .map(|message| to_rig_message(&message))
                    .collect::<Vec<_>>(),
            )
            .preamble(request.system)
            .model(request.model)
    }
}

fn to_rig_message(message: &ChatMessage) -> Message {
    match message.role {
        ChatRole::User => Message::user(message.text.clone()),
        ChatRole::Assistant => Message::assistant(message.text.clone()),
    }
}

/// Resolves a credential through the helper command: `agentkit-credential-
/// {helper} get {identity}` — the switchboard protocol. The helper's
/// stdout is JSON carrying `access_token`.
pub fn resolve_credential(helper: &str, identity: &str) -> Option<String> {
    let output = std::process::Command::new(format!("agentkit-credential-{helper}"))
        .arg("get")
        .arg(identity)
        .output()
        .ok()?;
    if !output.status.success() {
        tracing::warn!(
            "credential helper agentkit-credential-{helper} failed for {identity}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    value.get("access_token")?.as_str().map(str::to_owned)
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

    match endpoint.wire_format {
        crate::config::WireFormat::OpenaiChatCompletions => {
            let credential = match endpoint.auth {
                crate::config::Auth::None => String::new(),
                crate::config::Auth::Helper => {
                    let helper = config
                        .credential_helper
                        .clone()
                        .unwrap_or_else(|| "agentkit-credential".into());
                    resolve_credential(&helper, endpoint_name).ok_or_else(|| {
                        ModelError::Credential {
                            identity: endpoint_name.to_owned(),
                        }
                    })?
                }
            };
            let credential = secrecy::SecretString::new(credential.into());
            RigProvider::openai_completions(&endpoint.base_url, credential.expose_secret(), model)
        }
        format => Err(ModelError::UnsupportedWireFormat {
            endpoint: endpoint_name.to_owned(),
            format,
        }),
    }
}

impl ModelProvider for RigProvider {
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        let completion = self.completion(request);
        let response = completion
            .send()
            .await
            .map_err(|err| ModelError::Completion(err.to_string()))?;
        let text = response
            .choice
            .iter()
            .filter_map(|content| match content {
                AssistantContent::Text(text) => Some(text.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");
        Ok(ModelResponse {
            text,
            usage: ModelUsage {
                input_tokens: response.usage.input_tokens,
                output_tokens: response.usage.output_tokens,
            },
        })
    }
}

#[allow(unused)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Auth, EndpointConfig, WireFormat};
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
        writeln!(file, "#!/bin/sh\necho '{body}'").unwrap();
        file.flush().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn credential_resolution_reads_the_helper_stdout() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_helper(dir.path(), "{\"access_token\": \"secret\"}");
        let path_var = std::env::var("PATH").unwrap();
        std::env::set_var("PATH", format!("{}:{}", dir.path().display(), path_var));
        let credential = resolve_credential("test", "my-endpoint");
        std::env::set_var("PATH", path_var);
        assert_eq!(credential.as_deref(), Some("secret"));
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
        let expectation = wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/chat/completions"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "id": "chatcmpl-1",
                    "object": "chat.completion",
                    "created": 0,
                    "model": "test-model",
                    "choices": [{
                        "index": 0,
                        "message": {"role": "assistant", "content": "Hello there"},
                        "finish_reason": "stop"
                    }],
                    "usage": {"prompt_tokens": 10, "completion_tokens": 4, "total_tokens": 14}
                })),
            )
            .mount(&server)
            .await;

        let config = config_with_endpoint(&server.uri(), Auth::None);
        let provider = provider_for("test/test-model", &config).unwrap();
        let response = provider
            .complete(ModelRequest {
                model: "test-model".into(),
                system: "You are testing.".into(),
                messages: vec![
                    ChatMessage {
                        role: ChatRole::User,
                        text: "prior".into(),
                    },
                    ChatMessage {
                        role: ChatRole::Assistant,
                        text: "prior answer".into(),
                    },
                    ChatMessage {
                        role: ChatRole::User,
                        text: "say hello".into(),
                    },
                ],
            })
            .await
            .unwrap();

        assert_eq!(response.text, "Hello there");
        assert_eq!(response.usage.input_tokens, 10);
        assert_eq!(response.usage.output_tokens, 4);

        let received = &server.received_requests().await.unwrap()[0];
        let body: serde_json::Value = serde_json::from_slice(&received.body).unwrap();
        assert_eq!(body["model"], "test-model");
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
    async fn helper_auth_resolves_the_credential() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_helper(dir.path(), "{\"access_token\": \"secret\"}");
        let path_var = std::env::var("PATH").unwrap();
        std::env::set_var("PATH", format!("{}:{}", dir.path().display(), path_var));

        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST")).respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "chatcmpl-2",
                "object": "chat.completion",
                "created": 0,
                "model": "model",
                "choices": [{"message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            })),
        )
        .mount(&server)
        .await;

        let mut config = config_with_endpoint(&server.uri(), Auth::Helper);
        config.credential_helper = Some("test".into());
        let provider = provider_for("test/model", &config).unwrap();
        let response = provider
            .complete(ModelRequest {
                model: "model".into(),
                system: String::new(),
                messages: vec![ChatMessage {
                    role: ChatRole::User,
                    text: "hi".into(),
                }],
            })
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
        std::env::set_var("PATH", path_var);
    }
}
