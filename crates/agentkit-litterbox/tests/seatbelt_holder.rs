//! End-to-end tests for the Seatbelt holder binary.
//!
//! The helper runs as a separate process, so the fork that applies the profile
//! happens in its single-threaded main, not in this test process.

#![cfg(target_os = "macos")]

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_litterbox-seatbelt-helper");

fn frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(12 + payload.len());
    frame.extend_from_slice(b"LBX1");
    frame.extend_from_slice(&1u16.to_le_bytes());
    frame.push(kind);
    frame.push(0);
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(payload);
    frame
}

fn string(value: &[u8]) -> Vec<u8> {
    let mut out = (value.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(value);
    out
}

fn config(workspace: &Path) -> Vec<u8> {
    let workspace_text = workspace.to_str().expect("utf8 workspace");
    let workspace = workspace.as_os_str().as_encoded_bytes();
    let mut payload = Vec::new();
    payload.extend_from_slice(&string(workspace));
    let baseline = [
        "/System/Library",
        "/System/Applications",
        "/Library/Frameworks",
        "/usr/bin",
        "/usr/lib",
        "/usr/share",
        "/usr/libexec",
        "/bin",
        "/sbin",
        "/private/etc",
        "/dev",
    ];
    let reads: Vec<&[u8]> = baseline.iter().map(|value| value.as_bytes()).collect();
    payload.extend_from_slice(&(reads.len() as u16).to_le_bytes());
    for read in reads {
        payload.extend_from_slice(&string(read));
    }
    payload.extend_from_slice(&1u16.to_le_bytes());
    payload.extend_from_slice(&string(workspace));
    payload.push(0);
    payload.extend_from_slice(&string(b"demo"));
    let environment = [
        ("PATH", "/usr/bin:/bin"),
        ("HOME", workspace_text),
        ("TMPDIR", workspace_text),
        ("PWD", workspace_text),
    ];
    payload.extend_from_slice(&(environment.len() as u32).to_le_bytes());
    for (name, value) in environment {
        payload.extend_from_slice(&(name.len() as u16).to_le_bytes());
        payload.extend_from_slice(name.as_bytes());
        payload.extend_from_slice(&string(value.as_bytes()));
    }
    payload.extend_from_slice(&(baseline.len() as u16).to_le_bytes());
    for value in baseline {
        payload.extend_from_slice(&string(value.as_bytes()));
    }
    payload.extend_from_slice(&0u16.to_le_bytes());
    frame(1, &payload)
}

fn exec(argv: &[&str]) -> Vec<u8> {
    let mut payload = (argv.len() as u16).to_le_bytes().to_vec();
    for value in argv {
        payload.extend_from_slice(&string(value.as_bytes()));
    }
    frame(3, &payload)
}

fn recv(stream: &mut UnixStream) -> (u8, Vec<u8>) {
    let mut header = [0u8; 12];
    stream.read_exact(&mut header).expect("header");
    let kind = header[6];
    let length = u32::from_le_bytes([header[8], header[9], header[10], header[11]]) as usize;
    let mut payload = vec![0u8; length];
    stream.read_exact(&mut payload).expect("payload");
    (kind, payload)
}

struct Holder {
    _root: TempDir,
    socket: PathBuf,
    log: PathBuf,
    child: Child,
}

impl Holder {
    fn start() -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        let socket = root.path().join("holder.sock");
        let log = root.path().join("holder.log");
        let workspace = root.path().join("ws");
        std::fs::create_dir_all(workspace.join("tmp")).expect("workspace");
        let child = Command::new(BIN)
            .arg("--hold")
            .arg(&socket)
            .arg(&log)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("helper starts");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !socket.exists() {
            assert!(
                Instant::now() < deadline,
                "the holder socket did not appear"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        Self {
            _root: root,
            socket,
            log,
            child,
        }
    }

    fn connect(&self) -> UnixStream {
        UnixStream::connect(&self.socket).expect("connect")
    }

    fn configure(&self, stream: &mut UnixStream) {
        let workspace = self.socket.parent().expect("parent").join("ws");
        stream.write_all(&config(&workspace)).expect("config");
        assert_eq!(recv(stream), (2, Vec::new()), "ready");
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn a_command_returns_its_output_and_status() {
    let holder = Holder::start();
    let mut config_stream = holder.connect();
    holder.configure(&mut config_stream);
    drop(config_stream);
    let mut stream = holder.connect();
    stream
        .write_all(&exec(&["/bin/sh", "-c", "echo out; echo err >&2; exit 7"]))
        .expect("exec");
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let exit = loop {
        let (kind, payload) = recv(&mut stream);
        match kind {
            4 => stdout.extend_from_slice(&payload),
            5 => stderr.extend_from_slice(&payload),
            6 => break payload,
            7 => panic!("error: {}", String::from_utf8_lossy(&payload)),
            other => panic!("unexpected frame {other}"),
        }
    };
    assert_eq!(stdout, b"out\n");
    assert!(String::from_utf8_lossy(&stderr).contains("err"));
    assert_eq!(i32::from_le_bytes(exit[..4].try_into().expect("code")), 7);
    assert_eq!(exit[4], 0);
}

#[test]
fn two_commands_run_concurrently() {
    let holder = Holder::start();
    let mut config_stream = holder.connect();
    holder.configure(&mut config_stream);
    drop(config_stream);
    let mut first = holder.connect();
    let mut second = holder.connect();
    first
        .write_all(&exec(&["/bin/sh", "-c", "sleep 0.5; echo first"]))
        .expect("first");
    second
        .write_all(&exec(&["/bin/sh", "-c", "sleep 0.5; echo second"]))
        .expect("second");

    let started = Instant::now();
    for stream in [&mut first, &mut second] {
        loop {
            let (kind, _) = recv(stream);
            if kind == 6 {
                break;
            }
            assert_ne!(kind, 7, "a command failed");
        }
    }
    assert!(
        started.elapsed() < Duration::from_millis(900),
        "the commands ran in sequence: {:?}",
        started.elapsed()
    );
}

#[test]
fn status_proves_a_live_holder() {
    let holder = Holder::start();
    let mut config_stream = holder.connect();
    holder.configure(&mut config_stream);
    drop(config_stream);
    let mut status = holder.connect();
    status.write_all(&frame(8, &[])).expect("status");
    assert_eq!(recv(&mut status), (9, Vec::new()));
}

#[test]
fn a_second_configuration_is_refused() {
    let holder = Holder::start();
    let mut config_stream = holder.connect();
    holder.configure(&mut config_stream);
    drop(config_stream);
    let mut second = holder.connect();
    second
        .write_all(&config(&holder.socket.parent().expect("parent").join("ws")))
        .expect("second config");
    let (kind, payload) = recv(&mut second);
    assert_eq!(kind, 7, "the second config is refused");
    assert!(!payload.is_empty());
}

#[test]
fn the_helper_log_names_no_environment_value() {
    let holder = Holder::start();
    let mut config_stream = holder.connect();
    holder.configure(&mut config_stream);
    drop(config_stream);
    let mut stream = holder.connect();
    stream
        .write_all(&exec(&["/bin/sh", "-c", "env | head -1"]))
        .expect("exec");
    loop {
        let (kind, _) = recv(&mut stream);
        if kind == 6 || kind == 7 {
            break;
        }
    }
    let log = std::fs::read_to_string(&holder.log).unwrap_or_default();
    assert!(!log.contains("PATH="), "{log}");
}
