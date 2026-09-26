//! The unified invokable registry: every dispatchable operation is
//! namespaced — `mcp.<server>.<tool>`, `prompt.<name>`, `agent.<actor>` —
//! with user-invokable and model-invokable visibility. The registry is
//! the only dispatch point: the ACP command surface and the model's tool
//! exposure are views over it, and the harness ships no built-in tools
//! beyond it.
//!
//! Namespacing makes cross-mechanism collisions impossible. Within a
//! namespace, layer precedence (project over user) resolves at load; a
//! residual same-layer collision is a configuration error, never a
//! silent shadow.

use crate::loader::Definitions;
use crate::mcp::{ElicitationSink, McpPool, NamespacedTool};
use agent_client_protocol::schema::v1::{
    AvailableCommand, AvailableCommandInput, UnstructuredCommandInput,
};
use std::collections::BTreeMap;

/// `mcp.<server>.<tool>`.
pub fn mcp_name(server: &str, tool: &str) -> String {
    format!("mcp.{server}.{tool}")
}

/// `prompt.<name>`.
pub fn prompt_name(name: &str) -> String {
    format!("prompt.{name}")
}

/// `agent.<name>`.
pub fn agent_name(name: &str) -> String {
    format!("agent.{name}")
}

#[derive(Debug, Clone, PartialEq)]
pub enum Invokable {
    /// An MCP tool: user-invokable (direct invocation) and
    /// model-invokable.
    Mcp {
        name: String,
        server: String,
        tool: String,
        description: String,
        schema: serde_json::Value,
    },
    /// A reusable prompt: visibility from its frontmatter. `parameters`
    /// are positional (`{{ name }}` placeholders, the last taking the
    /// remainder of the arguments).
    Prompt {
        name: String,
        description: String,
        parameters: Vec<String>,
        body: String,
        compaction: bool,
        user_invokable: bool,
        model_invokable: bool,
    },
    /// An actor invokable: model-invokable only — nested agent loops
    /// spawn from model tool calls, never user slash commands.
    Agent { name: String, description: String },
}

impl Invokable {
    /// The namespaced invokable name.
    pub fn name(&self) -> &str {
        match self {
            Invokable::Mcp { name, .. }
            | Invokable::Prompt { name, .. }
            | Invokable::Agent { name, .. } => name,
        }
    }

    pub fn user_invokable(&self) -> bool {
        match self {
            Invokable::Mcp { .. } => true,
            Invokable::Prompt { user_invokable, .. } => *user_invokable,
            Invokable::Agent { .. } => false,
        }
    }

    pub fn model_invokable(&self) -> bool {
        match self {
            Invokable::Mcp { .. } => true,
            Invokable::Prompt {
                model_invokable, ..
            } => *model_invokable,
            Invokable::Agent { .. } => true,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("invokable collision in the `{namespace}` namespace: `{name}` is declared twice")]
    Collision {
        namespace: &'static str,
        name: String,
    },
}

/// A prompt expansion failure: precise, carrying the usage string for
/// the JSON-RPC error. No state is ever stored.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ExpansionError {
    #[error("unknown command `{0}`")]
    Unknown(String),
    #[error("`{invokable}` is not user-invokable")]
    NotUserInvokable { invokable: String },
    #[error("{invokable} is not model-invokable")]
    NotModelInvokable { invokable: String },
    #[error("{usage}")]
    Arity { usage: String },
}

/// The usage string for a prompt: the slash command (the bare prompt
/// name) and its declared parameters.
fn usage_for(name: &str, parameters: &[String]) -> String {
    let bare = name.strip_prefix("prompt.").unwrap_or(name);
    if parameters.is_empty() {
        format!("usage: /{bare}")
    } else {
        format!(
            "usage: /{bare} {}",
            parameters
                .iter()
                .map(|parameter| format!("<{parameter}>"))
                .collect::<Vec<_>>()
                .join(" ")
        )
    }
}

