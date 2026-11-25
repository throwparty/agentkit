//! The composition point.
//!
//! This is the only module that constructs a VCS backend. The CLI and MCP
//! server call [`open`] and hold an orchestrator; neither they nor the
//! orchestrator name a backend.

use std::path::Path;
use std::sync::Arc;

use crate::config::Config;
use crate::domain::{SandboxError, ScmMode};
use crate::orchestrator::{Sandboxes, resolve_project_slug};
use crate::vcs::git::{GitVcs, GitVcsConfig, VcsStore};
use crate::vcs::jj::{JjVcs, JjVcsConfig};

/// Build the orchestrator for the project the current directory belongs to.
pub fn open(config: Config) -> Result<Sandboxes, SandboxError> {
    let project_slug = resolve_project_slug(&config)?;
    let tool = config.vcs_tool.clone().ok_or_else(|| {
        SandboxError::Config(
            "no VCS is selected; set `tool = 'git'` or `tool = 'jj'` under [vcs] in \
             .agentkit/litterbox/config.local.toml"
                .to_string(),
        )
    })?;

    if tool == "jj" {
        open_jj(config, &project_slug)
    } else {
        open_git_project(config, &project_slug)
    }
}

/// Build the orchestrator over the Git backend.
fn open_git_project(config: Config, project_slug: &str) -> Result<Sandboxes, SandboxError> {
    let git = git_settings(&config)?;
    let mode = git.snapshot_mode.unwrap_or_default();
    let remote = mode == ScmMode::Remote;
    let vcs = open_git(&config, project_slug, mode, git.config.as_deref())?;

    Sandboxes::with_vcs(config, Arc::new(vcs), remote, git.binding)
}

/// Build the orchestrator over the jj backend.
fn open_jj(config: Config, project_slug: &str) -> Result<Sandboxes, SandboxError> {
    let section = config
        .vcs_settings
        .get("jj")
        .cloned()
        .unwrap_or(toml::Value::Table(Default::default()));
    let jj = JjVcsConfig::from_section(&section).map_err(SandboxError::Config)?;
    let vcs = JjVcs::open(project_slug, jj.config)?;
    Sandboxes::with_vcs(config, Arc::new(vcs), true, jj.binding)
}

/// Git's section from the workspace's VCS settings.
fn git_settings(config: &Config) -> Result<GitVcsConfig, SandboxError> {
    let section = config
        .vcs_settings
        .get("git")
        .cloned()
        .unwrap_or(toml::Value::Table(Default::default()));
    GitVcsConfig::from_section(&section).map_err(SandboxError::Config)
}

