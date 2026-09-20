//! Built-in default assets.
//!
//! Shipped as embedded definitions registered at the lowest precedence
//! through the same discovery path as user definitions: any discovered
//! definition with the same name replaces the built-in. The shipped
//! scripts double as the customization examples (see T-030 for the full
//! inventory).

/// The built-in default persona, used by the default actor.
pub const DEFAULT_PERSONA: &str = include_str!("default-persona.md");

/// The built-in default actor: persona "default", model falling back to
/// `[defaults].model` from configuration.
pub const DEFAULT_ACTOR: &str = include_str!("default-actor.md");

/// The built-in summariser actor: the no-tools background actor the
/// shipped scripts fork as (titling, compaction summaries).
pub const SUMMARISER_ACTOR: &str = include_str!("summariser-actor.md");

/// The shipped titling script: ephemeral-fork summarisation into a
/// six-word title; failures silent.
pub const TITLING_SCRIPT: &str = include_str!("titling.rhai");

/// (`kind`, `name`, raw contents) for every built-in definition.
pub fn defaults() -> impl Iterator<Item = (&'static str, &'static str, &'static str)> {
    [
        ("persona", "default", DEFAULT_PERSONA),
        ("actor", "default", DEFAULT_ACTOR),
        ("actor", "summariser", SUMMARISER_ACTOR),
    ]
    .into_iter()
}
