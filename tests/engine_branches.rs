//! Engine tests for branch operations and merge.

mod common;

use common::{fixture_repo, sh, sh_out};
use worktree_tool::engine::branches;

#[test]
fn list_returns_local_branches_with_current_marker() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    sh(Some(tmp.path()), &["git", "checkout", "-qb", "feature"]);
    sh(Some(tmp.path()), &["git", "checkout", "-q", "main"]);

    let branches = branches::list(tmp.path()).unwrap();
    assert!(branches.len() >= 2);
    let main = branches.iter().find(|b| b.short == "main").unwrap();
    let feat = branches.iter().find(|b| b.short == "feature").unwrap();
    assert!(main.is_current, "main is checked out");
    assert!(!feat.is_current);
    assert!(!main.is_remote);
}

#[test]
fn list_reports_ahead_and_behind() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    sh(Some(tmp.path()), &["git", "checkout", "-qb", "dev"]);
    std::fs::write(tmp.path().join("a.txt"), "a").unwrap();
    sh(Some(tmp.path()), &["git", "add", "a.txt"]);
    sh(Some(tmp.path()), &["git", "commit", "-qm", "a"]);
    // main gets a commit too
    sh(Some(tmp.path()), &["git", "checkout", "-q", "main"]);
    std::fs::write(tmp.path().join("b.txt"), "b").unwrap();
    sh(Some(tmp.path()), &["git", "add", "b.txt"]);
    sh(Some(tmp.path()), &["git", "commit", "-qm", "b"]);
    // dev fetches main's commit and adds another
    sh(Some(tmp.path()), &["git", "checkout", "-q", "dev"]);
    sh(Some(tmp.path()), &["git", "cherry-pick", "main"]);
    sh(
        Some(tmp.path()),
        &["git", "branch", "--set-upstream-to=main", "dev"],
    );

    let branches = branches::list(tmp.path()).unwrap();
    let dev = branches.iter().find(|b| b.short == "dev").unwrap();
    assert!(dev.ahead >= 1 || dev.behind >= 1, "dev diverged");
}

#[test]
fn create_and_switch_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    branches::create(tmp.path(), "feature/test").unwrap();
    branches::switch(tmp.path(), "feature/test").unwrap();
    let branches = branches::list(tmp.path()).unwrap();
    let feat = branches.iter().find(|b| b.short == "feature/test").unwrap();
    assert!(feat.is_current);
}

#[test]
fn create_rejects_invalid_names() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    assert!(branches::create(tmp.path(), "-evil").is_err());
    assert!(branches::create(tmp.path(), "has space").is_err());
    assert!(branches::create(tmp.path(), "").is_err());
}

#[test]
fn rename_preserves_commits() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    sh(Some(tmp.path()), &["git", "checkout", "-qb", "old-name"]);
    let old_sha = sh_out(tmp.path(), &["git", "rev-parse", "HEAD"]);
    branches::rename(tmp.path(), "old-name", "new-name").unwrap();
    let new_sha = sh_out(tmp.path(), &["git", "rev-parse", "new-name"]);
    assert_eq!(old_sha, new_sha, "rename preserves the tip");
    let branches = branches::list(tmp.path()).unwrap();
    assert!(branches.iter().any(|b| b.short == "new-name"));
    assert!(!branches.iter().any(|b| b.short == "old-name"));
}

#[test]
fn delete_refuses_current_and_deletes_other() {
    let tmp = tempfile::tempdir().unwrap();
    two_branch_repo(tmp.path());
    let err = branches::delete(tmp.path(), "main", "main").unwrap_err();
    assert!(err.message.contains("current branch"), "{err}");
    branches::delete(tmp.path(), "feature", "main").unwrap();
    let branches = branches::list(tmp.path()).unwrap();
    assert!(!branches.iter().any(|b| b.short == "feature"));
}

fn two_branch_repo(dir: &std::path::Path) {
    fixture_repo(dir);
    sh(Some(dir), &["git", "checkout", "-qb", "feature"]);
    sh(Some(dir), &["git", "checkout", "-q", "main"]);
}

#[test]
fn merge_clean_fast_forward() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    sh(Some(tmp.path()), &["git", "checkout", "-qb", "feature"]);
    std::fs::write(tmp.path().join("feat.txt"), "feat").unwrap();
    sh(Some(tmp.path()), &["git", "add", "feat.txt"]);
    sh(Some(tmp.path()), &["git", "commit", "-qm", "feat"]);
    sh(Some(tmp.path()), &["git", "checkout", "-q", "main"]);

    let conflicts = branches::merge(tmp.path(), "feature").unwrap();
    assert!(conflicts.is_empty());
    let head_subject = sh_out(tmp.path(), &["git", "log", "-1", "--format=%s"]);
    assert_eq!(head_subject, "feat", "ff merge moved main to feature");
}

#[test]
fn merge_with_conflicts_reports_conflicted_files() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    sh(Some(tmp.path()), &["git", "checkout", "-qb", "side"]);
    std::fs::write(tmp.path().join("f.txt"), "side change").unwrap();
    sh(Some(tmp.path()), &["git", "commit", "-qam", "side"]);
    sh(Some(tmp.path()), &["git", "checkout", "-q", "main"]);
    std::fs::write(tmp.path().join("f.txt"), "main change").unwrap();
    sh(Some(tmp.path()), &["git", "commit", "-qam", "main"]);

    let err = branches::merge(tmp.path(), "side").unwrap_err();
    assert!(
        err.message.contains("conflicts"),
        "expected conflict report, got: {err}"
    );
}

#[test]
fn rebase_onto_another_branch() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    sh(Some(tmp.path()), &["git", "checkout", "-qb", "feature"]);
    std::fs::write(tmp.path().join("feat.txt"), "feat").unwrap();
    sh(Some(tmp.path()), &["git", "add", "feat.txt"]);
    sh(Some(tmp.path()), &["git", "commit", "-qm", "feat"]);
    sh(Some(tmp.path()), &["git", "checkout", "-q", "main"]);
    std::fs::write(tmp.path().join("main.txt"), "main").unwrap();
    sh(Some(tmp.path()), &["git", "add", "main.txt"]);
    sh(Some(tmp.path()), &["git", "commit", "-qm", "main"]);
    sh(Some(tmp.path()), &["git", "checkout", "-q", "feature"]);

    let conflicts = branches::rebase(tmp.path(), "main").unwrap();
    assert!(conflicts.is_empty());
    let log = sh_out(tmp.path(), &["git", "log", "--oneline", "feature"]);
    assert!(log.contains("main"), "rebased onto main: {log}");
}
