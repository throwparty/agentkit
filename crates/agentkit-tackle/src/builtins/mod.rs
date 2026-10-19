//! Built-in default assets.
//!
//! Shipped as embedded definitions registered at the lowest precedence
//! through the same discovery path as user definitions: any discovered
//! definition with the same name replaces the built-in. The shipped
//! scripts double as the customization examples.
//!
//! This module is declarations and re-exports only.

mod assets;

pub use assets::*;
