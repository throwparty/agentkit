use std::path::PathBuf;
use std::process::Stdio;

use tokio::process::Command;

/// Return a `tokio::process::Command` that spawns the shared `mcp-server`
/// binary with its stdio piped, ready to hand to any client transport.
///
/// Locates the compiled binary in the workspace target dir; the binary is
/// built ahead of time by `run-pocs.sh` (`cargo build --workspace`), never by
/// this library.
pub fn server_command() -> Command {
    let binary = mcp_server_binary();
    let mut cmd = Command::new(binary);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd
}

/// Return the path to the shared `mcp-server` binary.
pub fn server_program() -> PathBuf {
    mcp_server_binary()
}

/// Return the argv the shared `mcp-server` binary expects (none).
pub fn server_args() -> Vec<String> {
    Vec::new()
}

/// Locate the compiled `mcp-server` binary in the workspace target dir.
///
/// The binary is built ahead of time by `run-pocs.sh` (`cargo build
/// --workspace`); this library never invokes the build.
fn mcp_server_binary() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().expect("workspace parent");
    workspace_root.join("target").join("debug").join("mcp-server")
}
