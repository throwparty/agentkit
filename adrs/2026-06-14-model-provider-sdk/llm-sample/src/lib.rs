use llm::chat::ChatMessage;
use llm::builder::{FunctionBuilder, ParamBuilder};
use llm::{FunctionCall, ToolCall};

pub fn echo_function_builder() -> FunctionBuilder {
    FunctionBuilder::new("echo")
        .description("Echoes the input arguments back as a result")
        .param(
            ParamBuilder::new("message")
                .type_of("string")
                .description("The message to echo"),
        )
        .required(vec!["message".to_string()])
}

pub fn execute_echo(arguments: &str) -> String {
    let args: serde_json::Value =
        serde_json::from_str(arguments).unwrap_or(serde_json::json!({}));
    let message = args
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("no message");
    serde_json::json!({"result": message}).to_string()
}

pub fn build_tool_result(tc: &ToolCall, result: String) -> ChatMessage {
    ChatMessage::user()
        .tool_result(vec![ToolCall {
            id: tc.id.clone(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: tc.function.name.clone(),
                arguments: result.clone(),
            },
        }])
        .content(result)
        .build()
}

pub fn build_tool_use(tool_calls: Vec<ToolCall>) -> ChatMessage {
    ChatMessage::assistant().tool_use(tool_calls).build()
}

pub fn build_user_message(content: &str) -> ChatMessage {
    ChatMessage::user().content(content).build()
}

pub fn build_assistant_message(content: &str) -> ChatMessage {
    ChatMessage::assistant().content(content).build()
}
