use clap::Parser;
use rig_core::completion::{Chat, Message, ToolDefinition};
use rig_core::prelude::*;
use rig_core::providers::openai;
use rig_core::tool::Tool;
use serde::{Deserialize, Serialize};

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "gpt-5.4-mini")]
    model: String,
    #[arg(long, default_value = "Say hello")]
    prompt: String,
}

#[derive(Deserialize)]
struct EchoArgs {
    message: String,
}

#[derive(Serialize)]
struct EchoOutput {
    result: String,
}

struct EchoTool;

impl Tool for EchoTool {
    const NAME: &'static str = "echo";
    type Error = std::convert::Infallible;
    type Args = EchoArgs;
    type Output = EchoOutput;

    fn definition(&self, _prompt: String) -> impl std::future::Future<Output = ToolDefinition> {
        async {
            ToolDefinition {
                name: "echo".to_string(),
                description: "Echoes the input arguments back as a result".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "message": { "type": "string" }
                    },
                    "required": ["message"]
                }),
            }
        }
    }

    async fn call(&self, args: EchoArgs) -> Result<EchoOutput, Self::Error> {
        Ok(EchoOutput { result: args.message })
    }
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
    let agent = client.agent(&args.model)
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
