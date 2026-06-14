use rig_core::completion::{Chat, Message};
use rig_core::prelude::*;
use rig_core::providers::openai;
use rig_sample::EchoTool;

/// This test calls a paid API. You must explicitly opt in by setting
/// `AGENTKIT_ACCEPT_API_COST=true` and providing a valid API key.
#[ignore = "costs money; set AGENTKIT_ACCEPT_API_COST=true to run"]
#[tokio::test]
async fn test_live_agent_tool_call() {
    if std::env::var("AGENTKIT_ACCEPT_API_COST").unwrap_or_default() != "true" {
        panic!("Set AGENTKIT_ACCEPT_API_COST=true to confirm you accept API costs");
    }

    let api_key = std::env::var("SWITCHBOARD_OPENAI_API_KEY")
        .or_else(|_| std::env::var("OPENAI_API_KEY"))
        .expect("SWITCHBOARD_OPENAI_API_KEY or OPENAI_API_KEY must be set");

    let client = openai::Client::builder()
        .api_key(&api_key)
        .build()
        .expect("failed to build client");

    let agent = client
        .agent("gpt-5.4-mini")
        .preamble("You have access to an echo tool. Call it when asked.")
        .tool(EchoTool)
        .build();

    let mut history: Vec<Message> = Vec::new();

    let result = agent
        .chat(
            "Call the echo tool with message 'live test' and report what it said",
            &mut history,
        )
        .await
        .expect("agent chat failed");

    assert!(!result.is_empty(), "response should not be empty");
    assert!(
        result.contains("live test"),
        "response should contain the echoed message: {}",
        result
    );
    assert!(history.len() >= 2, "history should contain user+assistant messages");
}
