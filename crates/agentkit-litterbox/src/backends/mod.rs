//! The sandboxing mechanisms this binary can run.
//!
//! The set is compiled in: `backends/mod.rs` is the only place that knows which
//! mechanisms exist.

pub mod docker;
