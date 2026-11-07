//! Resolving a project's provisioning definitions for the selected backend.

pub mod kind;
pub mod prepare;
mod resolve;

pub use resolve::{PlannedStep, StepOutcome, preflight_steps, resolve};
