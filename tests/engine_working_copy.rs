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

mod mutate_tests {
    use worktree_tool::engine::mutate;
    use worktree_tool::engine::working_copy::status;

    use super::common;
    use common::sh;

    #[test]
    fn stage_unstage_discard_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        common::fixture_repo(tmp.path());
        std::fs::write(tmp.path().join("f.txt"), "changed").unwrap();
        std::fs::write(tmp.path().join("u.txt"), "untracked").unwrap();

        // stage file + untracked file, then unstage one
        mutate::stage(tmp.path(), &["f.txt".to_string(), "u.txt".to_string()]).unwrap();
        let wc = status(tmp.path()).unwrap();
        let f = wc.entries.iter().find(|e| e.path == "f.txt").unwrap();
        assert_eq!(f.index_status, 'M');
        let u = wc.entries.iter().find(|e| e.path == "u.txt").unwrap();
        assert_eq!(u.index_status, 'A'); // untracked → staged new file
        mutate::unstage(tmp.path(), &["u.txt".to_string()]).unwrap();
        let wc = status(tmp.path()).unwrap();
        assert!(
            wc.entries
                .iter()
                .find(|e| e.path == "u.txt")
                .unwrap()
                .untracked
        );

        // discard unstaged: restores the worktree file from the index, so
        // the staged delta survives. f.txt is staged as "changed"; add a
        // further unstaged edit, then throw away only that unstaged delta.
        std::fs::write(tmp.path().join("f.txt"), "changed2").unwrap();
        mutate::discard_unstaged(tmp.path(), "f.txt").unwrap();
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("f.txt")).unwrap(),
            "changed" // staged content survived
        );
        let wc = status(tmp.path()).unwrap();
        let f = wc.entries.iter().find(|e| e.path == "f.txt").unwrap();
        assert_eq!(f.index_status, 'M');
        // porcelain v2 marks a clean worktree delta as '.', it does not
        // omit the file while the index still differs from HEAD.
        assert_eq!(f.wt_status, '.');

        // discard untracked: file gone
        mutate::discard_untracked(tmp.path(), "u.txt").unwrap();
        assert!(!tmp.path().join("u.txt").exists());
    }

    #[test]
    fn dash_and_space_names_stay_positional() {
        let tmp = tempfile::tempdir().unwrap();
        common::fixture_repo(tmp.path());
        // A file literally named "-weird name.txt" must never be an option.
        let weird = "-weird name.txt";
        std::fs::write(tmp.path().join(weird), "x").unwrap();
        mutate::stage(tmp.path(), &[weird.to_string()]).unwrap();
        let wc = status(tmp.path()).unwrap();
        assert_eq!(wc.entries[0].path, weird);
        mutate::unstage(tmp.path(), &[weird.to_string()]).unwrap();
        let wc = status(tmp.path()).unwrap();
        assert!(wc.entries[0].untracked);

        // discard_unstaged must also stay positional: stage it, add an
        // unstaged delta on top, then restore the worktree from the index.
        mutate::stage(tmp.path(), &[weird.to_string()]).unwrap();
        std::fs::write(tmp.path().join(weird), "y").unwrap();
        mutate::discard_unstaged(tmp.path(), weird).unwrap();
        assert_eq!(
            std::fs::read_to_string(tmp.path().join(weird)).unwrap(),
            "x" // staged content survived
        );
    }

    #[test]
    fn empty_paths_are_noops() {
        let tmp = tempfile::tempdir().unwrap();
        common::fixture_repo(tmp.path());
        mutate::stage(tmp.path(), &[]).unwrap();
        mutate::unstage(tmp.path(), &[]).unwrap();
    }

    #[test]
    fn unstage_on_unborn_head_untracks_new_files() {
        let tmp = tempfile::tempdir().unwrap();
        sh(Some(tmp.path()), &["git", "init", "-q", "-b", "main"]);
        sh(Some(tmp.path()), &["git", "config", "user.email", "t@t.t"]);
        sh(Some(tmp.path()), &["git", "config", "user.name", "t"]);
        std::fs::write(tmp.path().join("n.txt"), "new file").unwrap();
        mutate::stage(tmp.path(), &["n.txt".to_string()]).unwrap();
        // No commit yet: `reset HEAD` would fail; unstage must still work.
        mutate::unstage(tmp.path(), &["n.txt".to_string()]).unwrap();
        let wc = worktree_tool::engine::working_copy::status(tmp.path()).unwrap();
        assert!(
            wc.entries.iter().any(|e| e.path == "n.txt" && e.untracked),
            "unstage on unborn HEAD drops the file back to untracked"
        );
    }

    #[test]
    fn status_overrides_show_untracked_files_config() {
        let tmp = tempfile::tempdir().unwrap();
        common::fixture_repo(tmp.path());
        std::fs::write(tmp.path().join("u.txt"), "untracked").unwrap();
        // A user config must not silently empty the app's Untracked group.
        sh(
            Some(tmp.path()),
            &["git", "config", "status.showUntrackedFiles", "no"],
        );
        let wc = worktree_tool::engine::working_copy::status(tmp.path()).unwrap();
        assert!(
            wc.entries.iter().any(|e| e.path == "u.txt" && e.untracked),
            "status() forces --untracked-files=normal"
        );
    }

    #[test]
    fn discard_untracked_refuses_directories() {
        let tmp = tempfile::tempdir().unwrap();
        common::fixture_repo(tmp.path());
        std::fs::create_dir(tmp.path().join("sub")).unwrap();
        assert!(mutate::discard_untracked(tmp.path(), "sub").is_err());
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
