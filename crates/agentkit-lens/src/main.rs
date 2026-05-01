
use agentkit_lens::config;
use agentkit_lens::mcp;
use clap::{CommandFactory, Parser};

/// Lens MCP server for web search and URL fetching
#[derive(Parser)]
#[command(
    author,
    version,
    about = "Lens MCP server for web search and URL fetching",
    long_about = "Lens provides MCP tools for Brave Search and web content fetching.\n\
                  Search results are cached, and fetched content is converted to markdown."
)]
struct Cli {
    #[command(flatten)]
    config: config::Config,

    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Run the MCP (Model Control Protocol) server over stdio
    ///
    /// Starts the Lens MCP server, enabling communication with AI agents and tools
    /// that support the Model Control Protocol. The server uses standard input/output
    /// for communication.
    Stdio,

    /// Generate reference documentation
    #[command(subcommand, hide = true)]
    Docgen(DocgenCommand),
}

#[derive(clap::Subcommand)]
enum DocgenCommand {
    /// Generate CLI reference documentation
    Cli,

    /// Generate MCP tools reference documentation
    Mcp,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Stdio => {
            if let Err(error) = mcp::run_stdio(cli.config).await {
                let msg = error.to_string().to_lowercase();
                if msg.contains("cancelled") || msg.contains("shutdown") || msg.contains("closed") {
                    // Normal exit — transport closed cleanly
                    return;
                }
                eprintln!("MCP server error: {error}");
                std::process::exit(1);
            }
        }
        Commands::Docgen(kind) => {
            let content = match kind {
                DocgenCommand::Cli => agentkit_docgen::generate_cli_docs(&Cli::command()),
                DocgenCommand::Mcp => agentkit_docgen::generate_mcp_docs(mcp::TOOL_DOCS),
            };
            print!("{content}");
        }
    }
}
