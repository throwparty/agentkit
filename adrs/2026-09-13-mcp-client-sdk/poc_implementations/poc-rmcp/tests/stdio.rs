use poc_rmcp::{list_tool_names, run_client, run_client_with_versions};

#[tokio::test]
async fn completes_handshake_lists_tools_and_calls_echo() {
    let report = run_client(mcp_server::server_command())
        .await
        .expect("client flow succeeds");

    assert_eq!(report.tool_count, 2, "echo and fail tools, got {report:?}");
    assert!(
        report
            .first_five_tool_names
            .iter()
            .any(|name| name == "echo"),
        "echo tool should be discovered, got {report:?}"
    );
    assert_eq!(report.echo_text, "Echo: Hello, MCP!");
}

#[tokio::test]
async fn surfaces_tool_descriptions_and_input_schemas() {
    let report = run_client(mcp_server::server_command())
        .await
        .expect("client flow succeeds");

    let echo_idx = report
        .first_five_tool_names
        .iter()
        .position(|name| name == "echo")
        .expect("echo tool should be discovered");

    assert_eq!(
        report.tool_descriptions[echo_idx],
        "Echo the provided message"
    );
    assert!(
        report.tool_input_schemas[echo_idx].contains("\"type\":\"object\""),
        "echo input schema should describe an object, got {:?}",
        report.tool_input_schemas[echo_idx]
    );
}

#[tokio::test]
async fn surfaces_tool_level_and_protocol_level_errors() {
    let report = run_client(mcp_server::server_command())
        .await
        .expect("client flow succeeds");

    // Tool-level error (isError result) is surfaced as text, not as an RPC error.
    assert_eq!(report.fail_tool_error, "fail tool always returns an error");
    // Protocol-level error (unknown tool) is surfaced as an error.
    assert!(
        !report.unknown_tool_error.is_empty(),
        "unknown tool should surface a protocol error, got {:?}",
        report.unknown_tool_error
    );
}

#[tokio::test]
async fn prints_fixed_output_shape() {
    let report = run_client(mcp_server::server_command())
        .await
        .expect("client flow succeeds");

    let shape = report.to_fixed_shape();
    assert!(shape.starts_with("handshake=complete\n"), "got {shape:?}");
    assert!(shape.contains("tool_count="), "missing tool_count line");
    assert!(
        shape.contains("first_five_tools="),
        "missing first_five_tools line"
    );
    assert!(
        shape.contains("tool_descriptions="),
        "missing tool_descriptions line"
    );
    assert!(
        shape.contains("tool_input_schemas="),
        "missing tool_input_schemas line"
    );
    assert!(shape.contains("echo_result="), "missing echo_result line");
    assert!(
        shape.contains("fail_tool_error="),
        "missing fail_tool_error line"
    );
    assert!(
        shape.contains("unknown_tool_error="),
        "missing unknown_tool_error line"
    );
}

#[tokio::test]
async fn spawns_server_with_explicit_argv_and_no_shell() {
    // `run_client` configures the child on its own; the shared server is
    // spawned with explicit argv (no shell), so a would-be shell metacharacter
    // in an extra argument must not be interpreted.
    let mut server = mcp_server::server_command();
    server.arg("arg-with;$>&-chars");

    let report = run_client(server)
        .await
        .expect("client flow succeeds with extra argv");
    assert_eq!(report.echo_text, "Echo: Hello, MCP!");
}

// EC-001: a spawned server that exits immediately must surface as an error.
#[tokio::test]
async fn surfaces_error_when_server_exits() {
    let server = tokio::process::Command::new("true");
    let result = run_client(server).await;
    assert!(result.is_err(), "server exit should surface as an error");
}

// EC-002: negotiating an unsupported protocol version must fail the handshake.
#[tokio::test]
async fn fails_handshake_on_unsupported_version() {
    let unsupported: rmcp::model::ProtocolVersion =
        serde_json::from_str("\"1999-01-01\"").expect("valid protocol version string");
    let result = run_client_with_versions(mcp_server::server_command(), vec![unsupported]).await;
    assert!(
        result.is_err(),
        "unsupported version should fail the handshake"
    );
}

// EC-003: a server with no tools returns an empty list and does not panic.
#[tokio::test]
async fn returns_empty_tool_list_and_does_not_panic() {
    let mut server = mcp_server::server_command();
    server.arg("--no-tools");

    let names = list_tool_names(server)
        .await
        .expect("listing tools on an empty server should not panic");
    assert!(names.is_empty(), "expected no tools, got {names:?}");
}
