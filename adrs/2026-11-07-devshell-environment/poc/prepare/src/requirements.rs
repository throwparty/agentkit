//! Requirements: what must exist for a step to be applied.
//!
//! Two scopes, two checkers. A host requirement is about this machine; a sandbox
//! requirement is about the view the backend is about to create, so it is checked
//! against the same view the launcher will use.

use std::path::{Component, PathBuf};

use crate::failure::{Class, Failure};
use crate::launch::View;
use crate::model::{Requirement, Scope};

/// Refuse a host requirement this machine does not meet.
pub fn check_host(requirements: &[Requirement]) -> Result<(), Failure> {
    for required in requirements.iter().filter(|r| r.scope == Scope::Host) {
        let path = normalise(&required.name)?;
        if !path.exists() {
            return Err(Failure::unavailable(
                format!("host requirement '{}'", required.name),
                format!("'{}' does not exist on this host", required.name),
            )
            .with_remedy("install what the step needs, or remove the requirement"));
        }
    }
    Ok(())
}

/// Refuse a sandbox requirement the view cannot meet.
pub fn check_sandbox(backend: &str, view: &View, requirements: &[Requirement]) -> Result<(), Failure> {
    for required in requirements.iter().filter(|r| r.scope == Scope::Sandbox) {
        let path = normalise(&required.name)?;
        if !view.provides(&path) {
            return Err(Failure::new(
                Class::Unsupported,
                format!(
                    "requirement '{}' ({} scope) is not inside the view '{}' provides: neither a mount, the workspace, nor a path the launcher creates",
                    required.name,
                    required.scope.as_str(),
                    backend
                ),
            )
            .with_backend(backend)
            .with_primitive(format!("requirement '{}'", required.name))
            .with_remedy("add the path to the contributions, or remove the requirement"));
        }
        // Everything the view binds is bound from the host, so a path that does
        // not exist here will not exist in the sandbox either. The launcher's own
        // directories are the exception: it creates them.
        if !view.is_synthetic(&path) && !path.exists() {
            return Err(Failure::new(
                Class::Unsupported,
                format!(
                    "requirement '{}' ({} scope) is inside the view '{}' provides, but '{}' does not exist",
                    required.name,
                    required.scope.as_str(),
                    backend,
                    required.name
                ),
            )
            .with_backend(backend)
            .with_primitive(format!("requirement '{}'", required.name))
            .with_remedy("create the path, contribute a mount that carries it, or remove the requirement"));
        }
    }
    Ok(())
}

/// A path with no `.` or `..` component, so a prefix comparison means what it
/// says: `Path::starts_with` does not resolve either.
pub fn normalise(named: &str) -> Result<PathBuf, Failure> {
    let path = PathBuf::from(named);
    if !path.is_absolute() {
        return Err(Failure::misconfigured(format!(
            "requirement '{named}' is not an absolute path"
        ))
        .with_remedy("name an absolute path"));
    }
    for component in path.components() {
        if matches!(component, Component::ParentDir | Component::CurDir) {
            return Err(Failure::misconfigured(format!(
                "requirement '{named}' contains '{}', which a requirement never needs",
                component.as_os_str().to_string_lossy()
            ))
            .with_remedy("name the path the sandbox will actually see"));
        }
    }
    Ok(path.components().collect())
}

