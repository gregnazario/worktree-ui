//! Engine tests for stash and remote operations.

mod common;

use common::{fixture_repo, sh, sh_out};
use worktree_tool::engine::{remotes, stash};

#[test]
fn stash_push_pop_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    std::fs::write(tmp.path().join("f.txt"), "dirty").unwrap();

    let entries = stash::list(tmp.path()).unwrap();
    assert!(entries.is_empty(), "no stash initially");

    stash::push(tmp.path(), Some("test stash")).unwrap();
    assert!(stash::list(tmp.path()).unwrap().len() == 1);
    let f = std::fs::read_to_string(tmp.path().join("f.txt")).unwrap();
    assert_eq!(f, "one", "working copy clean after stash");

    stash::pop(tmp.path()).unwrap();
    let f = std::fs::read_to_string(tmp.path().join("f.txt")).unwrap();
    assert_eq!(f, "dirty", "pop restored the changes");
    assert!(stash::list(tmp.path()).unwrap().is_empty(), "pop dropped");
}

#[test]
fn stash_list_reports_entries_newest_first() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    std::fs::write(tmp.path().join("f.txt"), "first").unwrap();
    stash::push(tmp.path(), Some("first stash")).unwrap();
    std::fs::write(tmp.path().join("f.txt"), "second").unwrap();
    stash::push(tmp.path(), Some("second stash")).unwrap();

    let entries = stash::list(tmp.path()).unwrap();
    assert_eq!(entries.len(), 2);
    assert!(entries[0].message.contains("second"), "newest first");
    assert!(entries[1].message.contains("first"), "oldest second");
}

#[test]
fn stash_drop_removes_entry() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    std::fs::write(tmp.path().join("f.txt"), "dirty").unwrap();
    stash::push(tmp.path(), None).unwrap();
    assert_eq!(stash::list(tmp.path()).unwrap().len(), 1);
    stash::drop(tmp.path(), 0).unwrap();
    assert!(stash::list(tmp.path()).unwrap().is_empty());
}

#[test]
fn stash_apply_without_pop_keeps_entry() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    std::fs::write(tmp.path().join("f.txt"), "dirty").unwrap();
    stash::push(tmp.path(), None).unwrap();

    stash::apply(tmp.path()).unwrap();
    let f = std::fs::read_to_string(tmp.path().join("f.txt")).unwrap();
    assert_eq!(f, "dirty", "apply restored");
    assert_eq!(stash::list(tmp.path()).unwrap().len(), 1, "still stashed");
}

