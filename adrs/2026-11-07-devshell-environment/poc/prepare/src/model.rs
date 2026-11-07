//! The step model: a kind the contract does not name.
//!
//! `parse_step` is the only place in the harness that branches on the kind. The
//! kind is not stored beside the settings: it is derived from them, so the two
//! cannot disagree.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::failure::Failure;

/// A declared step. The identity is what every error names it by.
#[derive(Debug, PartialEq, Eq)]
pub struct Step {
    pub identity: String,
    pub settings: Settings,
}

impl Step {
    /// The kind name, derived from the settings that define it.
    pub fn kind(&self) -> &'static str {
        self.settings.kind()
    }
}

/// The settings one kind defines. This is the closed set the harness keeps.
#[derive(Debug, PartialEq, Eq)]
pub enum Settings {
    Devshell { installable: String },
    Command { run: String },
}

impl Settings {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Devshell { .. } => "devshell",
            Self::Command { .. } => "command",
        }
    }
}

/// What a host step contributes to a sandbox: data, never a kind.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Contributions {
    pub environment: BTreeMap<String, String>,
    pub mounts: Vec<PathBuf>,
}

impl Contributions {
    pub fn new(environment: BTreeMap<String, String>, mounts: Vec<PathBuf>) -> Self {
        Self { environment, mounts }
    }
}

/// What a step needs, and where.
#[derive(Debug, PartialEq, Eq)]
pub struct Requirement {
    pub scope: Scope,
    pub name: String,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Scope {
    Host,
    Sandbox,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Sandbox => "sandbox",
        }
    }
}

pub fn parse_step(table: &toml::Table) -> Result<Step, Failure> {
    let identity = table
        .get("identity")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| Failure::misconfigured("every step needs an identity"))?
        .to_string();
    let kind = table
        .get("kind")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| {
            Failure::misconfigured(format!("step '{identity}' needs a kind"))
                .with_step(&identity, "unknown")
        })?
        .to_string();

    // One arm per kind, and nowhere else in the harness.
    let settings = match kind.as_str() {
        "devshell" => {
            let installable = table
                .get("installable")
                .and_then(toml::Value::as_str)
                .ok_or_else(|| missing(&identity, &kind, "installable"))?;
            reject_unknown(table, &identity, &kind, &["identity", "kind", "installable"])?;
            Settings::Devshell {
                installable: installable.to_string(),
            }
        }
        "command" => {
            let run = table
                .get("run")
                .and_then(toml::Value::as_str)
                .ok_or_else(|| missing(&identity, &kind, "run"))?;
            reject_unknown(table, &identity, &kind, &["identity", "kind", "run"])?;
            Settings::Command {
                run: run.to_string(),
            }
        }
        other => {
            return Err(
                Failure::misconfigured(format!("step '{identity}': unknown kind '{other}'"))
                    .with_step(&identity, other),
            );
        }
    };

    Ok(Step { identity, settings })
}

/// Parse a backend's ordered sequence, refusing a second devshell step.
///
/// At most one devshell step may appear in one backend's sequence: two would
/// each contribute an environment and a view, and the later would silently win.
pub fn parse_sequence(tables: &[toml::Table]) -> Result<Vec<Step>, Failure> {
    let mut steps = Vec::new();
    let mut devshell: Option<String> = None;
    for table in tables {
        let step = parse_step(table)?;
        if step.kind() == "devshell" {
            if let Some(first) = &devshell {
                return Err(Failure::misconfigured(format!(
                    "step '{}' is a devshell step, and so is '{first}': at most one devshell step may appear in a backend's sequence",
                    step.identity
                ))
                .with_step(&step.identity, step.kind()));
            }
            devshell = Some(step.identity.clone());
        }
        steps.push(step);
    }
    Ok(steps)
}

fn missing(identity: &str, kind: &str, setting: &str) -> Failure {
    Failure::misconfigured(format!(
        "step '{identity}' ({kind}) needs the setting '{setting}'"
    ))
    .with_step(identity, kind)
    .with_remedy(format!("add '{setting}' to the step's settings"))
}

