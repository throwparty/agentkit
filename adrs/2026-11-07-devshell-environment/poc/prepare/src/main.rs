//! Prototype harness for the devshell-environment ADR.
//!
//! It imports nothing from the workspace and is not a workspace member. The
//! point is to measure what a step whose kind the contract does not name costs,
//! and what a backend had to be asked, not to become production code.

// The harness is built task by task, so a helper is reachable only once the
// task that calls it exists. Every one of them is used by the finished crate.
#![allow(dead_code)]
// A failure is built once per failed operation, never in a loop; boxing it
// would only add noise to every constructor and call site.
#![allow(clippy::result_large_err)]

mod checks;
mod closure;
mod failure;
mod model;
mod prepare;
mod util;

use failure::Failure;

fn main() {
    if let Err(failure) = run() {
        eprintln!("{failure}");
        std::process::exit(failure.class.exit_code());
    }
}

fn run() -> Result<(), Failure> {
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or_else(|| Failure::misconfigured(usage()))?;
    Err(Failure::misconfigured(format!(
        "unknown command: {command}\n{}",
        usage()
    )))
}

fn usage() -> String {
    "usage: devshell-poc <check|prepare|closure|run> --installable <ref> --out <dir>".to_string()
}
