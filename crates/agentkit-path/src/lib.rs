use std::path::PathBuf;

/// Returns the platform-specific data directory for an AgentKit component.
///
/// Creates a directory tree under the platform's standard data home:
/// - Linux:   `$HOME/.local/state/agentkit/<component>/`
/// - macOS:   `$HOME/Library/Application Support/AgentKit/<component>/`
/// - Windows: `$USERPROFILE/AppData/LocalLow/AgentKit/<component>/`
pub fn data_dir(component: &str) -> PathBuf {
    let base = match std::env::consts::OS {
        "macos" => {
            let home = home_dir();
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("AgentKit")
        }
        "windows" => {
            let home = home_dir();
            PathBuf::from(home)
                .join("AppData")
                .join("LocalLow")
                .join("AgentKit")
        }
        _ => {
            let home = home_dir();
            PathBuf::from(home)
                .join(".local")
                .join("state")
                .join("agentkit")
        }
    };
    base.join(component)
}

fn home_dir() -> String {
    #[cfg(target_os = "macos")]
    {
        std::env::var("HOME").unwrap_or_else(|_| ".".into())
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        std::env::var("HOME").unwrap_or_else(|_| ".".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_data_dir_ends_with_component() {
        let path = data_dir("switchboard");
        assert!(path.ends_with("switchboard"));
    }

    #[test]
    fn test_data_dir_is_absolute() {
        let path = data_dir("test");
        assert!(path.is_absolute());
    }
}
