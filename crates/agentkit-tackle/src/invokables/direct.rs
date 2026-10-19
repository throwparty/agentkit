//! Direct invocation: `/!<invokable>` parsing for user-typed input.

/// User-typed `/!name arguments` input, exact first-token match. Only
/// the ACP user-input path constructs these: never model output, seed
/// messages, script payloads, or script-sent prompts.
#[derive(Debug, Clone, PartialEq)]
pub struct DirectInvocation {
    /// The namespaced tool: `mcp.<server>.<tool>` or
    /// `client.<surface>.<method>`.
    pub invokable: String,
    /// The remainder of the line: the tool's JSON object arguments.
    pub arguments: String,
}

impl DirectInvocation {
    /// The arguments as a JSON object, or `Null` when none were given.
    /// Malformed or non-object input is a precise usage error.
    pub fn arguments_json(&self) -> Result<serde_json::Value, DirectError> {
        if self.arguments.trim().is_empty() {
            return Ok(serde_json::Value::Null);
        }
        let parsed: serde_json::Value = serde_json::from_str(&self.arguments)
            .map_err(|_| DirectError::Arguments(self.invokable.clone()))?;
        if !parsed.is_object() {
            return Err(DirectError::Arguments(self.invokable.clone()));
        }
        Ok(parsed)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DirectError {
    #[error("unknown invokable `{0}`")]
    Unknown(String),
    #[error(
        "`{0}` cannot be invoked directly — direct invocation supports MCP and client-capability tools"
    )]
    NotDirect(String),
    #[error("direct invocation arguments must be a JSON object: /!{0} {{...}}")]
    Arguments(String),
    #[error(transparent)]
    Pool(#[from] crate::mcp::McpPoolError),
}

/// Intercepts the `/!` prefix on user-typed input: exact first-token
/// match, the first token naming the invokable, the remainder its
/// arguments. Anything not starting with `/!` — pasted text, model
/// output mentioning the syntax — yields `None`.
pub fn parse_direct(input: &str) -> Option<DirectInvocation> {
    let rest = input.strip_prefix("/!")?.trim_start();
    // The invokable name runs to the first whitespace, or to the opening
    // brace when the client omitted the separating space. A tool name
    // cannot contain either, so both spellings parse to the same call.
    let boundary = rest
        .find(char::is_whitespace)
        .or_else(|| rest.find('{'))
        .unwrap_or(rest.len());
    let (invokable, arguments) = rest.split_at(boundary);
    if invokable.is_empty() {
        return None;
    }
    Some(DirectInvocation {
        invokable: invokable.to_owned(),
        arguments: arguments.trim().to_owned(),
    })
}
