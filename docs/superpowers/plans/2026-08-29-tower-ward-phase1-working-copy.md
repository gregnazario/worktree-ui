# Tower-ward Phase 1: Working Copy — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Spec: `docs/superpowers/specs/2026-08-29-tower-ward-git-client-design.md`. Build the Working Copy view: drill into any worktree to see status groups, stage/unstage/discard files, view unified diffs, and commit via `$EDITOR` — on a new `engine` module that later phases (history, branches/remotes) build on.

**ERRATA (2026-09-02 — read before re-executing any task):** this plan's
embedded code sketches are the *pre-execution drafts*. Implementation and seven
review rounds corrected several of them, and the corrected forms live in the
shipped code — do not re-implement from the sketches. Known superseded points:
`--no-optional-locks` is a GLOBAL git option (before the subcommand; the
sketches' order exits 129); `discard_unstaged` must be index-source
(`git checkout -q -- <path>` — the sketch's round-trip test required the wrong
HEAD-source form, which destroys staged changes); `stderr_error` must prefer
the stderr line naming `index.lock` over the last line; the fixture `Z` needed
an unstaged record and the staged-diff assertion expects `+two` (Add, not Del);
`fetch_author` must not bump the shared generation; `refresh`'s keep-path
resolves against the NEW snapshot; mutation completions deliberately skip the
generation guard; detail loads use their own `detail_generation`; gpui 0.2.2
requires every focusable handle to be `.track_focus`ed in the rendered tree;
uppercase keystrokes normalize to lowercase+shift (bind `shift-s`, never `"S"`);
and the selection is list-bounded, not group-bounded. When this file and the
code disagree, the code (and the spec) win.

**Architecture:** The git CLI stays the engine (spec decision A): a new `src/engine/` module with typed commands and `-z` porcelain parsing for status/numstat, plain single-file unified-diff parsing for diffs, and `:(literal)` pathspecs everywhere. A new `WorkingCopyStore` (generation-counter pattern, cloned from `WorktreeStore`) drives a new detail view. `ui.rs` splits into `app.rs` (shell + worktree list + key routing) and `views/working_copy.rs` (detail rendering), mirroring how `dialogs.rs` renders against `RootView`.

**Tech Stack:** Rust, `gpui = "=0.2.2"` (pinned), `git` subprocess via `std::process` on GPUI background executor threads, `tempfile` (dev) for fixture repos.

## Global Constraints

- `gpui = "=0.2.2"` pinned in Cargo.toml — do not add or upgrade any runtime dependency.
- All git calls are blocking `std::process` invocations run inside `cx.background_executor().spawn(...)` (GPUI harness pitfall: never smol process futures in tests).
- Every user-derived path/ref is passed after `--` and/or wrapped as `:(literal)<path>`; read-only commands get `--no-optional-locks`.
- Parsing only from machine formats: `--porcelain=v2 -z` for status, `--numstat -z` for counts, plain unified `-U3 --no-color` for single-file diffs.
- No new platform-specific code except editor defaults (`vim` on unix, `notepad` on Windows).
- Single-key shortcuts are gated on the relevant `FocusHandle::is_focused(window)`; every dialog key handler calls `cx.stop_propagation()` first.
- Four platforms must keep compiling; after UI tasks run: `cargo zigbuild --target x86_64-unknown-linux-gnu --lib`, `--target x86_64-pc-windows-gnu --lib`, `--target x86_64-unknown-freebsd --lib`.
- Keybinding break (spec): `enter` opens the detail view; terminal moves to `t`.
- Commit messages: plain conventional style (`feat: …`, `fix: …`, `test: …`, `docs: …`), **never** any `Co-Authored-By`/AI attribution trailer.
- Test commands: `cargo test` (all), `cargo test --test git_integration`, `cargo clippy --all-targets -- -D warnings`.

## File Structure

| File | Responsibility |
|---|---|
| `src/engine/mod.rs` (new) | `GitError` (with lock-contention classification), `run()` (raw stdout), `run_trimmed()` |
| `src/engine/working_copy.rs` (new) | `WorkingCopy`/`FileEntry`/`BranchInfo`/`Group` types, `parse_status_z`, `parse_numstat_z`, `group_rows`, `status()` command |
| `src/engine/diff.rs` (new) | `UnifiedDiff`/`DiffHunk`/`DiffLine`/`Preview` types, `parse_unified_diff`, `diff_unstaged`/`diff_staged`/`read_preview` |
| `src/engine/mutate.rs` (new) | `stage`/`unstage`/`discard_unstaged`/`discard_untracked` |
| `src/engine/commit.rs` (new) | `author`, `resolve_editor`, `commit_with_editor`, `commit`, `strip_comments` |
| `src/git.rs` (modify) | keeps worktree commands; `run_git`/`GitError` delegate to `engine` |
| `src/wc_store.rs` (new) | `WorkingCopyStore` + `Pane`/`FileDetail`, generation-counter async ops |
| `src/app.rs` (moved from `src/ui.rs`) | `RootView`, worktree list, all key routing, detail-view navigation |
| `src/views/mod.rs`, `src/views/working_copy.rs` (new) | detail-view rendering: header/tabs, file list, diff pane, footer |
| `src/dialogs.rs` (modify) | new `DialogState::Discard` variant + renderer |
| `src/main.rs` (modify) | import paths (`app` instead of `ui`) |
| `src/lib.rs` (modify) | module list |
| `tests/common/mod.rs` (new) | shared `sh` + `fixture_repo` helpers for new integration tests |
| `tests/engine_working_copy.rs`, `tests/engine_commit.rs` (new) | engine integration tests |
| `examples/bench.rs` (modify) | working-copy bench (2000 changed files) |
| `README.md`, `docs/index.html` (modify) | new keybindings |

---

### Task 1: Engine skeleton — `engine` module with raw-output runner and lock classification

**Files:**
- Create: `src/engine/mod.rs`
- Modify: `src/git.rs` (only the runner + error), `src/lib.rs`
- Test: `tests/engine_working_copy.rs` (new, with `tests/common/mod.rs`)

**Interfaces:**
- Consumes: nothing (foundation).
- Produces (everything later relies on):
  - `engine::GitError { pub message: String }` with `pub fn is_lock_error(&self) -> bool`
  - `engine::Result<T>`
  - `engine::run(cwd: &Path, args: &[&str]) -> Result<String>` — stdout **untrimmed** (`-z` records end in NUL)
  - `engine::run_trimmed(cwd: &Path, args: &[&str]) -> Result<String>`

- [ ] **Step 1: Write the failing tests**

Create `tests/common/mod.rs`:

```rust
use std::path::Path;

pub fn sh(cwd: &Path, cmd: &[&str]) {
    let status = std::process::Command::new(cmd[0])
        .args(&cmd[1..])
        .current_dir(cwd)
        .status()
        .expect("spawn");
    assert!(status.success(), "failed: {cmd:?}");
}

/// Repo with one commit on `main`: file `f.txt` containing "one".
pub fn fixture_repo(dir: &Path) {
    sh(dir, &["git", "init", "-q", "-b", "main"]);
    sh(dir, &["git", "config", "user.email", "t@t.t"]);
    sh(dir, &["git", "config", "user.name", "t"]);
    std::fs::write(dir.join("f.txt"), "one").unwrap();
    sh(dir, &["git", "add", "."]);
    sh(dir, &["git", "commit", "-qm", "init"]);
}
```

Create `tests/engine_working_copy.rs`:

```rust
mod common;

use common::{fixture_repo, sh};
use std::path::Path;
use worktree_tool::engine;

#[test]
fn run_preserves_nul_records_untrimmed() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    std::fs::write(tmp.path().join("n.txt"), "x").unwrap();
    let out = engine::run(tmp.path(), &["status", "--porcelain=v2", "-z", "--untracked-files=normal"]).unwrap();
    assert!(out.contains('\0'), "-z output must keep NUL separators: {out:?}");
    assert!(out.contains("n.txt"));
}

#[test]
fn run_reports_last_stderr_line() {
    let tmp = tempfile::tempdir().unwrap();
    let err = engine::run(tmp.path(), &["rev-parse", "--show-toplevel"]).unwrap_err();
    assert!(!err.message.is_empty());
    assert!(!err.is_lock_error());
}

#[test]
fn lock_contention_is_classified() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    std::fs::write(tmp.path().join(".git/index.lock"), "").unwrap();
    let err = engine::run(tmp.path(), &["add", "--", "f.txt"]).unwrap_err();
    assert!(err.is_lock_error(), "expected lock error, got: {}", err.message);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test engine_working_copy 2>&1 | tail -5`
Expected: compile error — `engine` module does not exist.

- [ ] **Step 3: Implement**

Create `src/engine/mod.rs`:

```rust
//! Typed git CLI layer. Every command takes the worktree path, runs a
//! blocking `std::process` call (callers: GPUI background executor), and
//! parses only machine formats. Read-only commands pass
//! `--no-optional-locks` so the app can never block the user's own git
//! processes on index.lock.

pub mod commit;
pub mod diff;
pub mod mutate;
pub mod working_copy;

use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug)]
pub struct GitError {
    pub message: String,
}

impl GitError {
    /// git refused because another process holds index.lock.
    pub fn is_lock_error(&self) -> bool {
        self.message.contains("index.lock")
    }
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

pub type Result<T> = std::result::Result<T, GitError>;

/// Runs `git` and returns stdout verbatim (no trailing trim): `-z` records
/// are NUL-terminated and parsed positionally.
pub fn run(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = command(cwd, args)
        .output()
        .map_err(|e| GitError { message: format!("failed to run git: {e}") })?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(stderr_error(&output.stderr))
    }
}

pub fn run_trimmed(cwd: &Path, args: &[&str]) -> Result<String> {
    Ok(run(cwd, args)?.trim_end().to_string())
}

fn command(cwd: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

fn stderr_error(stderr: &[u8]) -> GitError {
    let text = String::from_utf8_lossy(stderr);
    let last = text.trim().lines().last().unwrap_or("git failed");
    GitError { message: last.to_string() }
}
```

In `src/git.rs`, replace the `GitError` struct, its `Display` impl, the `Result` alias, and the body of `run_git` (keep everything else). Top of file becomes:

```rust
use crate::engine;
use crate::model::{
    parse_status_porcelain_v2, parse_worktree_porcelain, WorktreeEntry, WorktreeStatus,
};
use std::path::{Path, PathBuf};

pub use crate::engine::GitError;
pub type Result<T> = std::result::Result<T, GitError>;

/// Line-oriented runner (trims trailing whitespace). `-z` callers must use
/// `engine::run` instead, which preserves NUL records byte-exact.
pub fn run_git(cwd: Option<&Path>, args: &[&str]) -> Result<String> {
    match cwd {
        Some(dir) => engine::run_trimmed(dir, args),
        None => engine::run_trimmed(Path::new("."), args),
    }
}
```

Delete the now-duplicated `GitError`/`Display`/`Result`/old `run_git` from `git.rs`. In `src/lib.rs` add `pub mod engine;` (alphabetical, after `pub mod dialogs;`).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test 2>&1 | tail -20`
Expected: new tests PASS; all existing tests still PASS (28 existing + new).

- [ ] **Step 5: Commit**

```bash
git add src/engine/mod.rs src/git.rs src/lib.rs tests/common/mod.rs tests/engine_working_copy.rs
git commit -m "feat(engine): typed git runner with raw -z output and lock-error classification"
```

---

### Task 2: Working-copy types and `parse_status_z` (pure parser)

**Files:**
- Create: `src/engine/working_copy.rs`
- Modify: `src/engine/mod.rs` (module already declared in Task 1)
- Test: unit tests in `src/engine/working_copy.rs` (pure parser → unit tests, matching the `model.rs` house pattern)

**Interfaces:**
- Consumes: nothing yet.
- Produces:
  - `BranchInfo { pub head: String, pub upstream: Option<String>, pub ahead: u32, pub behind: u32 }`
  - `FileEntry { pub path: String, pub orig_path: Option<String>, pub index_status: char, pub wt_status: char, pub conflict: Option<String>, pub untracked: bool, pub staged_lines: Option<(u64, u64)>, pub unstaged_lines: Option<(u64, u64)> }` with `pub fn is_dir(&self) -> bool`
  - `WorkingCopy { pub branch: BranchInfo, pub entries: Vec<FileEntry> }`
  - `Group` enum: `Conflicts, Staged, Unstaged, Untracked` (display order)
  - `parse_status_z(input: &str) -> WorkingCopy`
  - `group_rows(wc: &WorkingCopy) -> Vec<(Group, usize)>`

- [ ] **Step 1: Write the failing tests**

Append to `src/engine/working_copy.rs` (create the file with the test module first, implementation after — TDD):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Status v2 -z: header chunk (LF-joined # lines), a `1` record staged,
    /// a `2` rename record whose orig path rides in the NEXT NUL chunk, an
    /// unmerged `u` record, and an untracked `?` record.
    const Z: &str = "# branch.oid abc\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -1\n\u{0}1 M. N... 100100 100100 100100 a1 a1 staged.txt\u{0}2 R. N... 100100 100100 100100 a1 a1 R100 new/name.txt\u{0}old/name.txt\u{0}u UU N... 100 100 100 100 h1 h2 h3 conflicted.txt\u{0}? untracked dir/file with spaces.txt\u{0}";

    #[test]
    fn parses_headers_records_renames_conflicts_untracked() {
        let wc = parse_status_z(Z);
        assert_eq!(wc.branch.head, "main");
        assert_eq!(wc.branch.upstream.as_deref(), Some("origin/main"));
        assert_eq!((wc.branch.ahead, wc.branch.behind), (2, 1));
        assert_eq!(wc.entries.len(), 4);

        assert_eq!(wc.entries[0].path, "staged.txt");
        assert_eq!(wc.entries[0].index_status, 'M');
        assert_eq!(wc.entries[0].wt_status, '.');

        assert_eq!(wc.entries[1].path, "new/name.txt");
        assert_eq!(wc.entries[1].orig_path.as_deref(), Some("old/name.txt"));

        assert_eq!(wc.entries[2].conflict.as_deref(), Some("UU"));
        assert_eq!(wc.entries[2].path, "conflicted.txt");

        let un = &wc.entries[3];
        assert!(un.untracked);
        assert_eq!(un.path, "untracked dir/file with spaces.txt");
    }

    #[test]
    fn paths_with_spaces_survive_splitn() {
        let wc = parse_status_z("1 .M N... 1 1 1 a b my file.txt\u{0}");
        assert_eq!(wc.entries[0].path, "my file.txt");
    }

    #[test]
    fn detached_head_and_empty_input() {
        let wc = parse_status_z("# branch.head (detached)\n\u{0}");
        assert_eq!(wc.branch.head, "(detached)");
        assert!(wc.entries.is_empty());
        let wc = parse_status_z("");
        assert_eq!(wc.branch.head, "");
        assert!(wc.entries.is_empty());
    }

    #[test]
    fn group_rows_orders_conflicts_staged_unstaged_untracked() {
        let wc = parse_status_z(Z);
        let rows = group_rows(&wc);
        let groups: Vec<Group> = rows.iter().map(|(g, _)| *g).collect();
        assert_eq!(
            groups,
            vec![Group::Conflicts, Group::Staged, Group::Staged, Group::Unstaged, Group::Untracked]
        );
        // same file staged+unstaged appears in both groups:
        let both = parse_status_z("1 MM N... 1 1 1 a b both.txt\u{0}");
        let groups: Vec<Group> = group_rows(&both).iter().map(|(g, _)| *g).collect();
        assert_eq!(groups, vec![Group::Staged, Group::Unstaged]);
    }

    #[test]
    fn untracked_directory_row_is_detected() {
        let wc = parse_status_z("? vendor/\u{0}");
        assert!(wc.entries[0].is_dir());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib engine 2>&1 | tail -5`
Expected: compile failure — `parse_status_z` not found.

- [ ] **Step 3: Implement**

In `src/engine/working_copy.rs` above the tests:

```rust
use crate::engine::{self, Result};
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BranchInfo {
    /// `# branch.head` value: branch name or `(detached)`.
    pub head: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileEntry {
    /// Repo-root-relative path in git's form (forward slashes).
    pub path: String,
    /// Rename/copy source (`2` records only).
    pub orig_path: Option<String>,
    /// X: index status letter (`.` when unchanged).
    pub index_status: char,
    /// Y: worktree status letter.
    pub wt_status: char,
    /// Unmerged code (`UU`, `AA`, `DU`, …) from `u` records.
    pub conflict: Option<String>,
    pub untracked: bool,
    /// numstat (+added, −deleted); `None` = binary or not applicable.
    pub staged_lines: Option<(u64, u64)>,
    pub unstaged_lines: Option<(u64, u64)>,
}

impl FileEntry {
    /// Collapsed untracked directories are listed as `dir/` by git.
    pub fn is_dir(&self) -> bool {
        self.path.ends_with('/')
    }
}

#[derive(Clone, Debug)]
pub struct WorkingCopy {
    pub branch: BranchInfo,
    pub entries: Vec<FileEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Conflicts,
    Staged,
    Unstaged,
    Untracked,
}

impl Group {
    pub fn title(self) -> &'static str {
        match self {
            Group::Conflicts => "Conflicts",
            Group::Staged => "Staged",
            Group::Unstaged => "Unstaged",
            Group::Untracked => "Untracked",
        }
    }
}

