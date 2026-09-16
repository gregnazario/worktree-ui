//! Engine tests for history-rewrite operations: cherry-pick, revert,
//! and the scripted interactive rebase.

mod common;

use common::{fixture_repo, sh, sh_allow_fail, sh_out};
use worktree_tool::engine::rewrite::{self, TodoAction, TodoStep};

/// Repo on `main` with one commit; `side` branches off with two commits
/// touching independent files. Returns (dir, side_tip, side_base).
fn two_side_commits(dir: &std::path::Path) -> (String, String) {
    fixture_repo(dir);
    sh(Some(dir), &["git", "checkout", "-qb", "side"]);
    std::fs::write(dir.join("a.txt"), "aaa").unwrap();
    sh(Some(dir), &["git", "add", "a.txt"]);
    sh(Some(dir), &["git", "commit", "-qm", "side one"]);
    let side_base = sh_out(dir, &["git", "rev-parse", "HEAD"]);
    std::fs::write(dir.join("b.txt"), "bbb").unwrap();
    sh(Some(dir), &["git", "add", "b.txt"]);
    sh(Some(dir), &["git", "commit", "-qm", "side two"]);
    let side_tip = sh_out(dir, &["git", "rev-parse", "HEAD"]);
    sh(Some(dir), &["git", "checkout", "-q", "main"]);
    (side_tip, side_base)
}

#[test]
fn cherry_pick_applies_a_foreign_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let (side_tip, _) = two_side_commits(tmp.path());

    rewrite::cherry_pick(tmp.path(), &side_tip).unwrap();
    let log = sh_out(tmp.path(), &["git", "log", "--oneline", "main"]);
    assert!(log.contains("side two"), "picked commit landed: {log}");
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("b.txt")).unwrap(),
        "bbb",
        "picked content present"
    );
}

#[test]
fn cherry_pick_conflict_aborts_and_reports() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    sh(Some(tmp.path()), &["git", "checkout", "-qb", "side"]);
    std::fs::write(tmp.path().join("f.txt"), "side change").unwrap();
    sh(Some(tmp.path()), &["git", "commit", "-qam", "side"]);
    let side_tip = sh_out(tmp.path(), &["git", "rev-parse", "HEAD"]);
    sh(Some(tmp.path()), &["git", "checkout", "-q", "main"]);
    std::fs::write(tmp.path().join("f.txt"), "main change").unwrap();
    sh(Some(tmp.path()), &["git", "commit", "-qam", "main"]);

    let conflicts = rewrite::cherry_pick(tmp.path(), &side_tip).unwrap();
    assert_eq!(conflicts, vec!["f.txt".to_string()]);

    let picking = tmp.path().join(".git").join("CHERRY_PICK_HEAD");
    assert!(!picking.exists(), "must not stay mid-cherry-pick");
    let content = std::fs::read_to_string(tmp.path().join("f.txt")).unwrap();
    assert_eq!(content, "main change", "worktree restored");
}

#[test]
fn revert_creates_a_revert_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let (side_tip, _) = two_side_commits(tmp.path());
    rewrite::cherry_pick(tmp.path(), &side_tip).unwrap();

    rewrite::revert(tmp.path(), &side_tip).unwrap();
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("b.txt"))
            .err()
            .map(|_| "gone")
            .unwrap_or("present"),
        "gone",
        "revert removed the file"
    );
    let log = sh_out(tmp.path(), &["git", "log", "--oneline", "main"]);
    assert!(log.contains("Revert"), "revert commit in log: {log}");
}

#[test]
fn revert_conflict_aborts_and_reports() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    std::fs::write(tmp.path().join("f.txt"), "one\ntwo\n").unwrap();
    sh(Some(tmp.path()), &["git", "commit", "-qam", "add two"]);
    let to_revert = sh_out(tmp.path(), &["git", "rev-parse", "HEAD"]);
    // A later commit rewrites the same lines: reverting "add two" now
    // conflicts.
    std::fs::write(tmp.path().join("f.txt"), "one\nrewritten\n").unwrap();
    sh(Some(tmp.path()), &["git", "commit", "-qam", "rewrite"]);

    let conflicts = rewrite::revert(tmp.path(), &to_revert).unwrap();
    assert_eq!(conflicts, vec!["f.txt".to_string()]);
    let reverting = tmp.path().join(".git").join("REVERT_HEAD");
    assert!(!reverting.exists(), "must not stay mid-revert");
    let content = std::fs::read_to_string(tmp.path().join("f.txt")).unwrap();
    assert_eq!(content, "one\nrewritten\n", "worktree restored");
}

