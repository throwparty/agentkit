use agentkit_lens::config;
use agentkit_lens::mcp;
use clap::Parser;
use tracing_subscriber::fmt;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    if let Err(error) = mcp::run_stdio(cfg).await {
        let msg = error.to_string().to_lowercase();
        if msg.contains("cancelled") || msg.contains("shutdown") || msg.contains("closed") {
            // Normal exit — transport closed cleanly
            return;
        }
        eprintln!("MCP server error: {error}");
        std::process::exit(1);
    }
}
