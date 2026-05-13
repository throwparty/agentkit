mod config;
mod error;
mod jsonrpc;
mod handlers;
mod session;
mod transports;

#[tokio::main]
async fn main() {
    let config = config::Config::from_args();

    eprintln!("Starting ACP server with transport: {}", config.transport);
    eprintln!("Config: {:?}", config);

    match config.transport.as_str() {
        "stdio" => {
            eprintln!("Starting stdio transport");
            transports::stdio::run_stdio().await;
        }
        "http" => {
            eprintln!("Starting HTTP transport on {}:{}", config.bind, config.port);
            transports::http::run_http(config.bind, config.port).await;
        }
        _ => {
            std::process::exit(1);
        }
    }

    eprintln!("Server stopped");
}