#[test]
fn rejects_malformed_oids() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    for bad in [
        "-main",
        "HEAD",
        "main",
        &"a".repeat(39),
        &"g".repeat(40),
        "",
    ] {
        assert!(
            rewrite::cherry_pick(tmp.path(), bad).is_err(),
            "oid {bad:?} must be rejected"
        );
        assert!(
            rewrite::revert(tmp.path(), bad).is_err(),
            "oid {bad:?} must be rejected"
        );
    }
}

fn step(action: TodoAction, oid: &str, subject: &str) -> TodoStep {
    TodoStep {
        action,
        oid: oid.to_string(),
        subject: subject.to_string(),
    }
}

#[test]
fn rebase_todo_drops_and_reorders() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    // Three independent-file commits on main: c2, c3, c4.
    for (name, file) in [("c2", "f2.txt"), ("c3", "f3.txt"), ("c4", "f4.txt")] {
        std::fs::write(tmp.path().join(file), name).unwrap();
        sh(Some(tmp.path()), &["git", "add", "."]);
        sh(Some(tmp.path()), &["git", "commit", "-qm", name]);
    }
    let subjects: Vec<(String, String)> = ["c2", "c3", "c4"]
        .iter()
        .map(|s| {
            let line = sh_out(
                tmp.path(),
                &["git", "log", "--format=%H %s", "--grep", s, "-1"],
            );
            let (oid, subject) = line.split_once(' ').unwrap();
            (oid.to_string(), subject.to_string())
        })
        .collect();
    let base = sh_out(tmp.path(), &["git", "rev-parse", "HEAD~3"]);

    // Drop c3, reorder c4 before c2.
    let steps = vec![
        step(TodoAction::Pick, &subjects[2].0, &subjects[2].1),
        step(TodoAction::Pick, &subjects[0].0, &subjects[0].1),
    ];
    rewrite::run_rebase_todo(tmp.path(), &base, &steps).unwrap();

    let log = sh_out(tmp.path(), &["git", "log", "--format=%s", "main"]);
    let order: Vec<&str> = log.lines().collect();
    assert_eq!(order, vec!["c2", "c4", "init"], "c3 dropped, c4 below c2");
    assert!(!tmp.path().join("f3.txt").exists(), "c3's file gone");
}

#[test]
fn rebase_todo_fixup_squashes_without_editor() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    std::fs::write(tmp.path().join("f.txt"), "one\nmore\n").unwrap();
    sh(Some(tmp.path()), &["git", "commit", "-qam", "more"]);
    std::fs::write(tmp.path().join("f.txt"), "one\nmore\nextra\n").unwrap();
    sh(Some(tmp.path()), &["git", "commit", "-qam", "extra"]);

    let entries: Vec<(String, String)> = ["more", "extra"]
        .iter()
        .map(|s| {
            let line = sh_out(
                tmp.path(),
                &["git", "log", "--format=%H %s", "--grep", s, "-1"],
            );
            let (oid, subject) = line.split_once(' ').unwrap();
            (oid.to_string(), subject.to_string())
        })
        .collect();
    let base = sh_out(tmp.path(), &["git", "rev-parse", "HEAD~2"]);

    let steps = vec![
        step(TodoAction::Pick, &entries[0].0, &entries[0].1),
        step(TodoAction::Fixup, &entries[1].0, &entries[1].1),
    ];
    rewrite::run_rebase_todo(tmp.path(), &base, &steps).unwrap();

    let log = sh_out(tmp.path(), &["git", "log", "--format=%s", "main"]);
    let order: Vec<&str> = log.lines().collect();
    assert_eq!(order, vec!["more", "init"], "extra fixupped into more");
    let content = std::fs::read_to_string(tmp.path().join("f.txt")).unwrap();
    assert_eq!(content, "one\nmore\nextra\n", "fixup kept the content");
}

