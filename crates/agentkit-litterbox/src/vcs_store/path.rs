use std::path::PathBuf;

pub fn resolve_path(slug: &str) -> PathBuf {
    let base = data_dir();
    base.join("vcs").join("git").join(slug)
}

pub(crate) fn data_dir() -> PathBuf {
    if let Ok(override_dir) = std::env::var("LITTERBOX_DATA_DIR") {
        return PathBuf::from(override_dir);
    }
    match std::env::consts::OS {
        "macos" => macos_data_dir(),
        "windows" => windows_data_dir(),
        _ => linux_data_dir(),
    }
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("~"))
}

fn macos_data_dir() -> PathBuf {
    home_dir().join("Library").join("Application Support").join("AgentKit").join("Litterbox")
}

fn linux_data_dir() -> PathBuf {
    home_dir().join(".local").join("state").join("agentkit").join("litterbox")
}

fn windows_data_dir() -> PathBuf {
    home_dir().join("AppData").join("LocalLow").join("AgentKit").join("Litterbox")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_path_uses_slug() {
        let path = resolve_path("my-project");
        assert!(path.to_string_lossy().ends_with("vcs/git/my-project"));
    }

    #[test]
    fn resolve_path_includes_vcs_git() {
        let path = resolve_path("test");
        let lossy = path.to_string_lossy();
        assert!(lossy.contains("vcs/git/test"), "path: {lossy}");
    }

    #[test]
    fn platform_resolves_to_known_base() {
        let base = data_dir();
        let lossy = base.to_string_lossy();
        match std::env::consts::OS {
            "macos" => assert!(lossy.contains("Application Support/AgentKit/Litterbox") || lossy.contains("Application Support")),
            "windows" => assert!(lossy.contains("AppData/LocalLow/AgentKit/Litterbox") || lossy.contains("AppData")),
            _ => assert!(lossy.contains(".local/state/agentkit/litterbox")),
        }
    }

    #[test]
    fn data_dir_is_absolute() {
        let base = data_dir();
        assert!(base.is_absolute(), "data_dir must be absolute, got: {base:?}");
    }
}
