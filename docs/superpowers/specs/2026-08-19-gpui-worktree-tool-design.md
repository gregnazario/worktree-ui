# Worktree Tool — Design

Date: 2026-08-19
Status: Approved for implementation (autonomous session — decisions documented below; user was not available for Q&A, so the recommended option was taken at each decision point)

## Summary

A native GUI application for managing git worktrees in a single repository,
built in Rust with [GPUI](https://gpui.rs) (Zed's GPU-accelerated UI
framework, consumed from crates.io). It lists every worktree with its branch
and status, creates and removes worktrees through a dialog flow, prunes stale
entries, and offers quick "open in terminal / reveal / copy path" actions.

## Goals

- List all worktrees of a repository: branch (or detached HEAD), path,
  main-vs-linked kind, dirty file count, ahead/behind vs upstream.
- Create a worktree from a new or existing branch, with a sensible default
  destination path.
- Remove a worktree with a confirmation step; surface git's refusal when the
  worktree is dirty; optional force removal.
- Prune stale worktree administrative records.
- Filter the list by typing (case-insensitive substring match on branch +
  path).
- Quick actions per worktree: Open in Terminal, Reveal in Finder, Copy Path.
- Keyboard-first: `n` new, `/` focus search, `r` refresh, `Enter` open in
  terminal, `Backspace/Delete` remove selected, `Esc` clear selection/close
  dialog.
- Responsive UI: every git operation runs off the main thread.

## Non-goals (v1)

- Multi-repository dashboard.
- Diff viewing, file browsing inside a worktree.
- Settings files / configuration persistence (sensible defaults only).
- Windows/Linux polish (it compiles cross-platform via GPUI's backends, but
  v1 targets and is tested on macOS; terminal/folder opening is abstracted
  behind a small platform module).

## Decision log (approaches considered)

### Git access: shell out to the `git` CLI — chosen

| Option | Verdict |
|---|---|
| `git` subprocess (porcelain output) | **Chosen.** `git worktree` subcommands are the source of truth; porcelain formats are stable contracts; zero FFI; trivially async. Requires `git` on `PATH` (safe assumption on a dev machine). |
| `git2-rs` (libgit2) | Worktree add/remove/prune APIs are awkward and less complete than the CLI; heavier FFI build; status still needs one `Repository` handle per worktree. |
| `gitoxide` | Pure Rust but worktree management support is still incomplete for our needs; risk of API churn. |

### GPUI dependency: crates.io `gpui = "=0.2.2"` — chosen

| Option | Verdict |
|---|---|
| crates.io pin | **Chosen.** Versioned, reproducible, documented on docs.rs. Bootstrap: `Application::new().run(|cx: &mut App| …)`. macOS rendering is Metal via default features. |
| Git dependency (master) | Master has split `gpui_platform` out (unreleased on crates.io as of this writing) and moves fast with breaking changes. |

Trade-off accepted: GPUI is pre-1.0; we pin the exact version and treat any
future upgrade as a deliberate migration. Exact 0.2.2 API shapes (context and
spawn signatures, element styling) are verified against docs.rs and the
compiler during implementation rather than assumed from master-branch docs.

### Feature scope: full single-repo manager — chosen

"Minimal CRUD" undershoots daily-driver usefulness (status at a glance is the
main value of a GUI here); "multi-repo dashboard" roughly doubles UI state and
parsing scope for v1. Full manager, single repo is the sweet spot.

## Architecture

```
src/
  main.rs      Bootstrap: Application, window, keymap, actions, fonts.
  git.rs       Async `git` CLI wrapper (all porcelain parsing lives in model.rs).
  model.rs     Data types + pure porcelain parsers (unit-tested).
  store.rs     WorktreeStore (GPUI entity): repo root, entries, selection,
               filter, status message; spawns background ops; cx.notify().
  ui.rs        Root view: toolbar, search field, worktree list, detail pane,
               status bar; renders from store.
  dialogs.rs   Modal flows: create worktree, remove confirmation.
  platform.rs  Small per-OS helpers: open terminal, reveal in file manager.
```

Layering: `ui.rs`/`dialogs.rs` render state and dispatch; `store.rs` owns all
mutable state and orchestrates; `git.rs` is pure I/O; `model.rs` is pure data.
`git.rs` and `model.rs` depend on `smol` only (for async process spawning), not
on `gpui`, so they build and test headlessly. `store.rs` bridges them to GPUI
by spawning git futures on the background executor.

### Data model

