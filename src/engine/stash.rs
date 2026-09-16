//! Stash operations: list, push, pop, apply, drop.

use crate::engine::{self, Result};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StashEntry {
    /// Stash index (0 = most recent).
    pub index: usize,
    /// Short descriptive message (e.g. "WIP on main: abc123 subject").
    pub message: String,
    /// The stash ref (e.g. `stash@{0}`).
    pub ref_name: String,
}

/// Lists stash entries, newest first.
pub fn list(worktree: &Path) -> Result<Vec<StashEntry>> {
    let out = engine::run(
        worktree,
        &[
            "--no-optional-locks",
            "stash",
            "list",
            "--format=%x00%gd%x01%gs",
        ],
    )?;
    let mut entries = Vec::new();
    for record in out.split('\u{0}') {
        if record.is_empty() {
            continue;
        }
        let mut parts = record.splitn(2, '\u{1}');
        let ref_name = parts.next().unwrap_or("").trim().to_string();
        let message = parts.next().unwrap_or("").trim_end().to_string();
        let index = ref_name
            .strip_prefix("stash@{")
            .and_then(|s| s.strip_suffix('}'))
            .and_then(|s| s.parse().ok())
            .unwrap_or(entries.len());
        entries.push(StashEntry {
            index,
            message,
            ref_name,
        });
    }
    Ok(entries)
}

/// Stashes all working-copy changes (tracked + untracked).
pub fn push(worktree: &Path, message: Option<&str>) -> Result<()> {
    let mut args = vec!["stash", "push", "-q", "--include-untracked"];
    if let Some(msg) = message {
        args.push("-m");
        args.push(msg);
    }
    engine::run_trimmed(worktree, &args).map(|_| ())
}

/// Applies the most recent stash without dropping it.
pub fn apply(worktree: &Path) -> Result<()> {
    engine::run_trimmed(worktree, &["stash", "apply", "-q"]).map(|_| ())
}

/// Applies a specific stash entry (by its `stash@{N}` index) without
/// dropping it.
pub fn apply_at(worktree: &Path, index: usize) -> Result<()> {
    engine::run_trimmed(
        worktree,
        &["stash", "apply", "-q", &format!("stash@{{{index}}}")],
    )
    .map(|_| ())
}

/// Applies and drops the most recent stash.
pub fn pop(worktree: &Path) -> Result<()> {
    engine::run_trimmed(worktree, &["stash", "pop", "-q"]).map(|_| ())
}

/// Applies and drops a specific stash entry.
pub fn pop_at(worktree: &Path, index: usize) -> Result<()> {
    engine::run_trimmed(
        worktree,
        &["stash", "pop", "-q", &format!("stash@{{{index}}}")],
    )
    .map(|_| ())
}

/// Drops a specific stash entry by index.
pub fn drop(worktree: &Path, index: usize) -> Result<()> {
    engine::run_trimmed(
        worktree,
        &["stash", "drop", "-q", &format!("stash@{{{index}}}")],
    )
    .map(|_| ())
}
