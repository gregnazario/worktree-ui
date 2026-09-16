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

pub mod branches;
pub mod commit;
pub mod diff;
pub mod history;
pub mod mutate;
pub mod remotes;
pub mod rewrite;
pub mod stash;
pub mod working_copy;

/// Runs `git` and returns stdout verbatim (no trailing trim): `-z` records
/// are NUL-terminated and parsed positionally.
pub fn run(cwd: &Path, args: &[&str]) -> Result<String> {
    Ok(String::from_utf8_lossy(&run_bytes(cwd, args)?).into_owned())
}

/// Raw stdout as bytes — required wherever output must survive byte-exact
/// even when it isn't valid UTF-8 (e.g. diff hunks destined for `git apply`).
pub fn run_bytes(cwd: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = command(cwd, args).output().map_err(|e| GitError {
        message: format!("failed to run git: {e}"),
    })?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(stderr_error(&output.stderr))
    }
}

/// Like [`run_bytes`], but `input` goes to the child's stdin and is
/// consumed (patches can be multi-MB — no reason to copy). All three
/// pipes are handled concurrently — stdin is written from a helper
/// thread, and stdout/stderr are each drained by their own reader
/// threads before `wait()` — so a child that fills any pipe (a `git
/// apply` rejecting many hunks writes large stderr) can never deadlock
/// the caller against unread output.
pub fn run_bytes_stdin(cwd: &Path, args: &[&str], input: Vec<u8>) -> Result<Vec<u8>> {
    use std::io::{Read as _, Write as _};
    let mut child = command(cwd, args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| GitError {
            message: format!("failed to run git: {e}"),
        })?;
    {
        let mut stdin = child.stdin.take().expect("just configured piped");
        std::thread::spawn(move || {
            let _ = stdin.write_all(&input); // EPIPE if git exited early — fine
        });
    }
    let mut stdout_pipe = child.stdout.take().expect("just configured piped");
    let mut stderr_pipe = child.stderr.take().expect("just configured piped");
    let stdout_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        buf
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        buf
    });
    let status = child.wait().map_err(|e| GitError {
        message: format!("failed to run git: {e}"),
    })?;
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    if status.success() {
        Ok(stdout)
    } else {
        Err(stderr_error(&stderr))
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
