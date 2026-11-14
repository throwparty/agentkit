//! Everything the helper does on macOS: run the readiness probe, hold a
//! sandbox and serve commands on its socket.
//!
//! The crate root declares this module unconditionally and gates it here, so
//! the platform caveat appears once instead of on every declaration.

#![cfg(target_os = "macos")]

#[path = "../../backends/seatbelt/policy.rs"]
mod policy;
#[path = "../../backends/seatbelt/protocol.rs"]
mod protocol;

use std::os::fd::AsRawFd;

unsafe extern "C" {
    fn sandbox_init_with_parameters(
        profile: *const libc::c_char,
        flags: u64,
        parameters: *const *const libc::c_char,
        errorbuf: *mut *mut libc::c_char,
    ) -> libc::c_int;
}

pub(crate) fn run() -> i32 {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--probe") => probe(),
        Some("--hold") => {
            let Some(socket) = args.next() else {
                eprintln!("--hold needs a socket path");
                return 64;
            };
            let log = args.next();
            hold(std::path::Path::new(&socket), log.as_deref())
        }
        _ => {
            eprintln!("usage: litterbox-seatbelt-helper --probe | --hold <socket> [log]");
            64
        }
    }
}

fn probe() -> i32 {
    let profile = match std::ffi::CString::new("(version 1)(deny default)") {
        Ok(profile) => profile,
        Err(_) => return 70,
    };
    let parameters: [*const libc::c_char; 1] = [std::ptr::null()];
    let mut error: *mut libc::c_char = std::ptr::null_mut();
    // SAFETY: the profile and parameter array outlive the call. The array holds
    // the terminating null pointer an empty parameter list needs.
    let result = unsafe {
        sandbox_init_with_parameters(profile.as_ptr(), 0, parameters.as_ptr(), &mut error)
    };
    if result == 0 {
        return 0;
    }
    eprintln!("{}", error_message(error));
    70
}

const MAX_CONNECTIONS: usize = 128;
const MAX_RUNNING: usize = 64;
const OUTPUT_CAP: usize = 1024 * 1024;

