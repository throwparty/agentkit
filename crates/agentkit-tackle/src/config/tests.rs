//! Configuration loading and merging tests.

use super::*;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn write_layer(dir: &Path, content: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(&path, content).unwrap();
    path
}

#[test]
fn missing_layers_yield_default_config() {
    let dir = tempfile::tempdir().unwrap();
    let loaded = load_layered(
        &dir.path().join("user"),
        Some(&dir.path().join("project")),
        None,
    )
    .unwrap();
    assert!(loaded.config.endpoints.is_empty());
    assert_eq!(loaded.config.defaults.actor, None);
    assert_eq!(loaded.user_layer, None);
    assert_eq!(loaded.project_layer, None);
}

#[test]
fn project_overrides_user_for_defaults() {
    let dir = tempfile::tempdir().unwrap();
    write_layer(
        &dir.path().join("user"),
        "[defaults]\nactor = \"user-actor\"\n",
    );
    write_layer(
        &dir.path().join("project"),
        "[defaults]\nactor = \"project-actor\"\n",
    );
    let loaded = load_layered(
        &dir.path().join("user"),
        Some(&dir.path().join("project")),
        None,
    )
    .unwrap();
    assert_eq!(
        loaded.config.defaults.actor,
        Some("project-actor".to_string())
    );
    assert!(loaded.user_layer.is_some());
    assert!(loaded.project_layer.is_some());
}

#[test]
fn project_mcp_servers_are_ignored_while_user_entries_load() {
    let dir = tempfile::tempdir().unwrap();
    write_layer(
        &dir.path().join("user"),
        "[mcp_servers.tools]\ntransport = \"http\"\nurl = \"https://user.example.com\"\n",
    );
    write_layer(
        &dir.path().join("project"),
        "[mcp_servers.rogue]\ntransport = \"http\"\nurl = \"https://project.example.com\"\n",
    );
    let loaded = load_layered(
        &dir.path().join("user"),
        Some(&dir.path().join("project")),
        None,
    )
    .unwrap();
    assert!(!loaded.config.mcp_servers.contains_key("rogue"));
    match loaded.config.mcp_servers.get("tools") {
        Some(McpServerConfig::Http { url }) => assert_eq!(url, "https://user.example.com"),
        other => panic!("unexpected server config: {other:?}"),
    }
}

#[test]
fn unimplemented_wire_formats_are_rejected_at_parse() {
    // v1 implements openai-chat-completions only; a config naming an
    // unimplemented format fails at load rather than at first prompt.
    let dir = tempfile::tempdir().unwrap();
    write_layer(
        &dir.path().join("user"),
        "[endpoints.anthropic]\nbase_url = \"https://api.anthropic.com\"\nwire_format = \"anthropic-messages\"\nauth = \"helper\"\n",
    );
    let err = load_layered(&dir.path().join("user"), None, None).unwrap_err();
    assert!(err.to_string().contains("wire_format"), "{err}");
}

#[test]
fn project_endpoints_are_rejected_by_name() {
    let dir = tempfile::tempdir().unwrap();
    write_layer(
        &dir.path().join("project"),
        "[endpoints.rogue]\nbase_url = \"https://rogue.example.com\"\nwire_format = \"openai-chat-completions\"\nauth = \"none\"\n",
    );
    let err = load_layered(
        &dir.path().join("user"),
        Some(&dir.path().join("project")),
        None,
    )
    .unwrap_err();
    assert!(err.to_string().contains("endpoints"), "{err}");
}

#[test]
fn project_credential_helper_is_rejected_by_name() {
    let dir = tempfile::tempdir().unwrap();
    write_layer(
        &dir.path().join("project"),
        "credential_helper = \"rogue\"\n",
    );
    let err = load_layered(
        &dir.path().join("user"),
        Some(&dir.path().join("project")),
        None,
    )
    .unwrap_err();
    assert!(err.to_string().contains("credential_helper"), "{err}");
}

#[test]
fn project_client_tools_are_ignored_while_user_entries_load() {
    let dir = tempfile::tempdir().unwrap();
    write_layer(
        &dir.path().join("project"),
        "[client_tools]\nfs_enabled = false\nterminal_enabled = false\n",
    );
    let loaded = load_layered(
        &dir.path().join("user"),
        Some(&dir.path().join("project")),
        None,
    )
    .unwrap();
    assert!(loaded.config.client_tools.is_none());

    write_layer(
        &dir.path().join("user"),
        "[client_tools]\nfs_enabled = false\nterminal_enabled = false\n",
    );
    let loaded = load_layered(
        &dir.path().join("user"),
        Some(&dir.path().join("project")),
        None,
    )
    .unwrap();
    assert!(loaded.config.client_tools.is_some());
}

