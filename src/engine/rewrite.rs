//! History-rewrite operations on the checked-out branch: cherry-pick,
//! revert, and scripted interactive rebase. Same conflict contract as
//! `branches::merge`/`branches::rebase`: on conflict the conflicted paths
//! come back as the `Ok` payload AND the operation is aborted — there is
//! no continue flow in this UI, and wedged state (CHERRY_PICK_HEAD,
//! rebase-merge) would block every later operation. All commands are
//! argv-based; commit-ish arguments are validated object IDs.

use crate::engine::{self, GitError, Result};
use std::path::{Path, PathBuf};

/// Object IDs only: 40 (SHA-1) or 64 (SHA-256) hex characters. Every
/// commit-ish this module accepts goes through here, so a malformed
/// ref can never be parsed as an option.
fn validate_oid(oid: &str) -> Result<()> {
    if !matches!(oid.len(), 40 | 64) || !oid.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(GitError {
            message: format!("invalid commit id: {oid:?}"),
        });
    }
    Ok(())
}

/// Paths with unresolved merge conflicts.
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

/// Applies commit `oid` onto the current branch (git cherry-pick). On
/// conflict: aborts and returns the conflicted paths.
pub fn cherry_pick(worktree: &Path, oid: &str) -> Result<Vec<String>> {
    validate_oid(oid)?;
    let result = engine::run_trimmed(worktree, &["cherry-pick", oid]);
    finish_with_abort(worktree, result, "cherry-pick")
}

/// Reverts commit `oid` with an auto-generated revert commit. On
/// conflict: aborts and returns the conflicted paths.
pub fn revert(worktree: &Path, oid: &str) -> Result<Vec<String>> {
    validate_oid(oid)?;
    let result = engine::run_trimmed(worktree, &["revert", "--no-edit", oid]);
    finish_with_abort(worktree, result, "revert")
}

/// Shared conflict resolution for cherry-pick/revert: a conflicted run
/// is aborted (best effort) and reported as `Ok(paths)`; any other
/// failure propagates the original error.
fn finish_with_abort(
    worktree: &Path,
    result: Result<String>,
    abort_command: &str,
) -> Result<Vec<String>> {
    match result {
        Ok(_) => Ok(Vec::new()),
        Err(e) => match conflicted_files(worktree) {
            Ok(conflicts) if !conflicts.is_empty() => {
                let _ = engine::run_trimmed(worktree, &[abort_command, "--abort"]);
                Ok(conflicts)
            }
            // Not a conflict (or the query failed) — propagate the
            // original error.
            _ => Err(e),
        },
    }
}

/// One line of a rebase todo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TodoAction {
    Pick,
    Drop,
    Fixup,
}

impl TodoAction {
    fn keyword(self) -> &'static str {
        match self {
            TodoAction::Pick => "pick",
            TodoAction::Drop => "drop",
            TodoAction::Fixup => "fixup",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TodoStep {
    pub action: TodoAction,
    /// Full object id of the commit this line applies.
    pub oid: String,
    /// Subject text (todo comment only — git matches on the oid).
    pub subject: String,
}

/// Rebases the current branch onto `base` (an object id) applying
/// `steps` as the todo — the scripted equivalent of editing the rebase
/// todo list in an editor: the todo file is written to a temp file and
/// injected through `sequence.editor`, which git invokes with the todo
/// path as its argument (`cp <ours> <theirs>`). `fixup` never opens a
/// message editor; reword/squash would, and are not offered in v1.
///
/// On conflict: aborts and returns the conflicted paths.
pub fn run_rebase_todo(worktree: &Path, base: &str, steps: &[TodoStep]) -> Result<Vec<String>> {
    validate_oid(base)?;
    if steps.is_empty() {
        return Err(GitError {
            message: "empty rebase plan".into(),
        });
    }
    for step in steps {
        validate_oid(&step.oid)?;
        // The subject is a todo COMMENT, but control characters could
        // still confuse line-oriented parsing of the file we write.
        if step.subject.bytes().any(|b| b.is_ascii_control()) {
            return Err(GitError {
                message: "invalid commit subject".into(),
            });
        }
    }
    // Unique per CALL, not per process: two rebases (two worktrees, two
    // threads) can run concurrently and must never share a todo file.
    let todo_path: PathBuf = std::env::temp_dir().join(format!(
        "worktree-tool-rebase-todo-{}-{}.txt",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    ));
    let mut todo = String::new();
    for step in steps {
        todo.push_str(&format!(
            "{} {} {}\n",
            step.action.keyword(),
            step.oid,
            step.subject
        ));
    }
    std::fs::write(&todo_path, todo).map_err(|e| GitError {
        message: format!("cannot write rebase todo: {e}"),
    })?;
    let editor = format!("cp {}", todo_path.display());
    let result = engine::run_trimmed(
        worktree,
        &[
            "-c",
            &format!("sequence.editor={editor}"),
            "rebase",
            "-i",
            base,
        ],
    );
    let _ = std::fs::remove_file(&todo_path);
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