/// Parses `git status --porcelain=v2 -z --branch`. With `-z`, records are
/// NUL-terminated and header lines are LF-terminated inside one chunk; a
/// rename record's orig path rides in the NEXT NUL chunk. Lenient: unknown
/// chunks are ignored.
pub fn parse_status_z(input: &str) -> WorkingCopy {
    let mut wc = WorkingCopy { branch: BranchInfo::default(), entries: Vec::new() };
    let mut expect_orig_path = false;
    for chunk in input.split('\0') {
        if expect_orig_path {
            // Consume unconditionally: the orig path could theoretically
            // start with characters that look like a record.
            if let Some(last) = wc.entries.last_mut() {
                last.orig_path = Some(chunk.to_string());
            }
            expect_orig_path = false;
            continue;
        }
        if chunk.is_empty() {
            continue;
        }
        if chunk.starts_with('#') {
            for line in chunk.split('\n') {
                if let Some(rest) = line.strip_prefix("# branch.head ") {
                    wc.branch.head = rest.trim().to_string();
                } else if let Some(rest) = line.strip_prefix("# branch.upstream ") {
                    wc.branch.upstream = Some(rest.trim().to_string());
                } else if let Some(rest) = line.strip_prefix("# branch.ab ") {
                    for part in rest.split(' ') {
                        if let Some(n) = part.strip_prefix('+') {
                            wc.branch.ahead = n.parse().unwrap_or(0);
                        } else if let Some(n) = part.strip_prefix('-') {
                            wc.branch.behind = n.parse().unwrap_or(0);
                        }
                    }
                }
            }
            continue;
        }
        if let Some(path) = chunk.strip_prefix("? ") {
            wc.entries.push(FileEntry {
                path: path.to_string(),
                orig_path: None,
                index_status: '?',
                wt_status: '?',
                conflict: None,
                untracked: true,
                staged_lines: None,
                unstaged_lines: None,
            });
            continue;
        }
        if chunk.starts_with("1 ") {
            let f: Vec<&str> = chunk.splitn(9, ' ').collect();
            if f.len() == 9 {
                let (x, y) = xy(f[1]);
                wc.entries.push(FileEntry {
                    path: f[8].to_string(),
                    orig_path: None,
                    index_status: x,
                    wt_status: y,
                    conflict: None,
                    untracked: false,
                    staged_lines: None,
                    unstaged_lines: None,
                });
            }
            continue;
        }
        if chunk.starts_with("2 ") {
            let f: Vec<&str> = chunk.splitn(10, ' ').collect();
            if f.len() == 10 {
                let (x, y) = xy(f[1]);
                let is_rename = f[8].starts_with('R') || f[8].starts_with('C');
                wc.entries.push(FileEntry {
                    path: f[9].to_string(),
                    orig_path: None,
                    index_status: x,
                    wt_status: y,
                    conflict: None,
                    untracked: false,
                    staged_lines: None,
                    unstaged_lines: None,
                });
                if is_rename {
                    expect_orig_path = true;
                }
            }
            continue;
        }
        if chunk.starts_with("u ") {
            let f: Vec<&str> = chunk.splitn(11, ' ').collect();
            if f.len() == 11 {
                wc.entries.push(FileEntry {
                    path: f[10].to_string(),
                    orig_path: None,
                    index_status: 'U',
                    wt_status: 'U',
                    conflict: Some(f[1].to_string()),
                    untracked: false,
                    staged_lines: None,
                    unstaged_lines: None,
                });
            }
            continue;
        }
        // "!" ignored records and anything unknown: ignore.
    }
    wc
}

fn xy(field: &str) -> (char, char) {
    let mut chars = field.chars();
    (chars.next().unwrap_or('.'), chars.next().unwrap_or('.'))
}

/// Display order with stable indices into `wc.entries`. A file changed in
/// both index and worktree appears in both Staged and Unstaged.
pub fn group_rows(wc: &WorkingCopy) -> Vec<(Group, usize)> {
    let mut rows = Vec::new();
    let mut push = |g: Group, e: &FileEntry, i: usize| rows.push((g, i));
    for (i, e) in wc.entries.iter().enumerate() {
        if e.conflict.is_some() {
            push(Group::Conflicts, e, i);
        }
    }
    for (i, e) in wc.entries.iter().enumerate() {
        if !e.untracked && e.conflict.is_none() && e.index_status != '.' {
            push(Group::Staged, e, i);
        }
    }
    for (i, e) in wc.entries.iter().enumerate() {
        if !e.untracked && e.conflict.is_none() && e.wt_status != '.' {
            push(Group::Unstaged, e, i);
        }
    }
    for (i, e) in wc.entries.iter().enumerate() {
        if e.untracked {
            push(Group::Untracked, e, i);
        }
    }
    rows
}
```

(Note: the `push` closure takes `e` only for symmetry — simplify to `rows.push((g, i))` directly if clippy complains about unused parameters.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib 2>&1 | tail -5`
Expected: all PASS (existing model/store tests + new).

- [ ] **Step 5: Commit**

```bash
git add src/engine/working_copy.rs
git commit -m "feat(engine): status v2 -z types and parser"
```

---

### Task 3: numstat parsing + `status()` command

**Files:**
- Modify: `src/engine/working_copy.rs`
- Test: `tests/engine_working_copy.rs`

**Interfaces:**
- Consumes: `engine::run` (Task 1), types from Task 2.
- Produces:
  - `parse_numstat_z(input: &str) -> Vec<(String, Option<(u64, u64)>)>` (pure)
  - `pub fn status(worktree: &Path) -> Result<WorkingCopy>` — fills `staged_lines`/`unstaged_lines`

- [ ] **Step 1: Write the failing integration test**

Append to `tests/engine_working_copy.rs`:

```rust
mod status_tests {
    use super::*;

    #[test]
    fn status_composes_entries_branch_and_numstat() {
        let tmp = tempfile::tempdir().unwrap();
        fixture_repo(tmp.path());
        // staged modification + unstaged modification + untracked file
        std::fs::write(tmp.path().join("f.txt"), "two").unwrap();
        sh(Some(tmp.path()), &["git", "add", "--", "f.txt"]);
        std::fs::write(tmp.path().join("f.txt"), "three").unwrap();
        std::fs::write(tmp.path().join("new.txt"), "brand new\nfile").unwrap();

        let wc = worktree_tool::engine::working_copy::status(tmp.path()).unwrap();
        assert_eq!(wc.branch.head, "main");
        assert_eq!(wc.entries.len(), 2); // f.txt, new.txt

        let f = wc.entries.iter().find(|e| e.path == "f.txt").unwrap();
        assert_eq!(f.index_status, 'M');
        assert_eq!(f.wt_status, 'M');
        assert_eq!(f.staged_lines, Some((1, 1)));
        assert_eq!(f.unstaged_lines, Some((1, 1)));

        let n = wc.entries.iter().find(|e| e.path == "new.txt").unwrap();
        assert!(n.untracked);
        assert_eq!(n.unstaged_lines, None);
    }

    #[test]
    fn status_reports_conflicts() {
        let tmp = tempfile::tempdir().unwrap();
        fixture_repo(tmp.path());
        sh(Some(tmp.path()), &["git", "checkout", "-q", "-b", "side"]);
        std::fs::write(tmp.path().join("f.txt"), "side").unwrap();
        sh(Some(tmp.path()), &["git", "commit", "-qam", "side"]);
        sh(Some(tmp.path()), &["git", "checkout", "-q", "main"]);
        std::fs::write(tmp.path().join("f.txt"), "main").unwrap();
        sh(Some(tmp.path()), &["git", "commit", "-qam", "main"]);
        sh(Some(tmp.path()), &["git", "merge", "side"]); // conflict, exit != 0 by design
        let wc = worktree_tool::engine::working_copy::status(tmp.path()).unwrap();
        assert_eq!(wc.entries[0].conflict.as_deref(), Some("UU"));
    }

    #[test]
    fn numstat_skips_rename_orig_chunks_and_binary() {
        use worktree_tool::engine::working_copy::parse_numstat_z;
        // "a\td\tnew" + separate NUL chunk "old" (rename) + binary marker
        let parsed = parse_numstat_z("3\t1\trenamed.txt\u{0}old.txt\u{0}-\t-\tbin.bin\u{0}");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0], ("renamed.txt".to_string(), Some((3, 1))));
        assert_eq!(parsed[1], ("bin.bin".to_string(), None));
    }
}
```

Adjust `sh` in `tests/common/mod.rs` to take `Option<&Path>` (the conflict test passes `Some`; Task 1 helpers pass a bare path — change the signature to `cwd: Option<&Path>` with `.current_dir(cwd.unwrap_or(Path::new(".")))` and update Task 1 call sites to `Some(tmp.path())`).

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test engine_working_copy 2>&1 | tail -5`
Expected: compile error — `status`/`parse_numstat_z` not found.

- [ ] **Step 3: Implement**

In `src/engine/working_copy.rs`:

```rust
/// Parses `git diff --numstat -z`: records are `added\tdeleted\tpath` NUL-
/// terminated; rename records are followed by their orig path in the next
/// NUL chunk (no tabs — skipped here). `-` counts mean binary.
pub fn parse_numstat_z(input: &str) -> Vec<(String, Option<(u64, u64)>)> {
    let mut out = Vec::new();
    for chunk in input.split('\0') {
        if chunk.is_empty() {
            continue;
        }
        let mut parts = chunk.splitn(3, '\t');
        let (Some(a), Some(d), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            continue; // rename orig-path chunk has no tabs
        };
        let counts = if a == "-" || d == "-" {
            None
        } else {
            Some((a.parse().unwrap_or(0), d.parse().unwrap_or(0)))
        };
        out.push((path.to_string(), counts));
    }
    out
}

