//! The backend's client for the Seatbelt holder.
//!
//! It starts the helper, sends the configuration once, and then opens one
//! connection per command. No value appears in an argv.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::BACKEND;
use super::host::{ProcessStart, SharedHost, Signal};
use super::protocol::{ConfigPayload, ErrorCode, Header, Message, ProtocolError};
use crate::backends::shared::CONTROL_TIMEOUT;

/// How long the holder may take to answer its configuration.
const CONFIG_TIMEOUT: Duration = Duration::from_secs(5);
use crate::sandbox::contract::{Error, Execution};

/// How long create waits for the holder socket and Ready.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);

/// A running holder.
pub(crate) struct Holder {
    pub(crate) pid: u32,
    pub(crate) start: ProcessStart,
    pub(crate) socket: PathBuf,
    pub(crate) log: PathBuf,
    host: SharedHost,
}

impl Holder {
    /// Address an already-running holder.
    pub(crate) fn open(
        host: SharedHost,
        pid: u32,
        start: ProcessStart,
        socket: PathBuf,
        log: PathBuf,
    ) -> Self {
        Self {
            pid,
            start,
            socket,
            log,
            host,
        }
    }

    /// Start a holder and send its configuration.
    pub(crate) fn start(
        host: SharedHost,
        helper: &Path,
        socket: &Path,
        log: &Path,
        config: ConfigPayload,
    ) -> Result<Self, Error> {
        if let Some(parent) = socket.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                Error::failed(format!(
                    "cannot create the socket directory '{}': {error}",
                    parent.display()
                ))
            })?;
        }

        tracing::debug!(
            helper = %helper.display(),
            socket = %socket.display(),
            log = %log.display(),
            "starting the seatbelt holder"
        );
        let mut child = Command::new(helper)
            .arg("--hold")
            .arg(socket)
            .arg(log)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .env_clear()
            .spawn()
            .map_err(|error| Error::Unavailable {
                backend: BACKEND.to_string(),
                reason: format!("the helper '{}' could not start: {error}", helper.display()),
                remedy: "check the helper is installed beside litterbox".to_string(),
            })?;
        let pid = child.id();

        let start = match wait_for_start(&host, pid) {
            Ok(start) => start,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };

        let stream = match connect(&host, socket) {
            Ok(stream) => stream,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::Unavailable {
                    backend: BACKEND.to_string(),
                    reason: format!(
                        "the holder did not answer on '{}': {error}",
                        socket.display()
                    ),
                    remedy: log_tail(log),
                });
            }
        };

        let mut stream = stream;
        let exchange = (|| {
            set_deadline(&stream, CONFIG_TIMEOUT)?;
            send(&mut stream, &Message::Config(Box::new(config)))?;
            receive(&mut stream)
        })();
        let reply = match exchange {
            Ok(reply) => reply,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        match reply {
            Message::Ready => {
                // The server that created the holder owns the Child handle.
                // Reap it in a thread so a killed holder does not become a
                // zombie. A later process that addresses the holder has no
                // handle; launchd reaps it after the creator exits.
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                Ok(Self {
                    pid,
                    start,
                    socket: socket.to_path_buf(),
                    log: log.to_path_buf(),
                    host,
                })
            }
            Message::Error { code, message } => {
                let _ = host.signal_group(pid, Signal::Kill);
                let _ = child.wait();
                Err(map_error(code, message))
            }
            other => {
                let _ = host.signal_group(pid, Signal::Kill);
                let _ = child.wait();
                Err(Error::failed(format!(
                    "the holder answered with an unexpected frame: {other:?}"
                )))
            }
        }
    }

    /// Run one command through the holder.
    pub(crate) fn exec(&self, argv: &[String]) -> Result<Execution, Error> {
        let mut stream = match self.host.connect_socket(&self.socket) {
            Ok(stream) => stream,
            Err(error) => {
                tracing::warn!(%error, "the holder socket is not reachable");
                return Err(Error::NotFound {
                    name: self.socket.display().to_string(),
                });
            }
        };
        let argv = argv.iter().map(|value| value.as_bytes().to_vec()).collect();
        send(&mut stream, &Message::Exec(argv))?;

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        loop {
            match receive(&mut stream)? {
                Message::Stdout(bytes) => append_capped(&mut stdout, &bytes),
                Message::Stderr(bytes) => append_capped(&mut stderr, &bytes),
                Message::Exit { code, flags } => {
                    let mut out = String::from_utf8_lossy(&stdout).into_owned();
                    let mut err = String::from_utf8_lossy(&stderr).into_owned();
                    if flags.stdout_truncated {
                        out.push_str("\n[truncated]");
                    }
                    if flags.stderr_truncated {
                        err.push_str("\n[truncated]");
                    }
                    return Ok(Execution {
                        exit_code: code,
                        stdout: out,
                        stderr: err,
                    });
                }
                Message::Error { code, message } => return Err(map_error(code, message)),
                other => {
                    return Err(Error::failed(format!(
                        "the holder sent an unexpected frame: {other:?}"
                    )));
                }
            }
        }
    }

    /// Prove the socket still answers.
    pub(crate) fn status(&self) -> Result<(), Error> {
        let mut stream = self.host.connect_socket(&self.socket).map_err(|error| {
            Error::failed(format!("the holder socket is not reachable: {error}"))
        })?;
        set_deadline(&stream, CONTROL_TIMEOUT)?;
        send(&mut stream, &Message::Status)?;
        match receive(&mut stream)? {
            Message::StatusReply => Ok(()),
            Message::Error { code, message } => Err(map_error(code, message)),
            other => Err(Error::failed(format!(
                "the holder sent an unexpected frame: {other:?}"
            ))),
        }
    }

    /// Send a signal to the holder's process group.
    pub(crate) fn signal(&self, signal: Signal) -> Result<(), Error> {
        self.host
            .signal_group(self.pid, signal)
            .map_err(|error| Error::failed(format!("cannot signal the holder: {error}")))
    }
}

