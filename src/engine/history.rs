//! The History surface: `git log` parsing, in-app graph lane computation,
//! per-commit file lists and diffs, and the checkout / new-worktree
//! actions. Read-heavy, write-light — like the rest of the engine, all
//! commands are blocking `std::process` calls for the background executor.

use crate::engine::{self, diff::UnifiedDiff, GitError, Result};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogCommit {
    /// Full 40-hex sha (the identity every action uses).
    pub hash: String,
    /// Abbreviated sha (display + worktree directory naming).
    pub short: String,
    /// Parent hashes, first parent first.
    pub parents: Vec<String>,
    pub author: String,
    /// Author date, unix seconds.
    pub timestamp: i64,
    /// Ref decorations at this commit (e.g. "HEAD -> main"), "" when none.
    pub refs: String,
    pub subject: String,
    /// Graph lane, assigned by [`assign_lanes`].
    pub lane: usize,
}

/// One row of the text graph: the commit's lane plus a cell per lane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphCell {
    Commit,
    Wire,
    Empty,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphRow {
    pub lane: usize,
    pub cells: Vec<GraphCell>,
}

/// `git log --topo-order`, newest first, batched. Decorations are pinned
/// to `--decorate=short`: piped output otherwise depends on git's
/// `--decorate=auto` default and the user's `log.decorate` config, so the
/// refs field would silently vary across environments.
pub fn log(worktree: &Path, skip: usize, max_count: usize) -> Result<Vec<LogCommit>> {
    // An unborn branch (no commits yet) is an empty history, not an
    // error — detected structurally: first the directory must BE a
    // repository (errors propagate), then HEAD may simply not exist.
    engine::run_trimmed(worktree, &["rev-parse", "--is-inside-work-tree"])?;
    if engine::run_trimmed(worktree, &["rev-parse", "--verify", "-q", "HEAD"]).is_err() {
        // Unborn branch: symbolic-ref still resolves to the branch name.
        // A broken/corrupt HEAD fails HERE too and propagates as an error
        // instead of masquerading as "No commits yet".
        engine::run_trimmed(worktree, &["symbolic-ref", "--short", "HEAD"])?;
        return Ok(Vec::new());
    }
    let out = engine::run_bytes(
        worktree,
        &[
            "--no-optional-locks",
            "log",
            "--topo-order",
            "--decorate=short",
            // Machine-parsed output must be immune to the user's
            // log.showSignature config, which interleaves gpg lines
            // between records.
            "--no-show-signature",
            &format!("--skip={skip}"),
            &format!("--max-count={max_count}"),
            "--format=%x00%H%x01%h%x01%P%x01%an%x01%at%x01%D%x01%s",
        ],
    )?;
    Ok(parse_log(&out))
}

/// Parses `%x00`-separated records with `%x01`-separated fields. Display
/// strings (author, refs, subject) are lossy-decoded; hashes are ASCII.
fn parse_log(bytes: &[u8]) -> Vec<LogCommit> {
    let mut commits = Vec::new();
    for record in bytes.split(|b| *b == 0u8) {
        if record.is_empty() {
            continue; // the format's leading NUL leaves an empty first record
        }
        // splitn(7): the subject is the LAST field, so SOH bytes inside
        // author/refs metadata shift nothing — the tail stays whole.
        let fields: Vec<&[u8]> = record.splitn(7, |b| *b == 1u8).collect();
        let field = |i: usize| -> String {
            fields
                .get(i)
                .map(|f| String::from_utf8_lossy(f).into_owned())
                .unwrap_or_default()
        };
        let hash = field(0);
        if hash.is_empty() {
            continue;
        }
        let timestamp = field(4).parse::<i64>().unwrap_or(0);
        commits.push(LogCommit {
            short: field(1),
            parents: field(2).split_whitespace().map(str::to_string).collect(),
            author: field(3),
            timestamp,
            refs: field(5),
            // The subject is the record's last field: git terminates it
            // with a newline before the next record's NUL.
            subject: field(6).trim_end().to_string(),
            hash,
            lane: 0,
        });
    }
    commits
}