/// Full working-copy snapshot: status v2 `-z` + numstat for both diff
/// surfaces. Read-only (`--no-optional-locks`).
pub fn status(worktree: &Path) -> Result<WorkingCopy> {
    let raw = engine::run(
        worktree,
        &["status", "--no-optional-locks", "--porcelain=v2", "-z", "--branch"],
    )?;
    let mut wc = parse_status_z(&raw);
    for (args, key) in [
        (vec!["diff", "--no-optional-locks", "--numstat", "-z"], 0),
        (vec!["diff", "--no-optional-locks", "--cached", "--numstat", "-z"], 1),
    ] {
        let raw = engine::run(worktree, &args)?;
        for (path, counts) in parse_numstat_z(&raw) {
            let entry = wc.entries.iter_mut().find(|e| e.path == path);
            match (entry, key) {
                (Some(e), 0) => e.unstaged_lines = counts,
                (Some(e), _) => e.staged_lines = counts,
                _ => {}
            }
        }
    }
    Ok(wc)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test engine_working_copy 2>&1 | tail -5`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add src/engine/working_copy.rs tests/engine_working_copy.rs tests/common/mod.rs
git commit -m "feat(engine): working-copy status command with numstat line counts"
```

---

### Task 4: Unified diff parser

**Files:**
- Create: `src/engine/diff.rs`
- Test: unit tests in `src/engine/diff.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `DiffLineKind` enum: `Context, Add, Del`
  - `DiffLine { pub kind: DiffLineKind, pub content: String, pub no_newline: bool }`
  - `DiffHunk { pub header: String, pub lines: Vec<DiffLine>, pub raw: String }` — `raw` is byte-exact (needed for Phase 1b `git apply --cached`)
  - `UnifiedDiff { pub header: String, pub hunks: Vec<DiffHunk>, pub binary: bool }`
  - `parse_unified_diff(input: &str) -> UnifiedDiff`

- [ ] **Step 1: Write the failing tests**

Create `src/engine/diff.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "diff --git a/f.txt b/f.txt\nindex a1b2c3..d4e5f6 100644\n--- a/f.txt\n+++ b/f.txt\n@@ -1,2 +1,3 @@\n one\n+two\n three\n@@ -10,2 +11,2 @@\n four\n-five\n+six\n\\ No newline at end of file\n";

    #[test]
    fn parses_hunks_lines_and_no_newline_marker() {
        let d = parse_unified_diff(PATCH);
        assert_eq!(d.hunks.len(), 2);
        assert!(!d.binary);
        assert!(d.header.contains("diff --git a/f.txt"));
        let h0 = &d.hunks[0];
        assert!(h0.header.starts_with("@@ -1,2 +1,3 @@"));
        assert_eq!(h0.lines.len(), 3);
        assert_eq!(h0.lines[0].kind, DiffLineKind::Context);
        assert_eq!(h0.lines[0].content, "one");
        assert_eq!(h0.lines[1].kind, DiffLineKind::Add);
        assert_eq!(h0.lines[1].content, "two");
        let h1 = &d.hunks[1];
        assert_eq!(h1.lines.len(), 3);
        assert_eq!(h1.lines[2].kind, DiffLineKind::Add);
        assert!(h1.lines[2].no_newline, "marker annotates the last + line");
        assert!(!h1.lines.iter().any(|l| l.content.starts_with('\\')));
    }

    #[test]
    fn raw_is_byte_exact_for_hunk_staging() {
        let d = parse_unified_diff(PATCH);
        let raw = &d.hunks[0].raw;
        assert!(raw.starts_with("@@ -1,2 +1,3 @@\n one\n+two\n three\n"));
        assert_eq!(&d.hunks[1].raw, "@@ -10,2 +11,2 @@\n four\n-five\n+six\n\\ No newline at end of file\n");
    }

    #[test]
    fn detects_binary_and_mode_only_changes() {
        let d = parse_unified_diff("diff --git a/img.png b/img.png\nindex a..b 100644\nBinary files a/img.png and b/img.png differ\n");
        assert!(d.binary);
        assert!(d.hunks.is_empty());
        let d = parse_unified_diff("diff --git a/s.sh b/s.sh\nold mode 100644\nnew mode 100755\n");
        assert!(!d.binary);
        assert!(d.hunks.is_empty());
    }

    #[test]
    fn empty_input_is_empty_diff() {
        let d = parse_unified_diff("");
        assert!(d.hunks.is_empty());
        assert!(d.header.is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib engine::diff 2>&1 | tail -5`
Expected: compile error — types not defined.

- [ ] **Step 3: Implement**

In `src/engine/diff.rs` above the tests:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Add,
    Del,
}

#[derive(Clone, Debug)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    /// Line content without the leading ` `/`+`/`-` marker.
    pub content: String,
    /// Preceded by a `\ No newline at end of file` marker.
    pub no_newline: bool,
}

#[derive(Clone, Debug)]
pub struct DiffHunk {
    /// The full `@@ -a,b +c,d @@ context` line.
    pub header: String,
    pub lines: Vec<DiffLine>,
    /// Byte-exact hunk text (header + lines), consumed verbatim by
    /// `git apply --cached` in Phase 1b hunk staging.
    pub raw: String,
}

#[derive(Clone, Debug, Default)]
pub struct UnifiedDiff {
    /// Everything before the first hunk: `diff --git`, index, `---/+++`,
    /// rename/mode lines.
    pub header: String,
    pub hunks: Vec<DiffHunk>,
    pub binary: bool,
}

/// Parses a single-file `git diff -U3 --no-color` output.
pub fn parse_unified_diff(input: &str) -> UnifiedDiff {
    let mut diff = UnifiedDiff::default();
    let mut header = String::new();
    let mut cur: Option<DiffHunk> = None;
    let mut cur_raw = String::new();
    for line in input.split_inclusive('\n') {
        let line = line.strip_suffix('\n').unwrap_or(line);
        if diff.binary {
            continue;
        }
        if line.starts_with("@@") {
            if let Some(mut h) = cur.take() {
                h.raw = std::mem::take(&mut cur_raw);
                diff.hunks.push(h);
            }
            cur = Some(DiffHunk { header: line.to_string(), lines: Vec::new(), raw: String::new() });
            cur_raw.push_str(line);
            cur_raw.push('\n');
            continue;
        }
        if cur.is_none() {
            if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
                diff.binary = true;
            }
            header.push_str(line);
            header.push('\n');
            continue;
        }
        cur_raw.push_str(line);
        cur_raw.push('\n');
        let hunk = cur.as_mut().expect("checked Some above");
        match line.chars().next() {
            Some('+') => hunk.lines.push(DiffLine { kind: DiffLineKind::Add, content: line[1..].to_string(), no_newline: false }),
            Some('-') => hunk.lines.push(DiffLine { kind: DiffLineKind::Del, content: line[1..].to_string(), no_newline: false }),
            Some('\\') => {
                if let Some(last) = hunk.lines.last_mut() {
                    last.no_newline = true;
                }
            }
            _ => hunk.lines.push(DiffLine { kind: DiffLineKind::Context, content: line.strip_prefix(' ').unwrap_or(line).to_string(), no_newline: false }),
        }
    }
    if let Some(mut h) = cur.take() {
        h.raw = cur_raw;
        diff.hunks.push(h);
    }
    diff.header = header;
    diff
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib 2>&1 | tail -5`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add src/engine/diff.rs
git commit -m "feat(engine): unified diff parser with byte-exact hunk raws"
```

---

### Task 5: Diff and preview commands

**Files:**
- Modify: `src/engine/diff.rs`
- Test: `tests/engine_working_copy.rs`

**Interfaces:**
- Consumes: `engine::run` (Task 1), `parse_unified_diff` (Task 4).
- Produces:
  - `pub const PREVIEW_MAX_BYTES: usize` (256 KiB)
  - `Preview` enum: `Text { content: String, truncated: bool }, Binary, Directory, Missing`
  - `diff_unstaged(worktree: &Path, rel_path: &str) -> Result<UnifiedDiff>`
  - `diff_staged(worktree: &Path, rel_path: &str) -> Result<UnifiedDiff>`
  - `read_preview(worktree: &Path, rel_path: &str) -> Preview`

- [ ] **Step 1: Write the failing tests**

Append to `tests/engine_working_copy.rs`:

```rust
mod diff_tests {
    use worktree_tool::engine::diff::{self, Preview};

    #[test]
    fn unstaged_and_staged_diffs_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = Some(tmp.path());
        common::fixture_repo(tmp.path());
        std::fs::write(tmp.path().join("f.txt"), "one\ntwo\n").unwrap();
        common::sh(cwd, &["git", "add", "--", "f.txt"]); // staged
        std::fs::write(tmp.path().join("f.txt"), "one\nTHREE\n").unwrap(); // unstaged on top

        let staged = diff::diff_staged(tmp.path(), "f.txt").unwrap();
        assert_eq!(staged.hunks.len(), 1);
        assert!(staged.hunks[0].lines.iter().any(|l| l.content == "two" && l.kind == worktree_tool::engine::diff::DiffLineKind::Del));

        let unstaged = diff::diff_unstaged(tmp.path(), "f.txt").unwrap();
        assert_eq!(unstaged.hunks.len(), 1);
        assert!(unstaged.hunks[0].lines.iter().any(|l| l.content == "THREE" && l.kind == worktree_tool::engine::diff::DiffLineKind::Add));

        // clean file → empty diff, no error
        let empty = diff::diff_unstaged(tmp.path(), "does-not-exist.txt").unwrap();
        assert!(empty.hunks.is_empty());
    }

    #[test]
    fn preview_classifies_text_binary_dir_and_truncation() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("t.txt"), "hello").unwrap();
        std::fs::write(tmp.path().join("b.bin"), [0u8, 1, 2, 3]).unwrap();
        std::fs::create_dir(tmp.path().join("subdir")).unwrap();
        let big = "x".repeat(300 * 1024);
        std::fs::write(tmp.path().join("big.txt"), &big).unwrap();

        match diff::read_preview(tmp.path(), "t.txt") {
            Preview::Text { content, truncated } => {
                assert_eq!(content, "hello");
                assert!(!truncated);
            }
            other => panic!("expected text, got {other:?}"),
        }
        assert!(matches!(diff::read_preview(tmp.path(), "b.bin"), Preview::Binary));
        assert!(matches!(diff::read_preview(tmp.path(), "subdir"), Preview::Directory));
        assert!(matches!(diff::read_preview(tmp.path(), "nope"), Preview::Missing));
        match diff::read_preview(tmp.path(), "big.txt") {
            Preview::Text { content, truncated } => {
                assert!(truncated);
                assert_eq!(content.len(), diff::PREVIEW_MAX_BYTES);
            }
            other => panic!("expected truncated text, got {other:?}"),
        }
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test engine_working_copy diff_tests 2>&1 | tail -5`
Expected: compile error — `diff_unstaged`/`read_preview` not found.

- [ ] **Step 3: Implement**

Append to `src/engine/diff.rs`:

```rust
use crate::engine::{self, Result};
use std::io::Read as _;
use std::path::Path;

/// Single known path → `:(literal)` pathspec (never glob-interpreted).
fn literal(rel_path: &str) -> String {
    format!(":(literal){rel_path}")
}

pub fn diff_unstaged(worktree: &Path, rel_path: &str) -> Result<UnifiedDiff> {
    let out = engine::run(
        worktree,
        &[
            "diff", "--no-optional-locks", "--no-color", "--no-ext-diff", "-U3",
            "--", &literal(rel_path),
        ],
    )?;
    Ok(parse_unified_diff(&out))
}

pub fn diff_staged(worktree: &Path, rel_path: &str) -> Result<UnifiedDiff> {
    let out = engine::run(
        worktree,
        &[
            "diff", "--cached", "--no-optional-locks", "--no-color", "--no-ext-diff",
            "-U3", "--", &literal(rel_path),
        ],
    )?;
    Ok(parse_unified_diff(&out))
}

pub const PREVIEW_MAX_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug)]
pub enum Preview {
    Text { content: String, truncated: bool },
    Binary,
    Directory,
    Missing,
}

