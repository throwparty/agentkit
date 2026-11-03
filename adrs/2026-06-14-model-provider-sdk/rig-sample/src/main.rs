use clap::Parser;
use rig_core::completion::{Chat, Message};
use rig_core::prelude::*;
use rig_core::providers::openai;
use rig_sample::recording_client::RecordingClient;
use rig_sample::EchoTool;

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

    let recording = std::env::var("RECORDING").is_ok();
    let cassette_path = "tests/cassettes/echo_tool.json".to_string();
    let cassette_exists = std::path::Path::new(&cassette_path).exists();

    let client = if recording {
        let rec = RecordingClient::new(true, cassette_path);
        openai::Client::builder()
            .api_key(&api_key)
            .http_client(rec)
            .build()?
    } else if cassette_exists {
        let rec = RecordingClient::new(false, cassette_path);
        openai::Client::builder()
            .api_key(&api_key)
            .http_client(rec)
            .build()?
    } else {
        let rec = RecordingClient::new_passthrough(false, cassette_path);
        openai::Client::builder()
            .api_key(&api_key)
            .http_client(rec)
            .build()?
    };

    let agent = client
        .agent(&args.model)
        .preamble("You have access to an echo tool. Call it when asked.")
        .tool(EchoTool)
        .build();

    let mut history: Vec<Message> = Vec::new();

    let prompts = vec![
        &args.prompt,
        "What was the last thing I asked you to do?",
        "Call the echo tool with the word 'done'",
    ];

    for (i, prompt_text) in prompts.iter().enumerate() {
        println!("\n--- Turn {} ---", i + 1);
        println!("User: {}", prompt_text);

        match agent.chat(*prompt_text, &mut history).await {
            Ok(response) => {
                println!("Assistant: {}", response);
            }
            Err(e) => {
                println!("Error: {}", e);
            }
        }

        println!("History length: {} messages", history.len());
    }

    println!("\n--- Session Summary ---");
    println!("Total turns: {}", prompts.len());
    println!("Final history size: {} messages", history.len());

    Ok(())
}