fn hold(socket: &std::path::Path, log: Option<&str>) -> i32 {
    // SAFETY: setsid has no arguments and cannot fail the process state.
    if unsafe { libc::setsid() } < 0 {
        eprintln!("setsid failed: {}", std::io::Error::last_os_error());
        return 70;
    }
    // SAFETY: umask only changes the process file-creation mask.
    unsafe { libc::umask(0o077) };

    if let Ok(metadata) = std::fs::symlink_metadata(socket) {
        if metadata.file_type().is_symlink() {
            eprintln!("the socket path is a symlink");
            return 70;
        }
        if let Err(error) = std::fs::remove_file(socket) {
            eprintln!("cannot remove the stale socket: {error}");
            return 70;
        }
    }

    let listener = match std::os::unix::net::UnixListener::bind(socket) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("cannot bind the socket: {error}");
            return 70;
        }
    };
    if let Err(error) = set_mode(socket) {
        eprintln!("cannot set the socket mode: {error}");
        return 70;
    }
    if let Err(error) = listener.set_nonblocking(true) {
        eprintln!("cannot make the listener non-blocking: {error}");
        return 70;
    }
    set_cloexec(listener.as_raw_fd());

    if let Some(path) = log
        && let Err(error) = redirect_stderr(path)
    {
        eprintln!("cannot open the holder log: {error}");
        return 70;
    }
    eprintln!("seatbelt holder ready on {}", socket.display());

    let mut holder: Option<Holder> = None;
    let mut connections: Vec<Option<Connection>> = Vec::new();

    loop {
        let mut fds = Vec::with_capacity(connections.len() + 1);
        let mut events = Vec::with_capacity(connections.len() + 1);
        fds.push(libc::pollfd {
            fd: listener.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        });
        events.push(Event::Accept);
        for (index, connection) in connections.iter().enumerate() {
            let Some(connection) = connection else {
                continue;
            };
            match connection {
                Connection::Reading { stream, .. } => {
                    fds.push(poll_fd(stream.as_raw_fd()));
                    events.push(Event::Read(index));
                }
                Connection::Running { stdout, stderr, .. } => {
                    if let Some(fd) = stdout {
                        fds.push(poll_fd(*fd));
                        events.push(Event::Stdout(index));
                    }
                    if let Some(fd) = stderr {
                        fds.push(poll_fd(*fd));
                        events.push(Event::Stderr(index));
                    }
                }
            }
        }

        // SAFETY: the pollfd array is valid for its length.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            eprintln!("poll failed: {error}");
            return 1;
        }

        for (poll_fd, event) in fds.iter().zip(events.iter()) {
            if poll_fd.revents == 0 {
                continue;
            }
            match *event {
                Event::Accept => loop {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            if connections.len() >= MAX_CONNECTIONS {
                                let _ = send_blocking(
                                    &stream,
                                    &protocol::Message::Error {
                                        code: protocol::ErrorCode::Busy,
                                        message: "too many connections".to_string(),
                                    },
                                );
                                continue;
                            }
                            let _ = stream.set_nonblocking(true);
                            set_cloexec(stream.as_raw_fd());
                            connections.push(Some(Connection::Reading {
                                stream,
                                buffer: Vec::new(),
                                expected: None,
                            }));
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(error) => {
                            eprintln!("accept failed: {error}");
                            break;
                        }
                    }
                },
                Event::Read(index) => {
                    if let Some(Some(Connection::Reading {
                        stream,
                        buffer,
                        expected,
                    })) = connections.get_mut(index)
                    {
                        match read_available(stream, buffer, expected) {
                            Ok(true) => {}
                            Ok(false) => {
                                connections[index] = None;
                                continue;
                            }
                            Err(error) => {
                                let _ = send_blocking(
                                    stream,
                                    &protocol::Message::Error {
                                        code: protocol::ErrorCode::Protocol,
                                        message: error,
                                    },
                                );
                                connections[index] = None;
                                continue;
                            }
                        }
                        let Some(message) = take_message(buffer, expected) else {
                            continue;
                        };
                        let running = connections
                            .iter()
                            .filter(|connection| {
                                matches!(connection, Some(Connection::Running { .. }))
                            })
                            .count();
                        let Some(stream) = connection_stream(&mut connections[index]) else {
                            continue;
                        };
                        match message {
                            protocol::Message::Config(config) => {
                                if holder.is_some() {
                                    let _ = send_blocking(
                                        stream,
                                        &protocol::Message::Error {
                                            code: protocol::ErrorCode::Protocol,
                                            message: "the holder is already configured".to_string(),
                                        },
                                    );
                                    connections[index] = None;
                                    continue;
                                }
                                match prepare(*config) {
                                    Ok(prepared) => {
                                        holder = Some(prepared);
                                        let _ = send_blocking(stream, &protocol::Message::Ready);
                                    }
                                    Err(error) => {
                                        let _ = send_blocking(
                                            stream,
                                            &protocol::Message::Error {
                                                code: protocol::ErrorCode::Policy,
                                                message: error,
                                            },
                                        );
                                        return 71;
                                    }
                                }
                                connections[index] = None;
                            }
                            protocol::Message::Status if holder.is_some() => {
                                let _ = send_blocking(stream, &protocol::Message::StatusReply);
                                connections[index] = None;
                            }
                            protocol::Message::Exec(argv) if holder.is_some() => {
                                if running >= MAX_RUNNING {
                                    let _ = send_blocking(
                                        stream,
                                        &protocol::Message::Error {
                                            code: protocol::ErrorCode::Busy,
                                            message: "too many concurrent commands".to_string(),
                                        },
                                    );
                                    connections[index] = None;
                                    continue;
                                }
                                let Some(prepared) = holder.as_ref() else {
                                    continue;
                                };
                                match spawn(prepared, &argv, stream) {
                                    Ok(running_connection) => {
                                        connections[index] = Some(running_connection);
                                    }
                                    Err(error) => {
                                        let _ = send_blocking(
                                            stream,
                                            &protocol::Message::Error {
                                                code: protocol::ErrorCode::Internal,
                                                message: error,
                                            },
                                        );
                                        connections[index] = None;
                                    }
                                }
                            }
                            _ => {
                                let _ = send_blocking(
                                    stream,
                                    &protocol::Message::Error {
                                        code: protocol::ErrorCode::Protocol,
                                        message: "the holder is not configured".to_string(),
                                    },
                                );
                                connections[index] = None;
                            }
                        }
                    }
                }
                Event::Stdout(index) | Event::Stderr(index) => {
                    if let Some(Some(Connection::Running {
                        stdout,
                        stderr,
                        out,
                        err,
                        truncated_out,
                        truncated_err,
                        ..
                    })) = connections.get_mut(index)
                    {
                        let (slot, buffer, truncated) = match *event {
                            Event::Stdout(_) => (stdout, out, truncated_out),
                            _ => (stderr, err, truncated_err),
                        };
                        if let Some(fd) = *slot {
                            match read_output(fd, buffer, truncated) {
                                Ok(true) => {}
                                Ok(false) => {
                                    // SAFETY: the descriptor belongs to this connection.
                                    unsafe { libc::close(fd) };
                                    *slot = None;
                                }
                                Err(error) => {
                                    eprintln!("cannot read command output: {error}");
                                    // SAFETY: the descriptor belongs to this connection.
                                    unsafe { libc::close(fd) };
                                    *slot = None;
                                }
                            }
                        }
                        let done = connections[index].as_ref().is_some_and(|connection| {
                            matches!(
                                connection,
                                Connection::Running {
                                    stdout: None,
                                    stderr: None,
                                    ..
                                }
                            )
                        });
                        if done {
                            finish(index, &mut connections);
                        }
                    }
                }
            }
        }

        if connections.iter().all(Option::is_none) {
            connections.clear();
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Event {
    Accept,
    Read(usize),
    Stdout(usize),
    Stderr(usize),
}

enum Connection {
    Reading {
        stream: std::os::unix::net::UnixStream,
        buffer: Vec<u8>,
        expected: Option<usize>,
    },
    Running {
        stream: std::os::unix::net::UnixStream,
        pid: libc::pid_t,
        status: libc::c_int,
        stdout: Option<libc::c_int>,
        stderr: Option<libc::c_int>,
        out: Vec<u8>,
        err: Vec<u8>,
        truncated_out: bool,
        truncated_err: bool,
    },
}

fn poll_fd(fd: libc::c_int) -> libc::pollfd {
    libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    }
}