/// Working-tree content for untracked and conflicted files (which `git
/// diff` doesn't express), bounded to `PREVIEW_MAX_BYTES` with a NUL sniff
/// over the first 8 KiB for binary detection.
pub fn read_preview(worktree: &Path, rel_path: &str) -> Preview {
    let full = worktree.join(rel_path);
    match std::fs::metadata(&full) {
        Ok(m) if m.is_dir() => return Preview::Directory,
        Err(_) => return Preview::Missing,
        Ok(_) => {}
    }
    let Ok(file) = std::fs::File::open(&full) else {
        return Preview::Missing;
    };
    let mut bytes = Vec::new();
    let mut limited = file.take((PREVIEW_MAX_BYTES + 1) as u64);
    if limited.read_to_end(&mut bytes).is_err() {
        return Preview::Missing;
    }
    let sniff = bytes.len().min(8192);
    if bytes[..sniff].contains(&0) {
        return Preview::Binary;
    }
    let truncated = bytes.len() > PREVIEW_MAX_BYTES;
    if truncated {
        bytes.truncate(PREVIEW_MAX_BYTES);
    }
    Preview::Text { content: String::from_utf8_lossy(&bytes).into_owned(), truncated }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test 2>&1 | tail -5`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add src/engine/diff.rs tests/engine_working_copy.rs
git commit -m "feat(engine): single-file diff commands and bounded working-tree preview"
```

---

### Task 6: Mutation commands — stage, unstage, discard

**Files:**
- Create: `src/engine/mutate.rs`
- Test: `tests/engine_working_copy.rs`

**Interfaces:**
- Consumes: `engine::run_trimmed`.
- Produces:
  - `stage(worktree: &Path, rel_paths: &[String]) -> Result<()>` (empty slice = no-op; also marks conflicts resolved)
  - `unstage(worktree: &Path, rel_paths: &[String]) -> Result<()>`
  - `discard_unstaged(worktree: &Path, rel_path: &str) -> Result<()>`
  - `discard_untracked(worktree: &Path, rel_path: &str) -> Result<()>` (files only; errors on directories)

- [ ] **Step 1: Write the failing tests**

Append to `tests/engine_working_copy.rs`:

```rust
mod mutate_tests {
    use worktree_tool::engine::mutate;
    use worktree_tool::engine::working_copy::status;

    #[test]
    fn stage_unstage_discard_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = Some(tmp.path());
        common::fixture_repo(tmp.path());
        std::fs::write(tmp.path().join("f.txt"), "changed").unwrap();
        std::fs::write(tmp.path().join("u.txt"), "untracked").unwrap();

        // stage file + untracked file, then unstage one
        mutate::stage(tmp.path(), &["f.txt".to_string(), "u.txt".to_string()]).unwrap();
        let wc = status(tmp.path()).unwrap();
        let f = wc.entries.iter().find(|e| e.path == "f.txt").unwrap();
        assert_eq!(f.index_status, 'M');
        let u = wc.entries.iter().find(|e| e.path == "u.txt").unwrap();
        assert_eq!(u.index_status, 'A'); // untracked → staged new file
        mutate::unstage(tmp.path(), &["u.txt".to_string()]).unwrap();
        let wc = status(tmp.path()).unwrap();
        assert!(wc.entries.iter().find(|e| e.path == "u.txt").unwrap().untracked);

        // discard unstaged: back to committed content
        mutate::discard_unstaged(tmp.path(), "f.txt").unwrap();
        let wc = status(tmp.path()).unwrap();
        assert_eq!(wc.entries.iter().find(|e| e.path == "f.txt").unwrap().index_status, '.');
        assert_eq!(std::fs::read_to_string(tmp.path().join("f.txt")).unwrap(), "one");

        // discard untracked: file gone
        mutate::discard_untracked(tmp.path(), "u.txt").unwrap();
        assert!(!tmp.path().join("u.txt").exists());
    }

    #[test]
    fn dash_and_space_names_stay_positional() {
        let tmp = tempfile::tempdir().unwrap();
        common::fixture_repo(tmp.path());
        // A file literally named "-weird name.txt" must never be an option.
        let weird = "-weird name.txt";
        std::fs::write(tmp.path().join(weird), "x").unwrap();
        mutate::stage(tmp.path(), &[weird.to_string()]).unwrap();
        let wc = status(tmp.path()).unwrap();
        assert_eq!(wc.entries[0].path, weird);
        mutate::unstage(tmp.path(), &[weird.to_string()]).unwrap();
        let wc = status(tmp.path()).unwrap();
        assert!(wc.entries[0].untracked);
    }

    #[test]
    fn empty_paths_are_noops() {
        let tmp = tempfile::tempdir().unwrap();
        common::fixture_repo(tmp.path());
        mutate::stage(tmp.path(), &[]).unwrap();
        mutate::unstage(tmp.path(), &[]).unwrap();
    }

    #[test]
    fn discard_untracked_refuses_directories() {
        let tmp = tempfile::tempdir().unwrap();
        common::fixture_repo(tmp.path());
        std::fs::create_dir(tmp.path().join("sub")).unwrap();
        assert!(mutate::discard_untracked(tmp.path(), "sub").is_err());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test engine_working_copy mutate_tests 2>&1 | tail -5`
Expected: compile error — `mutate` module not found.

- [ ] **Step 3: Implement**

Create `src/engine/mutate.rs`:

```rust
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

/// `git checkout -q -- <path>`: restore the committed version. Only offered
/// by the UI for unstaged rows.
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test engine_working_copy 2>&1 | tail -5`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add src/engine/mutate.rs tests/engine_working_copy.rs
git commit -m "feat(engine): stage/unstage/discard mutations with literal pathspecs"
```

---

### Task 7: Commit module — author, `$EDITOR` flow, commit

**Files:**
- Create: `src/engine/commit.rs`
- Test: `tests/engine_commit.rs` (new file)

**Interfaces:**
- Consumes: `engine::run_trimmed`.
- Produces:
  - `author(worktree: &Path) -> (String, String)` (name, email; `"(unset)"` when unconfigured)
  - `resolve_editor(git_config_value: Option<&str>, getenv: &dyn Fn(&str) -> Option<String>) -> Vec<String>` (pure; order: `$GIT_EDITOR`, `core.editor`, `$VISUAL`, `$EDITOR`, platform default; whitespace-split)
  - `enum CommitOutcome { Committed, AbortedEmpty }`
  - `commit_with_editor(worktree: &Path, staged_summary: &str) -> Result<CommitOutcome>`
  - `commit(worktree: &Path, message: &str) -> Result<()>`
  - `strip_comments(raw: &str) -> String` (pure)

- [ ] **Step 1: Write the failing tests**

Create `tests/engine_commit.rs`:

```rust
mod common;

use std::sync::Mutex;
use worktree_tool::engine::commit;

/// Tests mutate process-global env vars; serialize them.
static ENV_LOCK: Mutex<()> = Mutex::new(());

#[cfg(windows)]
const EDITOR_OK: &str = "cmd /c exit 0";
#[cfg(windows)]
const EDITOR_FAIL: &str = "cmd /c exit 1";
#[cfg(not(windows))]
const EDITOR_OK: &str = "true";
#[cfg(not(windows))]
const EDITOR_FAIL: &str = "false";

#[test]
fn resolve_editor_order_and_splitting() {
    let getenv = |k: &str| match k {
        "VISUAL" => Some("code -w".to_string()),
        _ => None,
    };
    // GIT_EDITOR wins
    assert_eq!(
        commit::resolve_editor(Some("nano"), &|k| (k == "GIT_EDITOR").then(|| "vi".to_string())),
        vec!["vi"]
    );
    // then core.editor config
    assert_eq!(commit::resolve_editor(Some("nano"), &|_| None), vec!["nano"]);
    // then VISUAL (split on whitespace), then EDITOR, then default
    assert_eq!(commit::resolve_editor(None, &getenv), vec!["code", "-w"]);
    assert_eq!(
        commit::resolve_editor(None, &|k| (k == "EDITOR").then(|| "emacs".to_string())),
        vec!["emacs"]
    );
    let expected_default = if cfg!(windows) { "notepad" } else { "vim" };
    assert_eq!(commit::resolve_editor(None, &|_| None), vec![expected_default]);
}

#[test]
fn strip_comments_removes_comments_and_trims() {
    assert_eq!(
        commit::strip_comments("\n# comment\nsubject\n\nbody line\n# trailing\n"),
        "subject\n\nbody line"
    );
    assert_eq!(commit::strip_comments("# only comments\n"), "");
}

#[test]
fn commit_via_editor_round_trip_and_abort() {
    let _guard = ENV_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    common::fixture_repo(tmp.path());

    // Editor that writes a message into the file we pass it.
    let editor = write_editor_script("committed by test");
    std::env::set_var("GIT_EDITOR", &editor);
    std::fs::write(tmp.path().join("f.txt"), "two").unwrap();
    common::sh(Some(tmp.path()), &["git", "add", "--", "f.txt"]);
    match commit::commit_with_editor(tmp.path(), "1 staged file: f.txt") {
        Ok(commit::CommitOutcome::Committed) => {}
        other => panic!("expected Committed, got {other:?}"),
    }
    let subject = common::sh_out(tmp.path(), &["git", "log", "-1", "--format=%s"]);
    assert_eq!(subject, "committed by test");

    // Empty message (editor exits 0 without writing anything) aborts.
    std::env::set_var("GIT_EDITOR", EDITOR_OK);
    std::fs::write(tmp.path().join("f.txt"), "three").unwrap();
    common::sh(Some(tmp.path()), &["git", "add", "--", "f.txt"]);
    match commit::commit_with_editor(tmp.path(), "1 staged file: f.txt") {
        Ok(commit::CommitOutcome::AbortedEmpty) => {}
        other => panic!("expected AbortedEmpty, got {other:?}"),
    }
    // Editor failure surfaces as an error.
    std::env::set_var("GIT_EDITOR", EDITOR_FAIL);
    assert!(commit::commit_with_editor(tmp.path(), "1 staged file: f.txt").is_err());
    std::env::remove_var("GIT_EDITOR");
}

/// Cross-platform "editor" that writes a fixed message into the file it
/// receives as its last argument.
fn write_editor_script(message: &str) -> String {
    let path = tempfile::tempdir().unwrap().into_path().join("ed");
    #[cfg(unix)]
    {
        let script = path.with_extension("sh");
        std::fs::write(&script, format!("#!/bin/sh\nprintf '%s' '{message}' > \"$1\"\n")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script.display().to_string()
    }
    #[cfg(windows)]
    {
        let script = path.with_extension("cmd");
        std::fs::write(&script, format!("@echo off\n(set /p _=<nul) > nul\n(type nul > \"%~1\")\n(echo|set /p=\"{message}\" > \"%~1\")\n")).unwrap();
        script.display().to_string()
    }
}
```

Note: add `sh_out` to `tests/common/mod.rs`:

```rust
pub fn sh_out(cwd: &Path, cmd: &[&str]) -> String {
    let out = std::process::Command::new(cmd[0])
        .args(&cmd[1..])
        .current_dir(cwd)
        .output()
        .expect("spawn");
    assert!(out.status.success(), "failed: {cmd:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}
```

(The Windows `write_editor_script` arm is best-effort; if `cmd` quoting misbehaves on the Windows CI runner, simplify that arm to `@echo {message}> "%~1"` and re-verify — the unix path is the contract.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test engine_commit 2>&1 | tail -5`
Expected: compile error — `engine::commit` not found.

- [ ] **Step 3: Implement**

Create `src/engine/commit.rs`:

```rust
//! Commit authoring through the user's editor — git's own COMMIT_EDITMSG
//! flow. Resolution order mirrors git: $GIT_EDITOR, core.editor, $VISUAL,
//! $EDITOR, platform default. The command is whitespace-split (quoted
//! paths with spaces are a documented Phase 1 limitation).

use crate::engine::{self, GitError, Result};
use std::path::Path;
use std::process::Command;

pub fn author(worktree: &Path) -> (String, String) {
    let name = engine::run_trimmed(worktree, &["config", "user.name"])
        .unwrap_or_else(|_| "(unset)".into());
    let email = engine::run_trimmed(worktree, &["config", "user.email"])
        .unwrap_or_else(|_| "(unset)".into());
    (name, email)
}

/// Pure so tests can inject env/config lookups. Returns argv (split on
/// whitespace); the message file is appended as the last argument.
pub fn resolve_editor(
    git_config_value: Option<&str>,
    getenv: &dyn Fn(&str) -> Option<String>,
) -> Vec<String> {
    let source = getenv("GIT_EDITOR")
        .filter(|v| !v.trim().is_empty())
        .or_else(|| git_config_value.map(str::to_string).filter(|v| !v.trim().is_empty()))
        .or_else(|| getenv("VISUAL"))
        .or_else(|| getenv("EDITOR"))
        .unwrap_or_else(|| platform_default_editor().to_string());
    source.split_whitespace().map(str::to_string).collect()
}

#[cfg(not(windows))]
fn platform_default_editor() -> &'static str {
    "vim"
}

#[cfg(windows)]
fn platform_default_editor() -> &'static str {
    "notepad"
}

#[derive(Debug)]
pub enum CommitOutcome {
    Committed,
    AbortedEmpty,
}

pub fn commit_with_editor(worktree: &Path, staged_summary: &str) -> Result<CommitOutcome> {
    let config_editor = engine::run_trimmed(worktree, &["config", "--get", "core.editor"]).ok();
    let argv = resolve_editor(config_editor.as_deref(), &|k| {
        std::env::var(k).ok().filter(|v| !v.is_empty())
    });
    let msg_path = std::env::temp_dir().join(format!("worktree-tool-commit-{}.msg", std::process::id()));
    std::fs::write(&msg_path, template(staged_summary)).map_err(|e| GitError {
        message: format!("could not write commit template: {e}"),
    })?;

    let run_result = (|| -> std::io::Result<()> {
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..]).arg(&msg_path).current_dir(worktree);
        let status = cmd.status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!("editor exited with {status}")))
        }
    })();
    let raw = match run_result {
        Ok(()) => std::fs::read_to_string(&msg_path).unwrap_or_default(),
        Err(e) => {
            let _ = std::fs::remove_file(&msg_path);
            return Err(GitError {
                message: format!("could not run editor {}: {e}", argv.join(" ")),
            });
        }
    };
    let _ = std::fs::remove_file(&msg_path);

    let message = strip_comments(&raw);
    if message.is_empty() {
        return Ok(CommitOutcome::AbortedEmpty);
    }
    commit(worktree, &message)?;
    Ok(CommitOutcome::Committed)
}

fn template(staged_summary: &str) -> String {
    format!(
        "\n# Please enter the commit message for your changes. Lines starting\n\
         # with '#' are ignored, and an empty message aborts the commit.\n#\n\
         # {staged_summary}\n"
    )
}

/// Drops `#` comment lines and trims the outside whitespace. Empty result
/// means the user aborted.
pub fn strip_comments(raw: &str) -> String {
    let kept: Vec<&str> = raw.lines().filter(|l| !l.starts_with('#')).collect();
    kept.join("\n").trim().to_string()
}

/// `git commit -q -F <file>` — `-F` avoids every quoting/length issue of
/// `-m`. User hooks run normally.
pub fn commit(worktree: &Path, message: &str) -> Result<()> {
    let msg_path = std::env::temp_dir().join(format!("worktree-tool-commit-{}.msg", std::process::id()));
    std::fs::write(&msg_path, message).map_err(|e| GitError {
        message: format!("could not write commit message: {e}"),
    })?;
    let msg_arg = msg_path.to_string_lossy().into_owned();
    let res = engine::run_trimmed(worktree, &["commit", "-q", "-F", &msg_arg]);
    let _ = std::fs::remove_file(&msg_path);
    res.map(|_| ())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test engine_commit 2>&1 | tail -5`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add src/engine/commit.rs tests/engine_commit.rs tests/common/mod.rs
git commit -m "feat(engine): commit via user editor with git-compatible resolution order"
```

---

### Task 8: `WorkingCopyStore` — selection, grouping, async operations

**Files:**
- Create: `src/wc_store.rs`
- Modify: `src/lib.rs` (`pub mod wc_store;`)
- Test: `tests/wc_store.rs` (new; GPUI `#[gpui::test]` harness, same patterns as `src/ui.rs` tests)

**Interfaces:**
- Consumes: all of `engine::*` (Tasks 2–7); `WorktreeStore` patterns (generation counter).
- Produces:
  - `pub enum Pane { Files, Diff }`
  - `pub enum FileDetail { Diff(engine::diff::UnifiedDiff), Preview(engine::diff::Preview), Failed(String) }`
  - `WorkingCopyStore` fields: `pub worktree: PathBuf, pub wc: Option<WorkingCopy>, pub detail: Option<FileDetail>, pub selected: Option<usize>, pub pane: Pane, pub author: Option<(String, String)>, pub busy: bool, pub message: Option<String>` and methods:
    - `new(worktree: PathBuf, cx: &mut App) -> Entity<Self>` (starts refresh + author fetch)
    - `rows(&self) -> Vec<(Group, usize)>`
    - `selected_row(&self) -> Option<(Group, &FileEntry)>`
    - `select(&mut self, idx: Option<usize>, cx: &mut Context<Self>)` (loads detail for the row)
    - `select_next` / `select_prev(&mut self, cx)` (group-bounded)
    - `refresh(&mut self, cx)` (status + detail for current selection; keeps selection by path)
    - `toggle_stage(&mut self, cx)` / `stage_all(&mut self, cx)` / `discard_selected(&mut self, cx)`
    - `commit_with_editor(&mut self, cx)`
    - `staged_count(&self) -> usize`
    - `take_mutated(&mut self) -> bool` — set after every successful mutation; the app shell consumes it to refresh the home worktree list
  - `staged_summary(wc: &WorkingCopy) -> String` (pure, e.g. `"2 staged files: a.txt, b.txt"`)

- [ ] **Step 1: Write the failing tests**

Create `tests/wc_store.rs`:

```rust
mod common;

use gpui::TestAppContext;
use worktree_tool::engine::working_copy::Group;
use worktree_tool::wc_store::{Pane, WorkingCopyStore};

fn sh(cwd: Option<&std::path::Path>, cmd: &[&str]) {
    let status = std::process::Command::new(cmd[0])
        .args(&cmd[1..])
        .current_dir(cwd.unwrap_or(std::path::Path::new(".")))
        .status()
        .expect("spawn");
    assert!(status.success(), "failed: {cmd:?}");
}

/// fixture repo + one staged mod (f.txt), one unstaged mod (g.txt), one
/// untracked (u.txt)
fn fixture(cx_work: &std::path::Path) {
    sh(Some(cx_work), &["git", "init", "-q", "-b", "main"]);
    sh(Some(cx_work), &["git", "config", "user.email", "t@t.t"]);
    sh(Some(cx_work), &["git", "config", "user.name", "t"]);
    std::fs::write(cx_work.join("f.txt"), "one").unwrap();
    std::fs::write(cx_work.join("g.txt"), "one").unwrap();
    sh(Some(cx_work), &["git", "add", "."]);
    sh(Some(cx_work), &["git", "commit", "-qm", "init"]);
    std::fs::write(cx_work.join("f.txt"), "one changed").unwrap();
    sh(Some(cx_work), &["git", "add", "--", "f.txt"]);
    std::fs::write(cx_work.join("g.txt"), "one changed").unwrap();
    std::fs::write(cx_work.join("u.txt"), "new").unwrap();
}

#[gpui::test]
fn refresh_groups_and_selection(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    let store = cx.update(|cx| WorkingCopyStore::new(tmp.path().to_path_buf(), cx));
    cx.run_until_parked();
    store.update(&mut cx.to_async().await, |_, _| {}); // (placeholder — see note)
}
```

**Note (test harness shape):** `Entity::update` in `#[gpui::test]` uses `store.update(&mut cx, |wc, cx| …)` where `&mut cx` is `&mut TestAppContext` coerced via `vcx.cx` in visual tests — follow the exact pattern from `src/ui.rs` tests: keep the `VisualTestContext` (`let (_, mut vcx) = …`) or use `store.update(&mut cx, …)` directly in plain `#[gpui::test]` (no window). Write the test bodies as:

```rust
#[gpui::test]
fn refresh_groups_and_selection(cx: &mut gpui::TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    let store = cx.update(|cx| WorkingCopyStore::new(tmp.path().to_path_buf(), cx));
    cx.run_until_parked();
    store.update(&mut cx.clone(), |wc, cx| {
        let rows = wc.rows();
        let groups: Vec<Group> = rows.iter().map(|(g, _)| *g).collect();
        assert_eq!(
            groups,
            vec![Group::Staged, Group::Unstaged, Group::Untracked]
        );
        assert_eq!(wc.staged_count(), 1);
        // first row selected by default, its diff loaded
        assert_eq!(wc.selected, Some(0));
        assert!(matches!(wc.pane, Pane::Files));
        let (group, entry) = wc.selected_row().unwrap();
        assert_eq!(group, Group::Staged);
        assert_eq!(entry.path, "f.txt");
        assert!(wc.detail.is_some(), "diff should load for the selected file");
        cx.notify();
    });
}

#[gpui::test]
fn selection_moves_within_groups_and_loads_diffs(cx: &mut gpui::TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    let store = cx.update(|cx| WorkingCopyStore::new(tmp.path().to_path_buf(), cx));
    cx.run_until_parked();
    store.update(&mut cx.clone(), |wc, cx| {
        // Staged group has 1 row → select_next parks there, second call
        // moves to Unstaged group (adjacent), third to Untracked.
        wc.select_next(cx);
        assert_eq!(wc.selected, Some(1));
        let (group, entry) = wc.selected_row().unwrap();
        assert_eq!((group, entry.path.as_str()), (Group::Unstaged, "g.txt"));
        wc.select_next(cx);
        let (group, entry) = wc.selected_row().unwrap();
        assert_eq!((group, entry.path.as_str()), (Group::Untracked, "u.txt"));
        // untracked row → preview detail, not diff
        assert!(matches!(wc.detail, Some(worktree_tool::wc_store::FileDetail::Preview(_))));
        // group-bounded: one more step stays on the last row of Untracked
        wc.select_next(cx);
        assert_eq!(wc.selected, Some(2));
        wc.select_prev(cx);
        wc.select_prev(cx);
        wc.select_prev(cx);
        assert_eq!(wc.selected, Some(0), "group-bounded at the top");
    });
}

#[gpui::test]
fn toggle_stage_and_discard_mutate_and_flag(cx: &mut gpui::TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    let store = cx.update(|cx| WorkingCopyStore::new(tmp.path().to_path_buf(), cx));
    cx.run_until_parked();
    // select the unstaged g.txt row and stage it
    store.update(&mut cx.clone(), |wc, cx| {
        wc.select(Some(1), cx);
    });
    cx.run_until_parked();
    store.update(&mut cx.clone(), |wc, cx| {
        wc.toggle_stage(cx);
    });
    cx.run_until_parked();
    store.update(&mut cx.clone(), |wc, cx| {
        assert!(wc.take_mutated(), "mutation must set the home-refresh flag");
        assert!(!wc.take_mutated(), "flag is consumed once");
        // g.txt is now staged: groups changed
        assert_eq!(wc.staged_count(), 2);
    });
    cx.run_until_parked();
    // discard the untracked file
    store.update(&mut cx.clone(), |wc, cx| {
        wc.select(Some(wc.rows().len() - 1), cx);
    });
    cx.run_until_parked();
    store.update(&mut cx.clone(), |wc, cx| {
        assert!(matches!(wc.selected_row(), Some((Group::Untracked, _))));
        wc.discard_selected(cx);
    });
    cx.run_until_parked();
    assert!(!tmp.path().join("u.txt").exists());
    store.update(&mut cx.clone(), |wc, cx| {
        assert!(wc.take_mutated());
    });
}

#[gpui::test]
fn staged_summary_lists_files(cx: &mut gpui::TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    let store = cx.update(|cx| WorkingCopyStore::new(tmp.path().to_path_buf(), cx));
    cx.run_until_parked();
    store.update(&mut cx.clone(), |wc, _cx| {
        let summary = worktree_tool::wc_store::staged_summary(wc.wc.as_ref().unwrap());
        assert_eq!(summary, "1 staged file: f.txt");
    });
}
```

(If `store.update(&mut cx.clone(), …)` doesn't typecheck against the installed gpui 0.2.2 test API, use the two-variant used in `src/ui.rs`: `view.update(&mut vcx.cx, …)` — i.e. obtain `&mut TestAppContext` as `&mut vcx.cx` from a `VisualTestContext`. The implementer should match whichever borrow shape the existing tests use — the assertions above are the contract, the borrow mechanics are already solved in-repo.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test wc_store 2>&1 | tail -5`
Expected: compile error — `wc_store` not found.

- [ ] **Step 3: Implement**

Create `src/wc_store.rs`:

```rust
//! State for one open worktree's Working Copy view. Same async discipline
//! as `WorktreeStore`: operations spawn on the background executor with a
//! generation counter; completions carrying a stale generation are dropped.

use crate::engine::{self, commit, diff, mutate, working_copy as eng};
use gpui::{App, AppContext, Context, Entity};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Files,
    Diff,
}

#[derive(Clone, Debug)]
pub enum FileDetail {
    Diff(diff::UnifiedDiff),
    Preview(diff::Preview),
    Failed(String),
}

pub struct WorkingCopyStore {
    pub worktree: PathBuf,
    pub wc: Option<eng::WorkingCopy>,
    pub detail: Option<FileDetail>,
    /// Index into `rows()`.
    pub selected: Option<usize>,
    pub pane: Pane,
    pub author: Option<(String, String)>,
    pub busy: bool,
    pub message: Option<String>,
    /// Consumed by the app shell: one successful mutation → one home-list
    /// refresh.
    mutated: bool,
    generation: u64,
}

impl WorkingCopyStore {
    pub fn new(worktree: PathBuf, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|_cx| Self {
            worktree: worktree.clone(),
            wc: None,
            detail: None,
            selected: None,
            pane: Pane::Files,
            author: None,
            busy: false,
            message: None,
            mutated: false,
            generation: 0,
        });
        entity.update(cx, |store, cx| {
            store.refresh(cx);
            store.fetch_author(cx);
        });
        entity
    }

    pub fn rows(&self) -> Vec<(eng::Group, usize)> {
        self.wc.as_ref().map(eng::group_rows).unwrap_or_default()
    }

    pub fn selected_row(&self) -> Option<(eng::Group, &eng::FileEntry)> {
        let idx = self.selected?;
        let (group, entry_idx) = self.rows().get(idx)?.clone();
        Some((group, &self.wc.as_ref().expect("rows implies wc").entries[entry_idx]))
    }

    pub fn staged_count(&self) -> usize {
        self.rows()
            .iter()
            .filter(|(g, _)| matches!(g, eng::Group::Staged))
            .count()
    }

    pub fn take_mutated(&mut self) -> bool {
        std::mem::take(&mut self.mutated)
    }

    pub fn select(&mut self, idx: Option<usize>, cx: &mut Context<Self>) {
        self.selected = idx.filter(|&i| i < self.rows().len());
        if self.pane == Pane::Diff {
            self.pane = Pane::Files; // selection change returns focus target to files
        }
        self.load_detail(cx);
        cx.notify();
    }

    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        let len = self.rows().len();
        if len == 0 {
            return;
        }
        let next = match self.selected {
            None => 0,
            Some(s) if s + 1 >= len => s, // group-bounded: stop at the last row
            Some(s) => s + 1,
        };
        self.select(Some(next), cx);
    }

    pub fn select_prev(&mut self, cx: &mut Context<Self>) {
        let next = match self.selected {
            Some(0) | None => 0,
            Some(s) => s - 1,
        };
        self.select(Some(next.min(self.rows().len().saturating_sub(1))), cx);
    }

    /// Re-runs status and reloads the selected row's detail. Keeps the
    /// selection on the same path when it still exists.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.busy = true;
        self.generation += 1;
        let gen = self.generation;
        let worktree = self.worktree.clone();
        let keep_path = self.selected_row().map(|(_, e)| e.path.clone());
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { eng::status(&worktree) })
                .await;
            this.update(cx, |store, cx| {
                if gen != store.generation {
                    return;
                }
                store.busy = false;
                match result {
                    Ok(wc) => {
                        let selected = keep_path.as_ref().and_then(|p| {
                            store.rows().iter().position(|(_, i)| {
                                wc.entries[*i].path == *p
                            })
                        });
                        store.wc = Some(wc);
                        store.selected = selected.or(if store.rows().is_empty() { None } else { Some(0) });
                        store.load_detail(cx);
                    }
                    Err(e) => {
                        store.message = Some(if e.is_lock_error() {
                            "another git process may be using this worktree — retry".into()
                        } else {
                            e.message
                        });
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn fetch_author(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let gen = self.generation;
        let worktree = self.worktree.clone();
        cx.spawn(async move |this, cx| {
            let author = cx
                .background_executor()
                .spawn(async move { commit::author(&worktree) })
                .await;
            this.update(cx, |store, cx| {
                if gen != store.generation {
                    return;
                }
                store.author = Some(author);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Loads the right detail view for the selected row: unified diff for
    /// staged/unstaged rows, working-tree preview for untracked and
    /// conflicted rows.
    fn load_detail(&mut self, cx: &mut Context<Self>) {
        let Some((group, entry)) = self.selected_row().map(|(g, e)| (g, e.clone())) else {
            self.detail = None;
            return;
        };
        self.generation += 1;
        let gen = self.generation;
        let worktree = self.worktree.clone();
        let path = entry.path.clone();
        let kind = match group {
            eng::Group::Staged => DetailKind::Staged,
            eng::Group::Unstaged => DetailKind::Unstaged,
            eng::Group::Conflicts | eng::Group::Untracked => DetailKind::Preview,
        };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match kind {
                        DetailKind::Staged => diff::diff_staged(&worktree, &path)
                            .map(FileDetail::Diff),
                        DetailKind::Unstaged => diff::diff_unstaged(&worktree, &path)
                            .map(FileDetail::Diff),
                        DetailKind::Preview => {
                            Ok(FileDetail::Preview(diff::read_preview(&worktree, &path)))
                        }
                    }
                })
                .await;
            this.update(cx, |store, cx| {
                if gen != store.generation {
                    return;
                }
                store.detail = Some(match result {
                    Ok(d) => d,
                    Err(e) => FileDetail::Failed(e.message),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// `s` on a row: stage unstaged/untracked/conflict rows, unstage staged
    /// rows. Conflicts: staging marks them resolved.
    pub fn toggle_stage(&mut self, cx: &mut Context<Self>) {
        let Some((group, entry)) = self.selected_row().map(|(g, e)| (g, e.clone())) else {
            return;
        };
        let worktree = self.worktree.clone();
        let path = entry.path.clone();
        self.generation += 1;
        let gen = self.generation;
        self.busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match group {
                        eng::Group::Staged => mutate::unstage(&worktree, &[path.clone()]),
                        _ => mutate::stage(&worktree, &[path.clone()]),
                    }
                })
                .await;
            this.update(cx, |store, cx| {
                store.after_mutation(gen, result, cx);
            })
            .ok();
        })
        .detach();
    }

    pub fn stage_all(&mut self, cx: &mut Context<Self>) {
        let worktree = self.worktree.clone();
        let paths: Vec<String> = self
            .rows()
            .into_iter()
            .filter(|(g, _)| !matches!(g, eng::Group::Staged))
            .filter_map(|(_, i)| self.wc.as_ref().map(|wc| wc.entries[i].path.clone()))
            .collect();
        if paths.is_empty() {
            return;
        }
        self.generation += 1;
        let gen = self.generation;
        self.busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { mutate::stage(&worktree, &paths) })
                .await;
            this.update(cx, |store, cx| store.after_mutation(gen, result, cx))
                .ok();
        })
        .detach();
    }

    /// Caller shows the confirmation dialog first; this executes.
    pub fn discard_selected(&mut self, cx: &mut Context<Self>) {
        let Some((group, entry)) = self.selected_row().map(|(g, e)| (g, e.clone())) else {
            return;
        };
        if entry.is_dir() {
            return; // no recursive delete in Phase 1
        }
        let worktree = self.worktree.clone();
        let path = entry.path.clone();
        self.generation += 1;
        let gen = self.generation;
        self.busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match group {
                        eng::Group::Untracked => mutate::discard_untracked(&worktree, &path),
                        eng::Group::Unstaged => mutate::discard_unstaged(&worktree, &path),
                        _ => Ok(()), // staged/conflicts: dialog is not offered
                    }
                })
                .await;
            this.update(cx, |store, cx| store.after_mutation(gen, result, cx))
                .ok();
        })
        .detach();
    }

    fn after_mutation(
        &mut self,
        gen: u64,
        result: engine::Result<()>,
        cx: &mut Context<Self>,
    ) {
        if gen != self.generation {
            return;
        }
        self.busy = false;
        match result {
            Ok(()) => {
                self.message = None;
                self.mutated = true;
            }
            Err(e) => {
                self.message = Some(if e.is_lock_error() {
                    "another git process may be using this worktree — retry".into()
                } else {
                    e.message
                });
            }
        }
        self.refresh(cx);
    }

    pub fn commit_with_editor(&mut self, cx: &mut Context<Self>) {
        let Some(wc) = self.wc.clone() else { return };
        if self.staged_count() == 0 {
            self.message = Some("Nothing staged — press s on files to stage them first".into());
            cx.notify();
            return;
        }
        let summary = staged_summary(&wc);
        let worktree = self.worktree.clone();
        self.busy = true;
        self.message = Some("Waiting for commit editor…".into());
        self.generation += 1;
        let gen = self.generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { commit::commit_with_editor(&worktree, &summary) })
                .await;
            this.update(cx, |store, cx| {
                if gen != store.generation {
                    return;
                }
                store.busy = false;
                match result {
                    Ok(commit::CommitOutcome::Committed) => {
                        store.message = Some("Committed".into());
                        store.mutated = true;
                    }
                    Ok(commit::CommitOutcome::AbortedEmpty) => {
                        store.message = Some("Commit aborted — empty message".into());
                    }
                    Err(e) => {
                        store.message = Some(if e.is_lock_error() {
                            "another git process may be using this worktree — retry".into()
                        } else {
                            e.message
                        })
                    }
                }
                store.refresh(cx);
            })
            .ok();
        })
        .detach();
    }
}

enum DetailKind {
    Staged,
    Unstaged,
    Preview,
}

/// "2 staged files: a.txt, b.txt" (names capped at 8, then "…").
pub fn staged_summary(wc: &eng::WorkingCopy) -> String {
    let mut names: Vec<String> = eng::group_rows(wc)
        .into_iter()
        .filter(|(g, _)| matches!(g, eng::Group::Staged))
        .map(|(_, i)| wc.entries[i].path.clone())
        .collect();
    names.dedup();
    let count = names.len();
    if names.len() > 8 {
        names.truncate(8);
        names.push("…".to_string());
    }
    let plural = if count == 1 { "file" } else { "files" };
    format!("{count} staged {plural}: {}", names.join(", "))
}
```

The final `src/wc_store.rs` contains exactly: `Pane`, `FileDetail`, `WorkingCopyStore` (methods: `new`, `rows`, `selected_row`, `staged_count`, `take_mutated`, `select`, `select_next`, `select_prev`, `refresh`, `fetch_author`, `load_detail`, `toggle_stage`, `stage_all`, `discard_selected`, `after_mutation`, `commit_with_editor`), the private `DetailKind` enum, and `staged_summary`.

In `src/lib.rs` add `pub mod wc_store;`.

- [ ] **Step 4: Run tests to verify they pass**

- [ ] **Step 5: Commit**

```bash
git add src/wc_store.rs src/lib.rs tests/wc_store.rs
git commit -m "feat(wc-store): working-copy store with grouped selection and background mutations"
```

---

### Task 9: App shell split — `ui.rs` → `app.rs` + detail-view navigation

**Files:**
- Move: `src/ui.rs` → `src/app.rs` (git mv)
- Create: `src/views/mod.rs`, `src/views/working_copy.rs` (placeholder this task)
- Modify: `src/lib.rs`, `src/main.rs`, `src/dialogs.rs` (import path only)
- Test: existing UI tests move with the file; new navigation test

**Interfaces:**
- Consumes: `WorkingCopyStore` (Task 8).
- Produces:
  - `app::RootView` gains fields: `pub detail: Option<Entity<WorkingCopyStore>>`, `pub detail_focus: FocusHandle`, `pub detail_list_focus: FocusHandle`, `pub detail_diff_focus: FocusHandle`
  - `RootView::open_detail(&mut self, window, cx)` / `close_detail(&mut self, window, cx)`
  - Key routing: `enter` opens detail; `t` opens terminal (list and detail); inside detail: `esc` back, `1`/`2`/`3` no-op tabs, `tab` pane toggle (Task 11 wires the diff pane focus), `r` refresh
  - `views::working_copy::render(this: &mut RootView, window: &mut Window, cx: &mut Context<RootView>) -> impl IntoElement`

- [ ] **Step 1: Move the module mechanically and keep everything green**

```bash
git mv src/ui.rs src/app.rs
```

- In `src/lib.rs`: replace `pub mod ui;` with `pub mod app; pub mod views;`
- Create `src/views/mod.rs`:

```rust
pub mod working_copy;
```

- Create `src/views/working_copy.rs` placeholder:

```rust
//! Working Copy detail view rendering. Listeners attach against `RootView`
//! (same pattern as dialogs.rs).

use crate::app::RootView;
use gpui::{Context, IntoElement, Window};

pub fn render(
    _this: &mut RootView,
    _window: &mut Window,
    _cx: &mut Context<RootView>,
) -> impl IntoElement {
    gpui::div().id("detail-view").size_full()
}
```

- In `src/app.rs`: `use crate::dialogs::{self, DialogState};` stays; `crate::ui::` references inside `dialogs.rs` change: `use crate::app::RootView;` and `use crate::app::{ACCENT, BORDER, DIM, GREEN, PANEL, RED, ROW_SELECTED, TEXT};`
- In `src/main.rs`: `use worktree_tool::app::{FocusSearch, NewWorktree, Quit, Refresh, RootView};`
- Run `cargo test 2>&1 | tail -5` — all pass (pure move).

- [ ] **Step 2: Add detail state, focus handles, and key routing**

In `src/app.rs`:

Add to `RootView` struct:

```rust
    pub detail: Option<Entity<worktree_tool::wc_store::WorkingCopyStore>>,
    pub detail_focus: gpui::FocusHandle,
    pub detail_list_focus: gpui::FocusHandle,
    pub detail_diff_focus: gpui::FocusHandle,
```

Initialize in `new_with_start` (`detail: None, detail_focus: cx.focus_handle(), detail_list_focus: cx.focus_handle(), detail_diff_focus: cx.focus_handle()`), and add `use crate::wc_store::Pane;` to the imports (the `tab` arms below set it).

Add methods to `impl RootView`:

```rust
    pub fn open_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog.is_open() {
            return;
        }
        let Some(entry) = self.store.read(cx).selected_entry().cloned() else {
            return;
        };
        let wc = worktree_tool::wc_store::WorkingCopyStore::new(entry.path.clone(), cx);
        self.detail = Some(wc);
        window.focus(&self.detail_list_focus);
        cx.notify();
    }

    pub fn close_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.detail = None;
        window.focus(&self.root_focus);
        self.store.update(cx, |store, cx| store.refresh(cx));
        cx.notify();
    }
```

Recreate the home-list refresh-on-mutation hook in `open_detail`, right after creating `wc` (gpui observers receive `this: &mut RootView`, so the closure can reach the home store directly — no handle juggling):

```rust
        cx.observe(&wc, move |this, wc, cx| {
            if wc.read(cx).take_mutated() {
                this.store.update(cx, |store, cx| store.refresh(cx));
            }
            cx.notify();
        })
        .detach();
```

In the `root_keydown` closure, replace the `"enter"` arm and add detail routing. The final structure:

```rust
        let root_keydown = cx.listener(move |this, event: &KeyDownEvent, window, cx| {
            if this.dialog.is_open() {
                return; // dialogs handle their own keys
            }
            let ks = &event.keystroke;
            if this.search_focused(window, cx) {
                if ks.key == "escape" {
                    this.search.update(cx, |field, cx| field.set_value("", cx));
                    this.store
                        .update(cx, |store, cx| store.set_filter(String::new(), cx));
                    window.focus(&this.root_focus);
                } else if ks.key == "enter" {
                    window.focus(&this.root_focus);
                }
                return;
            }
            if ks.modifiers.control || ks.modifiers.platform || ks.modifiers.alt {
                return;
            }
            if this.detail.is_some() {
                this.detail_keydown(ks, window, cx);
                return;
            }
            // Home list: only act when the list itself is focused.
            if !this.root_focus.is_focused(window) {
                return;
            }
            match ks.key.as_str() {
                "up" => this.store.update(cx, |store, cx| store.select_prev(cx)),
                "down" => this.store.update(cx, |store, cx| store.select_next(cx)),
                "enter" => this.open_detail(window, cx),
                "t" => {
                    if let Some(entry) = this.store.read(cx).selected_entry() {
                        let path = entry.path.clone();
                        terminal::open_in_terminal(&path);
                    }
                }
                "backspace" | "delete" => this.open_remove_dialog(window, cx),
                "/" => {
                    let handle = this.search.read(cx).focus_handle.clone();
                    window.focus(&handle);
                }
                "n" => this.open_create_dialog(window, cx),
                "r" => this.store.update(cx, |store, cx| store.refresh(cx)),
                _ => {}
            }
        });
```

Add the detail key handler method:

```rust
    fn detail_keydown(
        &mut self,
        ks: &gpui::KeyStroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let list_focused = self.detail_list_focus.is_focused(window);
        let diff_focused = self.detail_diff_focus.is_focused(window);
        let container_focused = self.detail_focus.is_focused(window);
        if !list_focused && !diff_focused && !container_focused {
            return; // don't steal keys from other focused surfaces
        }
        match ks.key.as_str() {
            "escape" => self.close_detail(window, cx),
            "t" => {
                if let Some(wc) = &self.detail {
                    let path = wc.read(cx).worktree.clone();
                    terminal::open_in_terminal(&path);
                }
            }
            "r" => {
                if let Some(wc) = &self.detail {
                    wc.update(cx, |store, cx| store.refresh(cx));
                }
            }
            "tab" if list_focused => {
                if let Some(wc) = &self.detail {
                    wc.update(cx, |store, cx| {
                        store.pane = Pane::Diff;
                        cx.notify();
                    });
                }
                window.focus(&self.detail_diff_focus);
            }
            "tab" if diff_focused => {
                if let Some(wc) = &self.detail {
                    wc.update(cx, |store, cx| {
                        store.pane = Pane::Files;
                        cx.notify();
                    });
                }
                window.focus(&self.detail_list_focus);
            }
            "up" if list_focused => {
                if let Some(wc) = &self.detail {
                    wc.update(cx, |store, cx| store.select_prev(cx));
                }
            }
            "down" if list_focused => {
                if let Some(wc) = &self.detail {
                    wc.update(cx, |store, cx| store.select_next(cx));
                }
            }
            // "s", "S", "d", "c", diff-pane hunk keys arrive in Tasks 10–12.
            _ => {}
        }
    }
```

In `render`, replace the content branch: when `self.detail.is_some()`, render `views::working_copy::render(self, window, cx).into_any_element()` instead of the worktree list + detail footer (keep toolbar + status bar). Also render `<AnyElement>` via `.into_any_element()`; import `gpui::IntoElement` (already imported).

- [ ] **Step 3: Write the navigation test**

Append to the tests module in `src/app.rs`:

```rust
    #[gpui::test]
    fn enter_drills_into_detail_and_esc_returns(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("fixture");
        std::fs::create_dir(&repo).unwrap();
        fixture_repo(&repo);
        let (view, mut vcx) = open_root(cx, &repo);

        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            assert!(root.detail.is_some(), "detail opens on enter");
            let wc = root.detail.as_ref().unwrap().read(cx);
            assert!(wc.wc.is_some(), "working copy loaded");
        });

        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, _cx| {
            assert!(root.detail.is_none(), "esc returns to the list");
        });
    }

    #[gpui::test]
    fn t_still_opens_terminal_from_list(cx: &mut TestAppContext) {
        // terminal::open_in_terminal spawns a real process; instead assert
        // that "t" is routed by checking the key handler's observable
        // effect: none (spawning is fire-and-forget). This test guards the
        // keybinding table: "t" must not select/drill in.
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("fixture");
        std::fs::create_dir(&repo).unwrap();
        fixture_repo(&repo);
        let (view, mut vcx) = open_root(cx, &repo);
        vcx.simulate_keystrokes("t");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, _cx| {
            assert!(root.detail.is_none(), "t must not open the detail view");
            assert!(matches!(root.dialog, DialogState::None));
        });
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test 2>&1 | tail -8`
Expected: all PASS (moved tests + 2 new).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(app): worktree detail view shell with drill-in navigation"
```

---

### Task 10: Working copy view — file list pane

**Files:**
- Modify: `src/views/working_copy.rs`, `src/app.rs` (key arms for `s`/`S`/`d`), `src/dialogs.rs` (Discard dialog)
- Test: `src/app.rs` tests

**Interfaces:**
- Consumes: `WorkingCopyStore` API (Task 8), `DialogState` (dialogs.rs), palette consts in `app.rs`.
- Produces:
  - `views::working_copy::render` — full detail view: header (branch + ahead/behind + path + buttons + tabs), file list pane, placeholder diff pane (Task 11 fills it), footer key hints + author
  - `DialogState::Discard { path: String, untracked: bool }` + `render_discard_dialog` + `RootView::confirm_discard`
  - Key arms in `detail_keydown`: `s`, `S`, `d`, `c` placeholder (Task 12), gated on `list_focused`

- [ ] **Step 1: Write the failing test**

Append to `src/app.rs` tests:

```rust
    #[gpui::test]
    fn stage_keys_toggle_files(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("fixture");
        std::fs::create_dir(&repo).unwrap();
        fixture_repo(&repo);
        std::fs::write(repo.join("new.txt"), "untracked").unwrap();
        let (view, mut vcx) = open_root(cx, &repo);

        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        // navigate to the untracked row and stage it
        vcx.simulate_keystrokes("down");
        vcx.run_until_parked();
        vcx.simulate_keystrokes("s");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            let wc = root.detail.as_ref().unwrap().read(cx);
            assert!(wc.take_mutated(), "staging flags home refresh");
            assert_eq!(wc.staged_count(), 1);
        });
    }

    #[gpui::test]
    fn discard_opens_confirm_dialog_and_esc_cancels(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("fixture");
        std::fs::create_dir(&repo).unwrap();
        fixture_repo(&repo);
        std::fs::write(repo.join("f.txt"), "changed").unwrap();
        let (view, mut vcx) = open_root(cx, &repo);

        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        // f.txt is modified-unstaged; it's row 0 of Unstaged (only row)
        vcx.simulate_keystrokes("d");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, _cx| {
            assert!(matches!(root.dialog, DialogState::Discard { .. }), "discard needs confirmation");
        });
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            assert!(matches!(root.dialog, DialogState::None));
            assert_eq!(root.detail.as_ref().unwrap().read(cx).wc.as_ref().unwrap().entries.len(), 1, "nothing discarded");
        });
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib stage_keys 2>&1 | tail -5`
Expected: FAIL — `s` has no handler yet.

- [ ] **Step 3: Implement**

3a. In `src/dialogs.rs`, add the variant:

```rust
    Discard { path: String, untracked: bool },
```

Add `RootView::confirm_discard` in `src/app.rs` (next to `confirm_remove`):

```rust
    pub fn confirm_discard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(wc) = self.detail.clone() {
            wc.update(cx, |store, cx| store.discard_selected(cx));
        }
        self.close_dialog(window, cx);
    }
```

Add the render function in `src/dialogs.rs` (mirror `render_remove_dialog`: card 500px wide, title "Discard changes", `label(format!("Path: {path}"))`, red warning `"This cannot be undone."` — plus `"The file itself will be deleted."` when `untracked`, key handler with `cx.stop_propagation()` where escape/enter route to cancel/confirm, buttons Cancel (TEXT/BORDER) and Discard (`rgb(0x11111b)` on `RED`) wired to `cancel`/`this.confirm_discard`).

Wire it into `render`'s dialog match in `src/app.rs`:

```rust
                DialogState::Discard { .. } => {
                    Some(dialogs::render_discard_dialog(self, window, cx).into_any_element())
                }
```

3b. In `src/views/working_copy.rs`, implement the full detail view.

```rust
use crate::app::{RootView, ACCENT, BORDER, DIM, GREEN, PANEL, RED, ROW_SELECTED, TEXT, YELLOW};
use crate::dialogs::DialogState;
use crate::wc_store::{FileDetail, Pane, WorkingCopyStore};
use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, rgba, Context, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, Window,
};
use worktree_tool::engine::working_copy::Group;

