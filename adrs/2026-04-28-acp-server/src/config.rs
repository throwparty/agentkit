use clap::Parser;

#[derive(Parser, Debug, Clone)]
#[command(name = "acp-server", version, about = "Agent Communication Protocol server")]
pub struct Config {
    /// Transport to use (stdio or http)
    #[arg(short, long, default_value = "http")]
    pub transport: String,

    /// Bind address for HTTP transport
    #[arg(short, long, default_value = "127.0.0.1")]
    pub bind: String,

    /// Port for HTTP transport
    #[arg(short, long, default_value = "7860")]
    pub port: u16,
}

impl Config {
    pub fn from_args() -> Self {
        Config::parse()
    }
}
