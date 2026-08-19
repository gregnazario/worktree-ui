# GPUI Worktree Manager — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A native GPUI (Rust) GUI app that lists, creates, removes, and prunes git worktrees for a single repository, with per-worktree status, filtering, and quick-open actions.

**Architecture:** Pure data/parsing in `model.rs`, async git CLI wrapper in `git.rs` (smol processes, no GPUI deps), a GPUI `WorktreeStore` entity orchestrating background refresh, and a view layer (`ui.rs`, `dialogs.rs`, `text_field.rs`) rendering from that store. All git work runs on GPUI's background executor.

**Tech Stack:** Rust 2024 edition, `gpui = "=0.2.2"` (crates.io, Metal backend on macOS), `smol` for async process spawning, `tempfile` (dev) for integration fixtures.

## Global Constraints

- `gpui` pinned EXACTLY to `=0.2.2` (pre-1.0 crate; no caret ranges).
- `git.rs`/`model.rs` must not depend on `gpui` (headless-testable).
- All git porcelain parsing is lenient: unknown lines are ignored, never an error.
- Git errors surface in the UI status bar (one line, stderr verbatim); no panics on git failures.
- Render signature (0.2.2): `fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement`.
- Async closures for spawn: `cx.spawn(async move |this: WeakEntity<T>, cx: &mut AsyncApp| { ... })`.
- Background futures must be `Send`: `cx.background_executor().spawn(async move { ... })`.
- Commits: plain messages, no AI attribution trailers, on branch `feat/gpui-worktree-manager` (never `main`).
- Verified 0.2.2 APIs used throughout (from the crate source): `Application::new().run`, `cx.open_window(WindowOptions{..}, |window, cx| cx.new(..))`, `actions!`, `KeyBinding::new`, `cx.bind_keys`, `div().id().track_focus().on_key_down(cx.listener(...)).on_action(...)`, `Keystroke { key, key_char, modifiers }`, `ClipboardItem::new_string`, `cx.write_to_clipboard`, `cx.quit()`, `cx.on_window_closed`, `Element::id` required before interactive handlers.

---

### Task 1: Scaffold + hello-window build gate

**Files:**
- Create: `Cargo.toml` (edit), `src/main.rs` (replace), `.gitignore`, `README.md`

**Interfaces:**
- Produces: a compiling GPUI window; branch `feat/gpui-worktree-manager` already initialized (done during research).

- [ ] **Step 1: Write Cargo.toml, .gitignore, main.rs, README**

`Cargo.toml`:

```toml
[package]
name = "worktree-tool"
version = "0.1.0"
edition = "2021"

[dependencies]
gpui = "=0.2.2"
smol = "2"

[dev-dependencies]
tempfile = "3"
```

`.gitignore`:

```
/target
```

`src/main.rs`:

```rust
use gpui::{App, Application, Context, Window, WindowBounds, WindowOptions};

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = gpui::Bounds {
            origin: gpui::Point::default(),
            size: gpui::size(px(960.), px(640.)),
        };
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        };
        cx.open_window(options, |_, cx| {
            cx.new(|_| Hello(""))
        })
        .unwrap();
    });
}

struct Hello(String);

impl gpui::Render for Hello {
    fn render(&mut self, _w: &mut Window, _cx: &mut Context<Self>) -> impl gpui::IntoElement {
        gpui::div()
            .flex()
            .justify_center()
            .items_center()
            .size_full()
            .bg(gpui::rgb(0x1e1e2e))
            .text_color(gpui::rgb(0xcdd6f4))
            .child("worktree-tool")
    }
}
```

`README.md`: one-paragraph description (GPU git worktree manager, GPUI, macOS v1), build/run/test commands (`cargo run --release`, `cargo test`), note that `git` must be on PATH.

- [ ] **Step 2: Build gate**

Run: `cargo build 2>&1 | tail -5`
Expected: `Finished` (first build takes minutes: blade shaders + font stack). Fix any API mismatches against `~/.cargo/registry/src/*/gpui-0.2.2/src/` before proceeding — this gate validates every Global Constraint API assumption.

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m "chore: scaffold gpui app with build gate"
```

---

### Task 2: `model.rs` — worktree list data + porcelain parser

**Files:**
- Create: `src/model.rs`
- Test: `src/model.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Produces:

```rust
pub struct WorktreeEntry {
    pub path: PathBuf,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub is_main: bool,
    pub status: WorktreeStatus,
}
#[derive(Clone, Debug, PartialEq)]
pub enum WorktreeStatus { Pending, Unavailable(String), Clean { ahead: u32, behind: u32 }, Dirty { staged: u32, unstaged: u32, untracked: u32, ahead: u32, behind: u32 } }
pub fn parse_worktree_porcelain(input: &str) -> Vec<WorktreeEntry>;
pub fn sanitize_branch(branch: &str) -> String;        // '/' -> '-'
pub fn default_worktree_path(repo_root: &Path, branch: &str) -> PathBuf;
pub fn matches_filter(entry: &WorktreeEntry, filter: &str) -> bool; // case-insensitive substring on branch+path
```

