use std::process::ExitCode;

/// The Litterbox command line binary.
#[tokio::main]
async fn main() -> ExitCode {
    agentkit_litterbox::cli::run().await
}
