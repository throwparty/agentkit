//! Shared credential-helper crate.
//!
//! Two roles share this crate:
//!
//! - **Server side** — the helper binaries ([`bin`] targets
//!   `agentkit-credential-keychain` and `agentkit-credential-file`) that
//!   store and return secrets. They use [`read_stdin`].
//! - **Consumer side** — [`consumer`], the client for the helper protocol:
//!   resolving a helper on `PATH`, invoking it, parsing its output, and
//!   parsing `{cred:IDENTITY}` markers. Components (tackle, switchboard)
//!   depend on this instead of each reimplementing it.

pub mod consumer;

pub fn read_stdin() -> Result<serde_json::Value, String> {
    let mut input = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)
        .map_err(|e| format!("failed to read stdin: {e}"))?;
    serde_json::from_str(&input).map_err(|e| format!("failed to parse JSON: {e}"))
}
