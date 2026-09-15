use poc_rust_mcp_sdk::run_client;

#[tokio::test]
async fn completes_handshake_lists_tools_and_calls_echo() {
    let report = run_client().await.expect("client flow succeeds");

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
    let report = run_client().await.expect("client flow succeeds");

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
    let report = run_client().await.expect("client flow succeeds");

    assert_eq!(report.fail_tool_error, "fail tool always returns an error");
    assert!(
        !report.unknown_tool_error.is_empty(),
        "unknown tool should surface a protocol error, got {:?}",
        report.unknown_tool_error
    );
}

#[tokio::test]
async fn prints_fixed_output_shape() {
    let report = run_client().await.expect("client flow succeeds");

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