fn set_cloexec(fd: libc::c_int) {
    // SAFETY: fcntl only changes the descriptor flags.
    unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
}

fn connection_stream(
    connection: &mut Option<Connection>,
) -> Option<&std::os::unix::net::UnixStream> {
    match connection {
        Some(Connection::Reading { stream, .. }) => Some(stream),
        _ => None,
    }
}

fn read_available(
    stream: &std::os::unix::net::UnixStream,
    buffer: &mut Vec<u8>,
    expected: &mut Option<usize>,
) -> Result<bool, String> {
    use std::io::Read;

    let mut stream = stream;
    let mut chunk = [0u8; 64 * 1024];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => return Ok(false),
            Ok(read) => {
                buffer.extend_from_slice(&chunk[..read]);
                if buffer.len() > protocol::MAX_FRAME + 12 {
                    return Err("the request is too large".to_string());
                }
                if expected.is_none() && buffer.len() >= 12 {
                    let header = protocol::Header::decode(&buffer[..12])
                        .map_err(|error| error.to_string())?;
                    *expected = Some(12 + header.length as usize);
                }
                if let Some(expected) = *expected
                    && buffer.len() >= expected
                {
                    return Ok(true);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(true),
            Err(error) => return Err(format!("cannot read the request: {error}")),
        }
    }
}

