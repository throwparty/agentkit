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
use crate::mcp::NamespacedTool;
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

    /// The ACP command advertisement: user-invokable prompts as
    /// commands, with the unstructured input hint mapped from the
    /// prompt's declared parameters.
    pub fn available_commands(&self) -> Vec<AvailableCommand> {
        self.entries
            .values()
            .filter(|invokable| invokable.user_invokable())
            .filter_map(|invokable| match invokable {
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
                    Some(command)
                }
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::{ActorMeta, Definition, PersonaMeta, PromptMeta};
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
        assert_eq!(commands.len(), 2);

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
}
