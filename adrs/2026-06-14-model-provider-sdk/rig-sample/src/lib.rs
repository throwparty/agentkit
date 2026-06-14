pub mod recording_client;

use rig_core::completion::ToolDefinition;
use rig_core::tool::Tool;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct EchoArgs {
    pub message: String,
}

#[derive(Serialize)]
pub struct EchoOutput {
    pub result: String,
}

pub struct EchoTool;

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
