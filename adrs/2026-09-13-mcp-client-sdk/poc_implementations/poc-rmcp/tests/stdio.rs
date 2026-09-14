use poc_rmcp::run_client;

#[tokio::test]
async fn completes_handshake_lists_tools_and_calls_echo() {
    let report = run_client(mcp_server::server_command())
        .await
        .expect("client flow succeeds");

    assert!(report.tool_count > 0, "tool count should be non-zero");
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
async fn prints_fixed_output_shape() {
    let report = run_client(mcp_server::server_command())
        .await
        .expect("client flow succeeds");

    let shape = report.to_fixed_shape();
    assert!(shape.starts_with("handshake=complete\n"), "got {shape:?}");
    assert!(
        shape.contains("tool_count="),
        "missing tool_count line in {shape:?}"
    );
    assert!(
        shape.contains("first_five_tools="),
        "missing first_five_tools line in {shape:?}"
    );
    assert!(
        shape.contains("echo_result="),
        "missing echo_result line in {shape:?}"
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