fn take_message(buffer: &mut Vec<u8>, expected: &mut Option<usize>) -> Option<protocol::Message> {
    let length = (*expected)?;
    if buffer.len() < length {
        return None;
    }
    let frame: Vec<u8> = buffer.drain(..length).collect();
    *expected = None;
    let header = protocol::Header::decode(&frame[..12]).ok()?;
    protocol::Message::decode_payload(header.kind, &frame[12..]).ok()
}

fn read_output(
    fd: libc::c_int,
    buffer: &mut Vec<u8>,
    truncated: &mut bool,
) -> std::io::Result<bool> {
    let mut chunk = [0u8; 64 * 1024];
    // SAFETY: the buffer is valid for its length.
    let read = unsafe { libc::read(fd, chunk.as_mut_ptr() as *mut libc::c_void, chunk.len()) };
    if read == 0 {
        return Ok(false);
    }
    if read < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::WouldBlock {
            return Ok(true);
        }
        return Err(error);
    }
    let read = read as usize;
    if buffer.len() < OUTPUT_CAP {
        let keep = (OUTPUT_CAP - buffer.len()).min(read);
        buffer.extend_from_slice(&chunk[..keep]);
        if keep < read {
            *truncated = true;
        }
    } else {
        *truncated = true;
    }
    Ok(true)
}

fn spawn(
    holder: &Holder,
    argv: &[Vec<u8>],
    stream: &std::os::unix::net::UnixStream,
) -> Result<Connection, String> {
    let stream = stream
        .try_clone()
        .map_err(|error| format!("cannot clone the connection: {error}"))?;
    if argv.is_empty() {
        return Err("the command is empty".to_string());
    }
    let argv = cstrings(argv)?;
    let environment = cstrings(
        &holder
            .environment
            .iter()
            .map(|(name, value)| {
                let mut pair = name.clone();
                pair.push(b'=');
                pair.extend_from_slice(value);
                pair
            })
            .collect::<Vec<_>>(),
    )?;
    let program = resolve_program(&argv[0], &environment)?;
    let parameters: Vec<*const libc::c_char> = holder
        .profile
        .parameters
        .iter()
        .flat_map(|parameter| {
            [
                parameter.name.as_ptr() as *const libc::c_char,
                parameter.value.as_ptr() as *const libc::c_char,
            ]
        })
        .chain(std::iter::once(std::ptr::null()))
        .collect();
    let profile = std::ffi::CString::new(holder.profile.text.as_bytes())
        .map_err(|_| "the profile contains a NUL".to_string())?;
    let workspace = std::ffi::CString::new(holder.workspace.as_os_str().as_encoded_bytes())
        .map_err(|_| "the workspace contains a NUL".to_string())?;

    let (out_read, out_write) = pipe().map_err(|error| error.to_string())?;
    let (err_read, err_write) = pipe().map_err(|error| error.to_string())?;
    let (status_read, status_write) = pipe().map_err(|error| error.to_string())?;

    // SAFETY: fork returns twice; the child only runs the code below.
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(format!("fork failed: {}", std::io::Error::last_os_error()));
    }
    if pid == 0 {
        // SAFETY: the child owns these descriptors and exits immediately.
        unsafe {
            libc::close(out_read);
            libc::close(err_read);
            libc::close(status_read);
            libc::dup2(out_write, libc::STDOUT_FILENO);
            libc::dup2(err_write, libc::STDERR_FILENO);
            libc::fcntl(status_write, libc::F_SETFD, libc::FD_CLOEXEC);
            for fd in 3..1024 {
                if fd != status_write {
                    libc::close(fd);
                }
            }
            if libc::chdir(workspace.as_ptr()) != 0 {
                fail(
                    status_write,
                    3,
                    &std::io::Error::last_os_error().to_string(),
                );
            }
            let mut error: *mut libc::c_char = std::ptr::null_mut();
            if sandbox_init_with_parameters(profile.as_ptr(), 0, parameters.as_ptr(), &mut error)
                != 0
            {
                fail(status_write, 1, &error_message(error));
            }
            let argv: Vec<*const libc::c_char> = argv
                .iter()
                .map(|value| value.as_ptr())
                .chain(std::iter::once(std::ptr::null()))
                .collect();
            let environment: Vec<*const libc::c_char> = environment
                .iter()
                .map(|value| value.as_ptr())
                .chain(std::iter::once(std::ptr::null()))
                .collect();
            libc::execve(program.as_ptr(), argv.as_ptr(), environment.as_ptr());
            fail(
                status_write,
                3,
                &std::io::Error::last_os_error().to_string(),
            );
        }
    }

    // SAFETY: the parent owns the read ends and closes the write ends.
    unsafe {
        libc::close(out_write);
        libc::close(err_write);
        libc::close(status_write);
    }
    set_nonblocking(out_read)?;
    set_nonblocking(err_read)?;

    Ok(Connection::Running {
        stream,
        pid,
        status: status_read,
        stdout: Some(out_read),
        stderr: Some(err_read),
        out: Vec::new(),
        err: Vec::new(),
        truncated_out: false,
        truncated_err: false,
    })
}