- [ ] **Step 1: Write failing tests** (inline in `src/model.rs`)

Fixture for `git worktree list --porcelain` (blocks separated by blank lines; `bare`, `detached`, `branch refs/heads/x` lines):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = "worktree /Users/greg/git/myrepo\nHEAD abc123\nbranch refs/heads/main\n\nworktree /Users/greg/git/myrepo-worktrees/feature-x\nHEAD def456\nbranch refs/heads/feature-x\n\nworktree /Users/greg/git/myrepo-worktrees/det\nHEAD 789abc\ndetached\n";

    #[test]
    fn parses_main_linked_and_detached() {
        let entries = parse_worktree_porcelain(LIST);
        assert_eq!(entries.len(), 3);
        assert!(entries[0].is_main);
        assert_eq!(entries[0].branch.as_deref(), Some("main"));
        assert_eq!(entries[1].branch.as_deref(), Some("feature-x"));
        assert!(!entries[1].is_main);
        assert_eq!(entries[2].branch, None); // detached
        assert!(entries.iter().all(|e| matches!(e.status, WorktreeStatus::Pending)));
    }

    #[test]
    fn ignores_unknown_lines_and_trailing_blank() {
        let entries = parse_worktree_porcelain(&format!("{LIST}\nworktree /x\nlocked\nprunable\n"));
        assert_eq!(entries.len(), 4);
    }

    #[test]
    fn sanitizes_slashes_in_branch_names() {
        assert_eq!(sanitize_branch("feature/x-y"), "feature-x-y");
    }

    #[test]
    fn derives_default_path_next_to_repo() {
        let p = default_worktree_path(Path::new("/Users/greg/git/myrepo"), "feature/x");
        assert_eq!(p, PathBuf::from("/Users/greg/git/myrepo-worktrees/feature-x"));
    }

    #[test]
    fn filter_matches_branch_and_path_case_insensitively() {
        let e = WorktreeEntry { path: "/a/b/Feature-X".into(), branch: Some("feat".into()), head: None, is_main: false, status: WorktreeStatus::Pending };
        assert!(matches_filter(&e, "FEAT"));
        assert!(matches_filter(&e, "/a/b/"));
        assert!(!matches_filter(&e, "zzz"));
    }
}
```

- [ ] **Step 2: Run tests, verify they fail**

Run: `cargo test model` → Expected: compile error (functions not defined).

- [ ] **Step 3: Implement**

```rust
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub enum WorktreeStatus {
    Pending,
    Unavailable(String),
    Clean { ahead: u32, behind: u32 },
    Dirty { staged: u32, unstaged: u32, untracked: u32, ahead: u32, behind: u32 },
}

#[derive(Clone, Debug)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub is_main: bool,
    pub status: WorktreeStatus,
}

pub fn parse_worktree_porcelain(input: &str) -> Vec<WorktreeEntry> {
    let mut entries = Vec::new();
    let mut current: Option<WorktreeEntry> = None;
    for line in input.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            if let Some(e) = current.take() { entries.push(e); }
            continue;
        }
        if let Some(rest) = line.strip_prefix("worktree ") {
            if let Some(e) = current.take() { entries.push(e); }
            current = Some(WorktreeEntry { path: rest.into(), head: None, branch: None, is_main: false, status: WorktreeStatus::Pending });
        } else if let Some(rest) = line.strip_prefix("HEAD ") {
            if let Some(e) = current.as_mut() { e.head = Some(rest.to_string()); }
        } else if let Some(rest) = line.strip_prefix("branch ") {
            if let Some(e) = current.as_mut() { e.branch = Some(rest.trim_start_matches("refs/heads/").to_string()); }
        } else if line == "detached" || line == "bare" || line.starts_with("locked") || line.starts_with("prunable") {
            // recognized, nothing to record for v1
        }
    }
    if let Some(e) = current.take() { entries.push(e); }
    // first entry in git output is the main worktree
    if let Some(first) = entries.first_mut() { first.is_main = true; }
    entries
}

pub fn sanitize_branch(branch: &str) -> String {
    branch.replace('/', "-")
}

pub fn default_worktree_path(repo_root: &Path, branch: &str) -> PathBuf {
    let repo_name = repo_root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    repo_root.parent().unwrap_or(Path::new("."))
        .join(format!("{repo_name}-worktrees"))
        .join(sanitize_branch(branch))
}

