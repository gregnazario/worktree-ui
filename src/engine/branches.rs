//! Branch operations: list, create, switch, rename, delete. All commands
//! are argv-based with validated branch names (no shell interpolation).

use crate::engine::{self, GitError, Result};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchInfo {
    /// Full ref name (e.g. `refs/heads/main` or `refs/remotes/origin/main`).
    pub ref_name: String,
    /// Short display name (e.g. `main` or `origin/main`).
    pub short: String,
    /// True for the currently checked-out branch.
    pub is_current: bool,
    /// True for remote-tracking branches.
    pub is_remote: bool,
    /// Commits ahead of upstream (local only).
    pub ahead: u32,
    /// Commits behind of upstream (local only).
    pub behind: u32,
}

/// Lists local branches with current-branch markers and ahead/behind.
pub fn list(worktree: &Path) -> Result<Vec<BranchInfo>> {
    // %(HEAD) = * for current, %(refname:short), %(upstream:track)
    let out = engine::run(
        worktree,
        &[
            "--no-optional-locks",
            "branch",
            "--format=%(HEAD)%01%(refname:short)%01%(upstream:track)",
        ],
    )?;
    let mut branches = Vec::new();
    for line in out.lines() {
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(3, '\u{1}');
        let head_marker = parts.next().unwrap_or("").trim();
        let short = parts.next().unwrap_or("").to_string();
        let track = parts.next().unwrap_or("").to_string();
        let (ahead, behind) = parse_track(&track);
        branches.push(BranchInfo {
            ref_name: short.clone(),
            short,
            is_current: head_marker == "*",
            is_remote: false,
            ahead,
            behind,
        });
    }
    Ok(branches)
}

/// Parses `%(upstream:track)` output like `[ahead 1]`, `[behind 2]`, or
/// `[ahead 1, behind 3]` into (ahead, behind) counts.
fn parse_track(track: &str) -> (u32, u32) {
    let mut ahead = 0u32;
    let mut behind = 0u32;
    for token in track.split(',') {
        let token = token.trim().trim_start_matches('[').trim_end_matches(']');
        if let Some(n) = token.strip_prefix("ahead ") {
            ahead = n.trim().parse().unwrap_or(0);
        } else if let Some(n) = token.strip_prefix("behind ") {
            behind = n.trim().parse().unwrap_or(0);
        }
    }
    (ahead, behind)
}

/// Creates a new branch at the current HEAD (does not switch).
pub fn create(worktree: &Path, name: &str) -> Result<()> {
    validate_branch_name(name)?;
    engine::run_trimmed(worktree, &["branch", name]).map(|_| ())
}

/// Switches to an existing branch (or detached commit).
pub fn switch(worktree: &Path, target: &str) -> Result<()> {
    engine::run_trimmed(worktree, &["checkout", "-q", target]).map(|_| ())
}

/// Renames a local branch.
pub fn rename(worktree: &Path, old: &str, new: &str) -> Result<()> {
    validate_branch_name(new)?;
    engine::run_trimmed(worktree, &["branch", "-m", old, new]).map(|_| ())
}

/// Deletes a local branch (refuses the current branch).
pub fn delete(worktree: &Path, name: &str, current: &str) -> Result<()> {
    if name == current {
        return Err(GitError {
            message: "cannot delete the current branch — switch to another branch first".into(),
        });
    }
    engine::run_trimmed(worktree, &["branch", "-d", name]).map(|_| ())
}

/// Branch names cannot start with `-` (option injection) or contain
/// whitespace/control characters (git enforces most of this; we validate
/// early for clear error messages).
fn validate_branch_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.starts_with('-')
        || name.bytes().any(|b| b.is_ascii_control() || b == b' ')
    {
        return Err(GitError {
            message: format!("invalid branch name: {name:?}"),
        });
    }
    Ok(())
}

/// Merges a branch into the current branch. Returns the list of
/// conflicted file paths (empty on success).
pub fn merge(worktree: &Path, branch: &str) -> Result<Vec<String>> {
    let result = engine::run_trimmed(worktree, &["merge", "--no-edit", branch]);
    match result {
        Ok(_) => Ok(Vec::new()),
        Err(e) => {
            // Check for conflicts: git exits non-zero with CONFLICT markers
            let status = engine::run_bytes(
                worktree,
                &[
                    "--no-optional-locks",
                    "diff",
                    "--name-only",
                    "--diff-filter=U",
                ],
            )?;
            let conflicts: Vec<String> = status
                .split(|b| *b == b'\n')
                .filter(|r| !r.is_empty())
                .map(|r| String::from_utf8_lossy(r).into_owned())
                .collect();
            if conflicts.is_empty() {
                // Not a conflict — propagate the original error.
                Err(e)
            } else {
                Err(GitError {
                    message: format!(
                        "merge conflicts in {} files — resolve them in the Working Copy section",
                        conflicts.len()
                    ),
                })
            }
        }
    }
}

/// Rebases the current branch onto `onto`. Returns conflicted file paths
/// on failure (same as merge).
pub fn rebase(worktree: &Path, onto: &str) -> Result<Vec<String>> {
    let result = engine::run_trimmed(worktree, &["rebase", onto]);
    match result {
        Ok(_) => Ok(Vec::new()),
        Err(e) => {
            let status = engine::run_bytes(
                worktree,
                &[
                    "--no-optional-locks",
                    "diff",
                    "--name-only",
                    "--diff-filter=U",
                ],
            )?;
            let conflicts: Vec<String> = status
                .split(|b| *b == b'\n')
                .filter(|r| !r.is_empty())
                .map(|r| String::from_utf8_lossy(r).into_owned())
                .collect();
            if conflicts.is_empty() {
                Err(e)
            } else {
                Err(GitError {
                    message: format!(
                        "rebase conflicts in {} files — resolve them in the Working Copy section",
                        conflicts.len()
                    ),
                })
            }
        }
    }
}
