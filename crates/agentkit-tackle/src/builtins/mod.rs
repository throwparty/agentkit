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

/// The shipped compaction script: automatic compaction past the
/// utilisation threshold (post_turn) and the manual /compact summary
/// (compaction_requested).
pub const COMPACTION_SCRIPT: &str = include_str!("compaction.rhai");

/// The shipped /fork prompt: the fallback path for v1 clients — the
/// harness intercepts the command before this would expand.
pub const FORK_PROMPT: &str = include_str!("fork-prompt.md");

/// The shipped /compact prompt: compaction-tagged, intercepted by the
/// harness into a compaction_requested turn.
pub const COMPACT_PROMPT: &str = include_str!("compact-prompt.md");

/// Resolves a built-in script file reference (`builtin:<name>.rhai`) to
/// the shipped source.
pub fn script_source(file: &str) -> Option<&'static str> {
    match file.strip_prefix("builtin:")? {
        "compaction.rhai" => Some(COMPACTION_SCRIPT),
        "titling.rhai" => Some(TITLING_SCRIPT),
        _ => None,
    }
}

/// (`kind`, `name`, raw contents) for every built-in definition.
pub fn defaults() -> impl Iterator<Item = (&'static str, &'static str, &'static str)> {
    [
        ("persona", "default", DEFAULT_PERSONA),
        ("actor", "default", DEFAULT_ACTOR),
        ("actor", "summariser", SUMMARISER_ACTOR),
        ("prompt", "fork", FORK_PROMPT),
        ("prompt", "compact", COMPACT_PROMPT),
    ]
    .into_iter()
}
