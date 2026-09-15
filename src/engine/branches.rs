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

/// Lists local and remote-tracking branches: locals first (git's `-a`
/// order), each with current-branch markers and ahead/behind where an
/// upstream exists.
pub fn list(worktree: &Path) -> Result<Vec<BranchInfo>> {
    // %(HEAD) = * for current; %(refname) is the FULL name so locals and
    // remotes are unambiguous (`refs/remotes/origin/main` vs a local
    // branch literally named `origin/main`).
    let out = engine::run(
        worktree,
        &[
            "--no-optional-locks",
            "branch",
            "-a",
            "--format=%(HEAD)%01%(refname)%01%(upstream:track)",
        ],
    )?;
    let mut branches = Vec::new();
    for line in out.lines() {
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(3, '\u{1}');
        let head_marker = parts.next().unwrap_or("").trim();
        let refname = parts.next().unwrap_or("");
        let track = parts.next().unwrap_or("").to_string();
        let (is_remote, short) = match refname.strip_prefix("refs/heads/") {
            Some(short) => (false, short.to_string()),
            None => match refname.strip_prefix("refs/remotes/") {
                // The symbolic remote HEAD renders as a plain
                // `refs/remotes/<remote>/HEAD` refname with %(refname) —
                // skip it or every clone shows a phantom `origin/HEAD`.
                Some(short) if short.ends_with("/HEAD") => continue,
                Some(short) => (true, short.to_string()),
                None => continue,
            },
        };
        if short.is_empty() {
            continue;
        }
        // Ahead/behind only applies to local branches' upstreams; a
        // remote-tracking ref's own upstream tracking is noise.
        let (ahead, behind) = if is_remote {
            (0, 0)
        } else {
            parse_track(&track)
        };
        branches.push(BranchInfo {
            ref_name: refname.to_string(),
            short,
            is_current: head_marker == "*",
            is_remote,
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
    engine::run_trimmed(worktree, &["branch", "--", name]).map(|_| ())
}

/// Switches to an existing branch (or detached commit). No `--` here:
/// after `--`, git checkout treats the argument as a PATH, not a ref.
pub fn switch(worktree: &Path, target: &str) -> Result<()> {
    validate_branch_name(target)?;
    engine::run_trimmed(worktree, &["checkout", "-q", target]).map(|_| ())
}

/// Renames a local branch.
pub fn rename(worktree: &Path, old: &str, new: &str) -> Result<()> {
    validate_branch_name(old)?;
    validate_branch_name(new)?;
    engine::run_trimmed(worktree, &["branch", "-m", "--", old, new]).map(|_| ())
}

/// Deletes a local branch (refuses the current branch).
pub fn delete(worktree: &Path, name: &str, current: &str) -> Result<()> {
    validate_branch_name(name)?;
    if name == current {
        return Err(GitError {
            message: "cannot delete the current branch — switch to another branch first".into(),
        });
    }
    engine::run_trimmed(worktree, &["branch", "-d", "--", name]).map(|_| ())
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

/// Paths with unresolved merge conflicts (`git diff --name-only
/// --diff-filter=U`).
fn conflicted_files(worktree: &Path) -> Result<Vec<String>> {
    let status = engine::run_bytes(
        worktree,
        &[
            "--no-optional-locks",
            "diff",
            "--name-only",
            "--diff-filter=U",
        ],
    )?;
    Ok(status
        .split(|b| *b == b'\n')
        .filter(|r| !r.is_empty())
        .map(|r| String::from_utf8_lossy(r).into_owned())
        .collect())
}

/// Merges a branch into the current branch. On conflict the conflicted
/// paths are returned as the `Ok` payload AND the merge is aborted —
/// there is no continue flow in this UI, and a wedged MERGE_HEAD would
/// block every later operation; aborting restores the pre-merge state.
pub fn merge(worktree: &Path, branch: &str) -> Result<Vec<String>> {
    validate_branch_name(branch)?;
    let result = engine::run_trimmed(worktree, &["merge", "--no-edit", "--", branch]);
    match result {
        Ok(_) => Ok(Vec::new()),
        Err(e) => {
            // A failing conflict query must not swallow `e` — and must
            // not skip the abort below, which unwedges the repo.
            match conflicted_files(worktree) {
                Ok(conflicts) if !conflicts.is_empty() => {
                    let _ = engine::run_trimmed(worktree, &["merge", "--abort"]);
                    Ok(conflicts)
                }
                // Not a conflict (or the query failed) — propagate the
                // original error.
                _ => Err(e),
            }
        }
    }
}

/// Rebases the current branch onto `onto`. On conflict the conflicted
/// paths are returned as the `Ok` payload and the rebase is aborted
/// (same rationale as `merge`: no continue flow, never stay mid-rebase).
pub fn rebase(worktree: &Path, onto: &str) -> Result<Vec<String>> {
    validate_branch_name(onto)?;
    let result = engine::run_trimmed(worktree, &["rebase", "--", onto]);
    match result {
        Ok(_) => Ok(Vec::new()),
        Err(e) => match conflicted_files(worktree) {
            Ok(conflicts) if !conflicts.is_empty() => {
                let _ = engine::run_trimmed(worktree, &["rebase", "--abort"]);
                Ok(conflicts)
            }
            _ => Err(e),
        },
    }
}
