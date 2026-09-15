//! Remote operations: fetch, push, pull. All go through the system git so
//! credential helpers and SSH config just work.

use crate::engine::{self, Result};
use std::path::Path;

/// Fetches all remotes with prune (removes stale tracking refs).
pub fn fetch(worktree: &Path) -> Result<()> {
    engine::run_trimmed(worktree, &["fetch", "--all", "--prune", "-q"]).map(|_| ())
}

/// Pushes the current branch to its upstream (or origin with -u on first push).
pub fn push(worktree: &Path, branch: &str, set_upstream: bool) -> Result<()> {
    let mut args = vec!["push", "-q"];
    if set_upstream {
        args.push("--set-upstream");
        args.push("origin");
        args.push(branch);
    } else {
        args.push("origin");
        args.push(branch);
    }
    engine::run_trimmed(worktree, &args).map(|_| ())
}

/// Force-pushes with lease (safer than --force).
pub fn push_force_with_lease(worktree: &Path, branch: &str) -> Result<()> {
    engine::run_trimmed(
        worktree,
        &["push", "-q", "--force-with-lease", "origin", branch],
    )
    .map(|_| ())
}

/// Pulls with --ff-only (refuses to create merge commits; use merge/rebase
/// for divergent branches).
pub fn pull(worktree: &Path) -> Result<()> {
    engine::run_trimmed(worktree, &["pull", "--ff-only", "-q"]).map(|_| ())
}
