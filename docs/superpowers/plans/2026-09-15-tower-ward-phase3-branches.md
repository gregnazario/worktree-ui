# Phase 3 — Branches & Remotes

Implements the Phase 3 roadmap bullet from
`docs/superpowers/specs/2026-08-29-tower-ward-git-client-design.md`:
branch create/switch/rename/delete, merge/rebase with conflict surface,
fetch/push/pull with ahead/behind wired to real fetches, and
stash list/pop/apply/drop.

Stacked on the Phase 1b/2 merges — branched from main after both merged.

## Scope (Section 3 of the detail view)

- **Branch list**: local branches with ahead/behind, current branch marker;
  remote tracking branches grouped separately. `enter`/`x` to switch.
- **Branch actions**: `n` create, `d` delete (refuses current branch),
  `r` rename.
- **Merge**: `m` merges selected branch into current; conflict files
  surface in the Working Copy section.
- **Rebase**: `R` rebases current branch onto selected branch.
- **Fetch**: `f` fetches all remotes; ahead/behind updates after.
- **Push**: `p` pushes current branch to its remote; sets upstream if
  none. `P` pushes with `--force-with-lease` (confirmation).
- **Pull**: `u` pulls (fetch + merge --ff-only for simplicity in Phase 3).
- **Stash**: `s` stash push (all changes), `a` stash apply (pop latest),
  `A` stash pop, `D` stash drop (confirmation). Stash list renders as
  a subsection in the Working Copy file list.

## Out of scope

- Interactive rebase, cherry-pick, bisect
- Remote management (add/remove/edit remotes)
- Credential/SSH configuration
- Submodule support

## Design decisions

- **All git commands via CLI** (same engine pattern). `git branch`,
  `git checkout`, `git merge`, `git rebase`, `git fetch`, `git push`,
  `git pull`, `git stash` — all argv-based with `--` separators.
- **BranchStore** (new) alongside WorkingCopyStore and HistoryStore.
  Section 3 has its own focus handles, key routing, and observers.
- **Conflict surface**: after merge/rebase with conflicts, the Working
  Copy section's Conflicts group already renders conflicted files.
  The user resolves in their editor and presses `s` to mark resolved.
- **Force-push** requires explicit confirmation (two-step: first `P`
  shows confirmation, second `P` executes).

## Tasks

1. **Engine — branch operations.** `branch::list`, `branch::create`,
   `branch::switch`, `branch::rename`, `branch::delete`. Parse
   `git branch --format` for local + remote. TDD: CRUD round-trip,
   current branch detection, rename preserves tracking, delete refuses
   current.
2. **Engine — merge/rebase.** `merge::merge(branch)`, `merge::rebase(onto)`.
   Both return conflict file lists on failure. TDD: clean merge,
   conflict merge returns conflicted files, rebase fast-forward vs real.
3. **Engine — remote ops.** `remote::fetch`, `remote::push`,
   `remote::pull`. Set upstream, force-with-lease, prune. TDD: push to
   bare repo, fetch updates tracking, pull ff-only.
4. **Engine — stash.** `stash::list`, `stash::push`, `stash::pop`,
   `stash::apply`, `stash::drop`. Parse `git stash list --format`. TDD:
   push/pop round-trip, stash survives commit, drop removes.
5. **Store — BranchStore.** Branch list with selection, remote section,
   merge/rebase/checkout actions, stash operations, observer for home
   refresh.
6. **Shell + views — Section 3.** Section 3 tab active, Branches view
   with branch list / stash subsection, per-section key routing, focus
   handles, footer hints.
7. **Docs + gates.** README keybindings for section 3, version bump,
   ledger. Full gates.