pub fn matches_filter(entry: &WorktreeEntry, filter: &str) -> bool {
    if filter.is_empty() { return true; }
    let f = filter.to_lowercase();
    entry.branch.as_deref().map(|b| b.to_lowercase().contains(&f)).unwrap_or(false)
        || entry.path.to_string_lossy().to_lowercase().contains(&f)
}
```

Wire into `main.rs`: `mod model;`

- [ ] **Step 4: `cargo test model` → PASS. Commit:** `git add -A && git commit -m "feat: worktree list model with porcelain parser"`

---

### Task 3: `model.rs` — status porcelain v2 parser

**Interfaces (added to model.rs):**
- `pub fn parse_status_porcelain_v2(input: &str) -> WorktreeStatus;`

- [ ] **Step 1: Failing tests** — fixture lines: header `# branch.head main`, `# branch.ab +1 -2`, change entries `1 .M N... path` (unstaged mod), `1 M. ...` (staged mod), `? untracked`, `2 R. ...` (rename counts staged). Assert `Dirty { staged: 2, unstaged: 1, untracked: 1, ahead: 1, behind: 2 }`; no change lines → `Clean { ahead: 1, behind: 2 }`; `# branch.head (detached)` still parses; empty input → `Clean { ahead: 0, behind: 0 }`.

- [ ] **Step 2: Run `cargo test model` → new tests fail.**

- [ ] **Step 3: Implement:**

```rust
pub fn parse_status_porcelain_v2(input: &str) -> WorktreeStatus {
    let (mut staged, mut unstaged, mut untracked) = (0u32, 0u32, 0u32);
    let (mut ahead, mut behind) = (0u32, 0u32);
    for line in input.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("# branch.ab ") {
            let mut parts = rest.split(' ');
            if let Some(a) = parts.next() { ahead = a.trim_start_matches('+').parse().unwrap_or(0); }
            if let Some(b) = parts.next() { behind = b.trim_start_matches('-').parse().unwrap_or(0); }
        } else if let Some(rest) = line.strip_prefix("? ") {
            let _ = rest; untracked += 1;
        } else if line.starts_with("1 ") || line.starts_with("2 ") || line.starts_with("u ") {
            let mut fields = line.split(' ');
            let _ordinal = fields.next();
            if let Some(xy) = fields.next() {
                let x = xy.chars().next().unwrap_or('.');
                let y = xy.chars().nth(1).unwrap_or('.');
                if x != '.' && x != '!' { staged += 1; }
                if y != '.' && y != '!' { unstaged += 1; }
            }
        } // "# ..." headers and "3 "/"?" unknown forms ignored
    }
    if staged + unstaged + untracked == 0 { WorktreeStatus::Clean { ahead, behind } }
    else { WorktreeStatus::Dirty { staged, unstaged, untracked, ahead, behind } }
}
```

- [ ] **Step 4: `cargo test model` → PASS. Commit:** `git commit -am "feat: status porcelain v2 parser"`

---

### Task 4: `git.rs` — repo detection, list, status pass

**Files:**
- Create: `src/git.rs`, `tests/git_integration.rs`

**Interfaces:**
- Consumes: `model::{parse_worktree_porcelain, parse_status_porcelain_v2, WorktreeEntry, WorktreeStatus}`.
- Produces:

```rust
pub struct GitError { pub message: String }     // Display = message
pub type Result<T> = std::result::Result<T, GitError>;
pub async fn run_git(cwd: Option<&Path>, args: &[&str]) -> Result<String>; // trimmed stdout; Err carries stderr tail
pub async fn repo_root(start: &Path) -> Result<PathBuf>;                   // git rev-parse --show-toplevel
pub async fn list_worktrees(root: &Path) -> Result<Vec<WorktreeEntry>>;    // porcelain parsed; first entry is_main
pub async fn status_pass(entries: Vec<WorktreeEntry>) -> Vec<WorktreeEntry>; // concurrent per-entry status; failures -> Unavailable
```

- [ ] **Step 1: Write failing integration tests** (`tests/git_integration.rs`) — build a fixture repo helper used by Tasks 4–5:

