//! In-progress sequence state: merge / rebase / cherry-pick / revert
//! operations that paused on a conflict. The state is probed from git
//! (never app memory), so it survives restarts and is visible to any
//! other git client. Resolution happens in the Working Copy section
//! (edit + `s` to stage); these helpers continue, skip, or abort the
//! paused operation.

use crate::engine::{self, GitError, Result};
use std::path::Path;

/// A paused multi-step git operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InProgress {
    Merge,
    Rebase,
    CherryPick,
    Revert,
}

impl InProgress {
    /// The `--continue`-style subcommand this state resumes.
    fn command(self) -> &'static str {
        match self {
            InProgress::Merge => "merge",
            InProgress::Rebase => "rebase",
            InProgress::CherryPick => "cherry-pick",
            InProgress::Revert => "revert",
        }
    }

    /// Human name for banners and messages.
    pub fn label(self) -> &'static str {
        match self {
            InProgress::Merge => "merge",
            InProgress::Rebase => "rebase",
            InProgress::CherryPick => "cherry-pick",
            InProgress::Revert => "revert",
        }
    }

    /// Whether the operation has a current step that can be skipped.
    pub fn skippable(self) -> bool {
        matches!(self, InProgress::Rebase | InProgress::CherryPick)
    }
}

/// `--git-path` answers relative to the worktree it ran in (absolute
/// for linked-worktree state), so relative answers resolve against
/// `worktree` — not this process's cwd.
fn resolve_git_path(worktree: &Path, name: &str) -> Option<std::path::PathBuf> {
    let p = engine::run_trimmed(worktree, &["rev-parse", "--git-path", name])
        .ok()?
        .trim()
        .to_string();
    let path = Path::new(&p);
    Some(if path.is_absolute() {
        path.to_path_buf()
    } else {
        worktree.join(path)
    })
}

fn git_path_exists(worktree: &Path, name: &str) -> bool {
    resolve_git_path(worktree, name)
        .map(|p| p.exists())
        .unwrap_or(false)
}

/// The paused operation in this worktree, or None. Precedence matters:
/// a rebase conflict ALSO sets CHERRY_PICK_HEAD, so the rebase marker
/// is checked first.
pub fn operation_state(worktree: &Path) -> Option<InProgress> {
    if git_path_exists(worktree, "rebase-merge") || git_path_exists(worktree, "rebase-apply") {
        return Some(InProgress::Rebase);
    }
    if git_path_exists(worktree, "CHERRY_PICK_HEAD") {
        return Some(InProgress::CherryPick);
    }
    if git_path_exists(worktree, "REVERT_HEAD") {
        return Some(InProgress::Revert);
    }
    if git_path_exists(worktree, "MERGE_HEAD") {
        return Some(InProgress::Merge);
    }
    None
}

/// (current step, total steps) of an in-progress rebase, from the
/// sequencer's own bookkeeping files. None outside a rebase or when
/// the files are unreadable.
pub fn rebase_progress(worktree: &Path) -> Option<(u64, u64)> {
    if operation_state(worktree) != Some(InProgress::Rebase) {
        return None;
    }
    let dir = resolve_git_path(worktree, "rebase-merge")?;
    let read_num = |name: &str| -> Option<u64> {
        std::fs::read_to_string(dir.join(name))
            .ok()?
            .trim()
            .parse()
            .ok()
    };
    Some((read_num("msgnum")?, read_num("end")?))
}

/// Continues the paused operation (`git <op> --continue`). The stored
/// message (MERGE_MSG / the pick's original message / REVERT_MSG) is
/// accepted without an editor. On failure the state is left UNTOUCHED —
/// the user may be mid-resolution, and aborting here would destroy
/// their progress; unresolved conflicts come back as the `Ok` payload,
/// any other git refusal as `Err`.
pub fn continue_op(worktree: &Path) -> Result<Vec<String>> {
    let op = operation_state(worktree).ok_or_else(|| GitError {
        message: "no operation in progress".into(),
    })?;
    let result = engine::run_trimmed(
        worktree,
        &["-c", "core.editor=true", op.command(), "--continue"],
    );
    match result {
        Ok(_) => Ok(Vec::new()),
        Err(e) => {
            let conflicts = engine_conflicted(worktree);
            if conflicts.is_empty() {
                Err(e)
            } else {
                Ok(conflicts)
            }
        }
    }
}

/// Aborts the paused operation, restoring the pre-operation state.
pub fn abort_op(worktree: &Path) -> Result<()> {
    let op = operation_state(worktree).ok_or_else(|| GitError {
        message: "no operation in progress".into(),
    })?;
    engine::run_trimmed(worktree, &[op.command(), "--abort"]).map(|_| ())
}

/// Skips the current step (rebase / cherry-pick only). The NEXT step
/// may conflict immediately: its conflicted paths are returned and the
/// operation stays paused on the new state.
pub fn skip_op(worktree: &Path) -> Result<Vec<String>> {
    let op = operation_state(worktree).ok_or_else(|| GitError {
        message: "no operation in progress".into(),
    })?;
    if !op.skippable() {
        return Err(GitError {
            message: format!("{} has no current step to skip", op.label()),
        });
    }
    let result = engine::run_trimmed(
        worktree,
        &["-c", "core.editor=true", op.command(), "--skip"],
    );
    match result {
        Ok(_) => Ok(Vec::new()),
        Err(e) => {
            let conflicts = engine_conflicted(worktree);
            if conflicts.is_empty() {
                Err(e)
            } else {
                Ok(conflicts)
            }
        }
    }
}

fn engine_conflicted(worktree: &Path) -> Vec<String> {
    engine::run_bytes(
        worktree,
        &[
            "--no-optional-locks",
            "diff",
            "--name-only",
            "--diff-filter=U",
        ],
    )
    .map(|status| {
        status
            .split(|b| *b == b'\n')
            .filter(|r| !r.is_empty())
            .map(|r| String::from_utf8_lossy(r).into_owned())
            .collect()
    })
    .unwrap_or_default()
}