fn finish(index: usize, connections: &mut [Option<Connection>]) {
    let Some(Some(Connection::Running {
        stream,
        pid,
        status,
        out,
        err,
        truncated_out,
        truncated_err,
        ..
    })) = connections.get_mut(index)
    else {
        return;
    };

    let mut raw = 0;
    // SAFETY: the pid is this holder's child.
    unsafe { libc::waitpid(*pid, &mut raw, 0) };
    // SAFETY: the status descriptor belongs to this connection.
    let failure = unsafe { read_status(*status) };
    unsafe { libc::close(*status) };

    if let Some((code, message)) = failure {
        let code = match code {
            1 => protocol::ErrorCode::Apply,
            3 => protocol::ErrorCode::Exec,
            _ => protocol::ErrorCode::Internal,
        };
        let _ = send_blocking(stream, &protocol::Message::Error { code, message });
        connections[index] = None;
        return;
    }

    if !out.is_empty() {
        let _ = send_blocking(stream, &protocol::Message::Stdout(std::mem::take(out)));
    }
    if !err.is_empty() {
        let _ = send_blocking(stream, &protocol::Message::Stderr(std::mem::take(err)));
    }
    let code = if libc::WIFEXITED(raw) {
        libc::WEXITSTATUS(raw)
    } else if libc::WIFSIGNALED(raw) {
        128 + libc::WTERMSIG(raw)
    } else {
        1
    };
    let _ = send_blocking(
        stream,
        &protocol::Message::Exit {
            code,
            flags: protocol::ExitFlags {
                stdout_truncated: *truncated_out,
                stderr_truncated: *truncated_err,
            },
        },
    );
    connections[index] = None;
}

unsafe fn read_status(fd: libc::c_int) -> Option<(u8, String)> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        // SAFETY: the buffer is valid for its length.
        let read = unsafe { libc::read(fd, chunk.as_mut_ptr() as *mut libc::c_void, chunk.len()) };
        if read <= 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read as usize]);
    }
    if bytes.is_empty() {
        return None;
    }
    let code = bytes[0];
    let message = String::from_utf8_lossy(&bytes[1..]).into_owned();
    Some((code, message))
}

