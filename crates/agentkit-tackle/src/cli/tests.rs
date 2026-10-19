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
fn command_builds() {
    Cli::command().build();
}