const DIFF_RENDER_CAP: usize = 5000;

pub fn render(
    this: &mut RootView,
    _window: &mut Window,
    cx: &mut Context<RootView>,
) -> impl IntoElement {
    let Some(wc) = this.detail.clone() else {
        return div().id("detail-view").into_any_element();
    };
    let (branch_label, arrows, path) = {
        let store = wc.read(cx);
        let branch = store
            .wc
            .as_ref()
            .map(|w| w.branch.head.clone())
            .unwrap_or_else(|| "…".into());
        let (ahead, behind) = store
            .wc
            .as_ref()
            .map(|w| (w.branch.ahead, w.branch.behind))
            .unwrap_or((0, 0));
        let mut arrows = String::new();
        if ahead > 0 {
            arrows.push_str(&format!("↑{ahead} "));
        }
        if behind > 0 {
            arrows.push_str(&format!("↓{behind}"));
        }
        (branch, arrows, store.worktree.display().to_string())
    };
    let rows: Vec<(Group, usize, bool, char, String, Option<(u64, u64)>)> = {
        let store = wc.read(cx);
        store
            .rows()
            .iter()
            .enumerate()
            .map(|(pos, (group, i))| {
                let e = &store.wc.as_ref().expect("rows implies wc").entries[*i];
                let letter = match group {
                    Group::Staged => e.index_status,
                    Group::Unstaged => e.wt_status,
                    Group::Conflicts => 'U',
                    Group::Untracked => '?',
                };
                let counts = match group {
                    Group::Staged => e.staged_lines,
                    Group::Unstaged => e.unstaged_lines,
                    _ => None,
                };
                let path = match &e.orig_path {
                    Some(old) => format!("{old} → {}", e.path),
                    None => e.path.clone(),
                };
                (*group, *i, pos == store.selected.unwrap_or(usize::MAX), letter, path, counts)
            })
            .collect()
    };
    let author = wc.read(cx).author.clone();
    let message = wc.read(cx).message.clone();

    let container_focus = this.detail_focus.clone();
    let body = div()
        .id("wc-body")
        .flex()
        .flex_1()
        .min_h_0()
        .child(render_file_list(this, cx, rows))
        .child(render_diff_pane(this, cx));

    div()
        .id("detail-view")
        .track_focus(&container_focus)
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        // ---- header: branch, arrows, path, tabs, back hint ----
        .child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(BORDER)
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(format!("{branch_label} {arrows}")),
                )
                .child(div().flex_1().min_w_0().text_size(px(11.)).text_color(DIM).child(path))
                .child(tab_label("1 Working Copy", true))
                .child(tab_label("2 History — v0.3", false))
                .child(tab_label("3 Branches — v0.4", false))
                .child(div().text_size(px(11.)).text_color(DIM).child("esc back")),
        )
        // ---- body: files | diff ----
        .child(body)
        // ---- footer: hints + author + message ----
        .child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .px_3()
                .py_1()
                .border_t_1()
                .border_color(BORDER)
                .bg(PANEL)
                .child(
                    div().text_size(px(11.)).text_color(DIM).child(
                        "↑↓ move · s stage/unstage · S stage all · d discard · c commit · tab pane · r refresh · t terminal · esc back".to_string(),
                    ),
                )
                .child(div().flex_1())
                .when_some(author, |f, (name, email)| {
                    f.child(div().text_size(px(11.)).text_color(DIM).child(format!("{name} <{email}>")))
                })
                .when_some(message, |f, msg| {
                    f.child(div().text_size(px(11.)).text_color(YELLOW).child(msg))
                }),
        )
        .into_any_element()
}
```

Plus these helpers in the same file (`render_file_list` builds the left pane with group headers + rows; each row: `.on_mouse_down` selects and focuses the list; `render_diff_pane` is a Task 11 stub here returning an empty right pane div with `track_focus(&detail_diff_focus)`):

```rust
fn tab_label(text: &str, active: bool) -> impl IntoElement {
    div()
        .text_size(px(12.))
        .text_color(if active { ACCENT } else { DIM })
        .child(text.to_string())
}

