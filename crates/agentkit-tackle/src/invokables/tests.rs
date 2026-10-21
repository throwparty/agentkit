//! Registry and dispatch tests.

use super::*;
use crate::definitions::{ActorMeta, Definition, Definitions, PersonaMeta, PromptMeta};
use crate::mcp::NamespacedTool;
use agent_client_protocol::schema::v1::AvailableCommandInput;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[test]
fn kind_of_decodes_every_namespace() {
    assert_eq!(kind_of("mcp.litterbox.exec"), Some(Kind::Mcp));
    assert_eq!(kind_of("prompt.compact"), Some(Kind::Prompt));
    assert_eq!(kind_of("client.fs.read_text_file"), Some(Kind::Client));
    assert_eq!(kind_of("agent.reviewer"), Some(Kind::Agent));
    // A name with no known prefix is an unknown invokable, never an agent.
    assert_eq!(kind_of("unprefixed"), None);
}

fn prompt_def(name: &str, meta: PromptMeta, body: &str) -> (String, Definition<PromptMeta>) {
    (
        name.to_owned(),
        Definition {
            name: name.to_owned(),
            path: PathBuf::from(format!("/prompts/{name}.md")),
            directory: None,
            resources: Vec::new(),
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
            directory: None,
            resources: Vec::new(),
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
            directory: None,
            resources: Vec::new(),
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
            directory: None,
            resources: Vec::new(),
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
fn agent_tools_expose_a_required_prompt() {
    let mut definitions = Definitions::default();
    definitions.actors.insert(
        "worker".to_owned(),
        Definition {
            name: "worker".to_owned(),
            path: PathBuf::from("/actors/worker.md"),
            directory: None,
            resources: Vec::new(),
            raw: String::new(),
            frontmatter: ActorMeta {
                persona: "default".to_owned(),
                model: None,
            },
            body: String::new(),
        },
    );
    let registry = Registry::build(&definitions, &[]).unwrap();
    let tool = registry
        .model_tools()
        .into_iter()
        .find(|tool| tool.name == "agent.worker")
        .expect("agent tool advertised");
    assert_eq!(tool.parameters["properties"]["prompt"]["type"], "string");
    assert_eq!(tool.parameters["required"], serde_json::json!(["prompt"]));
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
            directory: None,
            resources: Vec::new(),
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
        let AvailableCommandInput::Unstructured(unstructured) = bang.input.as_ref().unwrap() else {
            panic!("expected an unstructured input hint");
        };
        assert_eq!(
            unstructured.hint,
            "mcp.<server>.<tool> | client.<surface>.<method> {json arguments}"
        );
    }

    #[test]
    fn client_tools_are_advertised_under_the_bang_prefix() {
        use crate::acp_v1::client_tools::{catalog, ClientCaps, EditorToggles};

        let definitions = definitions_with(Vec::new());
        let client_tools = catalog(
            ClientCaps {
                fs_read: true,
                fs_write: false,
                terminal: true,
            },
            EditorToggles {
                fs: true,
                terminal: true,
            },
        );
        let registry = Registry::build_with_client_tools(&definitions, &[], &client_tools).unwrap();
        let commands = registry.available_commands();
        let names: Vec<&str> = commands
            .iter()
            .map(|command| command.name.as_str())
            .collect();
        assert!(names.contains(&"!client.fs.read_text_file"), "{names:?}");
        assert!(names.contains(&"!client.terminal.create"), "{names:?}");
        assert!(
            names.contains(&"!client.terminal.wait_for_exit"),
            "{names:?}"
        );
    }

    #[test]
    fn interception_is_exact_first_token_on_user_typed_input() {
        let parsed = parse_direct("/!mcp.echo.echo {\"text\": \"hi\"}").unwrap();
        assert_eq!(parsed.invokable, "mcp.echo.echo");
        assert_eq!(parsed.arguments, "{\"text\": \"hi\"}");

        // The brace wins even when whitespace follows it.
        let parsed = parse_direct("/!mcp.echo.echo{\"text\": \"hi\"}").unwrap();
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
