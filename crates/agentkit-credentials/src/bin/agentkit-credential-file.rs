use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use agentkit_credentials::CredentialBlob;

fn credentials_path() -> PathBuf {
    if let Ok(dir) = std::env::var("AGENTKIT_DATA_DIR") {
        return PathBuf::from(dir).join("credentials.json");
    }
    let base = match std::env::consts::OS {
        "macos" => {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            PathBuf::from(home).join("Library").join("Application Support").join("AgentKit").join("switchboard")
        }
        "windows" => {
            let home = std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into());
            PathBuf::from(home).join("AppData").join("LocalLow").join("AgentKit").join("switchboard")
        }
        _ => {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            PathBuf::from(home).join(".local").join("state").join("agentkit").join("switchboard")
        }
    };
    base.join("credentials.json")
}

fn ensure_dir(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).ok();
        }
    }
    Ok(())
}

fn read_store() -> HashMap<String, CredentialBlob> {
    let path = credentials_path();
    if !path.exists() {
        return HashMap::new();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_store(store: &HashMap<String, CredentialBlob>) -> std::io::Result<()> {
    let path = credentials_path();
    ensure_dir(&path)?;
    let json = serde_json::to_string_pretty(store).unwrap_or_default();
    std::fs::write(&path, &json)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).ok();
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: agentkit-credential-file <get|store|erase> <identity>");
        return ExitCode::from(2);
    }

    let command = &args[1];
    let identity = &args[2];

    match command.as_str() {
        "get" => cmd_get(identity),
        "store" => cmd_store(identity),
        "erase" => cmd_erase(identity),
        _ => {
            eprintln!("unknown command: {command}");
            ExitCode::from(2)
        }
    }
}

fn cmd_get(identity: &str) -> ExitCode {
    let store = read_store();
    match store.get(identity) {
        Some(blob) => {
            let json = serde_json::to_string(blob).unwrap_or_default();
            println!("{json}");
            ExitCode::SUCCESS
        }
        None => ExitCode::from(1),
    }
}

fn cmd_store(identity: &str) -> ExitCode {
    let blob = match agentkit_credentials::read_stdin() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let mut store = read_store();
    store.insert(identity.to_string(), blob);
    match write_store(&store) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("failed to write credentials: {e}");
            ExitCode::FAILURE
        }
    }
}

fn cmd_erase(identity: &str) -> ExitCode {
    let mut store = read_store();
    store.remove(identity);
    match write_store(&store) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("failed to write credentials: {e}");
            ExitCode::FAILURE
        }
    }
}
