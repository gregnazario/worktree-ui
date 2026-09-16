//! Remote operations: fetch, push, pull. All go through the system git so
//! credential helpers and SSH config just work.

use crate::engine::{self, Result};
use std::path::Path;

/// Fetches all remotes with prune (removes stale tracking refs).
pub fn fetch(worktree: &Path) -> Result<()> {
    engine::run_trimmed(worktree, &["fetch", "--all", "--prune", "-q"]).map(|_| ())
}

/// The remote `branch`'s upstream tracks, e.g. `upstream` for a branch
/// whose upstream is `upstream/main`. None when the branch has no
/// upstream (or a detached HEAD).
fn upstream_remote(worktree: &Path, branch: &str) -> Option<String> {
    let out = engine::run_trimmed(
        worktree,
        &[
            "--no-optional-locks",
            "for-each-ref",
            "--format=%(upstream:remotename)",
            &format!("refs/heads/{branch}"),
        ],
    )
    .ok()?;
    let remote = out.trim();
    (!remote.is_empty()).then(|| remote.to_string())
}

/// Pushes `branch` to the remote its upstream tracks (falling back to
/// `origin` for a not-yet-pushed branch) — never a hardcoded remote name.
pub fn push(worktree: &Path, branch: &str, set_upstream: bool) -> Result<()> {
    if branch.starts_with('-') {
        return Err(crate::engine::GitError {
            message: format!("invalid branch name: {branch:?}"),
        });
    }
    let remote = upstream_remote(worktree, branch).unwrap_or_else(|| "origin".to_string());
    let mut args = vec!["push", "-q"];
    if set_upstream {
        args.push("--set-upstream");
    }
    args.push(&remote);
    args.push(branch);
    engine::run_trimmed(worktree, &args).map(|_| ())
}

/// Force-pushes with lease to the branch's tracked remote (safer than
/// `--force`: refuses when someone else pushed in the meantime).
pub fn push_force_with_lease(worktree: &Path, branch: &str) -> Result<()> {
    if branch.starts_with('-') {
        return Err(crate::engine::GitError {
            message: format!("invalid branch name: {branch:?}"),
        });
    }
    let remote = upstream_remote(worktree, branch).unwrap_or_else(|| "origin".to_string());
    engine::run_trimmed(
        worktree,
        &["push", "-q", "--force-with-lease", &remote, branch],
    )
    .map(|_| ())
}

/// Pulls with --ff-only (refuses to create merge commits; use merge/rebase
/// for divergent branches).
pub fn pull(worktree: &Path) -> Result<()> {
    engine::run_trimmed(worktree, &["pull", "--ff-only", "-q"]).map(|_| ())
}
