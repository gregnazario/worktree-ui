mod common;

use common::{fixture_repo, sh, sh_allow_fail};
use worktree_tool::engine;

#[test]
fn run_preserves_nul_records_untrimmed() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    std::fs::write(tmp.path().join("n.txt"), "x").unwrap();
    let out = engine::run(
        tmp.path(),
        &["status", "--porcelain=v2", "-z", "--untracked-files=normal"],
    )
    .unwrap();
    assert!(
        out.contains('\0'),
        "-z output must keep NUL separators: {out:?}"
    );
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
    assert!(
        err.is_lock_error(),
        "expected lock error, got: {}",
        err.message
    );
}

mod diff_tests {
    use worktree_tool::engine::diff::{self, DiffLineKind, Preview};

    use super::common;

    #[test]
    fn unstaged_and_staged_diffs_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = Some(tmp.path());
        common::fixture_repo(tmp.path());
        std::fs::write(tmp.path().join("f.txt"), "one\ntwo\n").unwrap();
        common::sh(cwd, &["git", "add", "--", "f.txt"]); // staged
        std::fs::write(tmp.path().join("f.txt"), "one\nTHREE\n").unwrap(); // unstaged on top

        let staged = diff::diff_staged(tmp.path(), "f.txt").unwrap();
        assert_eq!(staged.hunks.len(), 1);
        // `git diff --cached` is HEAD→index, so the staged "two" is an
        // addition (the brief's draft asserted Del here; real git emits
        // `+two`).
        assert!(staged.hunks[0]
            .lines
            .iter()
            .any(|l| l.content == "two" && l.kind == DiffLineKind::Add));

        let unstaged = diff::diff_unstaged(tmp.path(), "f.txt").unwrap();
        assert_eq!(unstaged.hunks.len(), 1);
        assert!(unstaged.hunks[0]
            .lines
            .iter()
            .any(|l| l.content == "THREE" && l.kind == DiffLineKind::Add));

        // clean file → empty diff, no error
        let empty = diff::diff_unstaged(tmp.path(), "does-not-exist.txt").unwrap();
        assert!(empty.hunks.is_empty());
    }

    #[test]
    fn preview_classifies_text_binary_dir_and_truncation() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("t.txt"), "hello").unwrap();
        std::fs::write(tmp.path().join("b.bin"), [0u8, 1, 2, 3]).unwrap();
        std::fs::create_dir(tmp.path().join("subdir")).unwrap();
        let big = "x".repeat(300 * 1024);
        std::fs::write(tmp.path().join("big.txt"), &big).unwrap();

        match diff::read_preview(tmp.path(), "t.txt") {
            Preview::Text { content, truncated } => {
                assert_eq!(content, "hello");
                assert!(!truncated);
            }
            other => panic!("expected text, got {other:?}"),
        }
        assert!(matches!(
            diff::read_preview(tmp.path(), "b.bin"),
            Preview::Binary
        ));
        assert!(matches!(
            diff::read_preview(tmp.path(), "subdir"),
            Preview::Directory
        ));
        assert!(matches!(
            diff::read_preview(tmp.path(), "nope"),
            Preview::Missing
        ));
        match diff::read_preview(tmp.path(), "big.txt") {
            Preview::Text { content, truncated } => {
                assert!(truncated);
                assert_eq!(content.len(), diff::PREVIEW_MAX_BYTES);
            }
            other => panic!("expected truncated text, got {other:?}"),
        }
    }
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
