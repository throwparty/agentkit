//! Resolving a project's provisioning definitions for the selected backend.

mod resolve;

pub use resolve::{ResolvedStep, StepOutcome, preflight_steps, resolve};
