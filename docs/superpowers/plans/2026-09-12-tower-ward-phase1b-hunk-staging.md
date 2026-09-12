# Phase 1b — Hunk-Level Staging

Implements the hunk-staging sketch in
`docs/superpowers/specs/2026-08-29-tower-ward-git-client-design.md`
("Hunk-level staging (Phase 1b)"). That sketch is the requirements source;
this plan fixes the task breakdown and the decisions taken while
implementing. The in-app multi-line commit editor (the other "fast-follow
in Phase 1b" item from the commit-authoring table) is deliberately NOT in
this phase — it is the riskiest GPUI surface and gets its own focused PR.

## Scope

With the diff pane focused on an Unstaged row's unified diff:

- `↑`/`↓` move a hunk cursor between the file's hunks.
- `s` stages the hovered hunk: the file header plus that hunk's byte-exact
  raw text go to `git apply --cached --whitespace=nowarn` on stdin.
- On apply failure git's stderr is surfaced with a "stage the whole file
  instead" hint (a stale patch can only fail cleanly — `apply --cached`
  touches the index, never the worktree, so no data loss is possible).
- Binary files, untracked files, conflicts, and non-UTF-8-named files are
  file-level only (`apply` cannot express them); the keys explain instead
  of no-op.

Out of scope (recorded for follow-ups): per-hunk UNstage of staged hunks
(`apply --cached --reverse` of the staged diff), per-hunk discard, and the
in-app editor.

Version stays 0.2.0: v0.2.0 was never tagged, so 1b rides in the same
release; tag after merge.

## Decisions

- **Header bytes.** `UnifiedDiff` gains `header_raw: Vec<u8>` (the parser
  keeps the lossy `header` for display). Reconstructing the patch from the
  lossy string would corrupt headers whose paths are not valid UTF-8;
  byte-exactness is the whole point of `raw`.
- **Patch reconstruction is `header_raw` + selected hunks' `raw` in order**
  — byte-identical to `git diff` output minus unselected hunks. No
  re-numbering: `git apply` locates hunks by context, so a subset applies
  cleanly.
- **Stdin, not argv.** The patch can be arbitrarily large; the runner gains
  `run_bytes_stdin`. (`git apply` reads the patch from stdin when no file
  argument is given.)
- **No `--no-optional-locks`** on `apply --cached` — it writes the index
  and must take index.lock, like every other mutation.
- **Cursor, not selection state machine.** `hunk_cursor: usize` on the
  store, clamped at every use and re-clamped when a detail load lands;
  reset to 0 on selection change. Hunks past the render cap stay
  reachable (cap is a display bound, like MAX_VISIBLE_ROWS).
- **Safety mirrors `discard_path`.** Unsupported entries are refused up
  front; the patch is cloned from the current detail at keypress time;
  staleness is handled by git's own preimage check (failure → clean
  error), not by a live re-probe.

## Tasks

1. **Engine — byte-exact header + stdin runner + `apply_patch`.**
   `header_raw` in `parse_unified_diff`; `engine::run_bytes_stdin`;
   `mutate::apply_cached(worktree, patch: &[u8])`. TDD: parser test for
   header_raw; apply tests — stage hunk 1 of a two-hunk file, assert index
   holds only hunk 1 and the worktree is untouched; stale patch (index
   moved) fails with git's stderr; patch with `\ No newline` marker applies.
2. **Store — hunk cursor + `stage_hunk`.** Cursor accessors with clamping,
   reset on `select`, guards (mutating/loading; unsupported; binary →
   "binary files stage whole-file only"; group ≠ Unstaged or no hunks →
   hint; cursor out of range after refresh → hint), background apply from
   cloned bytes, `after_mutation`. TDD at store level with a real repo.
3. **Shell + view — keys, highlight, hints.** `up`/`down`/`s` on the diff
   focus; hovered hunk header highlighted with the accent when the diff
   pane is focused; footer hints gain `↑/↓ hunk · s stage hunk`. App-level
   GPUI test: cursor moves, `s` stages the hovered hunk only.
4. **Docs + gates.** README keybindings, design-doc 1b sketch ticked,
   ledger. Full gates: `cargo test`, fmt, clippy, `zigbuild
   x86_64-pc-windows-gnu --lib`.