```rust
use std::path::Path;
use worktree_tool::git;
use worktree_tool::model::WorktreeStatus;

fn sh(cwd: Option<&Path>, cmd: &[&str]) {
    let status = std::process::Command::new(cmd[0])
        .args(&cmd[1..]).current_dir(cwd.unwrap_or(Path::new(".")))
        .status().expect("spawn");
    assert!(status.success(), "failed: {cmd:?}");
}

fn fixture_repo(dir: &Path) {
    sh(Some(dir), &["git", "init", "-q", "-b", "main"]);
    sh(Some(dir), &["git", "config", "user.email", "t@t"]); 
    sh(Some(dir), &["git", "config", "user.name", "t"]);
    std::fs::write(dir.join("f.txt"), "one").unwrap();
    sh(Some(dir), &["git", "add", "."]);
    sh(Some(dir), &["git", "commit", "-qm", "init"]);
}

#[test]
fn detects_repo_root_and_lists_worktrees() {
    smol::block_on(async {
        let tmp = tempfile::tempdir().unwrap();
        fixture_repo(tmp.path());
        let sub = tmp.path().join("nested"); std::fs::create_dir(&sub).unwrap();
        let root = git::repo_root(&sub).await.unwrap();
        assert_eq!(root.canonicalize().unwrap(), tmp.path().canonicalize().unwrap());
        let entries = git::list_worktrees(&root).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].is_main);
        assert_eq!(entries[0].branch.as_deref(), Some("main"));
    });
}

#[test]
fn status_pass_marks_dirty_and_unavailable() {
    smol::block_on(async {
        let tmp = tempfile::tempdir().unwrap();
        fixture_repo(tmp.path());
        std::fs::write(tmp.path().join("f.txt"), "changed").unwrap();
        let mut entries = git::list_worktrees(tmp.path()).await.unwrap();
        entries.push(worktree_tool::model::WorktreeEntry { path: tmp.path().join("nope"), head: None, branch: None, is_main: false, status: WorktreeStatus::Pending });
        let done = git::status_pass(entries).await;
        assert!(matches!(done[0].status, WorktreeStatus::Dirty { unstaged: 1, .. }));
        assert!(matches!(done[1].status, WorktreeStatus::Unavailable(_)));
    });
}

#[test]
fn run_git_reports_stderr_on_failure() {
    smol::block_on(async {
        let err = git::run_git(None, &["rev-parse", "--show-toplevel"]).unwrap_err();
        assert!(!err.message.is_empty());
    });
}
```

Note: `run_git(None, ...)` at a non-repo cwd — run from the test's own cwd (inside the worktree-tool repo that IS a git repo!). Fix: `run_git(Some(tmp.path()), ...)` with tmp outside any repo. `tempfile::tempdir()` is under `$TMPDIR` (not a repo) — use `Some(tmp.path())`.

- [ ] **Step 2: `cargo test --test git_integration` → compile fail (git module missing).**

- [ ] **Step 3: Implement `src/git.rs`:**

```rust
use crate::model::{parse_status_porcelain_v2, parse_worktree_porcelain, WorktreeEntry, WorktreeStatus};
use smol::process::{Command, Stdio};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct GitError { pub message: String }
impl std::fmt::Display for GitError { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.message) } }
pub type Result<T> = std::result::Result<T, GitError>;

pub async fn run_git(cwd: Option<&Path>, args: &[&str]) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(dir) = cwd { cmd.current_dir(dir); }
    let output = cmd.output().await.map_err(|e| GitError { message: format!("failed to run git: {e}") })?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim_end().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().lines().last().unwrap_or("git failed").to_string();
        Err(GitError { message: stderr })
    }
}

pub async fn repo_root(start: &Path) -> Result<PathBuf> {
    Ok(PathBuf::from(run_git(Some(start), &["rev-parse", "--show-toplevel"]).await?))
}

pub async fn list_worktrees(root: &Path) -> Result<Vec<WorktreeEntry>> {
    let out = run_git(Some(root), &["worktree", "list", "--porcelain"]).await?;
    Ok(parse_worktree_porcelain(&out))
}

pub async fn status_pass(entries: Vec<WorktreeEntry>) -> Vec<WorktreeEntry> {
    let futures: Vec<_> = entries.into_iter().map(|mut e| async move {
        match run_git(Some(&e.path), &["status", "--porcelain=v2", "--branch"]).await {
            Ok(out) => e.status = parse_status_porcelain_v2(&out),
            Err(err) => e.status = WorktreeStatus::Unavailable(err.message),
        }
        e
    }).collect();
    futures::future::join_all(futures).await
}
```

Add `futures = "0.3"` to `[dependencies]` (smol pulls it; needed by `join_all`). Export from `main.rs`: `pub mod git; pub mod model;` (make crate lib-visible to integration tests by adding `src/lib.rs` with `pub mod git; pub mod model; pub mod platform;` and keeping `main.rs` binary using `worktree_tool::...`). Simplest: create `src/lib.rs`, move module decls there; `main.rs` references `use worktree_tool::{...}`.

- [ ] **Step 4: `cargo test` → PASS (unit + integration). Commit:** `git commit -am "feat: git wrapper with repo detection, listing, concurrent status pass"`

---

### Task 5: `git.rs` — mutations + branch metadata

**Interfaces (added):**
- `pub async fn add_worktree(root: &Path, path: &Path, branch: Option<&str>, base: &str) -> Result<()>;` — `branch: Some(name)` → `-b name base`; `None` → check out `base` as existing branch.
- `pub async fn remove_worktree(root: &Path, path: &Path, force: bool) -> Result<()>;`
- `pub async fn prune(root: &Path) -> Result<()>;`
- `pub async fn local_branches(root: &Path) -> Result<Vec<String>>;`
- `pub async fn default_branch(root: &Path) -> String;` — `symbolic-ref refs/remotes/origin/HEAD` basename, else `main` if exists else `master` else `main`.

