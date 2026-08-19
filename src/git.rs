use crate::model::{
    parse_status_porcelain_v2, parse_worktree_porcelain, WorktreeEntry, WorktreeStatus,
};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug)]
pub struct GitError {
    pub message: String,
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

pub type Result<T> = std::result::Result<T, GitError>;

/// Runs `git` with the given args, returning trimmed stdout. On failure the
/// error carries git's last stderr line.
///
/// Blocking on purpose: these calls run on GPUI background executor threads,
/// which keeps the UI responsive without any async-runtime plumbing.
pub fn run_git(cwd: Option<&Path>, args: &[&str]) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let output = cmd.output().map_err(|e| GitError {
        message: format!("failed to run git: {e}"),
    })?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let last = stderr.trim().lines().last().unwrap_or("git failed");
        Err(GitError {
            message: last.to_string(),
        })
    }
}

/// Resolves the repository root containing `start`.
pub fn repo_root(start: &Path) -> Result<PathBuf> {
    Ok(PathBuf::from(run_git(
        Some(start),
        &["rev-parse", "--show-toplevel"],
    )?))
}

pub fn list_worktrees(root: &Path) -> Result<Vec<WorktreeEntry>> {
    let out = run_git(Some(root), &["worktree", "list", "--porcelain"])?;
    Ok(parse_worktree_porcelain(&out))
}

/// Runs `git status` against every worktree, filling in each entry's status.
/// Failures degrade that entry to `Unavailable`; the batch never fails.
pub fn status_pass(entries: Vec<WorktreeEntry>) -> Vec<WorktreeEntry> {
    entries
        .into_iter()
        .map(|mut e| {
            match run_git(Some(&e.path), &["status", "--porcelain=v2", "--branch"]) {
                Ok(out) => e.status = parse_status_porcelain_v2(&out),
                Err(err) => e.status = WorktreeStatus::Unavailable(err.message),
            }
            e
        })
        .collect()
}

/// Adds a worktree at `path`. `branch: Some(name)` creates a new branch off
/// `base`; `branch: None` checks out `base` as an existing branch.
pub fn add_worktree(root: &Path, path: &Path, branch: Option<&str>, base: &str) -> Result<()> {
    let path_str = path.to_string_lossy();
    let res = match branch {
        Some(branch) => run_git(
            Some(root),
            &["worktree", "add", &path_str, "-b", branch, base],
        ),
        None => run_git(Some(root), &["worktree", "add", &path_str, base]),
    };
    res.map(|_| ())
}

pub fn remove_worktree(root: &Path, path: &Path, force: bool) -> Result<()> {
    let path_str = path.to_string_lossy();
    let res = if force {
        run_git(Some(root), &["worktree", "remove", "--force", &path_str])
    } else {
        run_git(Some(root), &["worktree", "remove", &path_str])
    };
    res.map(|_| ())
}

pub fn prune(root: &Path) -> Result<()> {
    run_git(Some(root), &["worktree", "prune"]).map(|_| ())
}

pub fn local_branches(root: &Path) -> Result<Vec<String>> {
    let out = run_git(Some(root), &["branch", "--format=%(refname:short)"])?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect())
}

/// Best-effort default branch: origin/HEAD if set, else main, else master.
pub fn default_branch(root: &Path) -> String {
    if let Ok(head) = run_git(
        Some(root),
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    ) {
        if let Some(name) = head.rsplit('/').next() {
            if !name.is_empty() {
                return name.to_string();
            }
        }
    }
    let branches = local_branches(root).unwrap_or_default();
    if branches.iter().any(|b| b == "main") {
        "main".into()
    } else if branches.iter().any(|b| b == "master") {
        "master".into()
    } else {
        "main".into()
    }
}
