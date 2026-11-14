//! The Seatbelt helper: apply a profile to a child and hold the sandbox.
//!
//! This binary is macOS only. It links `libc`, `policy` and `protocol` and
//! nothing from the server runtime. The implementation is gated as a whole in
//! `macos.rs`, so a build on another target still produces the binary.

#![allow(dead_code)]

#[path = "macos.rs"]
mod macos;

#[cfg(target_os = "macos")]
fn main() {
    std::process::exit(macos::run());
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("litterbox-seatbelt-helper is macOS only");
    std::process::exit(1);
}
