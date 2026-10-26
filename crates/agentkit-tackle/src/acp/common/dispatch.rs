//! The slash-command dispatch decision, shared by the version modules.
//!
//! Deciding *which* prompt case applies is protocol-neutral; only rendering
//! the result is version-specific, so both versions call this and must not
//! grow their own ladder.

/// What the first user-message text asks Tackle to do.
pub(crate) enum Dispatch {
    /// The built-in `/fork` fallback.
    Fork,
    /// A `/!` direct invocation, already parsed.
    Direct(crate::invokables::DirectInvocation),
    /// A `/mcp` or `/editor` runtime availability command.
    RuntimeCommand,
    /// The `/context` composition report.
    Context,
    /// A compaction-tagged prompt.
    Compaction,
    /// Nothing intercepted: run the model turn.
    Model,
}

/// Classifies the first user-message text. `is_compaction` is the caller's
/// already-resolved verdict for a compaction-tagged prompt, since that needs
/// the loaded definitions.
pub(crate) fn decide(first_text: &str, is_compaction: bool) -> Dispatch {
    // Exact first-token match on user-typed input, never a substring.
    if first_text.split_whitespace().next() == Some("/fork") {
        return Dispatch::Fork;
    }
    if let Some(input) = crate::invokables::parse_direct(first_text) {
        return Dispatch::Direct(input);
    }
    if first_text == "/mcp"
        || first_text.starts_with("/mcp ")
        || first_text == "/editor"
        || first_text.starts_with("/editor ")
    {
        return Dispatch::RuntimeCommand;
    }
    if first_text == "/context" {
        return Dispatch::Context;
    }
    if is_compaction {
        return Dispatch::Compaction;
    }
    Dispatch::Model
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_first_token_commands_are_classified() {
        assert!(matches!(decide("/fork", false), Dispatch::Fork));
        assert!(matches!(decide("/fork now", false), Dispatch::Fork));
        assert!(matches!(decide("/forkfoo", false), Dispatch::Model));
        assert!(matches!(decide("/!mcp.echo.echo {}", false), Dispatch::Direct(_)));
        assert!(matches!(decide("/mcp status", false), Dispatch::RuntimeCommand));
        assert!(matches!(decide("/editor fs true", false), Dispatch::RuntimeCommand));
        assert!(matches!(decide("/context", false), Dispatch::Context));
        assert!(matches!(decide("/compact", true), Dispatch::Compaction));
        assert!(matches!(decide("/compact", false), Dispatch::Model));
        assert!(matches!(decide("hello", false), Dispatch::Model));
    }

    #[test]
    fn command_lookalikes_are_not_intercepted() {
        assert!(matches!(decide("/contextx", false), Dispatch::Model));
        assert!(matches!(decide("/mcpx", false), Dispatch::Model));
        assert!(matches!(decide("look at /fork", false), Dispatch::Model));
    }
}
