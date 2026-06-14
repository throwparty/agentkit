use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use agentkit_path::data_dir;
use fs2::FileExt;
use serde_json::Value;

struct LockGuard {
    file: File,
    path: PathBuf,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
        let _ = std::fs::remove_file(&self.path);
    }
}

fn credentials_path(component: &str) -> PathBuf {
    data_dir(component).join("credentials.json")
}

fn lock_path(component: &str) -> PathBuf {
    let mut path = credentials_path(component);
    path.set_file_name("credentials.lock");
    path
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

fn acquire_lock(component: &str) -> Result<LockGuard, String> {
    let path = lock_path(component);
    ensure_dir(&path).map_err(|e| format!("cannot create lock directory: {e}"))?;

    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(|e| format!("cannot open lock file: {e}"))?;

    match file.try_lock_exclusive() {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
            let pid_hint = std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok())
                .map(|pid| format!(" (holder pid {pid})"))
                .unwrap_or_default();
            return Err(format!(
                "credential store locked{pid_hint}; try again later"
            ));
        }
        Err(e) => return Err(format!("cannot acquire lock: {e}")),
    }

    file.set_len(0).map_err(|e| format!("lock truncate: {e}"))?;
    let pid = std::process::id();
    {
        let mut writer = &file;
        writeln!(writer, "{pid}").map_err(|e| format!("lock write: {e}"))?;
        writer.flush().map_err(|e| format!("lock flush: {e}"))?;
    }

    Ok(LockGuard { file, path })
}

fn read_store(component: &str) -> HashMap<String, Value> {
    let path = credentials_path(component);
    if !path.exists() {
        return HashMap::new();
    }
    let content = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "[credential-file] warning: cannot read {}: {e}",
                path.display()
            );
            return HashMap::new();
        }
    };
    match serde_json::from_str(&content) {
        Ok(map) => map,
        Err(e) => {
            eprintln!(
                "[credential-file] warning: corrupted credential store {}: {e}",
                path.display()
            );
            HashMap::new()
        }
    }
}

fn write_store(component: &str, store: &HashMap<String, Value>) -> std::io::Result<()> {
    let path = credentials_path(component);
    ensure_dir(&path)?;
    let json = serde_json::to_string_pretty(store)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp_path = dir.join(format!(".credentials.json.tmp.{}", std::process::id()));

    {
        let mut tmp = std::fs::File::create(&tmp_path)?;
        tmp.write_all(json.as_bytes())?;
        tmp.flush()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o600)).ok();
            let _ = tmp.sync_all();
        }
    }
    std::fs::rename(&tmp_path, &path)?;
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
        eprintln!("usage: agentkit-credential-file <get|put|delete|location> <component> [<identity>]");
        return ExitCode::from(2);
    }

    let command = &args[1];
    let component = &args[2];

    if command == "location" {
        cmd_location(component);
        return ExitCode::SUCCESS;
    }

    if args.len() < 4 {
        eprintln!("usage: agentkit-credential-file <get|put|delete|location> <component> [<identity>]");
        return ExitCode::from(2);
    }

    let identity = &args[3];

    let _lock = match acquire_lock(component) {
        Ok(lock) => lock,
        Err(e) => {
            eprintln!("[credential-file] {e}");
            return ExitCode::FAILURE;
        }
    };

    match command.as_str() {
        "get" => cmd_get(component, identity),
        "put" => cmd_put(component, identity),
        "delete" => cmd_delete(component, identity),
        _ => {
            eprintln!("unknown command: {command}");
            ExitCode::from(2)
        }
    }
}

fn cmd_location(component: &str) {
    eprintln!("warning: credential file stores credentials in cleartext on disk.");
    eprintln!("         consider using 'keychain' for better security.");
    println!("{}", credentials_path(component).display());
}

fn cmd_get(component: &str, identity: &str) -> ExitCode {
    let store = read_store(component);
    match store.get(identity) {
        Some(blob) => {
            let json = serde_json::to_string(blob).unwrap_or_default();
            println!("{json}");
            ExitCode::SUCCESS
        }
        None => ExitCode::from(1),
    }
}

fn cmd_put(component: &str, identity: &str) -> ExitCode {
    let blob = match agentkit_credentials::read_stdin() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let mut store = read_store(component);
    store.insert(identity.to_string(), blob);
    match write_store(component, &store) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("failed to write credentials: {e}");
            ExitCode::FAILURE
        }
    }
}

fn cmd_delete(component: &str, identity: &str) -> ExitCode {
    let mut store = read_store(component);
    store.remove(identity);
    match write_store(component, &store) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("failed to write credentials: {e}");
            ExitCode::FAILURE
        }
    }
}
