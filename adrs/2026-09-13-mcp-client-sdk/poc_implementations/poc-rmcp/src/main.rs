use poc_rmcp::run_client;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let report = run_client(mcp_server::server_command()).await?;
    print!("{}", report.to_fixed_shape());

    Ok(())
}