unsafe fn fail(fd: libc::c_int, code: u8, message: &str) -> ! {
    let mut bytes = vec![code];
    bytes.extend_from_slice(message.as_bytes());
    // SAFETY: the descriptor is the status pipe and the buffer is valid.
    unsafe { libc::write(fd, bytes.as_ptr() as *const libc::c_void, bytes.len()) };
    // SAFETY: the child must not return into the holder loop.
    unsafe { libc::_exit(if code == 1 { 70 } else { 126 }) }
}

fn pipe() -> std::io::Result<(libc::c_int, libc::c_int)> {
    let mut fds = [0; 2];
    // SAFETY: the array holds two descriptors after a successful call.
    let result = unsafe { libc::pipe(fds.as_mut_ptr()) };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok((fds[0], fds[1]))
}

fn set_nonblocking(fd: libc::c_int) -> Result<(), String> {
    // SAFETY: fcntl only changes the descriptor flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(format!(
            "cannot read descriptor flags: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: fcntl only changes the descriptor flags.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(format!(
            "cannot set descriptor flags: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn resolve_program(
    program: &std::ffi::CString,
    environment: &[std::ffi::CString],
) -> Result<std::ffi::CString, String> {
    let name = program.to_bytes();
    if name.contains(&b'/') {
        return Ok(program.clone());
    }
    let path = environment
        .iter()
        .find_map(|value| {
            let bytes = value.to_bytes();
            bytes.strip_prefix(b"PATH=").map(<[u8]>::to_vec)
        })
        .unwrap_or_else(|| b"/usr/bin:/bin:/usr/sbin:/sbin".to_vec());
    for directory in path.split(|byte| *byte == b':') {
        let directory = if directory.is_empty() {
            b".".as_slice()
        } else {
            directory
        };
        let mut candidate = directory.to_vec();
        candidate.push(b'/');
        candidate.extend_from_slice(name);
        let candidate =
            std::ffi::CString::new(candidate).map_err(|_| "the PATH contains a NUL".to_string())?;
        // SAFETY: access only checks the path, and the candidate is NUL terminated.
        if unsafe { libc::access(candidate.as_ptr(), libc::X_OK) } == 0 {
            return Ok(candidate);
        }
    }
    Err(format!(
        "the command '{}' is not on PATH",
        String::from_utf8_lossy(name)
    ))
}

fn cstrings(values: &[Vec<u8>]) -> Result<Vec<std::ffi::CString>, String> {
    values
        .iter()
        .map(|value| {
            std::ffi::CString::new(value.clone())
                .map_err(|_| "a command or environment value contains a NUL".to_string())
        })
        .collect()
}

fn send_blocking(
    stream: &std::os::unix::net::UnixStream,
    message: &protocol::Message,
) -> std::io::Result<()> {
    let _ = stream.set_nonblocking(false);
    send(stream, message)
}

struct Holder {
    environment: Vec<(Vec<u8>, Vec<u8>)>,
    workspace: std::path::PathBuf,
    profile: policy::RenderedProfile,
}

