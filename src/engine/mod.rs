//! Typed git CLI layer. Every command takes the worktree path, runs a
//! blocking `std::process` call (callers: GPUI background executor), and
//! parses only machine formats. Read-only commands pass
//! `--no-optional-locks` so the app can never block the user's own git
//! processes on index.lock.

use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug)]
pub struct GitError {
    pub message: String,
}

impl GitError {
    /// git refused because another process holds index.lock.
    pub fn is_lock_error(&self) -> bool {
        self.message.contains("index.lock")
    }
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

pub type Result<T> = std::result::Result<T, GitError>;

/// Runs `git` and returns stdout verbatim (no trailing trim): `-z` records
/// are NUL-terminated and parsed positionally.
pub fn run(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = command(cwd, args)
        .output()
        .map_err(|e| GitError { message: format!("failed to run git: {e}") })?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(stderr_error(&output.stderr))
    }
}

pub fn run_trimmed(cwd: &Path, args: &[&str]) -> Result<String> {
    Ok(run(cwd, args)?.trim_end().to_string())
}

fn command(cwd: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

fn stderr_error(stderr: &[u8]) -> GitError {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text.trim().lines().collect();
    // git's index.lock diagnostic is multi-line: only its first line names
    // index.lock (the last line is the advice "remove the file manually to
    // continue."). Prefer a line naming the lock so `GitError::is_lock_error`
    // can classify the failure; every other error keeps the last stderr line.
    let message = lines
        .iter()
        .rev()
        .find(|l| l.contains("index.lock"))
        .or(lines.last())
        .copied()
        .unwrap_or("git failed");
    GitError {
        message: message.to_string(),
    }
}
