# Phase 2 — History & Graph

Implements the Phase 2 bullet of the roadmap in
`docs/superpowers/specs/2026-08-29-tower-ward-git-client-design.md`: commit
list, in-app graph lane computation from `git log --topo-order` parent
data, commit detail reusing the Phase 1 diff parsing, and the actions
(copy hash, checkout, open-this-commit-in-a-new-worktree).

Stacked on `feat/tower-ward-phase1b-hunk-staging` (PR #16) — Phase 2
touches the same view/store seams (section switching in `app.rs`, tab
labels). Rebase onto main after #16 merges.

## Scope

The worktree detail view gains real sections: `1` = Working Copy,
`2` = History (the tab labels already exist as greyed text). Each drill-in
creates a `HistoryStore` alongside the `WorkingCopyStore`; `esc` still
closes the whole detail view.

History section layout mirrors Working Copy: commit list (left) + selected
commit's changed files and diff (right).

- **Commit list**: `git log --topo-order`, newest first, batched 500 at a
  time (`L` loads 500 more). Each row: text graph glyph (lane-accurate
  `*`/`│` cells computed in-app from parent data), short hash, subject,
  author + relative-ish date.
- **Commit detail**: changed files (name-status, `--root`-aware) and the
  selected file's unified diff against the first parent (root commits diff
  against nothing). Read-only — no staging from history.
- **Actions**: `y` copy the full hash to the clipboard; `x` checkout the
  commit in this worktree (detached; refused with a hint when the working
  copy has changes, or when git reports the commit is checked out
  elsewhere); `w` open a new worktree at the commit
  (`<repo>-<shortsha>` sibling directory, `-2`/`-3` on collision, detached)
  — flagged as a home-list mutation so the new worktree appears
  immediately.
- **Refusals over no-ops** everywhere (the Phase 1 rule): ineligible keys
  explain themselves in the footer.

Out of scope (follow-ups): `--all`/branch filters, fetching, searching the
log, per-commit actions beyond the three above, renames' second-parent
display niceties, true virtualization (a 500-row render cap + load-more
keeps the frame cheap; the file list already works this way).

Version → 0.3.0 (the tab label promised v0.3 for History).

## Decisions

- **Log transport**: one `git log --topo-order --format=%x00%H%x01%h%x01%P%x01%an%x01%at%x01%D%x01%s`
  invocation parsed from raw bytes (NUL record starts, SOH field
  separators) — same strict-then-lossy decode discipline as status. Hashes
  are ASCII; subjects may not be.
- **Lanes are computed in-app**, not parsed from `--graph`: the wire-list
  algorithm over topo-ordered parent data is pure, deterministic, and
  unit-testable (linear history = lane 0 forever; side branch opens lane
  1 at the fork and closes it at the merge; multiple wires reserve their
  lanes across the span).
- **Graph glyphs**: one text cell per lane per row — `*` on the commit's
  lane, `│` on passing wires. No inter-row curve rows (MVP; the lane data
  is the artifact other renderings can reuse).
- **Commit diff**: `git show --format= <sha> -- <path>` (root-commit-safe),
  parsed by the Phase 1 `parse_unified_diff`. Files via
  `diff-tree --no-commit-id --name-status -r -z --root`.
- **Checkout refuses on a dirty working copy** (checked via the engine, not
  git's error), and surfaces git's own stderr for the "checked out in
  another worktree" case — that refusal is git protecting the worktree.
- **Section state lives on `RootView`** (`Section` enum + per-section
  stores); `1`/`2` swap section, focus moves to that section's list
  handle; the header tab labels render the active section. `close_detail`
  tears the whole drill-in down as before.
- **Clipboard** via gpui (`write_to_clipboard`), message confirms the
  short hash.

## Tasks

1. **Engine — log parsing.** `history::LogCommit` + `history::log(worktree, max_count)`:
   byte parser (NUL/SOH), refs decoration capture (`D` field → branch/tag
   markers for display). TDD: linear fixture, merge fixture, non-UTF-8
   subject survives lossy decode, empty repo → empty.
2. **Engine — lanes.** `history::assign_lanes(&[LogCommit]) -> Vec<GraphRow>`
   with the wire-list algorithm. TDD: linear, fork+merge, two parallel
   branches, lane reuse after close.
3. **Engine — commit detail.** `history::commit_files` (name-status -z,
   `--root`) and `history::commit_diff` (`git show`). TDD: add/modify/delete
   letters, root commit, binary file flag.
4. **Engine — actions.** `history::checkout` (detached, refuses on dirty —
   caller passes the status), `history::open_worktree_at` (collision-suffixed
   path, `--detach`), both returning the created/checked-out info for
   messages. TDD: dirty refusal, collision suffixing, worktree appears and
   is registered (`git worktree list`).
5. **Store — `HistoryStore`.** Refresh (log+lanes), selection driving
   files/diff loads (detail_generation pattern from Phase 1), load_more,
   copy/checkout/worktree actions through the engine, `mutated` flag for
   the home refresh. Store tests with real fixtures.
6. **Shell + views — sections.** `RootView.section`, `1`/`2` routing,
   history list/diff focus handles, `views/history.rs` (list rows with
   graph glyphs, files pane, read-only diff), per-section footer hints and
   active tab label. App-level GPUI tests: section switch, selection,
   copy/checkout/worktree actions.
7. **Docs + gates.** README keybindings (history section), version 0.3.0,
   ledger. Full gates: test/fmt/clippy/zigbuild-windows.
