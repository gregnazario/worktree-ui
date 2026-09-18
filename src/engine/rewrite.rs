//! History-rewrite operations on the checked-out branch: cherry-pick,
//! revert, and scripted interactive rebase. Single operations pause on
//! conflict (the conflicted paths come back as the `Ok` payload and the
//! sequence state stays — resolve + continue via `sequence::continue_op`
//! or abort); the multi-commit rewrite chain is the one atomic operation
//! (its steps live only in app memory, so a paused mid-chain could never
//! be continued). All commands are argv-based; commit-ish arguments are
//! validated object IDs.

use crate::engine::{self, GitError, Result};
use std::path::Path;

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
    finish_with_conflicts(worktree, result)
}

/// Reverts commit `oid` with an auto-generated revert commit. On
/// conflict: pauses and returns the conflicted paths.
pub fn revert(worktree: &Path, oid: &str) -> Result<Vec<String>> {
    validate_oid(oid)?;
    let result = engine::run_trimmed(worktree, &["revert", "--no-edit", oid]);
    finish_with_conflicts(worktree, result)
}

/// Shared conflict reporting for cherry-pick/revert: a conflicted run
/// PAUSES (the sequence state stays for resolve-and-continue) and the
/// conflicted paths are returned; any other failure propagates the
/// original error.
fn finish_with_conflicts(worktree: &Path, result: Result<String>) -> Result<Vec<String>> {
    match result {
        Ok(_) => Ok(Vec::new()),
        Err(e) => {
            let conflicts = conflicted_files(worktree).unwrap_or_default();
            if conflicts.is_empty() {
                Err(e)
            } else {
                Ok(conflicts)
            }
        }
    }
}

/// One line of a rebase todo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TodoAction {
    Pick,
    Drop,
    Fixup,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TodoStep {
    pub action: TodoAction,
    /// Full object id of the commit this line applies.
    pub oid: String,
    /// Subject (dialog display only — the replay matches on the oid).
    pub subject: String,
}

/// Rewrites the current branch's recent history per `steps` (oldest
/// first, covering `base`'s child through the current HEAD): drops are
/// skipped, picks are replayed, fixups are replayed into the previous
/// commit. Implemented as a cherry-pick CHAIN rather than `rebase -i`:
/// the branch is moved to `base`, then each step replays via
/// `cherry-pick` (`-n` + `commit --amend --no-edit` for fixups). Pure
/// argv — a sequence editor would have to be executable on every
/// platform (git cannot spawn POSIX `cp` as one on Windows).
///
/// Guards: the working copy must be clean (a rewrite would otherwise
/// destroy uncommitted changes in the `reset --hard` that starts the
/// chain), and HEAD must still be the newest step's commit (the log the
/// dialog was built from is stale otherwise).
///
/// On conflict: the conflicted paths are probed FIRST, then the chain
/// unwinds completely — `cherry-pick --abort` plus `reset --hard` back
/// to the pre-operation tip — and the paths are returned as the `Ok`
/// payload. Same contract as the other rewrite ops: never wedged.
pub fn run_rewrite_plan(worktree: &Path, base: &str, steps: &[TodoStep]) -> Result<Vec<String>> {
    validate_oid(base)?;
    if steps.is_empty() {
        return Err(GitError {
            message: "empty rewrite plan".into(),
        });
    }
    for step in steps {
        validate_oid(&step.oid)?;
    }
    let dirty = engine::run_trimmed(worktree, &["--no-optional-locks", "status", "--porcelain"])?;
    if !dirty.is_empty() {
        return Err(GitError {
            message:
                "working copy has uncommitted changes — commit or stash before rewriting history"
                    .into(),
        });
    }
    let tip_before = engine::run_trimmed(worktree, &["rev-parse", "HEAD"])?;
    validate_oid(&tip_before)?;
    // HEAD must be part of the plan (as pick, fixup, or drop): the plan
    // was built from a log snapshot ending at HEAD, so a moved HEAD
    // means the branch advanced and the plan would rewrite the wrong
    // commits. Reordering means the newest commit need not be the LAST
    // step — membership is the invariant, not order.
    if !steps.iter().any(|s| s.oid == tip_before) {
        return Err(GitError {
            message: "history changed since this plan was opened — reopen it".into(),
        });
    }

    // One instrumented failure path: probe conflicts BEFORE unwinding,
    // so the report survives the `reset --hard` that restores the branch.
    macro_rules! unwind_on_failure {
        ($result:expr) => {
            match $result {
                Ok(v) => v,
                Err(e) => {
                    let conflicts = conflicted_files(worktree).unwrap_or_default();
                    let _ = engine::run_trimmed(worktree, &["cherry-pick", "--abort"]);
                    let _ = engine::run_trimmed(worktree, &["reset", "--hard", &tip_before]);
                    if conflicts.is_empty() {
                        return Err(e);
                    }
                    return Ok(conflicts);
                }
            }
        };
    }

    engine::run_trimmed(worktree, &["reset", "--hard", base])?;
    // A fixup folds into the PREVIOUS replayed commit — as the first
    // replayed step it would amend `base` itself, rewriting a commit
    // this operation does not own.
    let mut replayed = 0usize;
    for step in steps {
        match step.action {
            TodoAction::Drop => {}
            TodoAction::Pick => {
                unwind_on_failure!(engine::run_trimmed(worktree, &["cherry-pick", &step.oid]));
                replayed += 1;
            }
            TodoAction::Fixup => {
                if replayed == 0 {
                    let _ = engine::run_trimmed(worktree, &["reset", "--hard", &tip_before]);
                    return Err(GitError {
                        message: "the first commit of a plan cannot be a fixup — pick it instead"
                            .into(),
                    });
                }
                unwind_on_failure!(engine::run_trimmed(
                    worktree,
                    &["cherry-pick", "-n", &step.oid]
                ));
                // Fold the staged changes into the previous commit;
                // nothing staged (a fixup of an empty commit) is a
                // no-op amend.
                unwind_on_failure!(engine::run_trimmed(
                    worktree,
                    &["commit", "--amend", "--no-edit"]
                ));
            }
        }
    }
    Ok(Vec::new())
}