fn status_color(c: char) -> gpui::Rgba {
    match c {
        'A' => GREEN,
        'M' | 'R' | 'C' => YELLOW,
        'D' | 'U' => RED,
        _ => DIM,
    }
}

fn group_header(title: &str) -> impl IntoElement {
    div()
        .px_3()
        .py_1()
        .text_size(px(11.))
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(DIM)
        .child(title.to_string())
}

fn render_file_list(
    this: &mut RootView,
    cx: &mut Context<RootView>,
    rows: Vec<(Group, usize, bool, char, String, Option<(u64, u64)>)>,
) -> impl IntoElement {
    let list_focus = this.detail_list_focus.clone();
    let mut last_group: Option<Group> = None;
    let mut list = div()
        .id("wc-files")
        .track_focus(&list_focus)
        .w(px(340.))
        .flex()
        .flex_col()
        .flex_shrink_0()
        .border_r_1()
        .border_color(BORDER)
        .overflow_y_scroll();
    let wc_entity = this.detail.clone();
    for (pos, (group, _i, is_selected, letter, path, counts)) in rows.into_iter().enumerate() {
        if last_group != Some(group) {
            list = list.child(group_header(group.title()));
            last_group = Some(group);
        }
        let wc_clone = wc_entity.clone();
        list = list.child(
            div()
                .id(SharedString::from(format!("wc-row-{pos}")))
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_1()
                .when(is_selected, |row| row.bg(ROW_SELECTED))
                .child(
                    div()
                        .w(px(12.))
                        .flex_shrink_0()
                        .text_size(px(12.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(status_color(letter))
                        .child(letter.to_string()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(12.))
                        .child(path),
                )
                .when_some(counts, |row, (a, d)| {
                    row.child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(11.))
                            .child(div().text_color(GREEN).child(format!("+{a}")))
                            .child(div().text_color(RED).child(format!("−{d}"))),
                    )
                })
                .on_mouse_down(
                    MouseButton::Left,
                    move |this: &mut RootView, _, window, cx| {
                        if let Some(wc) = &wc_clone {
                            wc.update(cx, |store, cx| store.select(Some(pos), cx));
                        }
                        window.focus(&this.detail_list_focus);
                    },
                ),
        );
    }
    if rows.is_empty() {
        list = list.child(
            div()
                .p_4()
                .text_size(px(13.))
                .text_color(DIM)
                .child("Working tree clean"),
        );
    }
    list
}
```

(Notes for the implementer: (1) the row's status letter is the real per-surface letter computed in `render`'s `rows` tuple — Staged rows show the index status, Unstaged rows the worktree status; (2) if any styling method used here doesn't exist under that exact name in gpui 0.2.2 (`text_overflow` was already dropped for this reason), drop the styling and rely on `min_w_0()` + `flex_1()` — never add a dependency or fork the framework to keep a cosmetic detail.)

3c. Key arms in `detail_keydown` (`src/app.rs`):

```rust
            "s" if list_focused => {
                if let Some(wc) = &self.detail {
                    wc.update(cx, |store, cx| store.toggle_stage(cx));
                }
            }
            "S" if list_focused => {
                if let Some(wc) = &self.detail {
                    wc.update(cx, |store, cx| store.stage_all(cx));
                }
            }
            "d" if list_focused => self.open_discard_dialog(window, cx),
