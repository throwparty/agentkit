use rig_core::completion::{Chat, Message};
use rig_core::prelude::*;
use rig_core::providers::openai;
use rig_sample::recording_client::RecordingClient;
use rig_sample::EchoTool;

#[tokio::test]
async fn test_cassette_replay() {
    let rec = RecordingClient::new(
        false,
        "tests/cassettes/echo_tool.json".to_string(),
    );
    let api_key = std::env::var("SWITCHBOARD_OPENAI_API_KEY")
        .or_else(|_| std::env::var("OPENAI_API_KEY"))
        .unwrap_or_else(|_| "test-key".to_string());

    let client = openai::Client::builder()
        .api_key(&api_key)
        .http_client(rec)
        .build()
        .expect("failed to build client");

    let agent = client
        .agent("gpt-5.4-mini")
        .preamble("You have access to an echo tool. Call it when asked.")
        .tool(EchoTool)
        .build();

    let mut history: Vec<Message> = Vec::new();

    let result = agent
        .chat("Call the echo tool with message 'record' and report what it said", &mut history)
        .await
        .expect("agent chat failed");

    assert!(!result.is_empty(), "response should not be empty");
    assert!(history.len() >= 2, "history should contain at least user+assistant messages");
    println!("Response: {}", result);
}