fn reject_unknown(
    table: &toml::Table,
    identity: &str,
    kind: &str,
    known: &[&str],
) -> Result<(), Failure> {
    for key in table.keys() {
        if !known.contains(&key.as_str()) {
            return Err(Failure::misconfigured(format!(
                "step '{identity}' ({kind}): '{key}' is not a {kind} setting"
            ))
            .with_step(identity, kind)
            .with_remedy("remove the key or move it to a kind that defines it"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(text: &str) -> toml::Table {
        toml::from_str(text).expect("parses")
    }

    #[test]
    fn a_devshell_step_parses_with_its_kind_derived_from_its_settings() {
        let step = parse_step(&table(
            "identity = 'dev'\nkind = 'devshell'\ninstallable = './nix'\n",
        ))
        .expect("parses");
        assert_eq!(step.identity, "dev");
        assert_eq!(step.kind(), "devshell");
        assert_eq!(
            step.settings,
            Settings::Devshell {
                installable: "./nix".to_string()
            }
        );
    }

    #[test]
    fn a_command_step_parses_with_its_kind_derived_from_its_settings() {
        let step = parse_step(&table(
            "identity = 'fetch'\nkind = 'command'\nrun = 'true'\n",
        ))
        .expect("parses");
        assert_eq!(step.kind(), "command");
        assert_eq!(
            step.settings,
            Settings::Command {
                run: "true".to_string()
            }
        );
    }

    #[test]
    fn a_missing_installable_names_the_step_and_the_kind() {
        let error = parse_step(&table("identity = 'dev'\nkind = 'devshell'\n")).unwrap_err();
        assert_eq!(
            error.reason,
            "step 'dev' (devshell) needs the setting 'installable'"
        );
        assert_eq!(error.step.as_deref(), Some("dev"));
        assert_eq!(error.kind.as_deref(), Some("devshell"));
        assert!(error.remedy.is_some());
    }

    #[test]
    fn a_missing_run_names_the_step_and_the_kind() {
        let error = parse_step(&table("identity = 'fetch'\nkind = 'command'\n")).unwrap_err();
        assert_eq!(
            error.reason,
            "step 'fetch' (command) needs the setting 'run'"
        );
    }

    #[test]
    fn an_unknown_key_names_the_step_the_kind_and_the_key() {
        let error = parse_step(&table(
            "identity = 'dev'\nkind = 'devshell'\ninstallable = './nix'\nextra = 1\n",
        ))
        .unwrap_err();
        assert_eq!(
            error.reason,
            "step 'dev' (devshell): 'extra' is not a devshell setting"
        );
        assert_eq!(error.kind.as_deref(), Some("devshell"));
    }

    #[test]
    fn a_key_belonging_to_another_kind_is_refused() {
        let error =
            parse_step(&table("identity = 'fetch'\nkind = 'command'\nrun = 'true'\ninstallable = './nix'\n"))
                .unwrap_err();
        assert!(error.reason.contains("'installable' is not a command setting"), "{}", error.reason);
    }

    #[test]
    fn an_unknown_kind_is_refused_naming_the_step_and_the_kind() {
        let error = parse_step(&table("identity = 'x'\nkind = 'nix'\n")).unwrap_err();
        assert_eq!(error.reason, "step 'x': unknown kind 'nix'");
        assert_eq!(error.step.as_deref(), Some("x"));
        assert_eq!(error.kind.as_deref(), Some("nix"));
    }

    #[test]
    fn a_missing_identity_is_refused() {
        let error = parse_step(&table("kind = 'devshell'\ninstallable = './nix'\n")).unwrap_err();
        assert_eq!(error.reason, "every step needs an identity");
    }

    #[test]
    fn a_missing_kind_names_the_step() {
        let error = parse_step(&table("identity = 'dev'\ninstallable = './nix'\n")).unwrap_err();
        assert_eq!(error.reason, "step 'dev' needs a kind");
    }

    #[test]
    fn a_non_string_setting_is_refused_rather_than_coerced() {
        let error =
            parse_step(&table("identity = 'dev'\nkind = 'devshell'\ninstallable = 7\n")).unwrap_err();
        assert!(error.reason.contains("installable"), "{}", error.reason);
    }

    #[test]
    fn two_devshell_steps_in_one_sequence_are_refused_naming_both() {
        let first = table("identity = 'a'\nkind = 'devshell'\ninstallable = './nix'\n");
        let second = table("identity = 'b'\nkind = 'devshell'\ninstallable = './nix'\n");
        let error = parse_sequence(&[first, second]).unwrap_err();
        assert_eq!(
            error.reason,
            "step 'b' is a devshell step, and so is 'a': at most one devshell step may appear in a backend's sequence"
        );
        assert_eq!(error.step.as_deref(), Some("b"));
    }

    #[test]
    fn one_devshell_step_among_commands_is_accepted() {
        let sequence = vec![
            table("identity = 'fetch'\nkind = 'command'\nrun = 'true'\n"),
            table("identity = 'dev'\nkind = 'devshell'\ninstallable = './nix'\n"),
        ];
        let steps = parse_sequence(&sequence).expect("parses");
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[1].kind(), "devshell");
    }
}
