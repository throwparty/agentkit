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

/// (`kind`, `name`, raw contents) for every built-in definition.
pub fn defaults() -> impl Iterator<Item = (&'static str, &'static str, &'static str)> {
    [
        ("persona", "default", DEFAULT_PERSONA),
        ("actor", "default", DEFAULT_ACTOR),
    ]
    .into_iter()
}
