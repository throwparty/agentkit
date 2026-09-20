//! The MCP pool's happy-path and failure tests, driven through the
//! tackle-mcp-echo stdio fixture.

use agentkit_tackle::config::McpServerConfig;
use agentkit_tackle::mcp::McpPool;
use std::collections::BTreeMap;

fn echo_server_config() -> McpServerConfig {
    McpServerConfig::Stdio {
        command: env!("CARGO_BIN_EXE_tackle-mcp-echo").into(),
        args: Vec::new(),
        env: BTreeMap::new(),
    }
}

#[tokio::test]
async fn connects_lists_and_calls_across_the_pool() {
    let mut pool = McpPool::new();
    pool.connect("echo", &echo_server_config()).await;

    assert_eq!(
        pool.statuses().get("echo"),
        Some(&agentkit_tackle::mcp::ServerStatus::Connected)
    );
    assert!(pool.is_connected("echo"));

    let tools = pool.list_tools().await;
    let echo = tools
        .iter()
        .find(|tool| tool.name == "mcp.echo.echo")
        .expect("echo tool");
    assert!(echo.description.contains("Echo"));

    let result = pool
        .call_tool("echo", "echo", serde_json::json!({ "text": "hello" }))
        .await
        .unwrap();
    assert!(!result.is_error);
    let blocks: serde_json::Value = serde_json::from_str(&result.content_json).unwrap();
    assert_eq!(blocks[0]["text"], "hello");
}

#[tokio::test]
async fn unknown_server_and_truncation_are_handled() {
    let mut pool = McpPool::new();
    pool.connect("echo", &echo_server_config()).await;

    // A missing server's call is a pool error, never a panic.
    let err = pool
        .call_tool("missing", "echo", serde_json::json!({}))
        .await;
    assert!(err.is_err());

    // Oversized results truncate at the fixed internal limit with the
    // marker set.
    let huge: String = "x".repeat(agentkit_tackle::mcp::TOOL_RESULT_LIMIT + 1024);
    let result = pool
        .call_tool("echo", "echo", serde_json::json!({ "text": huge }))
        .await
        .unwrap();
    assert!(result.truncated);
    assert_eq!(
        result.content_json.len(),
        agentkit_tackle::mcp::TOOL_RESULT_LIMIT
    );
}

#[tokio::test]
async fn failed_connections_are_statuses_not_panics() {
    let mut pool = McpPool::new();
    let config = McpServerConfig::Stdio {
        command: "/nonexistent/tackle-mcp-nothing".into(),
        args: Vec::new(),
        env: BTreeMap::new(),
    };
    pool.connect("broken", &config).await;
    assert!(matches!(
        pool.statuses().get("broken"),
        Some(agentkit_tackle::mcp::ServerStatus::Failed { .. })
    ));
    assert!(!pool.is_connected("broken"));
}
