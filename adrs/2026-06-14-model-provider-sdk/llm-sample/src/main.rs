use clap::Parser;
use llm::builder::{LLMBackend, LLMBuilder};
use llm::chat::ChatMessage;
use llm_sample::{
    build_assistant_message, build_tool_result, build_tool_use, build_user_message,
    echo_function_builder, execute_echo,
};

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "gpt-5.4-mini")]
    model: String,
    #[arg(long, default_value = "Say hello")]
    prompt: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let api_key = std::env::var("SWITCHBOARD_OPENAI_API_KEY")
        .or_else(|_| std::env::var("OPENAI_API_KEY"))
        .expect("SWITCHBOARD_OPENAI_API_KEY or OPENAI_API_KEY must be set");

    let provider = LLMBuilder::new()
        .backend(LLMBackend::OpenAI)
        .api_key(&api_key)
        .model(&args.model)
        .system("You have access to an echo tool. Call it when asked.")
        .function(echo_function_builder())
        .build()?;

    let mut history: Vec<ChatMessage> = vec![];

    let prompts = vec![
        &args.prompt,
        "What was the last thing I asked you to do?",
        "Call the echo tool with the word 'done'",
    ];

    for (i, prompt_text) in prompts.iter().enumerate() {
        println!("\n--- Turn {} ---", i + 1);
        println!("User: {}", prompt_text);

        history.push(build_user_message(prompt_text));

        loop {
            let response = provider.chat_with_tools(&history, None).await?;

            if let Some(tool_calls) = response.tool_calls() {
                history.push(build_tool_use(tool_calls.clone()));

                for tc in &tool_calls {
                    if tc.function.name == "echo" {
                        let result = execute_echo(&tc.function.arguments);
                        history.push(build_tool_result(tc, result));
                    }
                }
            } else if let Some(text) = response.text() {
                println!("Assistant: {}", text);
                history.push(build_assistant_message(&text));
                break;
            }
        }

        println!("History length: {} messages", history.len());
    }

    println!("\n--- Session Summary ---");
    println!("Total turns: {}", prompts.len());
    println!("Final history size: {} messages", history.len());

    Ok(())
}
