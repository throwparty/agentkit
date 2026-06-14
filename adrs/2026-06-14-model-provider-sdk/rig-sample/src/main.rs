use clap::Parser;
use rig_core::completion::Prompt;
use rig_core::prelude::*;
use rig_core::providers::openai;

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "gpt-5.4-mini")]
    model: String,
    #[arg(long, default_value = "Say hello")]
    prompt: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    let api_key = std::env::var("SWITCHBOARD_OPENAI_API_KEY")
        .or_else(|_| std::env::var("OPENAI_API_KEY"))
        .expect("SWITCHBOARD_OPENAI_API_KEY or OPENAI_API_KEY must be set");

    let client = openai::Client::builder()
        .api_key(&api_key)
        .build()?;
    let agent = client.agent(&args.model).build();
    let response = agent.prompt(&args.prompt).await?;
    println!("{}", response);

    Ok(())
}
