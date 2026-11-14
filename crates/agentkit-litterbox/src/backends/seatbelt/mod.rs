//! Seatbelt as a sandboxing mechanism.
//!
//! The backend is compiled on macOS only. The helper binary applies an SBPL
//! profile to a forked child and execs; the server never confines itself.

// The module is built in stages; the registry wires it up in T-017.
#![allow(dead_code)]

/// The backend's own name in records and reports.
pub(crate) const BACKEND: &str = "seatbelt";

pub(crate) mod backend;
pub(crate) mod config;
pub(crate) mod construct;
pub(crate) mod defaults;
pub(crate) mod environment;
pub(crate) mod holder;
pub(crate) mod host;
pub(crate) mod mounts;
pub(crate) mod path;
pub(crate) mod payload;
pub(crate) mod policy;
pub(crate) mod protocol;
pub(crate) mod workspace;
