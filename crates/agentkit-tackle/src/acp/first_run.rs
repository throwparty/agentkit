//! First-run experience (T-030): session/new always succeeds with empty
//! configuration — the built-in defaults carry it — and the first
//! message of every new session documents the loaded configuration and
//! the `/` and `/!` syntaxes. Built-in overrides surface there.

use crate::acp::TackleState;

/// The seed message: harness-authored static user-facing content listing
/// the loaded configuration layers with counts, the overridden built-ins,
/// and the command syntaxes.
pub fn first_run_seed(state: &TackleState) -> String {
    let config = &state.config.config;
    let definitions = &state.definitions;
    let user_layer = state
        .config
        .user_layer
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "none (built-in defaults)".to_owned());
    let project_layer = state
        .config
        .project_layer
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "none".to_owned());

    let mut lines = Vec::new();
    lines.push("Welcome to tackle. Loaded configuration:".to_owned());
    lines.push(format!(
        "- User layer: {user_layer}; project layer: {project_layer}"
    ));
    lines.push(format!(
        "- {} persona(s), {} actor(s), {} prompt(s), {} script(s)",
        definitions.personas.len(),
        definitions.actors.len(),
        definitions.prompts.len(),
        config.scripts.len(),
    ));
    if !definitions.builtin_overrides.is_empty() {
        let overrides = definitions
            .builtin_overrides
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("- Built-in definitions overridden: {overrides}"));
    }
    lines.push(
        "Commands: /name [arguments] runs a prompt (the last parameter takes the \
         remainder); /!mcp.<server>.<tool> {json arguments} runs an MCP tool \
         directly with no model request and no permission ask."
            .to_owned(),
    );
    lines.join("\n")
}

/// Whether any configured endpoint's credential is unresolvable: those
/// endpoints surface via `authMethods` advertisement and the ACP
/// `authenticate` method instead of failing at first prompt.
pub fn missing_credentials(state: &TackleState) -> bool {
    use crate::config::Auth;
    for (name, endpoint) in &state.config.config.endpoints {
        if endpoint.auth != Auth::Helper {
            continue;
        }
        let Some(helper) = &state.config.config.credential_helper else {
            continue;
        };
        if crate::agent::provider::resolve_credential(helper, name).is_none() {
            return true;
        }
    }
    false
}
