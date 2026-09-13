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
/// arguments are batched into chunks bounded by BOTH count and accumulated
/// length (deep monorepo paths run 150+ chars each — 200 of those is
/// already ~32 KB). 16 KiB of arguments leaves generous headroom.
const MAX_PATHS_PER_INVOCATION: usize = 200;
const MAX_CHUNK_BYTES: usize = 16 * 1024;

fn for_each_chunk(worktree: &Path, prefix: &[&str], rel_paths: &[String]) -> Result<()> {
    // Chunks hold RAW paths; `run_chunk` applies the `:(literal)` wrapper
    // (via literal_args) exactly once. The byte budget counts the wrapper
    // (~11 chars) so the bound reflects what git actually receives.
    let mut chunk: Vec<String> = Vec::new();
    let mut chunk_bytes = 0usize;
    for path in rel_paths {
        let cost = path.len() + ":()".len() + "literal".len() + 1;
        if !chunk.is_empty()
            && (chunk.len() >= MAX_PATHS_PER_INVOCATION || chunk_bytes + cost > MAX_CHUNK_BYTES)
        {
            run_chunk(worktree, prefix, &chunk)?;
            chunk.clear();
            chunk_bytes = 0;
        }
        chunk_bytes += cost;
        chunk.push(path.clone());
    }
    if !chunk.is_empty() {
        run_chunk(worktree, prefix, &chunk)?;
    }
    Ok(())
}

fn run_chunk(worktree: &Path, prefix: &[&str], chunk: &[String]) -> Result<()> {
    let args = literal_args(prefix, chunk);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    engine::run_trimmed(worktree, &refs).map(|_| ())
}

/// Applies a reconstructed patch — the diff's byte-exact header plus the
/// selected hunks' `raw` — to the index only. The worktree is never
/// touched. Before applying, the index's staged blob is compared against
/// the diff header's abbreviated PRE-image (`expects_index_blob`): git's
/// preimage check catches most stale patches, but a PURE-INSERTION hunk
/// has no removal preimage — if the index moved and already contains the
/// inserted lines, apply would silently duplicate them — so the explicit
/// check refuses instead. Takes index.lock like every mutation, so no
/// `--no-optional-locks` here.
/// Why an apply refused. The pre-image refusal is distinct: its remedy is
/// a refresh (`r`), not the blanket "stage the whole file" that fits a
/// rejected patch.
#[derive(Debug)]
pub enum ApplyError {
    /// The index's staged blob no longer matches the diff's pre-image —
    /// the file changed since the diff was generated.
    StaleIndex,
    /// `git apply` itself failed (rejected hunk, lock contention, …).
    Git(crate::engine::GitError),
}

impl std::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApplyError::StaleIndex => f.write_str(
                "the file's staged state changed since this diff was loaded — press r and try again",
            ),
            ApplyError::Git(e) => f.write_str(&e.message),
        }
    }
}

/// Builds the patch for staging ONE content hunk: the diff header minus
/// its `old mode`/`new mode` lines, plus the hunk's byte-exact `raw`.
/// Stripping the mode lines keeps `git apply --cached` from flipping the
/// index entry's mode as a side effect of staging a content hunk — a
/// mode change is its own decision (`git add -p` asks separately).
pub fn content_patch(header_raw: &[u8], hunk_raw: &[u8]) -> Vec<u8> {
    let mut patch = Vec::with_capacity(header_raw.len() + hunk_raw.len());
    for line in header_raw.split_inclusive(|b| *b == b'\n') {
        if line.starts_with(b"old mode ") || line.starts_with(b"new mode ") {
            continue;
        }
        patch.extend_from_slice(line);
    }
    patch.extend_from_slice(hunk_raw);
    patch
}

pub fn apply_cached(
    worktree: &Path,
    rel_path: &str,
    patch: Vec<u8>,
    expects_index_blob: Option<&str>,
) -> std::result::Result<(), ApplyError> {
    if let Some(expected) = expects_index_blob {
        let staged = engine::run_trimmed(
            worktree,
            &["ls-files", "-s", "--", &format!(":(literal){rel_path}")],
        )
        .map_err(ApplyError::Git)?;
        // "100644 <full-sha> 0\t<path>" — the header's pre-image hash is
        // an abbreviation of this one.
        let index_blob = staged.split_whitespace().nth(1).unwrap_or_default();
        if !index_blob.starts_with(expected) {
            return Err(ApplyError::StaleIndex);
        }
    }
    engine::run_bytes_stdin(
        worktree,
        &["apply", "--cached", "--whitespace=nowarn"],
        patch,
    )
    .map_err(ApplyError::Git)
    .map(|_| ())
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