/// Substitutes `{{ name }}` placeholders (inner whitespace tolerated)
/// with the filled parameters' values; unknown placeholders are left
/// as written.
fn substitute(body: &str, filled: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                let key = after[..end].trim();
                match filled.get(key) {
                    Some(value) => out.push_str(value),
                    None => out.push_str(&rest[start..start + 2 + end + 2]),
                }
                rest = &after[end + 2..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Fills the declared parameters positionally: each parameter takes the
/// next whitespace token, except the last, which takes the remainder of
/// the arguments.
fn fill_positional(
    name: &str,
    parameters: &[String],
    arguments: &str,
) -> Result<BTreeMap<String, String>, ExpansionError> {
    let usage = usage_for(name, parameters);
    if parameters.is_empty() {
        if arguments.trim().is_empty() {
            return Ok(BTreeMap::new());
        }
        // A no-parameter prompt takes no input: an arity mismatch.
        return Err(ExpansionError::Arity { usage });
    }

    let arguments = arguments.trim();

    // Each of the first n-1 parameters takes the next whitespace
    // token; the last takes the remainder of the arguments.
    let head_count = parameters.len() - 1;
    let mut filled = BTreeMap::new();
    let mut consumed = 0usize;
    for (parameter, token) in parameters[..head_count]
        .iter()
        .zip(arguments.split_whitespace())
    {
        filled.insert(parameter.clone(), token.to_owned());
        consumed += token.len() + 1; // one separator
    }
    if filled.len() < head_count {
        return Err(ExpansionError::Arity { usage });
    }
    let last = parameters.last().unwrap();
    let remainder = arguments.get(consumed..).unwrap_or("").trim().to_owned();
    // Declared parameters are required: an empty remainder means the
    // arguments stopped short — an arity mismatch, not an empty fill.
    if remainder.is_empty() {
        return Err(ExpansionError::Arity { usage });
    }
    filled.insert(last.clone(), remainder);
    Ok(filled)
}

impl Registry {
    /// `/name arguments` expansion: positional filling of the declared
    /// parameters, `{{ name }}` substitution, body for the model. Pure —
    /// unknown commands and arity mismatches fail with the usage string
    /// and store nothing.
    pub fn expand_user(&self, name: &str, arguments: &str) -> Result<String, ExpansionError> {
        let invokable = self
            .resolve(&prompt_name(name))
            .ok_or_else(|| ExpansionError::Unknown(name.to_owned()))?;
        let Invokable::Prompt {
            name,
            parameters,
            body,
            user_invokable,
            ..
        } = invokable
        else {
            return Err(ExpansionError::Unknown(name.to_owned()));
        };
        if !user_invokable {
            return Err(ExpansionError::NotUserInvokable {
                invokable: name.clone(),
            });
        }
        let filled = fill_positional(name, parameters, arguments)?;
        Ok(substitute(body, &filled))
    }

    /// A model-invokable prompt invoked as a tool: the arguments object
    /// names the parameters, the expanded body loads into context. Pure
    /// — errors carry the usage string and store nothing.
    pub fn expand_model(
        &self,
        invokable_name: &str,
        arguments: &serde_json::Value,
    ) -> Result<String, ExpansionError> {
        let invokable = self
            .resolve(invokable_name)
            .ok_or_else(|| ExpansionError::Unknown(invokable_name.to_owned()))?;
        let Invokable::Prompt {
            name,
            parameters,
            body,
            model_invokable,
            ..
        } = invokable
        else {
            return Err(ExpansionError::Unknown(invokable_name.to_owned()));
        };
        if !model_invokable {
            return Err(ExpansionError::NotModelInvokable {
                invokable: name.clone(),
            });
        }
        let usage = usage_for(name, parameters);
        let object = arguments.as_object().ok_or_else(|| ExpansionError::Arity {
            usage: usage.clone(),
        })?;
        let mut filled = BTreeMap::new();
        for parameter in parameters {
            match object.get(parameter).and_then(|value| value.as_str()) {
                Some(value) => {
                    filled.insert(parameter.clone(), value.to_owned());
                }
                None => return Err(ExpansionError::Arity { usage }),
            }
        }
        for key in object.keys() {
            if !parameters.contains(key) {
                return Err(ExpansionError::Arity { usage });
            }
        }
        Ok(substitute(body, &filled))
    }
}

/// The single dispatch point: namespaced invokables with visibility.
#[derive(Debug, Default, Clone)]
pub struct Registry {
    entries: BTreeMap<String, Invokable>,
}

impl Registry {
    /// Builds the registry from loaded definitions and the MCP pool's
    /// namespaced tool list.
    pub fn build(
        definitions: &Definitions,
        mcp_tools: &[NamespacedTool],
    ) -> Result<Self, RegistryError> {
        let mut registry = Self::default();

        for tool in mcp_tools {
            let (server, tool_name) = tool
                .name
                .strip_prefix("mcp.")
                .and_then(|rest| rest.split_once('.'))
                .map(|(server, tool)| (server.to_owned(), tool.to_owned()))
                .unwrap_or_default();
            registry.insert(
                "mcp",
                tool.name.clone(),
                Invokable::Mcp {
                    name: tool.name.clone(),
                    server,
                    tool: tool_name,
                    description: tool.description.clone(),
                    schema: tool.schema.clone(),
                },
            )?;
        }

        for (name, def) in &definitions.prompts {
            registry.insert(
                "prompt",
                prompt_name(name),
                Invokable::Prompt {
                    name: prompt_name(name),
                    description: def.frontmatter.description.clone().unwrap_or_default(),
                    parameters: def.frontmatter.parameters.clone(),
                    body: def.body.clone(),
                    compaction: def.frontmatter.compaction,
                    user_invokable: def.frontmatter.user_invokable,
                    model_invokable: def.frontmatter.model_invokable,
                },
            )?;
        }

        for (name, def) in &definitions.actors {
            registry.insert(
                "agent",
                agent_name(name),
                Invokable::Agent {
                    name: agent_name(name),
                    description: def.frontmatter.persona.clone(),
                },
            )?;
        }

        Ok(registry)
    }

    /// The registration point: same-name re-registration within a
    /// namespace is a configuration error, never a silent shadow.
    fn insert(
        &mut self,
        namespace: &'static str,
        name: String,
        invokable: Invokable,
    ) -> Result<(), RegistryError> {
        if self.entries.insert(name.clone(), invokable).is_some() {
            return Err(RegistryError::Collision { namespace, name });
        }
        Ok(())
    }

    /// The dispatch point: resolves a namespaced invokable by name.
    pub fn resolve(&self, name: &str) -> Option<&Invokable> {
        self.entries.get(name)
    }

    /// The model's tool exposure: every model-invokable invokable.
    pub fn model_invokables(&self) -> impl Iterator<Item = &Invokable> {
        self.entries
            .values()
            .filter(|invokable| invokable.model_invokable())
    }

    /// The ACP command advertisement: the `!` prefix command (clients
    /// autocomplete `/!`) followed by the user-invokable prompts, with
    /// the unstructured input hint mapped from the prompt's declared
    /// parameters.
    pub fn available_commands(&self) -> Vec<AvailableCommand> {
        let mut commands =
            vec![
                AvailableCommand::new("!", "Execute an MCP tool directly with no model request")
                    .input(AvailableCommandInput::Unstructured(
                        UnstructuredCommandInput::new("mcp.<server>.<tool> {json arguments}"),
                    )),
            ];

        for invokable in self.entries.values() {
            if !invokable.user_invokable() {
                continue;
            }
            if let Invokable::Prompt {
                name,
                description,
                parameters,
                ..
            } = invokable
            {
                // The user types /<name>; the hint previews the
                // positional parameters the expansion will fill.
                let name = name.strip_prefix("prompt.").unwrap_or(name).to_owned();
                let mut command = AvailableCommand::new(name, description.clone());
                if !parameters.is_empty() {
                    let hint = parameters
                        .iter()
                        .map(|parameter| format!("<{parameter}>"))
                        .collect::<Vec<_>>()
                        .join(" ");
                    command = command.input(AvailableCommandInput::Unstructured(
                        UnstructuredCommandInput::new(hint),
                    ));
                }
                commands.push(command);
            }
        }
        commands
    }

    /// `/!name execution of MCP tools with no model request`, bypassing
    /// the permission pipeline — the user is the authority. Oversized
    /// outputs arrive truncated at the fixed internal limit with the
    /// original size recorded; storage keeps the full content.
    pub async fn execute_direct<S: ElicitationSink>(
        &self,
        pool: &McpPool<S>,
        input: &DirectInvocation,
    ) -> Result<crate::mcp::ToolResult, DirectError> {
        let invokable = self
            .resolve(&input.invokable)
            .ok_or_else(|| DirectError::Unknown(input.invokable.clone()))?;
        let invokable_name = invokable.name().to_owned();
        let Invokable::Mcp { server, tool, .. } = invokable else {
            // /! on a non-MCP invokable is a precise error — direct
            // invocation supports MCP tools only.
            return Err(DirectError::NotMcp(invokable_name));
        };

        let arguments = if input.arguments.trim().is_empty() {
            serde_json::Value::Null
        } else {
            let parsed: serde_json::Value = serde_json::from_str(&input.arguments)
                .map_err(|_| DirectError::Arguments(input.invokable.clone()))?;
            if !parsed.is_object() {
                return Err(DirectError::Arguments(input.invokable.clone()));
            }
            parsed
        };

        // The tool-call span: arguments redacted to their size.
        let _call_guard = crate::telemetry::tool_call_span(&input.invokable, input.arguments.len());
        pool.call_tool(server, tool, arguments)
            .await
            .map_err(DirectError::Pool)
    }
}

/// User-typed `/!name arguments` input, exact first-token match. Only
/// the ACP user-input path constructs these: never model output, seed
/// messages, script payloads, or script-sent prompts.
#[derive(Debug, Clone, PartialEq)]
pub struct DirectInvocation {
    /// The namespaced MCP tool: `mcp.<server>.<tool>`.
    pub invokable: String,
    /// The remainder of the line: the tool's JSON object arguments.
    pub arguments: String,
}

#[derive(Debug, thiserror::Error)]
pub enum DirectError {
    #[error("unknown invokable `{0}`")]
    Unknown(String),
    #[error("`{0}` is not an MCP tool — direct invocation supports MCP tools only")]
    NotMcp(String),
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
    let rest = input.strip_prefix("/!")?;
    let invokable = rest.split_whitespace().next()?;
    let arguments = rest[invokable.len()..].trim().to_owned();
    Some(DirectInvocation {
        invokable: invokable.to_owned(),
        arguments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::{ActorMeta, Definition, PersonaMeta, PromptMeta};
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    fn prompt_def(name: &str, meta: PromptMeta, body: &str) -> (String, Definition<PromptMeta>) {
        (
            name.to_owned(),
            Definition {
                name: name.to_owned(),
                path: PathBuf::from(format!("/prompts/{name}.md")),
                raw: body.to_owned(),
                frontmatter: meta,
                body: body.to_owned(),
            },
        )
    }

    /// The deserialised default for a prompt: user-invokable slash
    /// command, not model-exposed. (Derived `Default` does not apply
    /// serde's `default_true`, so tests construct it explicitly.)
    fn default_prompt_meta() -> PromptMeta {
        PromptMeta {
            user_invokable: true,
            model_invokable: false,
            ..PromptMeta::default()
        }
    }

    fn definitions_with(prompts: Vec<(String, Definition<PromptMeta>)>) -> Definitions {
        Definitions {
            personas: BTreeMap::new(),
            actors: BTreeMap::new(),
            prompts: prompts.into_iter().collect(),
            builtin_overrides: BTreeSet::new(),
        }
    }

    fn mcp_tool(server: &str, tool: &str) -> NamespacedTool {
        NamespacedTool {
            name: mcp_name(server, tool),
            description: format!("{tool} on {server}"),
            schema: serde_json::json!({"type": "object"}),
        }
    }

    #[test]
    fn registration_namespaces_by_mechanism() {
        let mut definitions = definitions_with(vec![prompt_def(
            "explain",
            PromptMeta::default(),
            "Explain it.",
        )]);
        definitions.personas.insert(
            "default".to_owned(),
            Definition {
                name: "default".to_owned(),
                path: PathBuf::from("/personas/default.md"),
                raw: String::new(),
                frontmatter: PersonaMeta { description: None },
                body: String::new(),
            },
        );
        definitions.actors.insert(
            "builder".to_owned(),
            Definition {
                name: "builder".to_owned(),
                path: PathBuf::from("/actors/builder.md"),
                raw: String::new(),
                frontmatter: ActorMeta {
                    persona: "default".to_owned(),
                    model: None,
                },
                body: String::new(),
            },
        );

        let registry = Registry::build(&definitions, &[mcp_tool("echo", "echo")]).unwrap();

        assert!(registry.resolve("mcp.echo.echo").is_some());
        assert!(registry.resolve("prompt.explain").is_some());
        assert!(registry.resolve("agent.builder").is_some());
        // Unnamespaced and unknown names never resolve.
        assert!(registry.resolve("echo").is_none());
        assert!(registry.resolve("prompt.missing").is_none());
    }

    #[test]
    fn visibility_flags_come_from_frontmatter_and_mechanism() {
        let definitions = definitions_with(vec![
            prompt_def("both", default_prompt_meta(), "Body."),
            prompt_def(
                "model_only",
                PromptMeta {
                    user_invokable: false,
                    model_invokable: true,
                    ..PromptMeta::default()
                },
                "Body.",
            ),
            prompt_def(
                "neither",
                PromptMeta {
                    user_invokable: false,
                    model_invokable: false,
                    ..PromptMeta::default()
                },
                "Body.",
            ),
        ]);
        let registry = Registry::build(&definitions, &[mcp_tool("echo", "echo")]).unwrap();

        let mcp = registry.resolve("mcp.echo.echo").unwrap();
        assert!(mcp.user_invokable() && mcp.model_invokable());

        assert!(registry.resolve("prompt.both").unwrap().user_invokable());
        assert!(!registry.resolve("prompt.both").unwrap().model_invokable());
        let model_only = registry.resolve("prompt.model_only").unwrap();
        assert!(!model_only.user_invokable() && model_only.model_invokable());
        let neither = registry.resolve("prompt.neither").unwrap();
        assert!(!neither.user_invokable() && !neither.model_invokable());

        // Agents are model-invokable only.
        let mut definitions = definitions_with(vec![]);
        definitions.actors.insert(
            "worker".to_owned(),
            Definition {
                name: "worker".to_owned(),
                path: PathBuf::from("/actors/worker.md"),
                raw: String::new(),
                frontmatter: ActorMeta {
                    persona: "default".to_owned(),
                    model: None,
                },
                body: String::new(),
            },
        );
        let registry = Registry::build(&definitions, &[]).unwrap();
        let agent = registry.resolve("agent.worker").unwrap();
        assert!(!agent.user_invokable() && agent.model_invokable());
    }

    #[test]
    fn model_exposure_includes_mcp_tools_and_model_invokable_prompts() {
        let definitions = definitions_with(vec![
            prompt_def(
                "hidden",
                PromptMeta {
                    user_invokable: false,
                    model_invokable: false,
                    ..PromptMeta::default()
                },
                "Never exposed.",
            ),
            prompt_def(
                "shown",
                PromptMeta {
                    user_invokable: true,
                    model_invokable: true,
                    ..PromptMeta::default()
                },
                "Body.",
            ),
        ]);
        let registry = Registry::build(&definitions, &[mcp_tool("echo", "echo")]).unwrap();

        let names: Vec<&str> = registry
            .model_invokables()
            .map(|invokable| invokable.name())
            .collect();
        assert_eq!(names, vec!["mcp.echo.echo", "prompt.shown"]);
    }

    #[test]
    fn same_layer_collision_is_an_error_naming_the_invokable() {
        // A server reporting the same tool twice collides in the mcp
        // namespace.
        let registry = Registry::build(
            &Definitions::default(),
            &[mcp_tool("echo", "echo"), mcp_tool("echo", "echo")],
        );

        let Err(err) = registry else {
            panic!("expected a collision error");
        };
        assert!(
            err.to_string()
                .contains("in the `mcp` namespace: `mcp.echo.echo` is declared twice"),
            "{err}"
        );
    }

    #[test]
    fn cross_namespace_names_cannot_collide() {
        let mut definitions =
            definitions_with(vec![prompt_def("echo", PromptMeta::default(), "Body.")]);
        definitions.actors.insert(
            "echo".to_owned(),
            Definition {
                name: "echo".to_owned(),
                path: PathBuf::from("/actors/echo.md"),
                raw: String::new(),
                frontmatter: ActorMeta {
                    persona: "default".to_owned(),
                    model: None,
                },
                body: String::new(),
            },
        );
        // The same bare name across the mcp, prompt, and agent
        // namespaces coexists.
        let registry = Registry::build(&definitions, &[mcp_tool("echo", "echo")]).unwrap();
        assert!(registry.resolve("mcp.echo.echo").is_some());
        assert!(registry.resolve("prompt.echo").is_some());
        assert!(registry.resolve("agent.echo").is_some());
    }

    #[test]
    fn command_advertisement_lists_user_invokable_prompts_with_hints() {
        let definitions = definitions_with(vec![
            prompt_def(
                "explain",
                PromptMeta {
                    description: Some("Explain the selection.".to_owned()),
                    parameters: vec!["subject".to_owned(), "depth".to_owned()],
                    ..default_prompt_meta()
                },
                "Body.",
            ),
            prompt_def("bare", default_prompt_meta(), "No parameters."),
            prompt_def(
                "model_only",
                PromptMeta {
                    user_invokable: false,
                    model_invokable: true,
                    ..PromptMeta::default()
                },
                "Body.",
            ),
        ]);
        let registry = Registry::build(&definitions, &[]).unwrap();

        let commands = registry.available_commands();
        // The `!` prefix command leads, then the user-invokable prompts.
        assert_eq!(commands.len(), 3);
        assert_eq!(commands[0].name, "!");

        let explain = commands
            .iter()
            .find(|command| command.name == "explain")
            .unwrap();
        assert_eq!(explain.description, "Explain the selection.");
        let input = explain.input.as_ref().unwrap();
        let AvailableCommandInput::Unstructured(unstructured) = input else {
            panic!("expected an unstructured input hint");
        };
        assert_eq!(unstructured.hint, "<subject> <depth>");

        let bare = commands
            .iter()
            .find(|command| command.name == "bare")
            .unwrap();
        assert!(bare.input.is_none());

        // Non-user-invokable prompts are not advertised.
        assert!(!commands.iter().any(|command| command.name == "model_only"));
    }

    mod expansion {
        use super::*;

        fn registry_with(body: &str, parameters: &[&str]) -> Registry {
            let definitions = definitions_with(vec![prompt_def(
                "deploy",
                PromptMeta {
                    parameters: parameters.iter().map(|p| p.to_string()).collect(),
                    ..default_prompt_meta()
                },
                body,
            )]);
            Registry::build(&definitions, &[]).unwrap()
        }

        #[test]
        fn positional_expansion_fills_the_last_parameter_with_the_remainder() {
            let registry = registry_with(
                "Ship {{ target }} to {{ where }}, noting: {{ notes }}",
                &["target", "where", "notes"],
            );

            let expanded = registry
                .expand_user("deploy", "api staging tonight after the release cut")
                .unwrap();

            // First parameters take single tokens; the last takes the
            // remainder of the arguments, spacing preserved.
            assert_eq!(
                expanded,
                "Ship api to staging, noting: tonight after the release cut"
            );
        }

        #[test]
        fn single_parameter_takes_the_whole_input() {
            let registry = registry_with("Hello {{ who }}!", &["who"]);
            assert_eq!(
                registry.expand_user("deploy", "  world  ").unwrap(),
                "Hello world!"
            );
        }

        #[test]
        fn arity_mismatches_fail_with_the_usage_string() {
            let registry = registry_with("Ship {{ a }} then {{ b }}", &["a", "b"]);

            // Too few arguments.
            let err = registry.expand_user("deploy", "only-one").unwrap_err();
            assert_eq!(err.to_string(), "usage: /deploy <a> <b>");

            // A no-parameter prompt given input.
            let registry = registry_with("Just deploy.", &[]);
            let err = registry.expand_user("deploy", "extra input").unwrap_err();
            assert_eq!(err.to_string(), "usage: /deploy");
        }

        #[test]
        fn unknown_commands_fail_and_store_nothing() {
            let registry = registry_with("Body.", &[]);
            let err = registry.expand_user("missing", "").unwrap_err();
            assert_eq!(err.to_string(), "unknown command `missing`");

            // The registry is pure: expansion wrote nothing anywhere.
            assert!(registry.resolve("prompt.deploy").is_some());
        }

        #[test]
        fn model_tool_invocation_loads_the_expanded_body() {
            let definitions = definitions_with(vec![prompt_def(
                "deploy",
                PromptMeta {
                    user_invokable: false,
                    model_invokable: true,
                    parameters: vec!["service".to_owned(), "env".to_owned()],
                    ..PromptMeta::default()
                },
                "Deploy {{ service }} to {{ env }}.",
            )]);
            let registry = Registry::build(&definitions, &[]).unwrap();

            let expanded = registry
                .expand_model(
                    "prompt.deploy",
                    &serde_json::json!({ "service": "api", "env": "staging" }),
                )
                .unwrap();
            assert_eq!(expanded, "Deploy api to staging.");
        }

        #[test]
        fn model_invocation_rejects_missing_and_unknown_arguments_with_usage() {
            let definitions = definitions_with(vec![prompt_def(
                "deploy",
                PromptMeta {
                    user_invokable: false,
                    model_invokable: true,
                    parameters: vec!["service".to_owned(), "env".to_owned()],
                    ..PromptMeta::default()
                },
                "Deploy {{ service }} to {{ env }}.",
            )]);
            let registry = Registry::build(&definitions, &[]).unwrap();

            let err = registry
                .expand_model("prompt.deploy", &serde_json::json!({ "service": "api" }))
                .unwrap_err();
            assert_eq!(err.to_string(), "usage: /deploy <service> <env>");

            let err = registry
                .expand_model(
                    "prompt.deploy",
                    &serde_json::json!({ "service": "api", "env": "s", "rogue": "x" }),
                )
                .unwrap_err();
            assert_eq!(err.to_string(), "usage: /deploy <service> <env>");

            // Non-object arguments are an arity mismatch.
            let err = registry
                .expand_model("prompt.deploy", &serde_json::json!("api"))
                .unwrap_err();
            assert_eq!(err.to_string(), "usage: /deploy <service> <env>");
        }

        #[test]
        fn visibility_gates_expansion_paths() {
            // model-only prompt: /name expansion refuses.
            let definitions = definitions_with(vec![prompt_def(
                "model_only",
                PromptMeta {
                    user_invokable: false,
                    model_invokable: true,
                    ..PromptMeta::default()
                },
                "Body.",
            )]);
            let registry = Registry::build(&definitions, &[]).unwrap();
            let err = registry.expand_user("model_only", "").unwrap_err();
            assert!(err.to_string().contains("not user-invokable"), "{err}");

            // user-only prompt: model-tool invocation refuses.
            let definitions = definitions_with(vec![prompt_def(
                "user_only",
                default_prompt_meta(),
                "Body.",
            )]);
            let registry = Registry::build(&definitions, &[]).unwrap();
            let err = registry
                .expand_model("prompt.user_only", &serde_json::json!({}))
                .unwrap_err();
            assert!(err.to_string().contains("not model-invokable"), "{err}");
        }
    }

    mod direct_invocation {
        use super::*;

        fn registry_with_mcp() -> Registry {
            let definitions =
                definitions_with(vec![prompt_def("deploy", default_prompt_meta(), "Body.")]);
            Registry::build(&definitions, &[mcp_tool("echo", "echo")]).unwrap()
        }

        #[test]
        fn the_prefix_command_is_advertised_for_autocomplete() {
            let registry = registry_with_mcp();

            let commands = registry.available_commands();
            let bang = commands.first().unwrap();
            assert_eq!(bang.name, "!");
            let AvailableCommandInput::Unstructured(unstructured) = bang.input.as_ref().unwrap()
            else {
                panic!("expected an unstructured input hint");
            };
            assert_eq!(unstructured.hint, "mcp.<server>.<tool> {json arguments}");
        }

        #[test]
        fn interception_is_exact_first_token_on_user_typed_input() {
            let parsed = parse_direct("/!mcp.echo.echo {\"text\": \"hi\"}").unwrap();
            assert_eq!(parsed.invokable, "mcp.echo.echo");
            assert_eq!(parsed.arguments, "{\"text\": \"hi\"}");

            // No arguments: empty remainder.
            let parsed = parse_direct("/!mcp.echo.echo").unwrap();
            assert_eq!(parsed.invokable, "mcp.echo.echo");
            assert_eq!(parsed.arguments, "");

            // Not the /! prefix — pasted text, or model output that
            // merely mentions the syntax: never intercepted.
            assert!(parse_direct("look at /!mcp.echo.echo").is_none());
            assert!(parse_direct("I'd run /!mcp.echo.echo for that").is_none());
            assert!(parse_direct("/mcp.echo.echo").is_none());
            assert!(parse_direct("/!").is_none());
        }
    }
}