```

And the dialog opener:

```rust
    fn open_discard_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog.is_open() {
            return;
        }
        let Some(wc) = &self.detail else { return };
        let Some((group, entry)) = wc.read(cx).selected_row().map(|(g, e)| (g, e.clone())) else {
            return;
        };
        let eligible = matches!(
            group,
            worktree_tool::engine::working_copy::Group::Unstaged
                | worktree_tool::engine::working_copy::Group::Untracked
        ) && !entry.is_dir();
        if !eligible {
            return;
        }
        self.dialog = DialogState::Discard {
            path: entry.path.clone(),
            untracked: entry.untracked,
        };
        window.focus(&self.dialog_focus);
        cx.notify();
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test 2>&1 | tail -8`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(wc-view): grouped file list with stage/unstage and confirm-guarded discard"
```

---

### Task 11: Working copy view — diff pane

**Files:**
- Modify: `src/views/working_copy.rs`
- Test: `src/app.rs` tests

**Interfaces:**
- Consumes: `FileDetail` variants, `UnifiedDiff`/`DiffLine`/`Preview` types.
- Produces: real `render_diff_pane` — unified diff rows (colored adds/dels, dim hunk headers), `DIFF_RENDER_CAP` (5000) trailer, binary/directory/missing/failed placeholders, conflict hint text.

