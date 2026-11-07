//! Resolving a project's provisioning definitions for the selected backend.

pub mod kind;
mod resolve;

pub use resolve::{ResolvedStep, StepOutcome, preflight_steps, resolve};