- [ ] **Step 1: Failing integration tests** (append to `tests/git_integration.rs`): create worktree for new branch → assert dir exists + `list_worktrees` len 2; create for existing branch; remove non-forced fails after touching a file (dirty) and succeeds with `force` or after clean; prune removes stale entry after `fs::remove_dir_all` of a worktree dir; `local_branches` contains `main` + created branch; `default_branch` returns `main` (no remote configured).

```rust
#[test]
fn add_remove_prune_roundtrip() {
    smol::block_on(async {
        let tmp = tempfile::tempdir().unwrap();
        fixture_repo(tmp.path());
        let wt = tmp.path().join("wt-a");
        git::add_worktree(tmp.path(), &wt, Some("feat-a"), "main").await.unwrap();
        assert!(wt.join("f.txt").exists());
        assert_eq!(git::list_worktrees(tmp.path()).await.unwrap().len(), 2);
        assert!(git::local_branches(tmp.path()).await.unwrap().contains(&"feat-a".to_string()));

        std::fs::write(wt.join("new.txt"), "x").unwrap();
        assert!(git::remove_worktree(tmp.path(), &wt, false).await.is_err());
        git::remove_worktree(tmp.path(), &wt, true).await.unwrap();
        assert_eq!(git::list_worktrees(tmp.path()).await.unwrap().len(), 1);

        let wt2 = tmp.path().join("wt-b");
        git::add_worktree(tmp.path(), &wt2, Some("feat-b"), "main").await.unwrap();
        std::fs::remove_dir_all(&wt2).unwrap();
        git::prune(tmp.path()).await.unwrap();
        assert_eq!(git::list_worktrees(tmp.path()).await.unwrap().len(), 1);
        assert_eq!(git::default_branch(tmp.path()).await, "main");
    });
}
```

- [ ] **Step 2: Fail → implement:**

```rust
pub async fn add_worktree(root: &Path, path: &Path, branch: Option<&str>, base: &str) -> Result<()> {
    let path_str = path.to_string_lossy();
    let out = match branch {
        Some(branch) => run_git(Some(root), &["worktree", "add", &path_str, "-b", branch, base]).await,
        None => run_git(Some(root), &["worktree", "add", &path_str, base]).await,
    };
    out.map(|_| ())
}

pub async fn remove_worktree(root: &Path, path: &Path, force: bool) -> Result<()> {
    let path_str = path.to_string_lossy();
    let res = if force {
        run_git(Some(root), &["worktree", "remove", "--force", &path_str]).await
    } else {
        run_git(Some(root), &["worktree", "remove", &path_str]).await
    };
    res.map(|_| ())
}

pub async fn prune(root: &Path) -> Result<()> {
    run_git(Some(root), &["worktree", "prune"]).await.map(|_| ())
}

pub async fn local_branches(root: &Path) -> Result<Vec<String>> {
    let out = run_git(Some(root), &["branch", "--format=%(refname:short)"]).await?;
    Ok(out.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect())
}

pub async fn default_branch(root: &Path) -> String {
    if let Ok(head) = run_git(Some(root), &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]).await {
        if let Some(name) = head.rsplit('/').next() { if !name.is_empty() { return name.to_string(); } }
    }
    let branches = local_branches(root).await.unwrap_or_default();
    if branches.iter().any(|b| b == "main") { "main".into() }
    else if branches.iter().any(|b| b == "master") { "master".into() }
    else { "main".into() }
}
```

- [ ] **Step 3: `cargo test` → PASS. Commit:** `git commit -am "feat: worktree add/remove/prune and branch metadata"`

---

### Task 6: `platform.rs` — open terminal / reveal / copy path

**Files:**
- Create: `src/platform.rs` (+ `pub mod platform;` in `lib.rs`)

**Interfaces:**
- Produces: `pub fn open_in_terminal(path: &Path)`, `pub fn reveal_in_file_manager(path: &Path)`, `pub fn copy_path(path: &Path)` (all fire-and-forget `std::process::Command`; no Result — best effort).

- [ ] **Step 1: Implement + `#[cfg(test)]` path-derivation smoke test (commands not executed in tests):**

```rust
use std::path::Path;

#[cfg(target_os = "macos")]
pub fn open_in_terminal(path: &Path) {
    let _ = std::process::Command::new("open").arg("-a").arg("Terminal").arg(path).spawn();
}
#[cfg(all(unix, not(target_os = "macos")))]
pub fn open_in_terminal(path: &Path) {
    for term in ["xdg-terminal-exec", "gnome-terminal", "konsole", "xterm"] {
        let ok = std::process::Command::new(term).arg("--working-directory").arg(path).spawn().is_ok()
            || std::process::Command::new(term).arg(path).spawn().is_ok();
        if ok { return; }
    }
}
#[cfg(target_os = "windows")]
pub fn open_in_terminal(path: &Path) {
    let _ = std::process::Command::new("cmd").args(["/C", "start", "wt", "-d"]).arg(path).spawn()
        .or_else(|_| std::process::Command::new("cmd").args(["/C", "start", "cmd", "/K", "cd", "/D"]).arg(path).spawn());
}

#[cfg(target_os = "macos")]
pub fn reveal_in_file_manager(path: &Path) { let _ = std::process::Command::new("open").arg(path).spawn(); }
#[cfg(not(target_os = "macos"))]
pub fn reveal_in_file_manager(path: &Path) {
    #[cfg(target_os = "windows")]
    { let _ = std::process::Command::new("explorer").arg(path).spawn(); }
    #[cfg(all(unix, not(target_os = "macos")))]
    { let _ = std::process::Command::new("xdg-open").arg(path).spawn(); }
}

pub fn copy_path(path: &Path) -> crate::platform_bridge::copy_to_clipboard(path.to_string_lossy().into_owned())
```

