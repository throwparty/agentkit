//! Process execution for host checks and preparation.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How often a running child is checked against the deadline.
const POLL: Duration = Duration::from_millis(10);

/// What a command wrote and how it exited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandOutput {
    /// The exit status, or -1 when the process was signalled.
    pub status: i32,
    /// Everything written to standard output.
    pub stdout: Vec<u8>,
    /// Everything written to standard error.
    pub stderr: Vec<u8>,
}

impl CommandOutput {
    /// Whether the command exited successfully.
    pub fn success(&self) -> bool {
        self.status == 0
    }

    /// Standard output as text.
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// Standard error as text.
    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }

    /// The first line of standard error, trimmed.
    pub fn first_line(&self) -> String {
        self.stderr_text()
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_string()
    }

    /// The last non-empty line of standard error, trimmed.
    pub fn last_line(&self) -> String {
        self.stderr_text()
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .map(|line| line.trim().to_string())
            .unwrap_or_default()
    }
}

/// Run one host command.
pub trait CommandRunner: Send + Sync {
    /// Execute a program with arguments, an optional environment and working
    /// directory.
    fn run(
        &self,
        program: &str,
        args: &[String],
        env: Option<&BTreeMap<String, String>>,
        cwd: Option<&Path>,
    ) -> Result<CommandOutput, String>;

    /// Execute a program, retaining at most `limit` bytes of each stream.
    ///
    /// Runners that read a process directly bound the memory they use; the
    /// default keeps the leading bytes of what the runner already captured.
    fn run_bounded(
        &self,
        program: &str,
        args: &[String],
        env: Option<&BTreeMap<String, String>>,
        cwd: Option<&Path>,
        limit: usize,
    ) -> Result<CommandOutput, String> {
        let mut output = self.run(program, args, env, cwd)?;
        output.stdout.truncate(limit);
        output.stderr.truncate(limit);
        Ok(output)
    }
}

/// The runner that spawns a real process.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemRunner {
    /// When the command must have finished. A command still running at the
    /// deadline is killed and reaped.
    pub deadline: Option<Instant>,
}

impl SystemRunner {
    /// A runner that kills and reaps commands past `deadline`.
    pub fn with_deadline(deadline: Instant) -> Self {
        Self {
            deadline: Some(deadline),
        }
    }

    fn execute(
        &self,
        program: &str,
        args: &[String],
        env: Option<&BTreeMap<String, String>>,
        cwd: Option<&Path>,
        limit: Option<usize>,
    ) -> Result<CommandOutput, String> {
        let mut command = Command::new(program);
        command.args(args);
        if let Some(env) = env {
            command.env_clear();
            command.envs(env);
        }
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command
            .spawn()
            .map_err(|error| format!("{program}: {error}"))?;
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");
        let stdout = std::thread::spawn(move || read_stream(stdout, limit));
        let stderr = std::thread::spawn(move || read_stream(stderr, limit));

        let status = self.wait(program, &mut child)?;
        Ok(CommandOutput {
            status: status.code().unwrap_or(-1),
            stdout: stdout.join().unwrap_or_default(),
            stderr: stderr.join().unwrap_or_default(),
        })
    }

    fn wait(
        &self,
        program: &str,
        child: &mut std::process::Child,
    ) -> Result<std::process::ExitStatus, String> {
        loop {
            if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
                return Ok(status);
            }
            if self
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
            {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{program} did not finish before the host preparation deadline"
                ));
            }
            std::thread::sleep(POLL);
        }
    }
}

impl CommandRunner for SystemRunner {
    fn run(
        &self,
        program: &str,
        args: &[String],
        env: Option<&BTreeMap<String, String>>,
        cwd: Option<&Path>,
    ) -> Result<CommandOutput, String> {
        self.execute(program, args, env, cwd, None)
    }

    fn run_bounded(
        &self,
        program: &str,
        args: &[String],
        env: Option<&BTreeMap<String, String>>,
        cwd: Option<&Path>,
        limit: usize,
    ) -> Result<CommandOutput, String> {
        self.execute(program, args, env, cwd, Some(limit))
    }
}

/// Read a stream, keeping at most `limit` bytes and draining the rest.
fn read_stream(mut stream: impl Read, limit: Option<usize>) -> Vec<u8> {
    let mut retained = Vec::with_capacity(limit.unwrap_or(0).min(8 * 1024));
    let mut buffer = [0u8; 8192];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => match limit {
                Some(limit) if retained.len() >= limit => {}
                Some(limit) => {
                    let keep = (limit - retained.len()).min(read);
                    retained.extend_from_slice(&buffer[..keep]);
                }
                None => retained.extend_from_slice(&buffer[..read]),
            },
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    retained
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Vec<String> {
        vec!["-c".to_string(), script.to_string()]
    }

    #[test]
    fn a_bounded_run_retains_only_the_limit() {
        let runner = SystemRunner::default();
        let output = runner
            .run_bounded("sh", &sh("head -c 200000 /dev/zero"), None, None, 4096)
            .expect("run");
        assert!(output.success(), "{output:?}");
        assert_eq!(output.stdout.len(), 4096);
    }

    #[test]
    fn an_unbounded_run_keeps_everything() {
        let runner = SystemRunner::default();
        let output = runner
            .run("sh", &sh("head -c 200000 /dev/zero"), None, None)
            .expect("run");
        assert!(output.success(), "{output:?}");
        assert_eq!(output.stdout.len(), 200000);
    }

    #[test]
    fn a_deadline_terminates_and_reaps_the_command() {
        let runner = SystemRunner {
            deadline: Some(Instant::now() + Duration::from_millis(100)),
        };
        let started = Instant::now();
        let error = runner
            .run("sh", &sh("sleep 30"), None, None)
            .expect_err("a command past the deadline is stopped");
        assert!(error.contains("deadline"), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the command must not outlive the deadline"
        );
    }
}
