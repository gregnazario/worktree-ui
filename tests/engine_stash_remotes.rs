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
        &["git", "init", "-q", "--bare", remote.to_str().unwrap()],
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
        &["git", "init", "-q", "--bare", remote.to_str().unwrap()],
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
