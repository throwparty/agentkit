//! CLI argument parsing and path-resolution tests.

use super::*;
use clap::CommandFactory;
use clap::Parser;
use std::path::PathBuf;

#[test]
fn parses_stdio() {
    let args = Cli::parse_from(["tackle", "stdio"]);
    assert!(matches!(args.command, Commands::Stdio { .. }));
}

#[test]
fn parses_http_defaults() {
    let args = Cli::parse_from(["tackle", "http"]);
    match args.command {
        Commands::Http {
            bind, http_port, ..
        } => {
            assert_eq!(bind, "127.0.0.1");
            assert_eq!(http_port, 3811);
        }
        _ => panic!("expected http"),
    }
}

#[test]
fn resolves_defaults() {
    let args = Cli::parse_from(["tackle", "stdio"]);
    let (db, cfg) = match &args.command {
        Commands::Stdio {
            db_path,
            config_dir,
        } => (db_path.as_ref(), config_dir.as_ref()),
        _ => panic!("expected stdio"),
    };
    assert!(db_path(db).parent().unwrap().ends_with("tackle"));
    assert!(config_dir(cfg).ends_with("tackle"));
}

#[test]
fn honours_overrides() {
    let args = Cli::parse_from([
        "tackle",
        "http",
        "--config-dir",
        "/tmp/tackle-cfg",
        "--db-path",
        "/tmp/tackle.db",
    ]);
    match &args.command {
        Commands::Http {
            db_path: db,
            config_dir: cfg,
            ..
        } => {
            assert_eq!(config_dir(cfg.as_ref()), PathBuf::from("/tmp/tackle-cfg"));
            assert_eq!(db_path(db.as_ref()), PathBuf::from("/tmp/tackle.db"));
        }
        _ => panic!("expected http"),
    }
}

#[test]
fn definitions_and_def_are_equivalent() {
    let long = Cli::parse_from(["tackle", "definitions", "list"]);
    let short = Cli::parse_from(["tackle", "def", "list"]);
    assert!(matches!(
        long.command,
        Commands::Definitions {
            command: DefinitionsCommands::List,
            ..
        }
    ));
    assert!(matches!(
        short.command,
        Commands::Definitions {
            command: DefinitionsCommands::List,
            ..
        }
    ));
}

#[test]
fn init_writes_a_parseable_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join(".agentkit").join("tackle");
    definitions::init(&project).unwrap();
    crate::definitions::manifest::Manifest::load(&project).unwrap();
}

#[test]
fn add_and_remove_update_selections() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join(".agentkit").join("tackle");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        project.join("definitions.toml"),
        r#"
[[source]]
name = "local"
kind = "path"

[source.path]
dir = "shared"
"#,
    )
    .unwrap();
    definitions::add(&project, "local", "prompts", "triage").unwrap();
    let manifest = crate::definitions::manifest::Manifest::load(&project).unwrap();
    assert_eq!(
        manifest.definitions["local"].prompts,
        vec!["triage".to_string()]
    );
    definitions::remove(&project, "local", "prompts", "triage").unwrap();
    let manifest = crate::definitions::manifest::Manifest::load(&project).unwrap();
    assert!(manifest.definitions.is_empty());
}

#[test]
fn command_builds() {
    Cli::command().build();
}