fn prepare(config: protocol::ConfigPayload) -> Result<Holder, String> {
    let posture = match config.posture {
        protocol::WirePosture::Denied => policy::Posture::Denied,
        protocol::WirePosture::Host => policy::Posture::Host,
        protocol::WirePosture::Loopback => policy::Posture::Loopback(
            config
                .endpoints
                .iter()
                .map(|endpoint| String::from_utf8(endpoint.clone()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| "an endpoint is not UTF-8".to_string())?,
        ),
    };
    let input = policy::ProfileInput {
        workspace: path(&config.workspace),
        reads: config.reads.iter().map(|value| path(value)).collect(),
        writes: config.writes.iter().map(|value| path(value)).collect(),
        posture,
        tag: String::from_utf8(config.tag).map_err(|_| "the tag is not UTF-8".to_string())?,
        baseline_reads: config
            .baseline_reads
            .iter()
            .map(|value| path(value))
            .collect(),
        mach_services: config
            .mach_services
            .iter()
            .map(|value| {
                String::from_utf8(value.clone())
                    .map_err(|_| "a mach service is not UTF-8".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?,
    };
    let profile = policy::render(&input).map_err(|error| error.to_string())?;
    Ok(Holder {
        environment: config.environment,
        workspace: path(&config.workspace),
        profile,
    })
}

fn path(value: &[u8]) -> std::path::PathBuf {
    use std::os::unix::ffi::OsStrExt;

    std::path::PathBuf::from(std::ffi::OsStr::from_bytes(value).to_os_string())
}

fn read(stream: &std::os::unix::net::UnixStream) -> Result<protocol::Message, String> {
    use std::io::Read;

    let mut stream = stream;
    let mut header = [0u8; 12];
    stream
        .read_exact(&mut header)
        .map_err(|error| format!("cannot read the header: {error}"))?;
    let header = protocol::Header::decode(&header).map_err(|error| error.to_string())?;
    let mut payload = vec![0u8; header.length as usize];
    stream
        .read_exact(&mut payload)
        .map_err(|error| format!("cannot read the payload: {error}"))?;
    protocol::Message::decode_payload(header.kind, &payload).map_err(|error| error.to_string())
}

fn send(
    stream: &std::os::unix::net::UnixStream,
    message: &protocol::Message,
) -> std::io::Result<()> {
    use std::io::Write;

    let frame = message
        .encode()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let mut stream = stream;
    stream.write_all(&frame)
}

fn peer_is_ours(stream: &std::os::unix::net::UnixStream) -> bool {
    use std::os::fd::AsRawFd;

    let mut uid = 0;
    let mut gid = 0;
    // SAFETY: the descriptor is open for the lifetime of the stream.
    let result = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    result == 0 && uid == unsafe { libc::getuid() }
}

fn set_mode(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

fn redirect_stderr(path: &str) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;

    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)?;
    // SAFETY: both descriptors are open; dup2 replaces stderr for this process.
    let result = unsafe { libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO) };
    if result < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn error_message(error: *mut libc::c_char) -> String {
    if error.is_null() {
        return "sandbox_init_with_parameters failed".to_string();
    }
    // SAFETY: libsandbox returned a NUL-terminated message.
    unsafe { std::ffi::CStr::from_ptr(error) }
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::protocol::{ConfigPayload, WirePosture};
    use super::{path, prepare};

    #[test]
    fn a_bare_command_is_resolved_through_path() {
        let environment = vec![std::ffi::CString::new("PATH=/usr/bin:/bin").expect("env")];
        let ls = std::ffi::CString::new("ls").expect("name");
        let resolved = super::resolve_program(&ls, &environment).expect("resolved");
        assert!(resolved.to_string_lossy().ends_with("/ls"));
        let missing = std::ffi::CString::new("lbx-no-such-command").expect("name");
        assert!(super::resolve_program(&missing, &environment).is_err());
    }

    #[test]
    fn a_config_becomes_a_profile_input() {
        std::fs::create_dir_all("/private/tmp/lbx/ws").expect("workspace");
        std::fs::create_dir_all("/private/tmp/lbx/read").expect("read");
        let config = ConfigPayload {
            workspace: b"/private/tmp/lbx/ws".to_vec(),
            reads: vec![b"/private/tmp/lbx/read".to_vec()],
            writes: vec![b"/private/tmp/lbx/ws".to_vec()],
            posture: WirePosture::Denied,
            endpoints: Vec::new(),
            tag: b"demo".to_vec(),
            environment: vec![(b"PATH".to_vec(), b"/usr/bin".to_vec())],
            baseline_reads: vec![b"/usr/bin".to_vec()],
            mach_services: vec![b"com.apple.example".to_vec()],
        };
        let holder = prepare(config).expect("prepared");
        assert_eq!(holder.workspace, path(b"/private/tmp/lbx/ws"));
        assert_eq!(holder.environment[0].0, b"PATH");
        assert!(holder.profile.text.contains("(param"));
    }
}