/// Bound one control exchange so a wedged holder cannot hang the caller.
fn set_deadline(stream: &std::os::unix::net::UnixStream, timeout: Duration) -> Result<(), Error> {
    stream
        .set_read_timeout(Some(timeout))
        .and_then(|()| stream.set_write_timeout(Some(timeout)))
        .map_err(|error| Error::failed(format!("cannot bound the holder exchange: {error}")))
}

fn wait_for_start(host: &SharedHost, pid: u32) -> Result<ProcessStart, Error> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match host.process_start(pid) {
            Ok(start) => return Ok(start),
            Err(error) if Instant::now() < deadline => {
                tracing::debug!(%error, "waiting for the holder start time");
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => {
                return Err(Error::failed(format!(
                    "cannot read the holder start time: {error}"
                )));
            }
        }
    }
}

fn connect(host: &SharedHost, socket: &Path) -> std::io::Result<UnixStream> {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        match host.connect_socket(socket) {
            Ok(stream) => return Ok(stream),
            Err(error) if Instant::now() < deadline => {
                tracing::debug!(%error, "waiting for the holder socket");
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error),
        }
    }
}

fn send(stream: &mut UnixStream, message: &Message) -> Result<(), Error> {
    let frame = message
        .encode()
        .map_err(|error| Error::failed(format!("cannot encode a frame: {error}")))?;
    stream
        .write_all(&frame)
        .map_err(|error| Error::failed(format!("cannot write to the holder: {error}")))
}

fn receive(stream: &mut UnixStream) -> Result<Message, Error> {
    let mut header = [0u8; 12];
    stream
        .read_exact(&mut header)
        .map_err(|error| Error::failed(format!("cannot read from the holder: {error}")))?;
    let header = Header::decode(&header).map_err(protocol_error)?;
    let mut payload = vec![0u8; header.length as usize];
    stream
        .read_exact(&mut payload)
        .map_err(|error| Error::failed(format!("cannot read from the holder: {error}")))?;
    Message::decode_payload(header.kind, &payload).map_err(protocol_error)
}

fn protocol_error(error: ProtocolError) -> Error {
    Error::failed(format!("the holder protocol failed: {error}"))
}

fn map_error(code: ErrorCode, message: String) -> Error {
    match code {
        ErrorCode::Apply => Error::failed(format!("sandbox_init_with_parameters: {message}")),
        ErrorCode::Policy => Error::Misconfigured { message },
        ErrorCode::Exec => Error::failed(format!("the command could not be executed: {message}")),
        ErrorCode::Busy => Error::failed(message),
        ErrorCode::Protocol | ErrorCode::Internal => Error::failed(message),
    }
}

fn append_capped(buffer: &mut Vec<u8>, bytes: &[u8]) {
    let remaining = crate::backends::shared::EXEC_OUTPUT_CAP.saturating_sub(buffer.len());
    if remaining > 0 {
        buffer.extend_from_slice(&bytes[..remaining.min(bytes.len())]);
    }
}

fn log_tail(path: &Path) -> String {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let mut lines: Vec<&str> = text.lines().rev().take(8).collect();
            lines.reverse();
            lines.join("\n")
        }
        Err(_) => "the holder log is not readable".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::map_error;
    use crate::backends::seatbelt::protocol::ErrorCode;
    use crate::sandbox::contract::Error;

    #[test]
    fn every_error_code_maps_to_its_contract_error() {
        let apply = map_error(ErrorCode::Apply, "bad profile".to_string());
        assert!(
            apply.to_string().contains("sandbox_init_with_parameters"),
            "{apply}"
        );
        assert!(apply.to_string().contains("bad profile"), "{apply}");

        let policy = map_error(ErrorCode::Policy, "too large".to_string());
        assert!(matches!(policy, Error::Misconfigured { .. }), "{policy}");

        let exec = map_error(ErrorCode::Exec, "enoent".to_string());
        assert!(exec.to_string().contains("could not be executed"), "{exec}");
        assert!(exec.to_string().contains("enoent"), "{exec}");

        let busy = map_error(ErrorCode::Busy, "too many".to_string());
        assert!(busy.to_string().contains("too many"), "{busy}");

        let internal = map_error(ErrorCode::Internal, "broken".to_string());
        assert!(internal.to_string().contains("broken"), "{internal}");

        let protocol = map_error(ErrorCode::Protocol, "bad frame".to_string());
        assert!(protocol.to_string().contains("bad frame"), "{protocol}");
    }
}
