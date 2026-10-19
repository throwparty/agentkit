//! The registry: the single dispatch point over namespaced invokables, and the expansion of prompt invokables.

use super::client_tools::ClientTool;
use super::direct::{DirectError, DirectInvocation};
use super::invokable::Invokable;
use super::name::{agent_name, prompt_name};
use crate::definitions::Definitions;
use crate::mcp::{ElicitationSink, McpPool, NamespacedTool};
use agent_client_protocol::schema::v1::{
    AvailableCommand, AvailableCommandInput, UnstructuredCommandInput,
};
use std::collections::BTreeMap;

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
    /// namespaced tool list, with no client-capability tools.
    pub fn build(
        definitions: &Definitions,
        mcp_tools: &[NamespacedTool],
    ) -> Result<Self, RegistryError> {
        Self::build_with_client_tools(definitions, mcp_tools, &[])
    }

    /// Builds the registry including the client-capability tools the
    /// connection advertised and enabled.
    pub fn build_with_client_tools(
        definitions: &Definitions,
        mcp_tools: &[NamespacedTool],
        client_tools: &[ClientTool],
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

        for tool in client_tools {
            registry.insert(
                "client",
                tool.name.clone(),
                Invokable::Client {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    schema: tool.schema.clone(),
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

    /// The tool definitions offered to the model: each
    /// model-invokable with a JSON-schema parameter object.
    pub fn model_tools(&self) -> Vec<crate::providers::ToolDefinition> {
        self.model_invokables()
            .map(|invokable| {
                let parameters = match invokable {
                    Invokable::Mcp { schema, .. } => schema.clone(),
                    Invokable::Prompt { parameters, .. } => {
                        let mut properties = serde_json::Map::new();
                        for parameter in parameters {
                            properties
                                .insert(parameter.clone(), serde_json::json!({ "type": "string" }));
                        }
                        serde_json::json!({ "type": "object", "properties": properties })
                    }
                    Invokable::Agent { .. } => {
                        // The invocation arguments form the nested
                        // agent's prompt.
                        serde_json::json!({
                            "type": "object",
                            "properties": {
                                "prompt": {
                                    "type": "string",
                                    "description": "The task for the sub-agent."
                                }
                            },
                            "required": ["prompt"],
                        })
                    }
                    Invokable::Client { schema, .. } => schema.clone(),
                };
                crate::providers::ToolDefinition {
                    name: invokable.name().to_owned(),
                    description: invokable.description().to_owned(),
                    parameters,
                }
            })
            .collect()
    }

    /// The invokable listing injected into the system prompt.
    pub fn manifest(&self) -> String {
        let mut lines = vec!["Invokables available to you (invoke them as tools):".to_owned()];
        for invokable in self.model_invokables() {
            let name = match invokable {
                Invokable::Mcp { name, .. } | Invokable::Client { name, .. } => format!("!{name}"),
                _ => invokable.name().to_owned(),
            };
            lines.push(format!("- {}: {}", name, invokable.description()));
        }
        lines.push(
            "Never emit slash-command syntax (for example `/name` or `/!tool`) as literal text."
                .to_owned(),
        );
        lines.join("\n")
    }

    /// The ACP command advertisement: the `!` prefix command (clients
    /// autocomplete `/!`) followed by the user-invokable prompts, with
    /// the unstructured input hint mapped from the prompt's declared
    /// parameters.
    pub fn available_commands(&self) -> Vec<AvailableCommand> {
        let mut commands = vec![
            AvailableCommand::new("!", "Execute a tool directly with no model request").input(
                AvailableCommandInput::Unstructured(UnstructuredCommandInput::new(
                    "mcp.<server>.<tool> | client.<surface>.<method> {json arguments}",
                )),
            ),
        ];

        for invokable in self.entries.values() {
            if !invokable.user_invokable() {
                continue;
            }
            match invokable {
                Invokable::Prompt {
                    name,
                    description,
                    parameters,
                    ..
                } => {
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
                Invokable::Mcp {
                    name, description, ..
                } => {
                    // Each tool is advertised under its own `!`-prefixed
                    // namespaced name. Advertising the bare `!` alone is
                    // not enough: a client that exact-matches the
                    // advertised command name rejects
                    // `/!mcp.<server>.<tool>` outright, because no such
                    // command was ever offered.
                    commands.push(
                        AvailableCommand::new(format!("!{name}"), description.clone()).input(
                            AvailableCommandInput::Unstructured(UnstructuredCommandInput::new(
                                "{json arguments}",
                            )),
                        ),
                    );
                }
                Invokable::Agent { .. } => {}
                Invokable::Client {
                    name, description, ..
                } => {
                    // Client-capability tools are advertised like MCP
                    // tools: each under its own `!`-prefixed name so a
                    // client that exact-matches commands offers
                    // `/!client.<surface>.<method>`.
                    commands.push(
                        AvailableCommand::new(format!("!{name}"), description.clone()).input(
                            AvailableCommandInput::Unstructured(UnstructuredCommandInput::new(
                                "{json arguments}",
                            )),
                        ),
                    );
                }
            }
        }
        commands
    }

    /// `/!name` execution of MCP tools with no model request, bypassing
    /// the permission pipeline — the user is the authority. Oversized
    /// outputs arrive truncated at the fixed internal limit with the
    /// original size recorded; storage keeps the full content. Client
    /// capability tools take the equivalent path in the ACP layer, which
    /// owns the connection their requests travel over.
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
            // /! reaches MCP tools and client-capability tools; the
            // latter execute on the connection (handled by the ACP
            // layer), so here anything else is a precise error.
            return Err(DirectError::NotDirect(invokable_name));
        };

        let arguments = input.arguments_json()?;

        // The tool-call span: arguments redacted to their size.
        let _call_guard = crate::telemetry::tool_call_span(&input.invokable, input.arguments.len());
        pool.call_tool(server, tool, arguments)
            .await
            .map_err(DirectError::Pool)
    }

    /// Executes a model-initiated invokable call: MCP tools run against
    /// the pool; model-invokable prompts expand their body into context.
    /// The ACP layer dispatches `agent` and `client` invokables itself
    /// (nested loops and the connection's client tools), so anything else
    /// reaching here is a caller error, reported without naming a
    /// category this layer cannot execute.
    pub async fn execute_model<S: ElicitationSink>(
        &self,
        pool: &McpPool<S>,
        name: &str,
        arguments: &serde_json::Value,
    ) -> Result<String, String> {
        let invokable = self
            .resolve(name)
            .ok_or_else(|| format!("unknown invokable `{name}`"))?;
        match invokable {
            Invokable::Mcp { server, tool, .. } => pool
                .call_tool(server, tool, arguments.clone())
                .await
                .map(|result| result.content_json)
                .map_err(|err| err.to_string()),
            Invokable::Prompt { .. } => self
                .expand_model(name, arguments)
                .map_err(|err| err.to_string()),
            other => Err(format!(
                "`{}` is not executable through the registry",
                other.name()
            )),
        }
    }
}
