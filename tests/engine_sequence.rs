//! Engine tests for the paused-sequence surface: state detection,
//! continue / skip / abort.

mod common;

use common::{fixture_repo, sh, sh_out};
use worktree_tool::engine::sequence::{self, InProgress};

/// Two branches that conflict on f.txt, with `main` checking out the
/// conflicting content first. Returns after `git merge side` has been
/// run and conflicted (paused).
fn paused_merge(dir: &std::path::Path) {
    fixture_repo(dir);
    sh(Some(dir), &["git", "checkout", "-qb", "side"]);
    std::fs::write(dir.join("f.txt"), "side change").unwrap();
    sh(Some(dir), &["git", "commit", "-qam", "side"]);
    sh(Some(dir), &["git", "checkout", "-q", "main"]);
    std::fs::write(dir.join("f.txt"), "main change").unwrap();
    sh(Some(dir), &["git", "commit", "-qam", "main"]);
    // Conflicts; deliberately left paused via the low-level command.
    let status = std::process::Command::new("git")
        .args(["merge", "side"])
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(!status.success(), "fixture merge must conflict");
}

#[test]
fn detects_no_state_on_a_clean_repo() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    assert_eq!(sequence::operation_state(tmp.path()), None);
    assert_eq!(sequence::rebase_progress(tmp.path()), None);
}

#[test]
fn detects_a_paused_merge() {
    let tmp = tempfile::tempdir().unwrap();
    paused_merge(tmp.path());
    assert_eq!(
        sequence::operation_state(tmp.path()),
        Some(InProgress::Merge)
    );
    sequence::abort_op(tmp.path()).unwrap();
    assert_eq!(sequence::operation_state(tmp.path()), None);
}

#[test]
fn continue_refuses_while_unresolved_and_persists() {
    let tmp = tempfile::tempdir().unwrap();
    paused_merge(tmp.path());
    // Unresolved files come back as the Ok payload (same convention as
    // the action ops); the pause survives.
    let conflicts = sequence::continue_op(tmp.path()).unwrap();
    assert_eq!(conflicts, vec!["f.txt".to_string()]);
    assert_eq!(
        sequence::operation_state(tmp.path()),
        Some(InProgress::Merge),
        "the pause survives the refusal"
    );
    sequence::abort_op(tmp.path()).unwrap();
}

#[test]
fn continue_completes_a_resolved_merge() {
    let tmp = tempfile::tempdir().unwrap();
    paused_merge(tmp.path());
    std::fs::write(tmp.path().join("f.txt"), "resolved\n").unwrap();
    sh(Some(tmp.path()), &["git", "add", "f.txt"]);
    let conflicts = sequence::continue_op(tmp.path()).unwrap();
    assert!(conflicts.is_empty());
    assert_eq!(sequence::operation_state(tmp.path()), None);
    let log = sh_out(tmp.path(), &["git", "log", "--format=%s", "-1"]);
    assert!(log.contains("Merge"), "merge commit created: {log}");
}

#[test]
fn rebase_state_precedes_cherry_pick_head() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    sh(Some(tmp.path()), &["git", "checkout", "-qb", "side"]);
    std::fs::write(tmp.path().join("f.txt"), "side change").unwrap();
    sh(Some(tmp.path()), &["git", "commit", "-qam", "side"]);
    sh(Some(tmp.path()), &["git", "checkout", "-q", "main"]);
    std::fs::write(tmp.path().join("f.txt"), "main change").unwrap();
    sh(Some(tmp.path()), &["git", "commit", "-qam", "main"]);
    let _ = std::process::Command::new("git")
        .args(["rebase", "side"])
        .current_dir(tmp.path())
        .status()
        .unwrap();
    assert_eq!(
        sequence::operation_state(tmp.path()),
        Some(InProgress::Rebase),
        "rebase detected while paused"
    );
    // Older gits ALSO set CHERRY_PICK_HEAD during a rebase pick: the
    // rebase marker must still win. Simulate by planting the marker.
    std::fs::write(tmp.path().join(".git").join("CHERRY_PICK_HEAD"), "x").unwrap();
    assert_eq!(
        sequence::operation_state(tmp.path()),
        Some(InProgress::Rebase),
        "rebase outranks the cherry-pick marker"
    );
    let (cur, total) = sequence::rebase_progress(tmp.path()).expect("progress files");
    assert_eq!(cur, 1);
    assert!(total >= 1);
    sequence::abort_op(tmp.path()).unwrap();
}

#[test]
fn skip_advances_a_paused_rebase() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    sh(Some(tmp.path()), &["git", "checkout", "-qb", "side"]);
    std::fs::write(tmp.path().join("f.txt"), "side change").unwrap();
    sh(Some(tmp.path()), &["git", "commit", "-qam", "side"]);
    sh(Some(tmp.path()), &["git", "checkout", "-q", "main"]);
    std::fs::write(tmp.path().join("f.txt"), "main change").unwrap();
    sh(Some(tmp.path()), &["git", "commit", "-qam", "main"]);
    let _ = std::process::Command::new("git")
        .args(["rebase", "side"])
        .current_dir(tmp.path())
        .status()
        .unwrap();

    // Skip the conflicting step: the rebase completes (single-pick plan).
    let conflicts = sequence::skip_op(tmp.path()).unwrap();
    assert!(conflicts.is_empty(), "skip landed clean: {conflicts:?}");
    assert_eq!(sequence::operation_state(tmp.path()), None);
    let log = sh_out(tmp.path(), &["git", "log", "--format=%s"]);
    assert!(
        log.contains("side"),
        "rebase completed past the skip: {log}"
    );
}

#[test]
fn skip_refuses_on_a_paused_merge() {
    let tmp = tempfile::tempdir().unwrap();
    paused_merge(tmp.path());
    let err = sequence::skip_op(tmp.path()).unwrap_err();
    assert!(err.message.contains("skip"), "merge skip refusal: {err}");
    sequence::abort_op(tmp.path()).unwrap();
}

#[test]
fn abort_restores_the_worktree_exactly() {
    let tmp = tempfile::tempdir().unwrap();
    paused_merge(tmp.path());
    sequence::abort_op(tmp.path()).unwrap();
    let content = std::fs::read_to_string(tmp.path().join("f.txt")).unwrap();
    assert_eq!(content, "main change", "pre-merge content restored");
    assert_eq!(sequence::operation_state(tmp.path()), None);
    let status = sh_out(tmp.path(), &["git", "status", "--porcelain"]);
    assert!(status.is_empty(), "clean tree after abort: {status:?}");
}

#[test]
fn abort_and_continue_without_state_refuse() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    assert!(sequence::abort_op(tmp.path()).is_err());
    assert!(sequence::continue_op(tmp.path()).is_err());
    assert!(sequence::skip_op(tmp.path()).is_err());
}
