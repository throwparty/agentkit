use thiserror::Error;

#[derive(Error, Debug)]
pub enum SandboxError {
    #[error("Invalid sandbox name: '{name}'. {reason}")]
    InvalidName { name: String, reason: String },
    #[error("Sandbox '{name}' already exists.")]
    SandboxExists { name: String },
    #[error("Sandbox '{name}' not found.")]
    SandboxNotFound { name: String },
    #[error("SCM error: {0}")]
    Scm(#[from] ScmError),
    #[error("Setup command failed with exit code {exit_code}: {stderr}")]
    SetupCommandFailed { exit_code: i32, stderr: String },
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Configuration error: {0}")]
    Config(String),
}

#[derive(Error, Debug)]
pub enum ScmError {
    #[error("Git repository open failed: {source}")]
    Open {
        #[source]
        source: git2::Error,
    },
    #[error("Git branch listing failed: {source}")]
    BranchList {
        #[source]
        source: git2::Error,
    },
    #[error("Git branch creation failed: {source}")]
    BranchCreate {
        #[source]
        source: git2::Error,
    },
    #[error("Git branch deletion failed: {source}")]
    BranchDelete {
        #[source]
        source: git2::Error,
    },
    #[error("Git archive failed: {source}")]
    Archive {
        #[source]
        source: git2::Error,
    },
    #[error("Git status failed: {source}")]
    Status {
        #[source]
        source: git2::Error,
    },
    #[error("Git index add failed: {source}")]
    IndexAdd {
        #[source]
        source: git2::Error,
    },
    #[error("Git index write failed: {source}")]
    IndexWrite {
        #[source]
        source: git2::Error,
    },
    #[error("Git index write tree failed: {source}")]
    IndexWriteTree {
        #[source]
        source: git2::Error,
    },
    #[error("Git commit failed: {source}")]
    Commit {
        #[source]
        source: git2::Error,
    },
    #[error("Git signature failed: {source}")]
    Signature {
        #[source]
        source: git2::Error,
    },
    #[error("Git head failed: {source}")]
    Head {
        #[source]
        source: git2::Error,
    },
    #[error("Git reference failed: {source}")]
    Reference {
        #[source]
        source: git2::Error,
    },
    #[error("failed to apply patch: {message}")]
    ApplyPatch { message: String },
    #[error("Git clone failed: {source}")]
    Clone {
        #[source]
        source: git2::Error,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScmMode {
    Direct,
    #[default]
    Remote,
}

pub fn slugify(name: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = false;

    for ch in name.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            slug.push(lower);
            last_was_dash = false;
        } else if !last_was_dash {
            slug.push('-');
            last_was_dash = true;
        }
    }

    slug.trim_matches('-').to_string()
}

const MAX_SLUG_LENGTH: usize = 63;

pub fn validate_slug(original: &str, slug: &str) -> Result<(), SandboxError> {
    // 63 keeps identifiers manageable and aligns with the spec requirement.
    let valid = !slug.is_empty()
        && slug.len() <= MAX_SLUG_LENGTH
        && slug
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-');

    if valid {
        Ok(())
    } else {
        Err(SandboxError::InvalidName {
            name: original.to_string(),
            reason: "Slugified names must be 1-63 characters and contain only [a-z0-9-]."
                .to_string(),
        })
    }
}

pub fn slugify_name(name: &str) -> Result<String, SandboxError> {
    let slug = slugify(name);
    validate_slug(name, &slug)?;
    Ok(slug)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_replaces_and_collapses() {
        let slug = slugify("My Feature Name!@#");
        assert_eq!(slug, "my-feature-name");
    }

    #[test]
    fn slugify_trims_dashes() {
        let slug = slugify("---Hello---World---");
        assert_eq!(slug, "hello-world");
    }

    #[test]
    fn slugify_name_rejects_empty_slug() {
        let err = slugify_name("----").expect_err("expected invalid name");
        assert_eq!(
            err.to_string(),
            "Invalid sandbox name: '----'. Slugified names must be 1-63 characters and contain only [a-z0-9-]."
        );
    }

    #[test]
    fn setup_command_failed_formats_error() {
        let err = SandboxError::SetupCommandFailed {
            exit_code: 1,
            stderr: "boom".to_string(),
        };
        let message = err.to_string();
        assert!(message.contains("exit code 1"));
        assert!(message.contains("boom"));
    }

    #[test]
    fn scm_mode_default_is_remote() {
        assert_eq!(ScmMode::default(), ScmMode::Remote);
    }
}
