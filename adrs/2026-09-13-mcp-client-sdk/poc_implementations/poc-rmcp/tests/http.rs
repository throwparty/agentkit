use mcp_server::serve_http;
use poc_rmcp::run_client_http;

#[tokio::test]
async fn connects_over_streamable_http_and_calls_echo() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let (url, ct) = serve_http(listener).await;

    let report = run_client_http(&url)
        .await
        .expect("http client flow succeeds");

    assert!(report.tool_count > 0, "tool count should be non-zero");
    assert!(
        report
            .first_five_tool_names
            .iter()
            .any(|name| name == "echo"),
        "echo tool should be discovered, got {report:?}"
    );
    assert_eq!(report.echo_text, "Echo: Hello, MCP!");

    ct.cancel();
}

// EC-004: a remote streamable HTTP server that is unreachable must surface an
// error (rather than hang or misbehave).
#[tokio::test]
async fn surfaces_error_when_http_server_is_unreachable() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local addr");
    drop(listener);

    let url = format!("http://{addr}/mcp");
    let result = run_client_http(&url).await;
    assert!(
        result.is_err(),
        "unreachable HTTP server should surface an error"
    );
}