/// Assigns graph lanes over the topo-ordered commits (in place: sets each
/// commit's `lane`) and returns one row per commit for rendering. A lane
/// keeps its column for as long as its wire lives; consumed or closed
/// wires leave a slot that the next new wire reuses, so lanes stay
/// compact without shifting existing columns.
pub fn assign_lanes(commits: &mut [LogCommit]) -> Vec<GraphRow> {
    // lane → the sha whose wire occupies it (None = free slot).
    let mut wires: Vec<Option<String>> = Vec::new();
    let mut rows = Vec::with_capacity(commits.len());
    for commit in commits.iter_mut() {
        let lane = wires
            .iter()
            .position(|w| w.as_deref() == Some(commit.hash.as_str()))
            .unwrap_or_else(|| {
                wires.iter().position(|w| w.is_none()).unwrap_or_else(|| {
                    wires.push(None);
                    wires.len() - 1
                })
            });
        wires[lane] = None; // the commit consumed its wire
                            // The FIRST parent continues the commit's column (index-based,
                            // not "first parent that happens to lack a wire": in criss-cross
                            // topologies a pre-wired first parent would hand the column to a
                            // later parent). Already-wired parents keep their column.
        for (i, parent) in commit.parents.iter().enumerate() {
            let slot = if i == 0 {
                lane
            } else if wires.iter().any(|w| w.as_deref() == Some(parent.as_str())) {
                continue;
            } else {
                wires.iter().position(|w| w.is_none()).unwrap_or_else(|| {
                    wires.push(None);
                    wires.len() - 1
                })
            };
            wires[slot] = Some(parent.clone());
        }
        commit.lane = lane;
        let mut cells: Vec<GraphCell> = (0..wires.len())
            .map(|i| {
                if i == lane {
                    GraphCell::Commit
                } else if wires[i].is_some() {
                    GraphCell::Wire
                } else {
                    GraphCell::Empty
                }
            })
            .collect();
        // Trim freed trailing lanes: a wire closing at the last lane must
        // not paint a phantom blank column on every subsequent row (the
        // commit's own cell is never Empty, so its lane always survives).
        while matches!(cells.last(), Some(GraphCell::Empty)) {
            cells.pop();
        }
        rows.push(GraphRow { lane, cells });
    }
    rows
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitFile {
    /// `diff-tree` status letter: A/M/D/R/C/T/U…
    pub letter: char,
    pub path: String,
    /// Rename/copy source (the `-z` record's second path).
    pub orig_path: Option<String>,
}

/// Files changed by a commit with one-letter statuses. `--root` makes the
/// root commit list its initial files as additions; pass `first_parent`
/// for MERGE commits — plain `diff-tree` emits nothing for them (no
/// changes against all parents at once), while first-parent mode diffs
/// against parent 1 like `commit_diff`.
pub fn commit_files(worktree: &Path, sha: &str, first_parent: bool) -> Result<Vec<CommitFile>> {
    // For merges, diff explicitly against parent 1: plain single-arg
    // diff-tree emits NOTHING for merges (--first-parent alone doesn't
    // change that). The two-argument form is a plain A..B diff.
    let parent1 = if first_parent {
        engine::run_trimmed(
            worktree,
            &["rev-parse", "-q", "--verify", &format!("{sha}^1")],
        )
        .ok()
        .filter(|p| !p.is_empty())
    } else {
        None
    };
    let mut args = vec![
        "--no-optional-locks",
        "diff-tree",
        // Rename pairing runs in diffcore independent of output format —
        // do NOT add `-p` here: with both --name-status and patch output,
        // git emits the patch text after the NUL records, which would be
        // consumed as path data by the parser below.
        "-M",
        "--no-commit-id",
        "--name-status",
        "-r",
        "-z",
        "--root",
    ];
    match &parent1 {
        Some(parent) => {
            args.push(parent);
            args.push(sha);
        }
        None => args.push(sha),
    }
    let out = engine::run_bytes(worktree, &args)?;
    let mut files = Vec::new();
    let mut records = out.split(|b| *b == 0u8).filter(|r| !r.is_empty());
    while let Some(status) = records.next() {
        let letter = status.first().copied().unwrap_or(b'?') as char;
        // R/C records carry TWO paths: git writes the PRE-image (old)
        // first, then the post-image (new) — the display order is
        // "old → new" and the diff must be queried with the NEW path.
        let first = String::from_utf8_lossy(records.next().unwrap_or_default()).into_owned();
        let (path, orig_path) = if letter == 'R' || letter == 'C' {
            let new_path = String::from_utf8_lossy(records.next().unwrap_or_default()).into_owned();
            (new_path, Some(first))
        } else {
            (first, None)
        };
        files.push(CommitFile {
            letter,
            path,
            orig_path,
        });
    }
    Ok(files)
}

/// The unified diff of one file in one commit, against its first parent
/// (`--first-parent` makes merge commits diff against parent 1 instead of
/// emitting combined `@@@` diffs this parser cannot read; root commits
/// are unaffected — `git show` diffs them against nothing).
pub fn commit_diff(worktree: &Path, sha: &str, rel_path: &str) -> Result<UnifiedDiff> {
    let out = engine::run_bytes(
        worktree,
        &[
            "--no-optional-locks",
            "show",
            "--format=",
            "--first-parent",
            // Same show.showSignature immunity as the log command.
            "--no-show-signature",
            // Rename headers under a pathspec need explicit detection:
            // without -M a pure rename renders as a full deletion.
            "-M",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "-U3",
            sha,
            "--",
            &format!(":(literal){rel_path}"),
        ],
    )?;
    Ok(engine::diff::parse_unified_diff(&out))
}

/// Checks out a commit in this worktree (detached HEAD). Refuses while the
/// working copy has changes — the user's uncommitted work outranks the
/// jump, and git's own carry-over behavior would be surprising here.
pub fn checkout(worktree: &Path, sha: &str) -> Result<()> {
    let status = crate::engine::working_copy::status(worktree)?;
    // Only TRACKED changes endanger a checkout: untracked files are
    // carried across untouched (and git itself refuses if one would be
    // clobbered, with that stderr surfaced here).
    let dirty = status.entries.iter().any(|entry| !entry.untracked);
    if dirty {
        return Err(GitError {
            message:
                "the working copy has tracked changes — commit or discard them before checking out a commit"
                    .into(),
        });
    }
    engine::run_trimmed(worktree, &["checkout", "-q", sha]).map(|_| ())
}

/// Creates a detached worktree at the commit, named `<repo>-<short>` in
/// the repo's parent directory (suffix `-2`, `-3`, … on collisions).
/// Returns the created path. The caller flags a home-list mutation so the
/// new worktree appears without a manual refresh.
pub fn open_worktree_at(worktree: &Path, sha: &str, short: &str) -> Result<PathBuf> {
    let name = worktree
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "worktree".into());
    let parent = worktree.parent().ok_or_else(|| GitError {
        message: "cannot place a worktree next to the repository root".into(),
    })?;
    // Collisions hide in two places: an existing DIRECTORY, and a
    // worktree still REGISTERED in .git/worktrees whose directory was
    // deleted by hand (git refuses to reuse the name). Both advance the
    // suffix.
    let registered: Vec<String> =
        engine::run_trimmed(worktree, &["worktree", "list", "--porcelain"])
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.strip_prefix("worktree "))
            .map(|p| p.replace('\\', "/"))
            .collect();
    let mut path = parent.join(format!("{name}-{short}"));
    let mut n = 2u32;
    loop {
        let candidate = path.display().to_string().replace('\\', "/");
        // Exact match: substring probing false-positives on
        // prefix-colliding names (repo-sha vs repo-sha-2).
        let taken = path.exists() || registered.contains(&candidate);
        if !taken {
            break;
        }
        path = parent.join(format!("{name}-{short}-{n}"));
        n += 1;
    }
    let path_str = path.to_string_lossy().into_owned();
    engine::run_trimmed(
        worktree,
        &["worktree", "add", "-q", "--detach", &path_str, sha],
    )?;
    Ok(path)
}
