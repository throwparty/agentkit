use poc_rmcp::{run_client, run_client_http};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if let Some(pos) = args.iter().position(|a| a == "--http") {
        let uri = args.get(pos + 1).ok_or("--http requires a server URI")?;
        let report = run_client_http(uri).await?;
        print!("{}", report.to_fixed_shape());
    } else {
        let report = run_client(mcp_server::server_command()).await?;
        print!("{}", report.to_fixed_shape());
    }

    Ok(())
}
