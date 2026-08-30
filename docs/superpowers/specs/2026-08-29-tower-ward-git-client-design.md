# Worktree Tool → Tower-ward Git Client — Design

Date: 2026-08-29
Status: Draft — pending user review (autonomous session; recommended option
taken at each decision point, same as the v1 spec)

## Summary

Evolve worktree-tool from a worktree manager into a keyboard-first git
client in the spirit of Tower, while keeping its identity: **worktrees are
the home screen; every worktree drills into a full git surface** (working
copy / staging / commit, history graph, branches & remotes). The scope is
too large for one release, so it is decomposed into sequenced phases, each
with its own spec → plan → implementation cycle. This document records the
global decisions and the roadmap, then specifies Phase 1 (Working Copy:
status, stage, diff view, commit) in full.

## Decision log

### App identity: worktree-first — chosen

| Option | Verdict |
|---|---|
| Worktree-first | **Chosen.** Preserves the product's identity and the existing UI investment; worktrees remain the organizing principle and the differentiator vs Tower. Selecting a worktree opens its detail view. |
| Repo-first (Tower's model) | Cleaner fit with Tower conventions but discards the current home screen and brand; revisit only if worktree-first proves awkward in practice. |
| Multi-repo dashboard | v1 non-goal, stays one for now; a repo picker can layer on top later without changing this architecture. |

### Git engine: hardened CLI layer — chosen

| Option | Verdict |
|---|---|
| CLI, hardened (`engine` module, porcelain `-z` formats) | **Chosen.** Zero new dependencies; the four-platform build story (macOS, Linux, Windows, FreeBSD; zigbuild cross-checks) stays exactly as clean as today. Network operations (fetch/push/pull) go through the user's system `git`, so credential helpers and ssh config just work. Hunk staging uses the lazygit-proven technique: `git diff` → filter hunks → `git apply --cached`. |
| gitoxide (reads) + CLI (writes) | Nice in-process object model for history, but 0.x API churn and heavier deps on a solo project; parsing `-z` output is stable and cheap. The `engine` module API is the seam — any read function can later be re-implemented on gix without touching callers. |
| git2 (libgit2) everywhere | Mature library, but adds a C toolchain to all builds (FreeBSD CI + cross-linking), push/pull auth still needs system git (libssh2 host-key pain), and its worktree support — the app's core feature — is partial, so the CLI stays regardless. |

### Platforms: keep all four — chosen

No new system dependencies are introduced. New platform-sensitive pieces and
their approach: commit editor resolution replicates git's own order
(`GIT_EDITOR` → `core.editor` → `VISUAL` → `EDITOR` → platform default
`vim`/`nano` on unix, `notepad` on Windows); `--no-optional-locks` on all
read-only commands (portable since git 2.3) so the app never blocks the
user's own git processes on index.lock.

### Commit authoring: `$EDITOR` flow for Phase 1 — chosen

| Option | Verdict |
|---|---|
| Spawn `$EDITOR` on a temp message file (git's own COMMIT_EDITMSG flow) | **Chosen for Phase 1.** Git-native, respects the user's configured editor, zero new GPUI work, ideal for keyboard-first users. On editor exit: non-empty first-comment-stripped content → commit; empty → abort. |
| In-app multi-line editor | The right long-term UX, but `text_field.rs` is single-line today and a multi-line editor (cursor movement, wrapping, selection, paste) is the riskiest GPUI piece of the phase. Fast-follow in Phase 1b once the working-copy view is stable. |
| Two single-line fields (subject / body) assembled on commit | Cheap, but unusual UX and still teaches the app a worse habit than `$EDITOR`. Rejected. |

## Roadmap

Each phase ships as its own release-sized unit. Phases 2 and 3 get their own
spec documents when their turn comes; the sketches below fix the sequencing,
not the details.

- **Phase 1 — Working Copy** (this spec): status, stage/unstage (file-level,
  then hunk-level as 1b), unified diff view, commit. Includes the app-shell
  restructure (worktree detail view) and the `engine` module that all later
  phases build on.
- **Phase 2 — History & Graph**: commit list (virtualized), in-app graph
  lane computation from `git log --topo-order` parent data, commit detail
  reusing the Phase 1 diff renderer, actions (copy hash, checkout; and the
  worktree-native "open this commit in a new worktree").
- **Phase 3 — Branches & Remotes**: branch create/switch/rename/delete,
  merge/rebase with conflict surface, fetch/push/pull with background
  progress and ahead/behind wired to real fetches, stash list/pop/apply/drop.

## Phase 1 design

### Navigation & app shell

The home screen is unchanged: the worktree list. Two changes:

- `enter` on a worktree now opens its **detail view** (previously: open in
  terminal — a breaking keybinding change, documented in the README and site).
  Opening a terminal moves to `t` (and stays on the toolbar and detail view).
- The detail view has sections, navigated with `1`/`2`/`3`:
  **Working Copy** (Phase 1), **History** (Phase 2), **Branches** (Phase 3).
  Sections that don't exist yet are shown greyed with "coming in v0.3/v0.4".
  `esc` returns to the list. A header line in the detail view shows the
  worktree's branch (or detached HEAD) and ahead/behind, from status v2's
  branch record.

`ui.rs` (885 lines) is split as part of this work: `app.rs` (shell + worktree
list) and `views/working_copy.rs`, so Phase 2/3 views have an obvious home.
`model.rs`, `store.rs` (generation-counter pattern reused for the new store),
`dialogs.rs`, `feedback.rs`, `terminal.rs` are carried over unchanged.

### Working copy view

Vertical split: file list (left) + unified diff of the selected file (right).

File list, four fixed groups in order:

1. **Conflicts** (`U`/`AA`/`DD`/… entries from status v2) — red accent.
2. **Staged** — index status ≠ `.`.
3. **Unstaged** — worktree status ≠ `.`.
4. **Untracked**.

A file with both staged and unstaged changes appears in both groups (like
Tower). Rows show the status letter, path (renames: `old → new`), and for
tracked dirty files the `+N/−M` line counts from `--numstat`.

Actions (keyboard, gated on the existing root-focus rule):

| Key | Scope | Action |
|---|---|---|
| `↑`/`↓` | file list | move selection (wraps within group) |
| `tab` | view | toggle focus: file list ↔ diff |
| `s` | file | stage (if unstaged/untracked) or unstage (if staged) |
| `S` | view | stage all remaining changes (unstaged + untracked); no unstage-all in Phase 1 |
| `d` | file | discard changes — **confirmation dialog**; offered on unstaged rows (`git checkout -- <path>`) and untracked rows (delete file); not offered on staged rows (unstage first) or conflicts |
| `c` | view | open commit flow — disabled with a hint while nothing is staged |
| `r` | view | refresh working copy + list row |

Hunk-level staging (Phase 1b): with the diff pane focused, `↑`/`↓` moves
between hunks and `s` stages the hovered hunk. Mechanism: take the file's
unified diff (bytes straight from git), keep header + selected hunks, feed to
`git apply --cached --whitespace=nowarn`. On apply failure, surface git's
stderr and suggest staging the whole file. Binary files and untracked files
are file-level only (apply can't express them).

Diff pane: unified format, added/removed/changed-line coloring consistent
with the Catppuccin-accent theme; hunk headers rendered dim. Renames show
`old → new`; mode changes get a one-line notice. Content caps for sanity:
render at most ~5,000 lines per file (trailer: "… N more lines — open in
editor"); untracked previews cap at 256 KB. Binary files: "binary file"
placeholder with size. Conflicted files render raw content (markers visible);
Phase 1 offers **mark resolved** (`git add`) only — conflict resolution
editing stays in the user's editor/terminal.

### Engine (`src/engine/`)

`git.rs` is promoted to a typed command layer; every command takes the
worktree path, runs blocking `std::process` on GPUI background executor
threads (existing pattern), uses `--` separators + path validation (existing
injection protections), and parses only machine formats:

- `status(path) → WorkingCopy` — `git status --porcelain=v2 -z --branch`
  (staged/unstaged/untracked/conflicted, renames, branch + ahead/behind).
- `diff_unstaged / diff_staged(path) → UnifiedDiff` — `git diff [-cached] -z
  --no-ext-diff --unified=3 --numstat`; `UnifiedDiff { header, hunks }`
  parsed into a typed structure so hunk staging is a filter, not a re-parse.
- `untracked_preview(path, file)` — bounded file read, never a git call.
- `stage(paths) / unstage(paths) / discard(…)` — `git add` / `git reset -q
  HEAD --` / per-file strategy as above.
- `stage_hunk(path, patch_bytes)` — `git apply --cached` (Phase 1b).
- `commit(path, message)` — `git commit -q -F <tempfile>` (no `-m`, so no
  quoting or length issues).
- `commit_editor(path)` — resolve editor (git's order, above), write temp
  file seeded with git's commit-template comments, spawn, wait, strip
  comments, return message or `None` for abort.
- `author(path)` — `git config user.name / user.email` (worktree-local
  config wins naturally via cwd), shown in the commit UI footline.

All read-only commands run with `--no-optional-locks`. Every parser is a
pure function unit-tested against recorded fixtures, including nasties:
rename/copy lines, CRLF files, quoted paths (`-z` avoids core.quotePath
issues), submodules (skipped rows, shown as `M (submodule)`), intent-to-add.

### Data flow

`WorkingCopyStore` (new, generation-counter pattern from `WorktreeStore`)
holds `WorkingCopy` + `UnifiedDiff` for the selected file, refreshed:
on drill-in, after every mutating action (working copy + the home list row's
status summary), and on manual `r`. No background polling in Phase 1 —
focus-triggered refresh and git-dir watching are later enhancements.
Out-of-order results are dropped by generation, exactly as today.

### Error handling

Through the existing `feedback.rs` surface. Specifics: parse failures render
a non-fatal error row in the affected pane; `git apply` failures show git's
stderr verbatim plus the fallback hint; index.lock contention ("another git
process seems to be running…") is detected and shown as a friendly
"another git process may be using this worktree — retry" instead of a raw
error; `c` is disabled with a hint while the index is empty, so an empty
commit can't be attempted.

### Testing

Existing patterns only (temp repos via `tempfile`, real `git` binary, GPUI
`test-support` + `VisualTestContext`, blocking processes on background
threads per the known harness pitfall):

- Parser fixtures: status v2 (all entry types, renames, conflicts, branch
  line), unified diff (hunks, mode changes, renames, binary markers,
  context at file start/end).
- Engine integration: status → stage/unstage/discard round-trips; hunk
  staging (Phase 1b) incl. adjacent-hunk context overlap; commit via
  `-F` file; editor resolution order (env-var matrix).
- Injection regressions: worktree/branch names with dashes, spaces, unicode,
  leading `-`.
- UI tests: drill-in/esc navigation, group ordering + selection movement,
  `s`/`S`/`d`-with-confirm flows against a scripted temp repo.
- Performance guard: bench extension in `examples/bench.rs` — status+diff on
  a repo with ~2k changed files stays comfortably under the ~111 ms startup
  budget's order of magnitude.

### Non-goals (Phase 1)

Hunk-level discard/edit; inline conflict resolution; image diffs; blame;
amend; stashes; fetch/push/pull (Phase 3); history (Phase 2); in-app
multi-line commit editor (Phase 1b fast-follow); IME input (standing GPUI
0.2.2 limitation); background auto-refresh.

## Versioning

Phase 1 → v0.2.0 (1b hunk staging → v0.2.1). Phases 2/3 → v0.3/v0.4, each
with its own spec before implementation.