```rust
struct WorktreeEntry {
    path: PathBuf,          // absolute
    head: Option<String>,   // commit-ish from porcelain
    branch: Option<String>, // short branch name; None => detached
    is_main: bool,          // main worktree flag from porcelain
    status: WorktreeStatus, // Unknown until the status pass completes
}

enum WorktreeStatus {
    Pending,                    // background status pass hasn't finished
    Unavailable(String),        // git status failed (e.g. missing dir)
    Clean { ahead: u32, behind: u32 },          // vs upstream; 0/0 when none
    Dirty { staged: u32, unstaged: u32, untracked: u32, ahead: u32, behind: u32 },
}
```

### Git command mapping

| Operation | Command |
|---|---|
| Detect repo | `git rev-parse --show-toplevel` (walk up from cwd) |
| List worktrees | `git worktree list --porcelain` |
| Status pass | `git -C <path> status --porcelain=v2 --branch` (all worktrees concurrently) |
| Create | `git worktree add <path> -b <new-branch> <base>` or `git worktree add <path> <existing-branch>` |
| Remove | `git worktree remove <path>` (`--force` from the confirm dialog's checkbox) |
| Prune | `git worktree prune` |
| Local branches | `git branch --format=%(refname:short)` (create dialog, existing-branch mode) |

Default destination for creation:
`<repo_parent>/<repo_name>-worktrees/<branch with '/' → '-'>`, editable in
the dialog. Base defaults to the repository's default branch (detected via
`git symbolic-ref refs/remotes/origin/HEAD`, falling back to `main`/`master`).

The create dialog has a "New branch" checkbox (default checked):
- **Checked:** fields are Branch name + Base (the `-b` form above).
- **Unchecked:** a single Existing branch field (with the repository's local
  branches offered from `git branch --format=%(refname:short)`); the command
  is `git worktree add <path> <branch>`.

### Data flow

1. User action (click / keybinding) → action handler → `WorktreeStore` method.
2. Store sets status message ("Refreshing…"), `cx.notify()`.
3. Store spawns work on the background executor (`cx.spawn` + `background_executor().spawn`).
4. Result returns via the async context; store updates its entity state
   (entries, error message), then `cx.notify()` re-renders.
5. Errors never panic: they land in the status bar (stderr from git,
   truncated to one line).

### UI layout

```
┌──────────────────────────────────────────────────────────────┐
│ [repo name ▾ path]   [Search ___________]  New  Refresh  Prune│  toolbar
├──────────────────────────────────────────────────────────────┤
│ ▸ main        ~/git/myrepo            main · clean           │
│ ▸ feature-x   ~/git/myrepo-worktrees/feature-x  ● 3 +2 ↑1 ↓1 │
│   … filtered rows, selected row highlighted …                │
├──────────────────────────────────────────────────────────────┤
│ selected: branch, path, status detail                          │
│ [Open in Terminal] [Reveal] [Copy Path]      [Remove…]        │  detail pane
├──────────────────────────────────────────────────────────────┤
│ status: refreshed 5s ago · last error (if any)                │  status bar
└──────────────────────────────────────────────────────────────┘
```

Dark theme, Zed-inspired palette, `ui.rs` keeps all colors/spacing constants.
List is a plain scrollable `v_flex` of rows (worktree counts are small; a
virtualized list is unnecessary at this scale).

If launch cwd is not inside a git repository, the window shows an empty state
with a path input + Load button; `git rev-parse` validates it. The window
title always shows the active repository name.

### Error handling

- `git.rs` returns `Result<T, GitError>`; `GitError::Git { code, stderr }`
  carries stderr through to the UI verbatim (one line, truncated).
- A failed status pass degrades that row to `Unavailable(reason)` — the list
  itself never fails wholesale.
- Missing `git` binary is reported at startup in the empty state.

## Testing

- **Unit (pure):** porcelain parsers (`worktree list --porcelain`,
  `status --porcelain=v2 --branch`) against fixture strings, including
  detached HEAD, missing upstream, renamed fields, CRLF. Branch-name
  sanitization and default-path derivation.
- **Integration:** against throwaway repos in `tempfile::tempdir()`: init
  repo, commit, create/remove/prune worktrees through the wrapper; asserts on
  resulting `git worktree list`. Requires `git` on PATH (assumed).
- **Build gates:** `cargo build`, `cargo test`, `cargo clippy -- -D warnings`.
- UI logic that matters (filtering, selection semantics) lives in
  `model.rs`/`store.rs` as pure or near-pure functions so it is unit-testable
  without a window. Visual verification is manual (screenshot at the end).

## Risks

- GPUI 0.2.2 API drift vs. master-branch examples → mitigated by pinning
  `=0.2.2`, consulting docs.rs for that exact version, and letting the
  compiler arbitrate; hello-world scaffold compiles before any feature work.
- `git status` porcelain v2 upstream fields vary by git version → parser is
  lenient (unknown lines ignored), tested against git 2.50 output.
- First GPUI build is slow (~minutes, blade shaders) → expected, not a fault.
