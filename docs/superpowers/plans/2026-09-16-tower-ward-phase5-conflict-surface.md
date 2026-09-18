# Phase 5 — Conflict-resolution surface

Date: 2026-09-16. Completes the design doc's "merge/rebase with conflict
surface" promise: Phases 3/4 ship abort-on-conflict; this phase makes
conflicts a FIRST-CLASS pause state you resolve and continue in-app.

## Design decisions

- **Single operations pause; multi-commit rewrites stay atomic.**
  `merge`, `rebase`, `cherry-pick`, `revert` now LEAVE the operation in
  progress on conflict (real git state — survives esc, restarts, and is
  visible to other git clients). The Phase 4 rewrite chain
  (`run_rewrite_plan`) still unwinds completely: its steps live only in
  app memory, so a paused mid-chain would be uncontinuable.
- **State detection is a git probe, not app memory.**
  `sequence::operation_state` checks, in precedence order: rebase
  (`--git-path rebase-merge` — checked FIRST, a rebase conflict also
  sets CHERRY_PICK_HEAD), cherry-pick (CHERRY_PICK_HEAD), revert
  (REVERT_HEAD), merge (MERGE_HEAD). Rebase progress comes from the
  `msgnum`/`end` files (e.g. "rebasing 3/5").
- **Continue uses git's stored messages.** `g` runs
  `git -c core.editor=true <op> --continue` — MERGE_MSG / the pick's
  original message / REVERT_MSG are accepted without an editor round
  trip. The user edits conflict CONTENT in their own editor and stages
  with `s` (the existing flow); `--continue` refuses via git's own
  error while unresolved files remain, surfaced verbatim.
- **Keys (Working Copy section, where resolution happens).**
  | Key | Action |
  | --- | --- |
  | `g` | Continue the in-progress operation |
  | `K` | Skip the current step (rebase / cherry-pick only) |
  | `A` | Abort the operation, restoring the pre-operation state |
  A banner above the file list names the operation (+ progress for
  rebase) and the keys. Other sections keep working read-only: their
  mutating entry points refuse with "finish or abort the in-progress
  …" via the existing busy-mirror plumbing.
- **Skip replays the next step and may conflict again** — same
  report-the-conflicts shape as continue.

## Tasks

1. **Engine — `engine/sequence.rs`.** `InProgress` + `operation_state`,
   `continue_op` (no abort on refusal — the user is mid-resolution),
   `abort_op`, `skip_op`. TDD: detection per op + precedence, continue
   refuses-then-completes, abort restores, skip advances.
2. **Engine — stop aborting single ops.** `branches::merge/rebase`,
   `rewrite::cherry_pick/revert` return Ok(conflicts) and leave state;
   the chain keeps its unwind. Update contract tests.
3. **Store — wc_store.** `in_progress` from the refresh batch; `g`/`K`/
   `A` actions on the bg executor (completion flags history_changed +
   mutated when the op completes). `sync_worktree_busy` adds the
   in-progress state to the other sections' refusal mirrors.
4. **View — banner + footer** in the Working Copy section.
5. **Docs + gates.** README, version 0.6.0, ledger, full gates.
