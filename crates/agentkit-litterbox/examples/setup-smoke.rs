//! Manual setup-command smoke test.
//!
//! Run from a Git repository whose project config declares a setup command that
//! writes `/setup-marker`:
//!
//! ```shell
//! cargo run -p agentkit-litterbox --example setup-smoke -- setup-smoke
//! ```

use agentkit_litterbox::config_loader;
use agentkit_litterbox::orchestrator::Sandboxes;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "setup-smoke".to_string());

    let config = config_loader::load_final()?;
    let sandboxes = Sandboxes::open(config)?;
    sandboxes.create(&name).await?;

    let result = sandboxes
        .exec(&name, &["cat".to_string(), "/setup-marker".to_string()])
        .await;
    let deleted = sandboxes.delete(&name).await;

    let result = result?;
    deleted?;

    if result.exit_code != 0 {
        return Err(std::io::Error::other(format!(
            "reading /setup-marker failed with exit {}: {}",
            result.exit_code, result.stderr
        ))
        .into());
    }
    let marker = result.stdout.trim();
    if marker != "setup" {
        return Err(std::io::Error::other(format!(
            "/setup-marker contained {marker:?}, not \"setup\""
        ))
        .into());
    }
    println!("{marker}");
    Ok(())
}
