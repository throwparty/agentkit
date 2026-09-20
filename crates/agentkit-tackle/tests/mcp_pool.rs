//! The MCP pool's happy-path and failure tests, driven through the
//! tackle-mcp-echo stdio fixture.

use agentkit_tackle::config::McpServerConfig;
use agentkit_tackle::mcp::{
    Elicitation, ElicitationResponse, ElicitationSink, McpPool, ServerStatus,
};
use agentkit_tackle::permissions::GrantStore;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

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

struct RecordingSink {
    received: Arc<Mutex<Vec<Elicitation>>>,
    response: ElicitationResponse,
}

impl ElicitationSink for RecordingSink {
    async fn elicit(&self, elicitation: Elicitation) -> ElicitationResponse {
        self.received.lock().unwrap().push(elicitation);
        self.response
    }
}

#[tokio::test]
async fn elicitations_surface_with_origin_and_never_touch_grants() {
    // The grant store in scope for the whole exchange: elicitation
    // forwarding must not write a single record into it.
    let grants = GrantStore::default();

    let received = Arc::new(Mutex::new(Vec::new()));
    let sink = RecordingSink {
        received: Arc::clone(&received),
        response: ElicitationResponse::Decline,
    };
    let mut pool = McpPool::with_sink(sink);
    pool.connect("echo", &echo_server_config()).await;
    assert_eq!(pool.statuses().get("echo"), Some(&ServerStatus::Connected));

    let result = pool
        .call_tool("echo", "elicit", serde_json::json!({}))
        .await
        .unwrap();
    assert!(!result.is_error);
    let text: serde_json::Value = serde_json::from_str(&result.content_json).unwrap();
    assert_eq!(text[0]["text"], "elicit:declined");

    let surfaced = received.lock().unwrap().clone();
    assert_eq!(surfaced.len(), 1);
    let elicitation = &surfaced[0];
    // Explicit origin attribution: the elicitation names its server,
    // semantically distinct from a tool permission prompt.
    assert_eq!(elicitation.server, "echo");
    assert_eq!(elicitation.message, "Name the project");
    assert!(elicitation.requested_schema.is_some());
    assert!(elicitation.url.is_none());

    assert!(grants.is_empty());
}

#[tokio::test]
async fn elicitation_cancellation_and_the_fail_closed_default() {
    // A cancelled permission response surfaces as an elicit cancel.
    let received = Arc::new(Mutex::new(Vec::new()));
    let mut pool = McpPool::with_sink(RecordingSink {
        received: Arc::clone(&received),
        response: ElicitationResponse::Cancel,
    });
    pool.connect("echo", &echo_server_config()).await;
    let result = pool
        .call_tool("echo", "elicit", serde_json::json!({}))
        .await
        .unwrap();
    let text: serde_json::Value = serde_json::from_str(&result.content_json).unwrap();
    assert_eq!(text[0]["text"], "elicit:cancelled");

    // The fail-closed default pool auto-declines every elicitation.
    let mut pool = McpPool::new();
    pool.connect("echo", &echo_server_config()).await;
    let result = pool
        .call_tool("echo", "elicit", serde_json::json!({}))
        .await
        .unwrap();
    let text: serde_json::Value = serde_json::from_str(&result.content_json).unwrap();
    assert_eq!(text[0]["text"], "elicit:declined");
}

mod direct_invocation {
    use super::*;
    use agentkit_tackle::invokables::{parse_direct, Registry};

    #[tokio::test]
    async fn execution_bypasses_the_pipeline_and_reports_the_shape() {
        let definitions = agentkit_tackle::loader::Definitions::default();
        let registry = Registry::build(
            &definitions,
            &[agentkit_tackle::mcp::NamespacedTool {
                name: "mcp.echo.echo".into(),
                description: "Echoes the provided text".into(),
                schema: serde_json::json!({"type": "object"}),
            }],
        )
        .unwrap();
        // The grant store in scope for the whole exchange: direct
        // invocation bypasses the pipeline and writes no records.
        let grants = agentkit_tackle::permissions::GrantStore::default();

        let mut pool = McpPool::new();
        pool.connect("echo", &echo_server_config()).await;

        let input = parse_direct("/!mcp.echo.echo {\"text\": \"hi\"}").unwrap();
        let result = registry.execute_direct(&pool, &input).await.unwrap();

        assert!(!result.is_error);
        assert!(!result.truncated);
        let text: serde_json::Value = serde_json::from_str(&result.content_json).unwrap();
        assert_eq!(text[0]["text"], "hi");
        assert!(grants.is_empty());
    }

    #[tokio::test]
    async fn non_mcp_invokables_are_a_precise_error() {
        // A registered user-invokable prompt: /! on it is the NotMcp
        // error, not the unknown-invokable one.
        let definitions = agentkit_tackle::loader::Definitions {
            prompts: BTreeMap::from([(
                "deploy".to_owned(),
                agentkit_tackle::loader::Definition {
                    name: "deploy".to_owned(),
                    path: std::path::PathBuf::from("/prompts/deploy.md"),
                    raw: String::new(),
                    frontmatter: agentkit_tackle::loader::PromptMeta::default(),
                    body: "Body.".to_owned(),
                },
            )]),
            ..Default::default()
        };
        let registry = Registry::build(&definitions, &[]).unwrap();
        let mut pool = McpPool::new();
        pool.connect("echo", &echo_server_config()).await;

        // A prompt is not an MCP tool — /! supports MCP tools only.
        let input = agentkit_tackle::invokables::DirectInvocation {
            invokable: "prompt.deploy".into(),
            arguments: String::new(),
        };
        let err = registry.execute_direct(&pool, &input).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("not an MCP tool — direct invocation supports MCP tools only"),
            "{err}"
        );

        // Unnamespaced or unknown names never execute.
        let input = parse_direct("/!echo").unwrap();
        let err = registry.execute_direct(&pool, &input).await.unwrap_err();
        assert!(err.to_string().contains("unknown invokable"), "{err}");
    }

    #[tokio::test]
    async fn oversized_outputs_truncate_with_the_original_size() {
        let definitions = agentkit_tackle::loader::Definitions::default();
        let registry = Registry::build(
            &definitions,
            &[agentkit_tackle::mcp::NamespacedTool {
                name: "mcp.echo.echo".into(),
                description: "Echoes the provided text".into(),
                schema: serde_json::json!({"type": "object"}),
            }],
        )
        .unwrap();
        let mut pool = McpPool::new();
        pool.connect("echo", &echo_server_config()).await;

        let huge = "x".repeat(agentkit_tackle::mcp::TOOL_RESULT_LIMIT + 1024);
        let input = parse_direct(&format!(
            "/!mcp.echo.echo {}",
            serde_json::json!({ "text": huge })
        ))
        .unwrap();
        let result = registry.execute_direct(&pool, &input).await.unwrap();

        assert!(result.truncated);
        assert_eq!(
            result.content_json.len(),
            agentkit_tackle::mcp::TOOL_RESULT_LIMIT
        );
        // The size marker records the original size — larger than the
        // limit; full content is what storage keeps.
        assert!(result.original_size > agentkit_tackle::mcp::TOOL_RESULT_LIMIT);
    }
}
