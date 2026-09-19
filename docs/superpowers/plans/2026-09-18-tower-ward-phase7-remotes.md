# Phase 7 — Remote management

Date: 2026-09-18. Remotes are repo-wide, so the surface lives on the
home screen (a Remotes dialog), not inside a worktree section.

## Design decisions

- **Dialog on the home screen**, opened from a toolbar button or `R`
  (shift+r — gpui delivers the lowercase key with shift; the guarded
  arm precedes the plain `r` refresh arm). Remotes belong to the repo:
  the dialog reads from `git remote -v` at open, not from app state.
- **Engine — `remotes::list/add/remove`.** `list` parses `remote -v`
  into name + fetch/push URLs (push shown when it differs). `add`
  validates name (no empty/dash-leading/whitespace/control chars) and
  URL (no control chars); both argv-only. `remove` is exact-name.
- **Remove confirms.** Deleting a remote is ref surgery; `d` opens a
  small confirm dialog (same shape as worktree removal) before
  `remote remove`.
- **Add is a two-field sub-dialog** (name + URL) reusing the
  multi-field create-worktree dialog pattern; enter confirms.
- **No edit-in-place in v1** — remove + re-add covers it; fetch/push
  per remote stays in the Branches section (`f`/`u`).

## Tasks

1. **Engine — remotes list/add/remove** with validation. TDD:
   round-trip against a local bare remote, duplicate-name refusal,
   remove-missing error.
2. **Dialogs — `RemotesDialog`** (list + selection + d-to-remove),
   **`AddRemote`** (two fields), **`RemoveRemote`** (confirm).
3. **Shell — toolbar button + `R` key** on the home screen.
4. **Docs + gates.** README, version bump, ledger, full gates.