`copy_path` needs GPUI's `App` — it does NOT belong here. Instead: `platform.rs` stays pure process-launching; clipboard is done inline in `ui.rs` via `cx.write_to_clipboard(ClipboardItem::new_string(...))`. Drop `copy_path` from this module.

- [ ] **Step 2: `cargo build` → clean. Commit:** `git commit -am "feat: platform open/reveal helpers"`

---

### Task 7: `text_field.rs` — reusable one-line input view

**Files:**
- Create: `src/text_field.rs`

**Interfaces:**
- Produces:

```rust
pub struct TextField { pub value: String, cursor: usize, placeholder: SharedString, pub focus_handle: FocusHandle, selected: bool /* for border tint when focused */ }
impl TextField {
    pub fn new(placeholder: &str, window: &mut Window, cx: &mut Context<Self>) -> Self; // creates focus_handle via cx.focus_handle(), focuses it
    pub fn set_value(&mut self, v: &str, cx: &mut Context<Self>);
}
impl Render for TextField { /* div with track_focus, on_key_down inserts chars / backspace / left-right / home-end, on_click focuses */ }
```

Key handling (verified 0.2.2 API): `cx.listener(move |this, event: &KeyDownEvent, window, cx| { let ks = &event.keystroke; ... })`. Insert when `ks.modifiers.control || ks.modifiers.meta` is false and `ks.key_char` or `ks.key` is a single char (`ks.key == "space"` → `' '`); `backspace` deletes before cursor; `left`/`right` move; `home`/`end` jump. Do NOT handle `enter`/`escape` (parents do, via bubbling). After mutation: `cx.notify()`. Focused state for styling: `self.focus_handle.is_focused(window)` checked in render.

Rendering: bordered rounded div, `min_w(px(180.))`, text child `value[..cursor] + "│" + value[cursor..]` (simple caret rendering; no blink in v1), placeholder in dim color when empty.

- [ ] **Step 1: Implement the view** (code above; full render fn ~70 lines).
- [ ] **Step 2: `cargo build` → clean** (visual check happens in Task 9 when it appears in the toolbar).
- [ ] **Step 3: Commit:** `git commit -am "feat: TextField input view with key event editing"`

---

### Task 8: `store.rs` — WorktreeStore entity

**Files:**
- Create: `src/store.rs`

**Interfaces:**
- Consumes: `git::*`, `model::*`, GPUI entity system.
- Produces:

```rust
pub struct WorktreeStore {
    pub repo_root: Option<PathBuf>,
    pub entries: Vec<WorktreeEntry>,
    pub filtered: Vec<usize>,              // indices into entries matching filter
    pub filter: String,
    pub selected: Option<usize>,           // index into filtered
    pub status_message: Option<String>,    // one-line error/info ("Refreshing…", stderr)
    pub busy: bool,                        // any op in flight
    pub last_refreshed: Option<std::time::Instant>,
}
impl WorktreeStore {
    pub fn new(cx: &mut App) -> Entity<Self>;                                 // repo_root from std::env::current_dir, detect via spawn
    pub fn load_repo(&mut self, root: PathBuf, cx: &mut Context<Self>);       // sets root, kicks refresh
    pub fn refresh(&mut self, cx: &mut Context<Self>);
    pub fn set_filter(&mut self, filter: String, cx: &mut Context<Self>);     // rebuilds filtered, clamps selected
    pub fn select(&mut self, idx: Option<usize>, cx: &mut Context<Self>);
    pub fn select_next/prev(&mut self, cx: &mut Context<Self>);
    pub fn selected_entry(&self) -> Option<&WorktreeEntry>;
    pub fn add(&mut self, path: PathBuf, branch: Option<String>, base: String, cx: &mut Context<Self>);
    pub fn remove(&mut self, path: PathBuf, force: bool, cx: &mut Context<Self>);
    pub fn prune(&mut self, cx: &mut Context<Self>);
}
```

Refresh pattern (the one spawn shape every method uses):