#[test]
fn rebase_todo_conflict_aborts_and_reports() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    // Two commits rewriting the SAME lines: dropping the first makes the
    // second unappliable.
    std::fs::write(tmp.path().join("f.txt"), "first edit\n").unwrap();
    sh(Some(tmp.path()), &["git", "commit", "-qam", "first edit"]);
    std::fs::write(tmp.path().join("f.txt"), "second edit\n").unwrap();
    sh(Some(tmp.path()), &["git", "commit", "-qam", "second edit"]);

    let first = sh_out(
        tmp.path(),
        &["git", "log", "--format=%H", "--grep", "first edit", "-1"],
    );
    let second = sh_out(
        tmp.path(),
        &["git", "log", "--format=%H", "--grep", "second edit", "-1"],
    );
    let base = sh_out(tmp.path(), &["git", "rev-parse", "HEAD~2"]);

    let steps = vec![
        step(TodoAction::Drop, &first, "first edit"),
        step(TodoAction::Pick, &second, "second edit"),
    ];
    let conflicts = rewrite::run_rebase_todo(tmp.path(), &base, &steps).unwrap();
    assert_eq!(conflicts, vec!["f.txt".to_string()]);

    let rebasing = tmp.path().join(".git").join("rebase-merge");
    let rebasing_apply = tmp.path().join(".git").join("rebase-apply");
    assert!(
        !rebasing.exists() && !rebasing_apply.exists(),
        "must not stay mid-rebase"
    );
    let log = sh_out(tmp.path(), &["git", "log", "--format=%s", "main"]);
    assert!(
        log.contains("second edit") && log.contains("first edit"),
        "both commits restored after abort: {log}"
    );
}

#[test]
fn rebase_todo_rejects_bad_input() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    let good = sh_out(tmp.path(), &["git", "rev-parse", "HEAD"]);

    let empty: Vec<TodoStep> = Vec::new();
    assert!(rewrite::run_rebase_todo(tmp.path(), &good, &empty).is_err());
    assert!(
        rewrite::run_rebase_todo(tmp.path(), "main", &[]).is_err(),
        "base must be an oid, not a ref"
    );
    let bad_step = vec![step(TodoAction::Pick, "-oops", "x")];
    assert!(rewrite::run_rebase_todo(tmp.path(), &good, &bad_step).is_err());
}

#[test]
fn cherry_pick_self_heals_a_wedged_sequence() {
    // A mid-sequence repo (deliberately conflicted cherry-pick, left
    // running via sh_allow_fail) — only reachable via external git use,
    // since the engine always aborts its own conflicts. The next pick
    // hits git's mid-sequence refusal; the conflicted-files probe then
    // finds the stale sequence's conflicts, aborts it, and reports the
    // paths: the repo is usable again in one step.
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    sh(Some(tmp.path()), &["git", "checkout", "-qb", "side"]);
    std::fs::write(tmp.path().join("f.txt"), "side").unwrap();
    sh(Some(tmp.path()), &["git", "commit", "-qam", "side"]);
    let side_tip = sh_out(tmp.path(), &["git", "rev-parse", "HEAD"]);
    sh(Some(tmp.path()), &["git", "checkout", "-q", "main"]);
    std::fs::write(tmp.path().join("f.txt"), "main").unwrap();
    sh(Some(tmp.path()), &["git", "commit", "-qam", "main"]);

    let _ = sh_allow_fail(
        Some(tmp.path()),
        &["git", "cherry-pick", &side_tip], // conflicts, left mid-sequence
    );
    let other = sh_out(tmp.path(), &["git", "rev-parse", "HEAD"]);
    let conflicts = rewrite::cherry_pick(tmp.path(), &other).unwrap();
    assert_eq!(conflicts, vec!["f.txt".to_string()]);
    assert!(
        !tmp.path().join(".git").join("CHERRY_PICK_HEAD").exists(),
        "sequence aborted by the self-heal"
    );
}