/// Every requirement, both scopes, in the order a refusal should be reported.
pub fn check(
    backend: &str,
    view: &View,
    requirements: &[Requirement],
) -> Result<(), Failure> {
    check_host(requirements)?;
    check_sandbox(backend, view, requirements)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn view(reads: &[&str], workspace: &str) -> View {
        View::new(
            reads.iter().map(PathBuf::from).collect(),
            std::path::Path::new(workspace),
            false,
        )
        .expect("a view")
    }

    fn requirement(scope: Scope, name: &str) -> Requirement {
        Requirement {
            scope,
            name: name.to_string(),
        }
    }

    fn scratch() -> PathBuf {
        let path = std::env::temp_dir().join(format!("devshell-poc-req-{}", std::process::id()));
        std::fs::create_dir_all(&path).expect("scratch");
        path
    }

    fn scratch_named(name: &str) -> PathBuf {
        let path = scratch().join(name);
        std::fs::create_dir_all(&path).expect("scratch");
        path
    }

    #[test]
    fn a_path_inside_a_mount_is_satisfied() {
        let workspace = scratch();
        let mount = scratch_named("mount");
        std::fs::create_dir_all(mount.join("bin")).expect("mount");
        std::fs::write(mount.join("bin/tool"), b"").expect("tool");
        let view = view(&[&mount.display().to_string()], &workspace.display().to_string());
        let tool = mount.join("bin/tool");
        let requirements = [requirement(Scope::Sandbox, &tool.display().to_string())];
        check_sandbox("bwrap", &view, &requirements).expect("satisfied");
    }

    #[test]
    fn a_path_inside_the_workspace_is_satisfied() {
        let workspace = scratch();
        let view = view(&[], &workspace.display().to_string());
        let inside = workspace.join("target");
        std::fs::create_dir_all(&inside).expect("the workspace has the path");
        let requirements = [requirement(Scope::Sandbox, &inside.display().to_string())];
        check_sandbox("bwrap", &view, &requirements).expect("satisfied");
    }

    #[test]
    fn a_workspace_path_that_does_not_exist_is_refused() {
        let workspace = scratch();
        let view = view(&[], &workspace.display().to_string());
        let absent = workspace.join("bin/tool");
        let requirements = [requirement(Scope::Sandbox, &absent.display().to_string())];
        let failure = check_sandbox("bwrap", &view, &requirements).unwrap_err();
        assert_eq!(failure.class, Class::Unsupported);
        assert!(failure.reason.contains("does not exist"), "{}", failure.reason);
    }

    #[test]
    fn a_path_the_launcher_creates_is_satisfied_without_a_host_equivalent() {
        let workspace = scratch();
        let view = view(&["/nix/store/aaa-dev"], &workspace.display().to_string());
        let requirements = [
            requirement(Scope::Sandbox, "/tmp"),
            requirement(Scope::Sandbox, "/dev/null"),
        ];
        check_sandbox("bwrap", &view, &requirements).expect("the launcher creates both");
    }

    #[test]
    fn a_host_file_the_view_binds_satisfies_a_requirement() {
        // /etc/hosts is in the view's allowlist, so asking for it must not be
        // refused just because it is not in the store closure.
        let workspace = scratch();
        let view = view(&["/nix/store/aaa-dev"], &workspace.display().to_string());
        if !Path::new("/etc/hosts").exists() {
            return;
        }
        let requirements = [requirement(Scope::Sandbox, "/etc/hosts")];
        check_sandbox("bwrap", &view, &requirements).expect("the same view is checked and mounted");
    }

    #[test]
    fn an_unsatisfied_requirement_names_the_backend_and_a_remedy() {
        let workspace = scratch();
        let view = view(&["/nix/store/aaa-dev"], &workspace.display().to_string());
        let requirements = [requirement(Scope::Sandbox, "/usr/bin/rg")];
        let failure = check_sandbox("bwrap", &view, &requirements).unwrap_err();
        assert_eq!(failure.class, Class::Unsupported);
        assert_eq!(failure.backend.as_deref(), Some("bwrap"));
        assert!(failure.reason.contains("/usr/bin/rg"), "{}", failure.reason);
        assert!(failure.remedy.is_some());
    }

    #[test]
    fn a_requirement_with_a_parent_component_is_refused() {
        let failure = normalise("/nix/store/aaa/../../etc/passwd").unwrap_err();
        assert_eq!(failure.class, Class::Misconfigured);
        assert!(failure.reason.contains("'..'"), "{}", failure.reason);
    }

    #[test]
    fn a_relative_requirement_is_refused() {
        let failure = normalise("bin/tool").unwrap_err();
        assert!(failure.reason.contains("not an absolute path"));
    }

    #[test]
    fn a_host_requirement_this_machine_lacks_is_unavailable() {
        let requirements = [requirement(Scope::Host, "/definitely/not/here")];
        let failure = check_host(&requirements).unwrap_err();
        assert_eq!(failure.class, Class::Unavailable);
        assert!(failure.remedy.is_some());
    }

    #[test]
    fn a_host_requirement_this_machine_has_is_satisfied() {
        let requirements = [requirement(Scope::Host, "/etc/hosts")];
        if Path::new("/etc/hosts").exists() {
            check_host(&requirements).expect("satisfied");
        }
    }

    #[test]
    fn each_scope_is_checked_by_its_own_oracle() {
        let workspace = scratch();
        let view = view(&["/nix/store/aaa-dev"], &workspace.display().to_string());
        // A host path that exists but is not in the view: refused for the
        // sandbox scope, accepted for the host scope.
        let host_only = "/usr/bin";
        if !Path::new(host_only).exists() {
            return;
        }
        let sandbox = [requirement(Scope::Sandbox, host_only)];
        assert!(check_sandbox("bwrap", &view, &sandbox).is_err());
        let host = [requirement(Scope::Host, host_only)];
        check_host(&host).expect("the host has it");
    }
}