```rust
pub fn refresh(&mut self, cx: &mut Context<Self>) {
    let Some(root) = self.repo_root.clone() else { return };
    self.busy = true;
    self.status_message = Some("Refreshing…".into());
    cx.notify();
    cx.spawn(async move |this, cx| {
        let result = cx.background_executor().spawn(async move {
            let entries = git::list_worktrees(&root).await?;   // Result flows through
            Ok(git::status_pass(entries).await) as git::Result<Vec<WorktreeEntry>>
        }).await;
        this.update(cx, |store, cx| {
            store.busy = false;
            match result {
                Ok(mut entries) => {
                    store.last_refreshed = Some(std::time::Instant::now());
                    store.status_message = None;
                    store.set_entries(entries, cx);
                }
                Err(e) => store.status_message = Some(e.message),
            }
            cx.notify();
        })
        .ok();
    })
    .detach();
}
```

`set_entries` re-applies the current filter and keeps selection on the same path when possible. `add/remove/prune` follow the identical shape; on success they call `refresh`-equivalent inline (update + status "Created …"). Also `pub fn default_base(&mut self, cx)` — spawns `git::default_branch` and stores `pub default_base: String` ("main" until loaded); `pub fn local_branches: Vec<String>` loaded the same way after repo load (used by the create dialog).

- [ ] **Step 1: Implement.** Filter/selection logic is pure — extract `fn apply_filter(entries: &[WorktreeEntry], filter: &str, keep: Option<&Path>) -> (Vec<usize>, Option<usize>)` as a free function with inline unit tests (kept selected by path; filter narrowed widens set; empty filter = all).
- [ ] **Step 2: `cargo test` (filter unit tests) + `cargo build` → clean.**
- [ ] **Step 3: Commit:** `git commit -am "feat: WorktreeStore entity with background refresh and mutations"`

---

### Task 9: `ui.rs` — root view

**Files:**
- Create: `src/ui.rs`
- Modify: `src/main.rs` (window opens `RootView`; keymap bindings here or main)

**Interfaces:**
- Consumes: `Entity<WorktreeStore>`, `Entity<TextField>`, `platform::{open_in_terminal, reveal_in_file_manager}`.
- Produces: `pub struct RootView { store: Entity<WorktreeStore>, search: Entity<TextField>, search_focused: bool }` + `pub fn new(store, window, cx) -> Entity<RootView>`; actions:

```rust
actions!(worktree_tool, [NewWorktree, Refresh, Prune, OpenSelected, RemoveSelected, FocusSearch, Quit]);
```

Layout (render, ~200 lines): outer `v_flex().size_full().bg(BG)`;
1. toolbar `h_flex` — repo name + root path (dim), spacer, search `TextField` wrapped in a div with `.on_key_down` for `escape` (clear + unfocus), then buttons New/Refresh/Prune: `div().id("new-btn").px_3().py_1().rounded_md().bg(ACCENT).text_color(BLACK).label("New").on_click(cx.listener(...))`.
2. list: `v_flex().flex_1().overflow_y_scroll()` — rows: each `h_flex().id(row-id).px_3().py_2()` with branch (bold), path (dim, smaller), status badge right: main → "main", clean+ahead/behind → `↑n ↓n`, dirty → `● n changed` in warn color, Unavailable → "unavailable" in red. `.when(selected) bg(ROW_SELECTED)`; `.on_mouse_down(MouseButton::Left, select)`.
3. detail pane for `selected_entry()`: branch, path, status detail line; buttons Open in Terminal / Reveal / Copy Path (`cx.write_to_clipboard(ClipboardItem::new_string(path))`) / Remove….
4. status bar `h_flex` — `busy` → "Refreshing…", else last-refreshed secs ago; `status_message` right-aligned, error color, `text_size(px(12.))`.

Empty state (no repo): centered column "Open a repository", path `TextField` + Load button → validates via spawn `git::repo_root`, then `store.load_repo`.

Key handling: root div `.key_context("Root").track_focus(&root_focus).on_action(...)` for each action above; keyboard nav via `.on_key_down` on the list container: `up`/`down` → select_prev/next, `enter` → OpenSelected, `backspace`/`delete` → RemoveSelected. Search focus: `/` handled in root `on_key_down` when search not focused → `window.focus(&search.read(cx).focus_handle)`. Buttons must `.id(...)` before `.on_click` (0.2.2 requirement).

`main.rs` after this task:

```rust
fn main() {
    Application::new().run(|cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-n", NewWorktree, Some("Root")),
            KeyBinding::new("cmd-r", Refresh, Some("Root")),
            KeyBinding::new("enter", OpenSelected, Some("Root")), // note: only when search not focused — guard in handler
        ]);
        let window = cx.open_window(window_options(), |window, cx| {
            let store = WorktreeStore::new(cx);
            RootView::new(store, window, cx)
        }).unwrap();
        window.update(cx, |_, window, cx| {
            window.titlebar_title("Worktree Tool"); // verify exact method name in 0.2.2; fallback: TitlebarOptions.title
        }).unwrap();
        cx.on_window_closed(|cx| cx.quit());
        cx.on_action(|_: &Quit, cx| cx.quit());
    });
}
```