#[test]
fn push_to_bare_remote_and_pull() {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("remote.git");
    std::fs::create_dir(&remote).unwrap();
    sh(
        None,
        &[
            "git",
            "init",
            "-q",
            "--bare",
            "--initial-branch=main",
            remote.to_str().unwrap(),
        ],
    );

    let work = tmp.path().join("work");
    std::fs::create_dir(&work).unwrap();
    fixture_repo(&work);
    sh(
        Some(&work),
        &["git", "remote", "add", "origin", remote.to_str().unwrap()],
    );
    sh(Some(&work), &["git", "push", "-q", "-u", "origin", "main"]);

    // Clone and make a divergent commit
    let clone = tmp.path().join("clone");
    sh(
        None,
        &[
            "git",
            "clone",
            "-q",
            remote.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    sh(Some(&clone), &["git", "config", "user.email", "t@t.t"]);
    sh(Some(&clone), &["git", "config", "user.name", "t"]);
    std::fs::write(clone.join("new.txt"), "new").unwrap();
    sh(Some(&clone), &["git", "add", "new.txt"]);
    sh(Some(&clone), &["git", "commit", "-qm", "new file"]);

    // Push from clone
    sh(Some(&clone), &["git", "push", "-q", "origin", "main"]);

    // Pull in the original work repo
    let result = remotes::pull(&work);
    assert!(result.is_ok(), "pull should succeed");
    assert!(
        work.join("new.txt").exists(),
        "pull brought in the new file"
    );
}

#[test]
fn fetch_updates_tracking_refs() {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("remote.git");
    std::fs::create_dir(&remote).unwrap();
    sh(
        None,
        &[
            "git",
            "init",
            "-q",
            "--bare",
            "--initial-branch=main",
            remote.to_str().unwrap(),
        ],
    );

    let work = tmp.path().join("work");
    std::fs::create_dir(&work).unwrap();
    fixture_repo(&work);
    sh(
        Some(&work),
        &["git", "remote", "add", "origin", remote.to_str().unwrap()],
    );
    sh(Some(&work), &["git", "push", "-q", "-u", "origin", "main"]);

    // Clone and push a new commit
    let clone = tmp.path().join("clone");
    sh(
        None,
        &[
            "git",
            "clone",
            "-q",
            remote.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    sh(Some(&clone), &["git", "config", "user.email", "t@t.t"]);
    sh(Some(&clone), &["git", "config", "user.name", "t"]);
    std::fs::write(clone.join("new.txt"), "new").unwrap();
    sh(Some(&clone), &["git", "add", "new.txt"]);
    sh(Some(&clone), &["git", "commit", "-qm", "new file"]);
    sh(Some(&clone), &["git", "push", "-q", "origin", "main"]);

    // Fetch in the original — the tracking ref should update
    remotes::fetch(&work).unwrap();
    let tracking = sh_out(&work, &["git", "rev-parse", "origin/main"]);
    let clone_tip = sh_out(&clone, &["git", "rev-parse", "main"]);
    assert_eq!(tracking, clone_tip, "fetch updated origin/main");
}

/// Fixture: a work repo tracking `main` on a local bare remote, plus a
/// second clone that can push competing commits. Returns (work, clone).
fn push_fixture(dir: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let remote = dir.join("remote.git");
    std::fs::create_dir(&remote).unwrap();
    sh(
        None,
        &[
            "git",
            "init",
            "-q",
            "--bare",
            "--initial-branch=main",
            remote.to_str().unwrap(),
        ],
    );
    let work = dir.join("work");
    std::fs::create_dir(&work).unwrap();
    fixture_repo(&work);
    sh(
        Some(&work),
        &["git", "remote", "add", "origin", remote.to_str().unwrap()],
    );
    sh(Some(&work), &["git", "push", "-q", "-u", "origin", "main"]);

    let clone = dir.join("clone");
    sh(
        None,
        &[
            "git",
            "clone",
            "-q",
            remote.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    sh(Some(&clone), &["git", "config", "user.email", "t@t.t"]);
    sh(Some(&clone), &["git", "config", "user.name", "t"]);
    (work, clone)
}

#[test]
fn push_sends_the_branch_to_the_tracked_remote() {
    let tmp = tempfile::tempdir().unwrap();
    let (work, clone) = push_fixture(tmp.path());

    // A new branch in work, pushed via the engine. The branch has NO
    // upstream yet — the engine falls back to `origin`.
    sh(Some(&work), &["git", "checkout", "-qb", "topic"]);
    std::fs::write(work.join("topic.txt"), "topic").unwrap();
    sh(Some(&work), &["git", "add", "topic.txt"]);
    sh(Some(&work), &["git", "commit", "-qm", "topic work"]);

    remotes::push(&work, "topic", true).unwrap();

    // The clone sees the pushed branch.
    sh(Some(&clone), &["git", "fetch", "-q", "--all"]);
    let tip = sh_out(&clone, &["git", "rev-parse", "origin/topic"]);
    let local = sh_out(&work, &["git", "rev-parse", "topic"]);
    assert_eq!(tip, local, "push landed on the remote");

    // Upstream was set by the engine's --set-upstream path.
    let upstream = sh_out(
        &work,
        &["git", "rev-parse", "--abbrev-ref", "topic@{upstream}"],
    );
    assert_eq!(upstream, "origin/topic");
}

#[test]
fn push_targets_the_upstream_remote_not_a_hardcoded_one() {
    let tmp = tempfile::tempdir().unwrap();
    let (work, _clone) = push_fixture(tmp.path());

    // A second bare remote; re-point main's upstream at it. A hardcoded
    // `origin` in the engine would push to the WRONG remote here.
    let other = tmp.path().join("other.git");
    std::fs::create_dir(&other).unwrap();
    sh(
        None,
        &[
            "git",
            "init",
            "-q",
            "--bare",
            "--initial-branch=main",
            other.to_str().unwrap(),
        ],
    );
    sh(
        Some(&work),
        &["git", "remote", "add", "upstream", other.to_str().unwrap()],
    );
    // Seed the other remote at main's tip and fetch it, so the tracking
    // ref exists and the later engine push is a fast-forward.
    sh(Some(&work), &["git", "push", "-q", "upstream", "main:main"]);
    sh(Some(&work), &["git", "fetch", "-q", "upstream"]);
    sh(
        Some(&work),
        &["git", "branch", "--set-upstream-to=upstream/main", "main"],
    );

    std::fs::write(work.join("more.txt"), "more").unwrap();
    sh(Some(&work), &["git", "add", "more.txt"]);
    sh(Some(&work), &["git", "commit", "-qm", "more work"]);

    // The engine's push (no explicit remote) must resolve `upstream`
    // from the branch's tracking config.
    remotes::push(&work, "main", false).unwrap();
    let pushed = sh_out(&work, &["git", "rev-parse", "upstream/main"]);
    let local = sh_out(&work, &["git", "rev-parse", "main"]);
    assert_eq!(pushed, local, "push went to the tracked remote");
}

#[test]
fn push_force_with_lease_moves_the_remote_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let (work, clone) = push_fixture(tmp.path());

    sh(Some(&work), &["git", "checkout", "-qb", "topic"]);
    std::fs::write(work.join("t.txt"), "1").unwrap();
    sh(Some(&work), &["git", "add", "t.txt"]);
    sh(Some(&work), &["git", "commit", "-qm", "t1"]);
    remotes::push(&work, "topic", true).unwrap();

    // Rewrite the local branch (amend) — a plain push would be refused;
    // force-with-lease succeeds because the lease matches the remote.
    sh(
        Some(&work),
        &["git", "commit", "-q", "--amend", "-m", "t1 amended"],
    );
    remotes::push_force_with_lease(&work, "topic").unwrap();
    sh(Some(&clone), &["git", "fetch", "-q", "--all"]);
    let remote_tip = sh_out(&clone, &["git", "rev-parse", "origin/topic"]);
    let local_tip = sh_out(&work, &["git", "rev-parse", "topic"]);
    assert_eq!(remote_tip, local_tip, "rewritten branch pushed");
}

#[test]
fn stash_apply_and_pop_target_a_specific_entry() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());

    // Two stashes, then apply/pop the OLDER one by index.
    std::fs::write(tmp.path().join("f.txt"), "newest").unwrap();
    stash::push(tmp.path(), Some("newest")).unwrap();
    std::fs::write(tmp.path().join("f.txt"), "dirty").unwrap();
    stash::push(tmp.path(), Some("oldest")).unwrap();
    // f.txt back to committed state for a clean apply.
    sh(Some(tmp.path()), &["git", "checkout", "-q", "--", "f.txt"]);

    stash::apply_at(tmp.path(), 1).unwrap();
    let f = std::fs::read_to_string(tmp.path().join("f.txt")).unwrap();
    assert_eq!(f, "newest", "applied stash@{{1}}");
    assert_eq!(stash::list(tmp.path()).unwrap().len(), 2, "apply keeps");

    sh(Some(tmp.path()), &["git", "checkout", "-q", "--", "f.txt"]);
    stash::pop_at(tmp.path(), 1).unwrap();
    assert_eq!(stash::list(tmp.path()).unwrap().len(), 1, "pop drops");
}