fn open_git(
    config: &Config,
    project_slug: &str,
    mode: ScmMode,
    vcs_config: Option<&str>,
) -> Result<GitVcs, SandboxError> {
    let host = Path::new(".");

    let vcs = match mode {
        ScmMode::Direct => {
            if vcs_config.is_some() {
                return Err(SandboxError::Config(
                    "vcs.git config applies only when Litterbox owns its clone; remove it or \
                     use snapshot-mode = 'remote'"
                        .to_string(),
                ));
            }
            GitVcs::open_with_mode_and_prefix(host, mode, config.project.slug.clone(), None)?
        }
        ScmMode::Remote => {
            let bare: std::path::PathBuf = VcsStore::clone_bare(host, project_slug)?;
            crate::vcs::git::apply_config(&bare, vcs_config)?;
            VcsStore::install_remote(host, &bare)?;
            let host_abs = host.canonicalize().unwrap_or_else(|_| host.to_path_buf());
            GitVcs::open_with_mode_and_prefix(
                &bare,
                mode,
                config.project.slug.clone(),
                Some(host_abs),
            )?
        }
    };

    Ok(vcs.set_store_slug(project_slug.to_string()))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use std::sync::Arc;

    use tempfile::TempDir;

    use crate::config::{Config, ProjectConfig};
    use crate::domain::ScmMode;
    use crate::orchestrator::Sandboxes;
    use crate::sandbox::contract::WorkspaceBinding;
    use crate::sandbox::selected::Selected;
    use crate::testing::TestBackend;
    use crate::vcs::git::GitVcs;

    #[test]
    fn git_settings_read_the_binding_from_the_section() {
        let mut config = Config::default();
        config.vcs_settings.insert(
            "git".to_string(),
            toml::from_str(
                "binding = 'bound'\nsnapshot-mode = 'direct'\nconfig = '[gc]\\n\\treflogExpire = 30.days\\n'",
            )
            .expect("section"),
        );

        let git = super::git_settings(&config).expect("settings");
        assert_eq!(git.binding, Some(WorkspaceBinding::Bound));
        assert_eq!(git.snapshot_mode, Some(ScmMode::Direct));
        assert!(
            git.config
                .as_deref()
                .is_some_and(|value| value.contains("reflogExpire")),
            "{:?}",
            git.config
        );
    }

    #[test]
    fn git_config_is_refused_under_direct() {
        let config = Config {
            project: ProjectConfig {
                slug: Some("project".to_string()),
                backends: None,
            },
            ..Config::default()
        };
        let result = super::open_git(
            &config,
            "project",
            ScmMode::Direct,
            Some("[gc]\n\treflogExpire = 1.day\n"),
        );
        let error = match result {
            Ok(_) => panic!("a direct clone has no place for the config"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("vcs.git config"), "{error}");
    }

    #[test]
    fn an_absent_binding_stays_unset() {
        let git = super::git_settings(&Config::default()).expect("settings");
        assert_eq!(
            git.binding, None,
            "no section must not become a silent copy"
        );
    }

    #[test]
    fn an_unknown_binding_fails_with_the_key_named() {
        let mut config = Config::default();
        config.vcs_settings.insert(
            "git".to_string(),
            toml::from_str("binding = 'hardlink'").expect("section"),
        );

        let error = super::git_settings(&config).expect_err("unknown value");
        let message = error.to_string();
        assert!(message.contains("vcs.git"), "{message}");
        assert!(
            message.contains("copied") && message.contains("bound"),
            "{message}"
        );
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .args(args)
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?}");
    }

    fn repository() -> TempDir {
        let dir = TempDir::new().expect("tempdir");
        git(dir.path(), &["init", "-q"]);
        fs::write(dir.path().join("README.md"), "hello").expect("write");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-qm", "init"]);
        dir
    }

    fn branch_exists(dir: &Path, name: &str) -> bool {
        Command::new("git")
            .current_dir(dir)
            .args(["rev-parse", "--verify", &format!("refs/heads/{name}")])
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    fn selected(backend: Arc<TestBackend>) -> Selected {
        Selected::new("test", backend.clone()).with_lifecycle(backend)
    }

    #[tokio::test]
    async fn create_list_and_delete_through_the_git_backend() {
        let dir = repository();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let config = Config {
            project: ProjectConfig {
                slug: Some("project".to_string()),
                backends: Some(vec!["test".to_string()]),
            },
            ..Config::default()
        };
        let vcs = GitVcs::open(dir.path()).expect("vcs");
        let sandboxes = Sandboxes::for_test(
            config,
            Arc::new(vcs),
            false,
            "project".to_string(),
            records.path().to_path_buf(),
            selected(backend),
        );

        let created = sandboxes.create("demo").await.expect("create");
        assert_eq!(created.value.name, "demo");
        assert!(
            branch_exists(dir.path(), "litterbox/demo"),
            "the branch is created"
        );
        assert_eq!(sandboxes.list().expect("list").sandboxes.len(), 1);

        sandboxes
            .delete("demo", crate::orchestrator::DeleteGuards::ALL)
            .await
            .expect("delete");
        assert!(
            !branch_exists(dir.path(), "litterbox/demo"),
            "the branch is removed"
        );
    }
}