Guard: `OpenSelected` via keybinding fires also when the search field is focused (bubbling) — in the handler, check `!self.search.read(cx).focus_handle.is_focused(window)` before acting (Enter in the search field instead just keeps filter; optionally Enter focuses first row).

- [ ] **Step 1: Implement.** Build. Commit: `git commit -am "feat: root view with toolbar, list, detail pane, status bar"`
- [ ] **Step 2: Manual smoke (blocker for next task):** run against a scratch repo — create 2 worktrees + 1 dirty via CLI, `cargo run` from that repo via `cd` into a fixture and running the binary (`(cd /tmp/wt-fixture && cargo run --manifest-path …/Cargo.toml)`); confirm rows, filter, selection, refresh. Screenshot for the final report.

---

### Task 10: `dialogs.rs` — create + remove modals

**Files:**
- Create: `src/dialogs.rs` (+ wire into `RootView` as `enum Dialog { None, Create(CreateDialog), Remove(RemoveDialog) }` field; render overlays when active)

**Interfaces:**

```rust
pub struct CreateDialog { branch: Entity<TextField>, base: Entity<TextField>, dest: Entity<TextField>, new_branch: bool, branches: Vec<String>, store: Entity<WorktreeStore> }
pub struct RemoveDialog { entry_path: PathBuf, branch: Option<String>, dirty: bool, force: bool, store: Entity<WorktreeStore> }
```

- Create dialog: three labeled TextFields (Branch, Base, Destination) pre-filled from store (`default_base`, `default_worktree_path(root, branch)` live-updated on branch change via `cx.observe(&self.branch, ...)`), checkbox "Existing branch" toggles `new_branch` (Base field becomes Existing-branch picker listing `store.local_branches` — v1: still a text field with hint text listing branches). Buttons: Create (validates non-empty branch; disabled styling when busy) → `store.add(dest, new_branch.then(branch), base, cx)` → close. Cancel / `escape` closes.
- Remove dialog: shows branch/path + warning line when `dirty` ("contains uncommitted changes"), `force` checkbox ("Delete even if dirty"), Remove → `store.remove(path, force, cx)` → close.
- Overlay pattern: `RootView::render` wraps content: `if dialog active { div().size_full().bg(black_50pct).flex().justify_center().items_center().child(dialog_card) }` — dialog card `w(px(480.)).rounded_lg().bg(PANEL).p_4().shadow_lg()`, `z_index` above list (use `.z_10()` on overlay wrapper). Modal key handling: card div `.track_focus(dialog_focus).key_context("Dialog").on_key_down(escape → close, enter → confirm)`. When a dialog is open, RootView suppresses list keybindings (guard in root handlers: `if self.dialog.is_open() { return; }`).

- [ ] **Step 1: Implement both dialogs + RootView wiring (NewWorktree opens Create pre-filled; RemoveSelected opens Remove).**
- [ ] **Step 2: `cargo build`; manual smoke: create worktree "smoke-branch" from dialog, see it appear after refresh; remove it dirty (fails without force, checkbox + force succeeds). Commit:** `git commit -am "feat: create and remove worktree dialogs"`

---

### Task 11: Polish + final verification

- [ ] `cargo fmt`
- [ ] `cargo clippy -- -D warnings` — fix all.
- [ ] `cargo test` — all pass.
- [ ] Full manual pass on a fixture repo: list/status/filter/select/keyboard nav/create/remove/prune/open-in-terminal/reveal/copy/empty-state (run from non-repo cwd: `cd /tmp && cargo run --manifest-path ~/git/worktree-tool/Cargo.toml`) / dirty warnings / error surfacing (`git` absent path impossible to test locally; error path unit-covered).
- [ ] Update `README.md` (features, shortcuts table, screenshots path if taken).
- [ ] Commit: `git commit -am "chore: fmt, clippy, docs"` and leave branch `feat/gpui-worktree-manager` ready for review (do NOT merge to main; no remote exists yet).

## Self-review notes (already applied)

- Spec coverage: list/status→T2-4, create/remove/prune→T5/T10, filter→T2/T8, quick actions→T6/T9, keyboard→T9, responsive→T8 background executor, empty state→T9, error handling→T4(run_git stderr)+T8 status bar, tests→per-task TDD + integration + filter unit tests. Non-goals respected (no settings persistence, no diff view).
- API pins verified against `~/.cargo/registry/src/*/gpui-0.2.2/src/`: render takes `(Window, Context)`; `Context::spawn(AsyncFnOnce(WeakEntity<T>, &mut AsyncApp))`; `ElementInputHandler` NOT used (raw `on_key_down` chosen instead — IME limitation documented); `.id()` required before `on_click`; `Keystroke.key_char` for typed char; `ClipboardItem::new_string`; `on_window_closed`→`quit`.
- Type consistency: `WorktreeEntry`/`WorktreeStatus` field names identical across model/git/store/ui tasks; `apply_filter` shared by store tasks.
