//! The embedded default assets and their lookup.

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

/// The shipped /mcp prompt: the discoverability stub for the harness
/// command that toggles configured MCP server availability. The prompt
/// body is never expanded — the harness intercepts the command — but
/// without a registered prompt the command is not advertised to the
/// client, and a command no client offers is one no user finds.
pub const MCP_PROMPT: &str = include_str!("mcp-prompt.md");

/// The shipped /editor prompt: the discoverability stub for the harness
/// command that toggles the editor's file-system and terminal tools. Like
/// /mcp, the body is never expanded — the harness intercepts the command.
pub const EDITOR_PROMPT: &str = include_str!("editor-prompt.md");

/// The shipped /context prompt: the discoverability stub for the
/// ephemeral context-composition report. The body is never expanded.
pub const CONTEXT_PROMPT: &str = include_str!("context-prompt.md");

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
        ("prompt", "mcp", MCP_PROMPT),
        ("prompt", "editor", EDITOR_PROMPT),
        ("prompt", "context", CONTEXT_PROMPT),
    ]
    .into_iter()
}