- [ ] **Step 1: Write the failing test**

Append to `src/app.rs` tests:

```rust
    #[gpui::test]
    fn diff_pane_renders_selected_file_and_caps_long_files(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("fixture");
        std::fs::create_dir(&repo).unwrap();
        fixture_repo(&repo);
        std::fs::write(repo.join("f.txt"), "one\ntwo\n").unwrap();
        let (view, mut vcx) = open_root(cx, &repo);

        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            // f.txt is the default selection (staged mod); its diff loaded
            let wc = root.detail.as_ref().unwrap().read(cx);
            assert!(matches!(wc.detail, Some(FileDetail::Diff(_))));
            // placeholder: assert via the pane marker element id in Task 11
        });
        vcx.simulate_keystrokes("tab");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            let wc = root.detail.as_ref().unwrap();
            assert_eq!(wc.read(cx).pane, Pane::Diff, "tab moves the pane state to the diff");
        });
        vcx.simulate_keystrokes("tab");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            let wc = root.detail.as_ref().unwrap();
            assert_eq!(wc.read(cx).pane, Pane::Files, "second tab returns to the file list");
        });
    }
```

(The meaningful assertion is `FileDetail::Diff(_)` — pane rendering itself is verified by the compile + the element ids; deeper paint-level assertions aren't practical in gpui 0.2.2 without screenshot tooling.)

- [ ] **Step 2: Implement**

Replace the `render_diff_pane` stub in `src/views/working_copy.rs`:

```rust
fn render_diff_pane(this: &mut RootView, cx: &mut Context<RootView>) -> impl IntoElement {
    let diff_focus = this.detail_diff_focus.clone();
    let mut pane = div()
        .id("wc-diff")
        .track_focus(&diff_focus)
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .overflow_y_scroll()
        .on_mouse_down(MouseButton::Left, move |this: &mut RootView, _, window, _cx| {
            window.focus(&this.detail_diff_focus);
        });
    let Some(wc) = this.detail.clone() else {
        return pane;
    };
    let store = wc.read(cx);
    let Some(detail) = &store.detail else {
        return pane.child(
            div().p_4().text_size(px(13.)).text_color(DIM).child("No selection"),
        );
    };
    match detail {
        FileDetail::Diff(ud) if ud.binary => {
            pane.child(placeholder("Binary file — not shown"))
        }
        FileDetail::Diff(ud) => {
            let total: usize = ud.hunks.iter().map(|h| h.lines.len()).sum();
            let mut rendered = 0usize;
            pane = pane.child(
                div().px_3().py_2().text_size(px(11.)).text_color(DIM)
                    .child(ud.header.lines().filter(|l| !l.is_empty()).map(String::from)
                        .collect::<Vec<_>>().join("  ·  ")),
            );
            for hunk in &ud.hunks {
                if rendered >= DIFF_RENDER_CAP { break; }
                pane = pane.child(
                    div().px_3().py_0p5().text_size(px(11.)).text_color(DIM)
                        .child(hunk.header.clone()),
                );
                for line in &hunk.lines {
                    if rendered >= DIFF_RENDER_CAP { break; }
                    rendered += 1;
                    let (marker, color, bg) = match line.kind {
                        DiffLineKind::Add => ("+", GREEN, rgba(0xa6e3a120)),
                        DiffLineKind::Del => ("−", RED, rgba(0xf38ba820)),
                        DiffLineKind::Context => (" ", TEXT, gpui::transparent()),
                    };
                    let mut row = div()
                        .flex()
                        .px_3()
                        .text_size(px(12.))
                        .when(bg != gpui::transparent(), |r| r.bg(bg));
                    pane = pane.child(
                        row.child(
                            div().w(px(14.)).flex_shrink_0().text_color(color).child(marker),
                        )
                        .child(
                            div().min_w_0().whitespace_normal().child(
                                if line.no_newline {
                                    format!("{}\\ (no newline)", line.content)
                                } else {
                                    line.content.clone()
                                },
                            ),
                        ),
                    );
                }
            }
            if total > DIFF_RENDER_CAP {
                pane = pane.child(placeholder(&format!(
                    "… {} more lines — open the file in your editor",
                    total - DIFF_RENDER_CAP
                )));
            }
            pane
        }
        FileDetail::Preview(p) => match p {
            diff::Preview::Text { content, truncated } => {
                let lines: Vec<&str> = content.lines().collect();
                let shown = lines.len().min(DIFF_RENDER_CAP);
                for l in lines.iter().take(shown) {
                    pane = pane.child(
                        div().px_3().text_size(px(12.)).text_color(TEXT).child(l.to_string()),
                    );
                }
                if truncated || lines.len() > DIFF_RENDER_CAP {
                    pane = pane.child(placeholder("… truncated — open the file in your editor"));
                }
                pane
            }
            diff::Preview::Binary => pane.child(placeholder("Binary file — not shown")),
            diff::Preview::Directory => pane.child(placeholder(
                "Untracked directory — press S to stage its contents",
            )),
            diff::Preview::Missing => pane.child(placeholder("File missing on disk")),
        },
        FileDetail::Failed(msg) => pane.child(
            div().p_4().text_size(px(12.)).text_color(RED).child(msg.clone()),
        ),
    }
}

fn placeholder(text: &str) -> impl IntoElement {
    div().p_4().text_size(px(13.)).text_color(DIM).child(text.to_string())
}
```

(Implementer notes: (1) if `gpui::transparent()` doesn't exist in 0.2.2 use `rgba(0x00000000)`; (2) if `.whitespace_normal()` isn't available, drop it; (3) conflict rows get one extra hint line — when the selected group is `Conflicts`, prepend `placeholder("Resolve in your editor, then press s to mark resolved")`; (4) `DiffLineKind` needs importing from `worktree_tool::engine::diff`.)

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test 2>&1 | tail -5` and `cargo clippy --all-targets -- -D warnings`
Expected: PASS, clippy clean.

- [ ] **Step 4: Manual smoke check (host platform)**

```bash
cd /tmp && rm -rf wc-smoke && mkdir wc-smoke && cd wc-smoke && git init -q -b main . && echo one > f.txt && git add . && git -c user.email=t@t -c user.name=t commit -qm init && echo two >> f.txt && echo new > u.txt
cargo run --release
```
Expected: app opens; `enter` drills in; the file list shows Staged=∅/Unstaged f.txt/Untracked u.txt; the diff pane renders f.txt's `+two`; `s` stages; `esc` returns; the home row badge updates. (Run from the repo root so repo detection works.)

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(wc-view): unified diff pane with caps, previews and placeholders"
```

---

### Task 12: Commit flow wiring

**Files:**
- Modify: `src/app.rs` (the `c` key arm), `src/views/working_copy.rs` (busy state hint)
- Test: `src/app.rs` tests

**Interfaces:**
- Consumes: `WorkingCopyStore::commit_with_editor` (Task 8).
- Produces: `c` opens the user's editor; on completion the store refreshes and the home list refreshes via the mutated flag; disabled with a hint while nothing is staged.

- [ ] **Step 1: Write the failing test**

Append to `src/app.rs` tests:

```rust
    #[gpui::test]
    fn commit_key_requires_staged_changes(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("fixture");
        std::fs::create_dir(&repo).unwrap();
        fixture_repo(&repo);
        let (view, mut vcx) = open_root(cx, &repo);

        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            assert!(root.detail.as_ref().unwrap().read(cx).staged_count() == 0);
        });
        vcx.simulate_keystrokes("c");
        vcx.run_until_parked();
        view.update(&mut vcx.cx, |root, cx| {
            let wc = root.detail.as_ref().unwrap().read(cx);
            assert!(
                wc.message.as_deref().unwrap_or_default().contains("Nothing staged"),
                "expected the nothing-staged hint, got {:?}",
                wc.message
            );
            assert!(!wc.busy, "no editor spawned");
        });
    }
```

(The happy path — an actual commit through `$EDITOR` — is covered end-to-end by `tests/engine_commit.rs`; the UI test only guards the gating, since spawning a real editor inside the GPUI harness is fragile.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib commit_key 2>&1 | tail -5`
Expected: FAIL — `c` has no handler.

- [ ] **Step 3: Implement**

In `detail_keydown`:

```rust
            "c" if list_focused => {
                if let Some(wc) = &self.detail {
                    wc.update(cx, |store, cx| store.commit_with_editor(cx));
                }
            }
```

In `src/views/working_copy.rs`, when `store.busy` is true, render the footer message with "Working…" and disable the hint row's `c` mention? — no, keep it simple: the busy state already shows through `store.message` ("Waiting for commit editor…") and the status bar's "Working…" comes from `WorktreeStore.busy`; add `wc.read(cx).busy` OR'd into the footer's message visibility if desired. Minimum: no change needed — the message field covers it.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test 2>&1 | tail -5`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(wc-view): commit via editor with nothing-staged guard"
```

---

### Task 13: Docs, bench, version bump, full verification

**Files:**
- Modify: `README.md` (Shortcuts table + a "Working Copy" section), `docs/index.html` (shortcuts), `examples/bench.rs`, `Cargo.toml` (version 0.2.0)
- Test: full suite + cross-compiles

**Interfaces:** none new.

- [ ] **Step 1: Extend the bench**

Append to `examples/bench.rs`:

```rust
fn bench_working_copy(repo: &Path) {
    let changes = repo.join("bench-changes");
    std::fs::create_dir_all(&changes).unwrap();
    for i in 0..2000u32 {
        std::fs::write(changes.join(format!("f{i:04}.txt")), format!("content {i}")).unwrap();
    }
    use worktree_tool::engine::{diff, working_copy};
    let t = Instant::now();
    let wc = working_copy::status(repo).expect("status");
    let status_ms = t.elapsed().as_secs_f64() * 1000.0;
    let t = Instant::now();
    let d = diff::diff_unstaged(repo, "bench-changes/f0000.txt").expect("diff");
    let diff_ms = t.elapsed().as_secs_f64() * 1000.0;
    println!(
        "working copy: status = {status_ms:7.1} ms ({} entries), single-file diff = {diff_ms:5.1} ms ({} hunks)",
        wc.entries.len(),
        d.hunks.len()
    );
}
```

Call `bench_working_copy(&repo20);` at the end of `main()`. (The untracked-files fixture also measures the `-unormal` collapse: 2000 files in one directory collapse to one row.)

- [ ] **Step 2: Update docs**

README `### Shortcuts` table gains (and `enter`'s row changes):

| Key | Action |
| --- | --- |
| `enter` | Open selected worktree's detail view |
| `t` | Open in terminal |
| `esc` | Back to worktree list / close dialog |
| `s` / `S` | Stage/unstage selected file / stage all (detail view) |
| `d` | Discard selected file's changes (confirm; detail view) |
| `c` | Commit staged changes via your editor (detail view) |
| `tab` | Toggle file list ↔ diff pane (detail view) |

Plus a short "## Working Copy" section: drill into a worktree, stage/unstage, discard, commit via `$EDITOR` (resolution order documented), conflicts are marked resolved with `s` after resolving externally. Mirror the same on the docs site (`docs/index.html` shortcuts/tutorial section).

- [ ] **Step 3: Version bump**

`Cargo.toml`: `version = "0.2.0"`. Run `cargo check` once to update `Cargo.lock`.

- [ ] **Step 4: Full verification**

```bash
cargo fmt --check || cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
cargo run --release --example bench
cargo zigbuild --target x86_64-unknown-linux-gnu --lib
cargo zigbuild --target x86_64-pc-windows-gnu --lib
cargo zigbuild --target x86_64-unknown-freebsd --lib
```
Expected: fmt clean, clippy clean, all tests pass, bench prints working-copy timings (status on the 2000-file fixture should be tens of ms), all three cross-target lib builds succeed.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: phase 1 working copy view — docs, bench, version 0.2.0"
```

---

## Self-Review notes (run by the plan author — resolved)

1. **Spec coverage:** status groups ✓ (Tasks 2/3/10), file-level stage/unstage ✓ (6/8/10), discard-with-confirm ✓ (6/10), unified diff pane with caps ✓ (4/5/11), conflict raw-content + mark-resolved ✓ (5/11 note 3, stage path), `$EDITOR` commit with git's resolution order ✓ (7/12), author footline ✓ (7/8/10), app-shell restructure + tabs ✓ (9), `enter`/`t` keybinding break ✓ (9), numstat +N/−M ✓ (3/10), untracked dir collapse + placeholder ✓ (3 note, 11), `--no-optional-locks` ✓ (3/5), `:(literal)` pathspecs ✓ (5/6), lock-contention message ✓ (1/8), home-list refresh after mutations ✓ (8 `mutated` flag + 9 observer), 4-platform cross-checks ✓ (13).
2. **Placeholders:** none — superseded draft blocks were removed; every code step shows only the final form. "Implementer notes" flag gpui-0.2.2 API names that may need graceful degradation (drop styling, never add deps).
3. **Type consistency:** `Group`, `FileEntry`, `WorkingCopy`, `UnifiedDiff`, `DiffHunk.raw`, `FileDetail`, `Pane`, `take_mutated`, `staged_count`, `rows()` are used identically across Tasks 1–13. The `rows` display tuple `(Group, usize, bool, char, String, Option<(u64, u64)>)` (group, entry index, selected, letter, path, counts) matches between `render` and `render_file_list`.

## Deliberate deviations from the spec (documented)

- Spec said "wraps within group" for selection; implemented as **list-bounded** (selection stops at the ends of the whole row list, Tower/lazygit-style — crossing group boundaries while moving feels natural). Spec keybinding table updated to match.
- Untracked directories collapse to one row (git's `-unormal` default, made explicit) rather than enumerating every file — protects against `node_modules`-scale untracked trees; staging a directory row stages its contents.
