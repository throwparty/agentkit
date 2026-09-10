use async_trait::async_trait;
use pmcp::types::capabilities::ServerCapabilities;
use pmcp::{Server, ToolHandler};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize)]
struct CalculatorArgs {
    operation: String,
    a: f64,
    b: f64,
}

#[derive(Debug, Serialize)]
struct CalculatorResult {
    result: f64,
    expression: String,
}

struct CalculatorTool;

#[async_trait]
impl ToolHandler for CalculatorTool {
    async fn handle(&self, args: Value, _extra: pmcp::RequestHandlerExtra) -> pmcp::Result<Value> {
        let params: CalculatorArgs = serde_json::from_value(args)
            .map_err(|e| pmcp::Error::validation(format!("Invalid arguments: {}", e)))?;

        let result = match params.operation.as_str() {
            "add" => params.a + params.b,
            _ => return Err(pmcp::Error::validation("Unknown operation")),
        };

        Ok(serde_json::to_value(CalculatorResult {
            result,
            expression: format!(
                "{} {} {} = {}",
                params.a, params.operation, params.b, result
            ),
        })?)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let server = Server::builder()
        .name("test-server")
        .version("1.0.0")
        .capabilities(ServerCapabilities::tools_only())
        .tool("calculator", CalculatorTool)
        .build()?;

    server.run_stdio().await?;
    Ok(())
}
