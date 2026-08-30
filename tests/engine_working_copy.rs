mod common;

use common::fixture_repo;
use worktree_tool::engine;

#[test]
fn run_preserves_nul_records_untrimmed() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    std::fs::write(tmp.path().join("n.txt"), "x").unwrap();
    let out = engine::run(tmp.path(), &["status", "--porcelain=v2", "-z", "--untracked-files=normal"]).unwrap();
    assert!(out.contains('\0'), "-z output must keep NUL separators: {out:?}");
    assert!(out.contains("n.txt"));
}

#[test]
fn run_reports_last_stderr_line() {
    let tmp = tempfile::tempdir().unwrap();
    let err = engine::run(tmp.path(), &["rev-parse", "--show-toplevel"]).unwrap_err();
    assert!(!err.message.is_empty());
    assert!(!err.is_lock_error());
}

#[test]
fn lock_contention_is_classified() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    std::fs::write(tmp.path().join(".git/index.lock"), "").unwrap();
    let err = engine::run(tmp.path(), &["add", "--", "f.txt"]).unwrap_err();
    assert!(err.is_lock_error(), "expected lock error, got: {}", err.message);
}
