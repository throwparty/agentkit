//! One repair at a time, per project and sandbox name.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

use crate::sandbox::contract::Error;

/// A held per-sandbox lock.
pub struct Guard {
    _file: Option<std::fs::File>,
    _local: OwnedMutexGuard<()>,
}

/// Acquire the lock for one sandbox.
///
/// The in-process guard serialises threads in this process; the advisory file
/// lock serialises separate processes such as a CLI invocation and the MCP
/// server. The file lock is released when the descriptor closes, including on
/// process exit.
pub async fn acquire(records_dir: &Path, name: &str) -> Result<Guard, Error> {
    let key = records_dir.join(name);
    let local = local_lock(&key).lock_owned().await;

    let directory = records_dir.join(".locks");
    std::fs::create_dir_all(&directory).map_err(|error| {
        Error::failed(format!(
            "{} could not be created: {error}",
            directory.display()
        ))
    })?;
    let path = directory.join(format!("{name}.lock"));
    let file = tokio::task::spawn_blocking(move || open_and_lock(&path))
        .await
        .map_err(|error| Error::failed(format!("the sandbox lock task failed: {error}")))??;

    Ok(Guard {
        _file: Some(file),
        _local: local,
    })
}

fn local_lock(key: &Path) -> Arc<AsyncMutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<AsyncMutex<()>>>>> = OnceLock::new();
    let locks = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut locks = locks.lock().unwrap_or_else(|poison| poison.into_inner());
    locks
        .entry(key.to_path_buf())
        .or_insert_with(|| Arc::new(AsyncMutex::new(())))
        .clone()
}

fn open_and_lock(path: &Path) -> Result<std::fs::File, Error> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .map_err(|error| {
            Error::failed(format!("{} could not be opened: {error}", path.display()))
        })?;

    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        // SAFETY: flock operates on an open descriptor and holds no memory
        // invariants. The lock is advisory and released when the file closes.
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if result != 0 {
            return Err(Error::failed(format!(
                "{} could not be locked: {}",
                path.display(),
                std::io::Error::last_os_error()
            )));
        }
    }

    Ok(file)
}
