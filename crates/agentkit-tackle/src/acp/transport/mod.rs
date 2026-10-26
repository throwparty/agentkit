//! Version-neutral transports: stdio and HTTP.
//!
//! Both hand the connection to the version router; neither knows which
//! protocol version the client will negotiate.

mod http;
mod stdio;

pub use http::run_http;
pub use stdio::run_stdio;