#[test]
fn client_tools_default_on_and_parse_toggles() {
    let dir = tempfile::tempdir().unwrap();
    let absent = load_layered(
        &dir.path().join("user"),
        Some(&dir.path().join("project")),
        None,
    )
    .unwrap();
    assert_eq!(
        absent.config.client_tools.as_ref(),
        None,
        "an absent section leaves both categories at their defaults"
    );

    write_layer(
        &dir.path().join("user"),
        "[client_tools]\nterminal_enabled = false\n",
    );
    let parsed = load_layered(
        &dir.path().join("user"),
        Some(&dir.path().join("project")),
        None,
    )
    .unwrap();
    let section = parsed.config.client_tools.expect("section parses");
    assert!(section.fs_enabled);
    assert!(!section.terminal_enabled);
}

#[test]
fn user_layer_configures_endpoints() {
    let dir = tempfile::tempdir().unwrap();
    write_layer(
        &dir.path().join("user"),
        "[endpoints.switchboard]\nbase_url = \"http://localhost:3812/openai/v1\"\nwire_format = \"openai-chat-completions\"\nauth = \"none\"\n\n[defaults]\nmodel = \"switchboard/claude-sonnet-4-5\"\n",
    );
    let loaded = load_layered(
        &dir.path().join("user"),
        Some(&dir.path().join("project")),
        None,
    )
    .unwrap();
    assert_eq!(loaded.config.endpoints.len(), 1);
    assert_eq!(
        loaded.config.defaults.model,
        Some("switchboard/claude-sonnet-4-5".to_string())
    );
}

#[test]
fn stdio_mcp_server_with_env_parses() {
    let dir = tempfile::tempdir().unwrap();
    write_layer(
        &dir.path().join("user"),
        "[mcp_servers.litterbox]\ntransport = \"stdio\"\ncommand = \"litterbox\"\nargs = [\"mcp\"]\n\n[mcp_servers.litterbox.env]\nLITTERBOX_LOG = \"debug\"\n",
    );
    let loaded = load_layered(
        &dir.path().join("user"),
        Some(&dir.path().join("project")),
        None,
    )
    .unwrap();
    match loaded.config.mcp_servers.get("litterbox") {
        Some(McpServerConfig::Stdio { command, args, env }) => {
            assert_eq!(command, "litterbox");
            assert_eq!(args, &["mcp"]);
            assert_eq!(env.get("LITTERBOX_LOG").map(String::as_str), Some("debug"));
        }
        other => panic!("unexpected server config: {other:?}"),
    }
}

#[test]
fn script_entry_parses_with_enabled_default() {
    let dir = tempfile::tempdir().unwrap();
    write_layer(
        &dir.path().join("user"),
        "[scripts.deny-secrets]\nevents = [\"pre_tool_use\"]\nfile = \"deny-secrets.rhai\"\n",
    );
    let loaded = load_layered(
        &dir.path().join("user"),
        Some(&dir.path().join("project")),
        None,
    )
    .unwrap();
    let script = loaded.config.scripts.get("deny-secrets").unwrap();
    assert!(script.enabled);
    assert_eq!(script.events, &["pre_tool_use"]);
}

#[test]
fn approved_project_script_resolves_to_its_project_file() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(project.join("scripts")).unwrap();
    std::fs::write(
        project.join("scripts/deny.rhai"),
        "fn pre_tool_use(r) { r }",
    )
    .unwrap();
    // A user-layer file of the same name must not shadow the project
    // script: the pin selects the project file.
    let user = dir.path().join("user");
    std::fs::create_dir_all(user.join("scripts")).unwrap();
    std::fs::write(user.join("scripts/deny.rhai"), "user shadow").unwrap();

    let project_config: Config =
        toml::from_str("[scripts.deny]\nevents = [\"pre_tool_use\"]\nfile = \"deny.rhai\"\n")
            .unwrap();
    let retained = retain_project_entries(project_config, &project, &BTreeSet::new());
    let script = retained.scripts.get("deny").unwrap();

    let source = resolve_script_source("deny", &script.file, Some(&user)).unwrap();
    assert_eq!(source, "fn pre_tool_use(r) { r }");
}

#[test]
fn project_scripts_load_without_consent() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    let project_config: Config =
        toml::from_str("[scripts.deny]\nevents = [\"pre_tool_use\"]\nfile = \"deny.rhai\"\n")
            .unwrap();
    let retained = retain_project_entries(project_config, &project, &BTreeSet::new());
    assert!(retained.scripts.contains_key("deny"));
}

#[test]
fn project_scripts_do_not_create_trust_entries() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(project.join("scripts")).unwrap();
    std::fs::write(
        project.join("scripts/deny.rhai"),
        "fn pre_tool_use(r) { r }",
    )
    .unwrap();
    let config: Config =
        toml::from_str("[scripts.deny]\nevents = [\"pre_tool_use\"]\nfile = \"deny.rhai\"\n")
            .unwrap();
    assert!(project_trust_entries(&config, &project).is_empty());

    std::fs::write(
        project.join("scripts/deny.rhai"),
        "fn pre_tool_use(r) { () }",
    )
    .unwrap();
    assert!(project_trust_entries(&config, &project).is_empty());
}
