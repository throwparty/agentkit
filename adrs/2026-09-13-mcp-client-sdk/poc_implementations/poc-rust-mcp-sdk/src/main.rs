use poc_rust_mcp_sdk::run_client;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let report = run_client().await?;
    print!("{}", report.to_fixed_shape());

    Ok(())
}
