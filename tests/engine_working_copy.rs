mod common;

use common::{fixture_repo, sh, sh_allow_fail};
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

mod status_tests {
    use super::*;

    #[test]
    fn status_composes_entries_branch_and_numstat() {
        let tmp = tempfile::tempdir().unwrap();
        fixture_repo(tmp.path());
        // staged modification + unstaged modification + untracked file
        std::fs::write(tmp.path().join("f.txt"), "two").unwrap();
        sh(Some(tmp.path()), &["git", "add", "--", "f.txt"]);
        std::fs::write(tmp.path().join("f.txt"), "three").unwrap();
        std::fs::write(tmp.path().join("new.txt"), "brand new\nfile").unwrap();

        let wc = worktree_tool::engine::working_copy::status(tmp.path()).unwrap();
        assert_eq!(wc.branch.head, "main");
        assert_eq!(wc.entries.len(), 2); // f.txt, new.txt

        let f = wc.entries.iter().find(|e| e.path == "f.txt").unwrap();
        assert_eq!(f.index_status, 'M');
        assert_eq!(f.wt_status, 'M');
        assert_eq!(f.staged_lines, Some((1, 1)));
        assert_eq!(f.unstaged_lines, Some((1, 1)));

        let n = wc.entries.iter().find(|e| e.path == "new.txt").unwrap();
        assert!(n.untracked);
        assert_eq!(n.unstaged_lines, None);
    }

    #[test]
    fn status_reports_conflicts() {
        let tmp = tempfile::tempdir().unwrap();
        fixture_repo(tmp.path());
        sh(Some(tmp.path()), &["git", "checkout", "-q", "-b", "side"]);
        std::fs::write(tmp.path().join("f.txt"), "side").unwrap();
        sh(Some(tmp.path()), &["git", "commit", "-qam", "side"]);
        sh(Some(tmp.path()), &["git", "checkout", "-q", "main"]);
        std::fs::write(tmp.path().join("f.txt"), "main").unwrap();
        sh(Some(tmp.path()), &["git", "commit", "-qam", "main"]);
        sh_allow_fail(Some(tmp.path()), &["git", "merge", "side"]); // conflict, exit != 0 by design
        let wc = worktree_tool::engine::working_copy::status(tmp.path()).unwrap();
        assert_eq!(wc.entries[0].conflict.as_deref(), Some("UU"));
    }

    #[test]
    fn numstat_skips_rename_orig_chunks_and_binary() {
        use worktree_tool::engine::working_copy::parse_numstat_z;
        // "a\td\tnew" + separate NUL chunk "old" (rename) + binary marker
        let parsed = parse_numstat_z("3\t1\trenamed.txt\u{0}old.txt\u{0}-\t-\tbin.bin\u{0}");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0], ("renamed.txt".to_string(), Some((3, 1))));
        assert_eq!(parsed[1], ("bin.bin".to_string(), None));
    }
}
