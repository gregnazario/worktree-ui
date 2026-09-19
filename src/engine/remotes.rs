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

/// One configured remote from `git remote -v`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteInfo {
    pub name: String,
    /// The fetch URL (what pulls and fetches use).
    pub fetch_url: String,
    /// The push URL; equals `fetch_url` unless explicitly split.
    pub push_url: String,
}

/// Lists the repo's remotes with their fetch/push URLs.
pub fn list(worktree: &Path) -> Result<Vec<RemoteInfo>> {
    let out = engine::run(worktree, &["--no-optional-locks", "remote", "-v"])?;
    let mut remotes: Vec<RemoteInfo> = Vec::new();
    for line in out.lines() {
        // "name<TAB>url (fetch|push)"
        let Some((name, rest)) = line.split_once('\t') else {
            continue;
        };
        let Some((url, kind)) = rest.rsplit_once(' ') else {
            continue;
        };
        let url = url.trim();
        if name.is_empty() || url.is_empty() {
            continue;
        }
        let kind = kind.trim_start_matches('(').trim_end_matches(')');
        match remotes.iter_mut().find(|r| r.name == name) {
            Some(existing) => {
                if kind == "push" {
                    existing.push_url = url.to_string();
                }
            }
            None => {
                remotes.push(RemoteInfo {
                    name: name.to_string(),
                    fetch_url: url.to_string(),
                    push_url: if kind == "push" {
                        url.to_string()
                    } else {
                        String::new()
                    },
                });
            }
        }
    }
    // The fetch line always comes first per remote, so any remote still
    // missing a fetch URL here had no fetch entry at all — skip it.
    remotes.retain(|r| !r.fetch_url.is_empty());
    for r in &mut remotes {
        if r.push_url.is_empty() {
            r.push_url = r.fetch_url.clone();
        }
    }
    Ok(remotes)
}

/// Remote names cannot be empty, start with `-` (option injection), or
/// contain whitespace/control characters (git rejects most of these;
/// validating early gives clear errors).
fn validate_remote_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.starts_with('-')
        || name.contains('/')
        || name.bytes().any(|b| b.is_ascii_control() || b == b' ')
    {
        return Err(crate::engine::GitError {
            message: format!("invalid remote name: {name:?}"),
        });
    }
    Ok(())
}

/// Adds a remote.
pub fn add(worktree: &Path, name: &str, url: &str) -> Result<()> {
    validate_remote_name(name)?;
    if url.is_empty() || url.bytes().any(|b| b.is_ascii_control()) {
        return Err(crate::engine::GitError {
            message: format!("invalid remote url: {url:?}"),
        });
    }
    engine::run_trimmed(worktree, &["remote", "add", name, url]).map(|_| ())
}

/// Removes a remote (its tracking refs go with it — git's own
/// semantics, confirmed by the confirm dialog before this runs).
pub fn remove(worktree: &Path, name: &str) -> Result<()> {
    validate_remote_name(name)?;
    engine::run_trimmed(worktree, &["remote", "remove", name]).map(|_| ())
}
