use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use agentkit_path::data_dir;
use serde_json::Value;
use sysinfo::{Pid, ProcessesToUpdate, System};

struct LockGuard {
    path: PathBuf,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
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
    loop {
        ensure_dir(&path).map_err(|e| format!("cannot create lock directory: {e}"))?;
        match File::create_new(&path) {
            Ok(file) => {
                let pid = std::process::id();
                let mut writer = file;
                writeln!(writer, "{pid}").map_err(|e| format!("lock write: {e}"))?;
                writer.flush().map_err(|e| format!("lock flush: {e}"))?;
                return Ok(LockGuard { path });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let content = std::fs::read_to_string(&path).unwrap_or_default();
                let pid: u32 = content.trim().parse().unwrap_or(0);

                if pid == 0 || !is_credential_helper_alive(pid) {
                    eprintln!("[credential-file] removing stale lock file (pid {pid})");
                    if let Err(e) = std::fs::remove_file(&path) {
                        return Err(format!("cannot remove stale lock: {e}"));
                    }
                    continue;
                }

                return Err(format!(
                    "credential store locked by process {pid}; try again later"
                ));
            }
            Err(e) => return Err(format!("cannot create lock file: {e}")),
        }
    }
}

fn is_credential_helper_alive(pid: u32) -> bool {
    let mut system = System::new();
    let sys_pid = Pid::from(pid as usize);
    system.refresh_processes(ProcessesToUpdate::Some(&[sys_pid]), false);
    system
        .process(sys_pid)
        .and_then(|p| p.exe())
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.contains("agentkit-credential-file"))
}

fn read_store(component: &str) -> HashMap<String, Value> {
    let path = credentials_path(component);
    if !path.exists() {
        return HashMap::new();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_store(component: &str, store: &HashMap<String, Value>) -> std::io::Result<()> {
    let path = credentials_path(component);
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
