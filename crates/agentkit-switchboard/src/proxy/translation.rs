use serde_json::{Value, json};

#[derive(Debug)]
pub enum TranslationError {
    MissingField(String),
    UnknownRole(String),
    StreamingNotSupported,
    InvalidFormat(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProviderKind {
    ChatCompletions,
    ResponsesApi,
}

pub fn translate_request(body: &Value, target: ProviderKind) -> Result<Value, TranslationError> {
    match target {
        ProviderKind::ChatCompletions => Ok(body.clone()),
        ProviderKind::ResponsesApi => chat_to_responses(body),
    }
}

pub fn translate_response(body: &Value, source: ProviderKind) -> Result<Value, TranslationError> {
    match source {
        ProviderKind::ChatCompletions => Ok(body.clone()),
        ProviderKind::ResponsesApi => responses_to_chat(body),
    }
}

fn chat_to_responses(body: &Value) -> Result<Value, TranslationError> {
    let streaming = body.get("stream").and_then(|v| v.as_bool()).unwrap_or(false);
    if streaming {
        return Err(TranslationError::StreamingNotSupported);
    }

    let messages = body
        .get("messages")
        .and_then(|v| v.as_array())
        .ok_or_else(|| TranslationError::MissingField("messages".into()))?;

    let mut input = Vec::new();
    let mut instructions = None;

    for msg in messages {
        let role = msg
            .get("role")
            .and_then(|v| v.as_str())
            .ok_or_else(|| TranslationError::MissingField("role".into()))?;

        let content = msg
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| TranslationError::MissingField("content".into()))?;

        match role {
            "system" => {
                instructions = Some(content.to_string());
            }
            "user" => {
                input.push(json!({
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": content}]
                }));
            }
            "assistant" => {
                input.push(json!({
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": content}]
                }));
            }
            other => return Err(TranslationError::UnknownRole(other.to_string())),
        }
    }

    let mut result = json!({
        "input": input,
        "store": false,
        "reasoning": {"effort": "medium"}
    });

    if let Some(inst) = instructions {
        result["instructions"] = json!(inst);
    }

    if let Some(model) = body.get("model").and_then(|v| v.as_str()) {
        result["model"] = json!(model);
    }
    if let Some(temp) = body.get("temperature") {
        result["temperature"] = temp.clone();
    }
    if let Some(max_tokens) = body.get("max_tokens") {
        result["max_tokens"] = max_tokens.clone();
    }

    Ok(result)
}

fn responses_to_chat(body: &Value) -> Result<Value, TranslationError> {
    let output = body
        .get("output")
        .and_then(|v| v.as_array())
        .ok_or_else(|| TranslationError::MissingField("output".into()))?;

    let mut choices = Vec::new();
    for (i, item) in output.iter().enumerate() {
        let role = item
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("assistant");
        let content = item
            .get("content")
            .and_then(|v| v.as_array())
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();

        let finish_reason = item.get("type").and_then(|v| v.as_str()).map(|t| {
            if t == "message" {
                "stop"
            } else {
                t
            }
        });

        choices.push(json!({
            "index": i as u64,
            "message": {
                "role": role,
                "content": content
            },
            "finish_reason": finish_reason.unwrap_or("stop")
        }));
    }

    let mut result = json!({
        "choices": choices,
        "object": "chat.completion"
    });

    if let Some(usage) = body.get("usage") {
        result["usage"] = usage.clone();
    }
    if let Some(id) = body.get("id") {
        result["id"] = id.clone();
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn translate_request_basic() {
        let body = json!({
            "model": "gpt-4o",
            "messages": [
                {"role": "user", "content": "Hello"}
            ]
        });
        let result = chat_to_responses(&body).unwrap();
        assert_eq!(result["input"][0]["role"], "user");
        assert_eq!(result["input"][0]["content"][0]["text"], "Hello");
        assert_eq!(result["store"], false);
        assert!(result.get("instructions").is_none());
    }

    #[test]
    fn translate_request_system_message() {
        let body = json!({
            "model": "gpt-4o",
            "messages": [
                {"role": "system", "content": "You are helpful"},
                {"role": "user", "content": "Hello"}
            ]
        });
        let result = chat_to_responses(&body).unwrap();
        assert_eq!(result["instructions"], "You are helpful");
        assert_eq!(result["input"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn translate_request_streaming() {
        let body = json!({
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Hi"}],
            "stream": true
        });
        let result = chat_to_responses(&body);
        assert!(matches!(result, Err(TranslationError::StreamingNotSupported)));
    }

    #[test]
    fn translate_request_params() {
        let body = json!({
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Hi"}],
            "temperature": 0.5,
            "max_tokens": 500
        });
        let result = chat_to_responses(&body).unwrap();
        assert_eq!(result["temperature"], 0.5);
        assert_eq!(result["max_tokens"], 500);
    }

    #[test]
    fn translate_response_basic() {
        let body = json!({
            "id": "resp_123",
            "output": [
                {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Hello there"}]}
            ],
            "usage": {"input_tokens": 5, "output_tokens": 10}
        });
        let result = responses_to_chat(&body).unwrap();
        assert_eq!(result["choices"][0]["message"]["content"], "Hello there");
        assert_eq!(result["choices"][0]["finish_reason"], "stop");
        assert_eq!(result["usage"]["input_tokens"], 5);
    }

    #[test]
    fn translate_response_usage() {
        let body = json!({
            "output": [
                {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Hi"}]}
            ],
            "usage": {"input_tokens": 10, "output_tokens": 20, "total_tokens": 30}
        });
        let result = responses_to_chat(&body).unwrap();
        assert_eq!(result["usage"]["total_tokens"], 30);
    }

    #[test]
    fn translate_unknown_role() {
        let body = json!({
            "messages": [{"role": "function", "content": "{}"}]
        });
        let result = chat_to_responses(&body);
        assert!(matches!(result, Err(TranslationError::UnknownRole(_))));
    }
}
