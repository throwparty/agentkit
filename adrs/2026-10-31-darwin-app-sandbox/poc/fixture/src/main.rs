use std::io;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("HOME"))
}

enum Outcome {
    Denied,
    Leak,
    Refused,
    Error(String),
}

fn classify(kind: io::ErrorKind, message: String) -> Outcome {
    match kind {
        io::ErrorKind::PermissionDenied => Outcome::Denied,
        io::ErrorKind::ConnectionRefused => Outcome::Refused,
        _ => Outcome::Error(message),
    }
}

/// Report one check. A path that exists after the attempt is a leak regardless
/// of what the syscall claimed.
fn check(name: &str, target: Option<&Path>, attempt: impl FnOnce() -> io::Result<()>) -> u8 {
    let outcome = match attempt() {
        Ok(()) => Outcome::Leak,
        Err(error) => classify(error.kind(), error.to_string()),
    };

    if let Some(target) = target {
        if target.exists() {
            println!("{name}: leak (artefact present)");
            return 1;
        }
    }

    match outcome {
        Outcome::Denied => {
            println!("{name}: denied");
            0
        }
        Outcome::Leak => {
            println!("{name}: leak");
            1
        }
        Outcome::Refused => {
            println!("{name}: refused (reached the network stack)");
            1
        }
        Outcome::Error(message) => {
            println!("{name}: error ({message})");
            1
        }
    }
}

fn connect() -> io::Result<()> {
    let address: std::net::SocketAddr = "127.0.0.1:9".parse().expect("address");
    TcpStream::connect_timeout(&address, Duration::from_secs(2)).map(|_| ())
}

fn hostile() -> i32 {
    let home = home();
    // Unique per run: a leaked artefact from one prototype must not be read as
    // a leak by the next.
    let label = std::env::var("LBX_RUN_LABEL").unwrap_or_else(|_| "default".to_string());
    let outside = PathBuf::from(format!("/private/tmp/lbx-escape-{label}"));
    let home_escape = home.join("lbx-escape");
    let child_escape = home.join("lbx-child");
    let ssh_key = home.join(".ssh/id_ed25519");
    let mut failures = 0;

    failures += check("write-outside", Some(&outside), || {
        std::fs::write(&outside, b"x")
    }) as i32;

    failures += check("write-home", Some(&home_escape), || {
        std::fs::write(&home_escape, b"x")
    }) as i32;

    // A credential fixture that is absent is an error, not a pass. A stat that
    // is itself denied counts as confined, not absent: exists() would report
    // false for both.
    match std::fs::metadata(&ssh_key) {
        Ok(_) => {
            failures += check("read-ssh-key", None, || {
                std::fs::read(&ssh_key).map(|_| ())
            }) as i32;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            println!("read-ssh-key: error (fixture absent: {})", ssh_key.display());
            failures += 1;
        }
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            println!("read-ssh-key: denied (stat refused)");
        }
        Err(error) => {
            println!("read-ssh-key: error ({error})");
            failures += 1;
        }
    }

    // Read a keychain entry, not the directory: listing and reading are
    // different permissions and AC-002 names an entry.
    let keychain = home.join("Library/Keychains/login.keychain-db");
    failures += check("read-keychain", None, || {
        std::fs::read(&keychain).map(|_| ())
    }) as i32;

    failures += check("connect", None, connect) as i32;

    failures += check("child-write", Some(&child_escape), || {
        let output = std::process::Command::new("/usr/bin/touch")
            .arg(&child_escape)
            .output()?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let kind = if stderr.contains("Operation not permitted") {
            io::ErrorKind::PermissionDenied
        } else {
            io::ErrorKind::Other
        };
        Err(io::Error::new(kind, stderr.trim().to_string()))
    }) as i32;

    failures
}

/// Report the exact errno of one connect attempt, so a harness can tell a
/// sandbox denial from a refused connection.
fn probe_connect(address: &str) -> i32 {
    let address: std::net::SocketAddr = match address.parse() {
        Ok(address) => address,
        Err(err) => {
            println!("error: {err}");
            return 64;
        }
    };
    match TcpStream::connect_timeout(&address, Duration::from_secs(2)) {
        Ok(_) => println!("ok"),
        Err(error) => match error.raw_os_error() {
            Some(1) => println!("eperm"),
            Some(61) => println!("econnrefused"),
            Some(code) => println!("errno:{code}"),
            None => println!("error: {error}"),
        },
    }
    0
}

/// Report whether a hostname resolves at all, so the harness can distinguish a
/// DNS denial from a connect denial.
fn probe_resolve(name: &str) -> i32 {
    match (name, 443u16).to_socket_addrs() {
        Ok(mut addresses) => match addresses.next() {
            Some(address) => println!("resolved:{address}"),
            None => println!("dns-failed"),
        },
        Err(_) => println!("dns-failed"),
    }
    0
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(index) = args.iter().position(|arg| arg == "--probe-resolve") {
        let name = args.get(index + 1).map(String::as_str).unwrap_or("");
        std::process::exit(probe_resolve(name));
    }
    if let Some(index) = args.iter().position(|arg| arg == "--probe-connect") {
        let address = args.get(index + 1).map(String::as_str).unwrap_or("");
        std::process::exit(probe_connect(address));
    }
    if std::env::args().any(|arg| arg == "--hostile") {
        let failures = hostile();
        std::process::exit(if failures == 0 { 0 } else { 1 });
    }
    println!("fixture: {}", fixture::add(2, 3));
}
