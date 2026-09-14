use mcp_server::{serve_stdio, serve_stdio_empty};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--http") {
        let addr = args
            .iter()
            .position(|a| a == "--http")
            .and_then(|i| args.get(i + 1))
            .ok_or("--http requires an address")?;
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let (url, _ct) = mcp_server::serve_http(listener).await;
        println!("{url}");
        std::future::pending::<()>().await;
        Ok(())
    } else if args.iter().any(|a| a == "--no-tools") {
        serve_stdio_empty().await
    } else {
        serve_stdio().await
    }
}
