//! Engine tests for branch operations and merge.

mod common;

use common::{fixture_repo, sh, sh_out};
use worktree_tool::engine::{branches, sequence};

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
fn list_includes_remote_tracking_branches_grouped_after_locals() {
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
    sh(Some(&work), &["git", "push", "-q", "origin", "main"]);
    sh(Some(&work), &["git", "fetch", "-q", "--all"]);

    let branches = branches::list(&work).unwrap();
    let remote_pos = branches
        .iter()
        .position(|b| b.short == "origin/main")
        .expect("origin/main listed");
    assert!(branches[remote_pos].is_remote);
    assert_eq!(
        branches[remote_pos].ref_name, "refs/remotes/origin/main",
        "full refname preserved for remote rows"
    );
    // Locals come first: every remote row sits after every local row.
    let last_local = branches
        .iter()
        .rposition(|b| !b.is_remote)
        .expect("at least one local branch");
    assert!(remote_pos > last_local, "remotes grouped after locals");
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
fn merge_with_conflicts_reports_conflicted_files_and_pauses() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    sh(Some(tmp.path()), &["git", "checkout", "-qb", "side"]);
    std::fs::write(tmp.path().join("f.txt"), "side change").unwrap();
    sh(Some(tmp.path()), &["git", "commit", "-qam", "side"]);
    sh(Some(tmp.path()), &["git", "checkout", "-q", "main"]);
    std::fs::write(tmp.path().join("f.txt"), "main change").unwrap();
    sh(Some(tmp.path()), &["git", "commit", "-qam", "main"]);

    // Conflicts come back as the Ok payload and the merge STAYS PAUSED:
    // the conflict surface resolves and continues (or aborts) in-app.
    let conflicts = branches::merge(tmp.path(), "side").unwrap();
    assert_eq!(conflicts, vec!["f.txt".to_string()]);

    let merging = tmp.path().join(".git").join("MERGE_HEAD");
    assert!(merging.exists(), "merge paused for resolution");
    let content = std::fs::read_to_string(tmp.path().join("f.txt")).unwrap();
    assert!(
        content.contains("<<<<<<<"),
        "conflicted content is in the worktree"
    );
    // Resolve + continue completes the merge; abort unwinds.
    std::fs::write(tmp.path().join("f.txt"), "resolved\n").unwrap();
    sh(Some(tmp.path()), &["git", "add", "f.txt"]);
    let done = sequence::continue_op(tmp.path()).unwrap();
    assert!(done.is_empty(), "continue after resolution: {done:?}");
    assert!(!merging.exists(), "merge completed");
    sh(Some(tmp.path()), &["git", "reset", "-q", "--hard", "HEAD"]);
}

#[test]
fn rebase_with_conflicts_reports_conflicted_files_and_pauses() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    sh(Some(tmp.path()), &["git", "checkout", "-qb", "side"]);
    std::fs::write(tmp.path().join("f.txt"), "side change").unwrap();
    sh(Some(tmp.path()), &["git", "commit", "-qam", "side"]);
    sh(Some(tmp.path()), &["git", "checkout", "-q", "main"]);
    std::fs::write(tmp.path().join("f.txt"), "main change").unwrap();
    sh(Some(tmp.path()), &["git", "commit", "-qam", "main"]);
    sh(Some(tmp.path()), &["git", "checkout", "-q", "side"]);

    let conflicts = branches::rebase(tmp.path(), "main").unwrap();
    assert_eq!(conflicts, vec!["f.txt".to_string()]);

    // The rebase is paused on the conflicted step.
    let rebasing = tmp.path().join(".git").join("rebase-merge");
    assert!(rebasing.exists(), "rebase paused for resolution");
    // Abort unwinds (the tested escape hatch).
    sequence::abort_op(tmp.path()).unwrap();
    assert!(!rebasing.exists(), "abort unwound the rebase");
    let head_branch = sh_out(tmp.path(), &["git", "branch", "--show-current"]);
    assert_eq!(head_branch, "side", "back on the branch after abort");
    let content = std::fs::read_to_string(tmp.path().join("f.txt")).unwrap();
    assert_eq!(content, "side change", "worktree restored to pre-rebase");
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
