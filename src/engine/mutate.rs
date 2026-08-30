//! Working-copy mutations. All path arguments are wrapped as `:(literal)`
//! pathspecs so they can never be glob-interpreted or option-parsed.

use crate::engine::{self, Result};
use std::path::Path;

fn literal(rel_path: &str) -> String {
    format!(":(literal){rel_path}")
}

fn literal_args(prefix: &[&str], rel_paths: &[String]) -> Vec<String> {
    let mut args: Vec<String> = prefix.iter().map(|s| s.to_string()).collect();
    args.push("--".to_string());
    args.extend(rel_paths.iter().map(|p| literal(p)));
    args
}

/// `git add -- <paths>`. Also how a conflict is marked resolved. An empty
/// slice is a no-op (callers use this for "stage all" with nothing left).
pub fn stage(worktree: &Path, rel_paths: &[String]) -> Result<()> {
    if rel_paths.is_empty() {
        return Ok(());
    }
    let args = literal_args(&["add"], rel_paths);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    engine::run_trimmed(worktree, &refs).map(|_| ())
}

/// `git reset -q HEAD -- <paths>`.
pub fn unstage(worktree: &Path, rel_paths: &[String]) -> Result<()> {
    if rel_paths.is_empty() {
        return Ok(());
    }
    let args = literal_args(&["reset", "-q", "HEAD"], rel_paths);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    engine::run_trimmed(worktree, &refs).map(|_| ())
}

/// `git checkout -q -- <path>`: restore the worktree file from the index,
/// so staged changes survive — only the unstaged delta is discarded.
/// (Plain `git checkout -- <path>` copies index → worktree and leaves the
/// index untouched; the `HEAD` form would overwrite the index too, wiping
/// the staged part.)
pub fn discard_unstaged(worktree: &Path, rel_path: &str) -> Result<()> {
    engine::run_trimmed(worktree, &["checkout", "-q", "--", &literal(rel_path)]).map(|_| ())
}

/// Deletes an untracked file. Directories are refused here and by the UI —
/// recursive deletion is not a Phase 1 operation.
pub fn discard_untracked(worktree: &Path, rel_path: &str) -> Result<()> {
    let full = worktree.join(rel_path);
    std::fs::remove_file(&full).map_err(|e| engine::GitError {
        message: format!("could not delete {}: {e}", full.display()),
    })
}
