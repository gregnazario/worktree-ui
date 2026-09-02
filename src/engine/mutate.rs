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

/// Windows caps a single CreateProcess command line at ~32,767 chars and
/// even Linux can hit ARG_MAX on monorepo-scale path lists, so path
/// arguments are batched into bounded chunks per git invocation.
const MAX_PATHS_PER_INVOCATION: usize = 200;

fn for_each_chunk(worktree: &Path, prefix: &[&str], rel_paths: &[String]) -> Result<()> {
    for chunk in rel_paths.chunks(MAX_PATHS_PER_INVOCATION) {
        let args = literal_args(prefix, chunk);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        engine::run_trimmed(worktree, &refs)?;
    }
    Ok(())
}

/// `git add -- <paths>`, batched. Also how a conflict is marked resolved.
/// An empty slice is a no-op (callers use this for "stage all" with nothing
/// left).
pub fn stage(worktree: &Path, rel_paths: &[String]) -> Result<()> {
    if rel_paths.is_empty() {
        return Ok(());
    }
    for_each_chunk(worktree, &["add"], rel_paths)
}

/// `git reset -q HEAD -- <paths>`, batched. On an unborn HEAD (fresh repo,
/// no commits) there is nothing for `reset HEAD` to point at, so the
/// equivalent unstage is `git rm --cached`: the paths drop back to
/// untracked.
pub fn unstage(worktree: &Path, rel_paths: &[String]) -> Result<()> {
    if rel_paths.is_empty() {
        return Ok(());
    }
    let head_exists =
        engine::run_trimmed(worktree, &["rev-parse", "--verify", "-q", "HEAD"]).is_ok();
    if head_exists {
        for_each_chunk(worktree, &["reset", "-q", "HEAD"], rel_paths)
    } else {
        // `--force` only overrides git's staged-changes refusal — with
        // `--cached` the worktree file is never touched.
        for_each_chunk(worktree, &["rm", "--cached", "-q", "--force"], rel_paths)
    }
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
