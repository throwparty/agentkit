//! The v2 command surface: the registry view over the loaded definitions,
//! rendered with v2's `type`-discriminated command input.

use crate::state::TackleState;
use agent_client_protocol::schema::v1::AvailableCommandInput;
use agent_client_protocol::schema::v2::{
    AvailableCommand, AvailableCommandInput as WireInput, TextCommandInput,
};

/// The `available_commands` list for a session: the `!` prefix command plus
/// every user-invokable prompt, mapped onto v2's text input shape.
pub(super) fn available_commands(state: &TackleState) -> Vec<AvailableCommand> {
    let registry = match crate::invokables::Registry::build_with_client_tools(
        &state.definitions(),
        &[],
        &[],
    ) {
        Ok(registry) => registry,
        Err(err) => {
            tracing::warn!("registry build failed for v2 available commands: {err}");
            crate::invokables::Registry::default()
        }
    };
    registry
        .available_commands()
        .into_iter()
        // `/editor` toggles the v1 client-tool surface; v2 has none.
        .filter(|command| command.name != "editor")
        .map(|command| {
            let hint = match command.input.as_ref() {
                Some(AvailableCommandInput::Unstructured(input)) => {
                    if command.name == "!" {
                        // v2 has no client tools, so the v1 hint would be a lie.
                        "mcp.<server>.<tool> {json arguments}".to_owned()
                    } else {
                        input.hint.clone()
                    }
                }
                _ => String::new(),
            };
            let mut out = AvailableCommand::new(command.name, command.description);
            if !hint.is_empty() {
                out = out.input(WireInput::Text(TextCommandInput::new(hint)));
            }
            out
        })
        .collect()
}
