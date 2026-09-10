use std::path::Path;
use std::process::{Command, Stdio};

use crate::git_sdk::{
    AuthorInfo, CommitInfo, CommitRange, GitSdk, GitSdkError, GitSdkResult, StatusEntry, StatusKind,
};

#[derive(Debug, Default, Clone, Copy)]
pub struct GitCliSdk;

impl GitCliSdk {
    fn run_git(repo_path: &Path, args: &[&str]) -> GitSdkResult<String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo_path)
            .output()
            .map_err(GitSdkError::from)?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            Err(GitSdkError::Git(format!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr)
            )))
        }
    }

    fn run_git_with_input(repo_path: &Path, args: &[&str], input: &str) -> GitSdkResult<String> {
        let mut child = Command::new("git")
            .args(args)
            .current_dir(repo_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(GitSdkError::from)?;

        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            stdin
                .write_all(input.as_bytes())
                .map_err(GitSdkError::from)?;
        }

        let output = child.wait_with_output().map_err(GitSdkError::from)?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            Err(GitSdkError::Git(format!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr)
            )))
        }
    }

    fn status_kind(x: char, y: char) -> StatusKind {
        if x == '?' && y == '?' {
            return StatusKind::Untracked;
        }
        if x == 'U' || y == 'U' {
            return StatusKind::Conflicted;
        }
        if x == 'A' {
            return StatusKind::Added;
        }
        if x == 'R' || y == 'R' {
            return StatusKind::Renamed;
        }
        if x == 'C' || y == 'C' {
            return StatusKind::Copied;
        }
        if x == 'D' || y == 'D' {
            return StatusKind::Deleted;
        }
        if x == 'M' || y == 'M' {
            return StatusKind::Modified;
        }
        StatusKind::Clean
    }
}

impl GitSdk for GitCliSdk {
    fn init(&self, path: &Path) -> GitSdkResult<()> {
        Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(path)
            .output()
            .map_err(GitSdkError::from)?;
        Ok(())
    }

    fn add(&self, repo_path: &Path, paths: &[String]) -> GitSdkResult<()> {
        let mut args = vec!["add", "--"];
        let path_args: Vec<&str> = paths.iter().map(String::as_str).collect();
        args.extend(path_args);
        Self::run_git(repo_path, &args)?;
        Ok(())
    }

    fn status(&self, repo_path: &Path) -> GitSdkResult<Vec<StatusEntry>> {
        let output = Self::run_git(repo_path, &["status", "--porcelain=v1", "-z"])?;
        let mut entries = Vec::new();
        let mut iter = output
            .split('\0')
            .filter(|entry| !entry.is_empty())
            .peekable();
        while let Some(entry) = iter.next() {
            if entry.len() < 3 {
                continue;
            }
            let mut chars = entry.chars();
            let x = chars.next().unwrap_or(' ');
            let y = chars.next().unwrap_or(' ');
            let path = entry[3..].to_string();
            let mut final_path = path;
            if x == 'R' || x == 'C' {
                if let Some(next_path) = iter.next() {
                    if !next_path.is_empty() {
                        final_path = next_path.to_string();
                    }
                }
            }
            let kind = Self::status_kind(x, y);
            if kind != StatusKind::Clean {
                entries.push(StatusEntry {
                    path: final_path,
                    status: kind,
                });
            }
        }
        Ok(entries)
    }

    fn commit(&self, repo_path: &Path, message: &str, author: &AuthorInfo) -> GitSdkResult<String> {
        Self::run_git(repo_path, &["add", "-A"])?;
        let author_value = format!("{} <{}>", author.name, author.email);
        Self::run_git(
            repo_path,
            &[
                "-c",
                &format!("user.name={}", author.name),
                "-c",
                &format!("user.email={}", author.email),
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                message,
                "--author",
                &author_value,
            ],
        )?;
        let head = Self::run_git(repo_path, &["rev-parse", "HEAD"])?;
        Ok(head.trim().to_string())
    }

    fn log(&self, repo_path: &Path, max: usize) -> GitSdkResult<Vec<CommitInfo>> {
        let output = Self::run_git(
            repo_path,
            &[
                "log",
                "-n",
                &max.to_string(),
                "--pretty=format:%H%x1f%an%x1f%ae%x1f%at%x1f%s%x1e",
            ],
        );

        let output = match output {
            Ok(value) => value,
            Err(_) => return Ok(Vec::new()),
        };

        let mut commits = Vec::new();
        for record in output.split('\x1e') {
            if record.trim().is_empty() {
                continue;
            }
            let parts: Vec<&str> = record.split('\x1f').collect();
            if parts.len() < 5 {
                continue;
            }
            commits.push(CommitInfo {
                id: parts[0].trim().to_string(),
                message: parts[4].trim().to_string(),
                author: AuthorInfo {
                    name: parts[1].trim().to_string(),
                    email: parts[2].trim().to_string(),
                },
                timestamp: parts[3].trim().parse::<i64>().unwrap_or_default(),
            });
        }
        Ok(commits)
    }

    fn branch(&self, repo_path: &Path, name: &str, target: Option<&str>) -> GitSdkResult<()> {
        let mut args = vec!["branch", name];
        if let Some(reference) = target {
            args.push(reference);
        }
        Self::run_git(repo_path, &args)?;
        Ok(())
    }

    fn checkout(&self, repo_path: &Path, reference: &str) -> GitSdkResult<()> {
        Self::run_git(repo_path, &["checkout", reference])?;
        Ok(())
    }

    fn squash(
        &self,
        repo_path: &Path,
        range: &CommitRange,
        message: &str,
        author: &AuthorInfo,
    ) -> GitSdkResult<String> {
        Self::run_git(repo_path, &["checkout", &range.end])?;

        let base = Self::run_git(repo_path, &["rev-parse", &format!("{}^", range.start)]);
        match base {
            Ok(base_ref) => {
                let base_ref = base_ref.trim();
                Self::run_git(repo_path, &["reset", "--soft", base_ref])?;
            }
            Err(_) => {
                Self::run_git(repo_path, &["reset", "--soft", "--root"])?;
            }
        }

        let author_value = format!("{} <{}>", author.name, author.email);
        Self::run_git(
            repo_path,
            &[
                "-c",
                &format!("user.name={}", author.name),
                "-c",
                &format!("user.email={}", author.email),
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                message,
                "--author",
                &author_value,
            ],
        )?;

        let head = Self::run_git(repo_path, &["rev-parse", "HEAD"])?;
        Ok(head.trim().to_string())
    }

    fn diff(&self, repo_path: &Path, from: Option<&str>, to: Option<&str>) -> GitSdkResult<String> {
        let mut args = vec!["diff", "--binary"];
        if let Some(from_ref) = from {
            args.push(from_ref);
        }
        if let Some(to_ref) = to {
            args.push(to_ref);
        }
        Self::run_git(repo_path, &args)
    }

    fn apply_patch(&self, repo_path: &Path, patch: &str) -> GitSdkResult<()> {
        Self::run_git_with_input(repo_path, &["apply", "--whitespace=nowarn", "-"], patch)?;
        Ok(())
    }
}
