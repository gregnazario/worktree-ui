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
        // Worktree diverges from the index after staging: without --force
        // git refuses to `rm --cached` a file whose staged content is not
        // in HEAD (there IS no HEAD yet).
        std::fs::write(tmp.path().join("n.txt"), "new file\nedited").unwrap();
        // No commit yet: `reset HEAD` would fail; unstage must still work.
        mutate::unstage(tmp.path(), &["n.txt".to_string()]).unwrap();
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("n.txt")).unwrap(),
            "new file\nedited",
            "unstage never touches the worktree file"
        );
        let wc = worktree_tool::engine::working_copy::status(tmp.path()).unwrap();
        assert!(
            wc.entries.iter().any(|e| e.path == "n.txt" && e.untracked),
            "unstage on unborn HEAD drops the file back to untracked"
        );
    }

    #[test]
    fn stage_batches_by_count_and_byte_length() {
        let tmp = tempfile::tempdir().unwrap();
        common::fixture_repo(tmp.path());
        // 600 paths of ~90 chars each: ~54 KB of arguments total, so both
        // the 200-path count bound and the 16 KiB byte bound must trigger.
        let mut paths: Vec<String> = Vec::new();
        let dir = tmp
            .path()
            .join("deep")
            .join("w".repeat(60))
            .join("x".repeat(20));
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..600u32 {
            let rel = format!("deep/{}/f-{:04}-{}.txt", "w".repeat(60), i, "x".repeat(20));
            std::fs::write(tmp.path().join(&rel), "x").unwrap();
            paths.push(rel);
        }
        mutate::stage(tmp.path(), &paths).unwrap();
        let wc = worktree_tool::engine::working_copy::status(tmp.path()).unwrap();
        let staged = wc.entries.iter().filter(|e| e.index_status == 'A').count();
        assert_eq!(staged, 600, "every path staged across all batches");
    }

    // macOS rejects non-UTF-8 filenames at the syscall level (EILSEQ);
    // Linux and FreeBSD accept raw bytes.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn status_flags_non_utf8_names_unsupported_only_on_lossy() {
        use std::os::unix::ffi::OsStrExt;
        use worktree_tool::engine::working_copy::status;
        let tmp = tempfile::tempdir().unwrap();
        common::fixture_repo(tmp.path());
        // A file whose NAME is not valid UTF-8 (raw bytes).
        let bad = tmp
            .path()
            .join(std::ffi::OsStr::from_bytes(b"bad\xffname.txt"));
        std::fs::write(&bad, "x").unwrap();

        let wc = status(tmp.path()).unwrap();
        assert!(
            wc.entries.iter().any(|e| e.unsupported),
            "lossy-decoded mangled name flagged unsupported"
        );

        // A name that legitimately contains U+FFFD (valid UTF-8) must NOT
        // be flagged: its pathspec round-trips fine.
        let ok = tmp.path().join("ok\u{FFFD}name.txt");
        std::fs::write(&ok, "y").unwrap();
        let wc = status(tmp.path()).unwrap();
        let entry = wc
            .entries
            .iter()
            .find(|e| e.path == "ok\u{FFFD}name.txt")
            .unwrap();
        assert!(!entry.unsupported, "valid UTF-8 name is stgable");
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

mod apply_tests {
    use super::common::{fixture_repo, sh};
    use std::path::Path;
    use worktree_tool::engine::{self, diff, mutate};

    /// A committed 12-line file edited on line 1 and line 10 — the two
    /// edits are far enough apart that `git diff -U3` yields two hunks.
    fn two_hunk_repo(dir: &Path) -> Vec<String> {
        fixture_repo(dir);
        let lines: Vec<String> = (1..=12).map(|i| format!("line {i}")).collect();
        std::fs::write(dir.join("h.txt"), lines.join("\n") + "\n").unwrap();
        sh(Some(dir), &["git", "add", "h.txt"]);
        sh(Some(dir), &["git", "commit", "-qm", "h"]);
        let mut edited = lines.clone();
        edited[0] = "line 1 edited".into();
        edited[9] = "line 10 edited".into();
        std::fs::write(dir.join("h.txt"), edited.join("\n") + "\n").unwrap();
        lines
    }

    #[test]
    fn parse_preserves_header_bytes_verbatim() {
        let input: &[u8] =
            b"diff --git a/h.txt b/h.txt\nindex 11aa..22bb 100644\n--- a/h.txt\n+++ b/h.txt\n@@ -1 +1 @@\n-a\n+b\n";
        let ud = diff::parse_unified_diff(input);
        let first_hunk = input.iter().position(|b| *b == b'@').unwrap();
        assert_eq!(ud.header_raw, &input[..first_hunk]);
        assert_eq!(ud.hunks.len(), 1);
    }

    #[test]
    fn apply_cached_stages_only_the_selected_hunk() {
        let tmp = tempfile::tempdir().unwrap();
        two_hunk_repo(tmp.path());

        let ud = diff::diff_unstaged(tmp.path(), "h.txt").unwrap();
        assert_eq!(ud.hunks.len(), 2, "edits on lines 1 and 10 make two hunks");

        // Reconstruct the patch exactly as the app will: header bytes plus
        // the hovered hunk's byte-exact raw.
        let mut patch = ud.header_raw.clone();
        patch.extend_from_slice(&ud.hunks[0].raw);
        let expected = ud.index_pre_image.clone();
        mutate::apply_cached(tmp.path(), "h.txt", patch.clone(), expected.as_deref()).unwrap();

        // The index holds only the hunk-1 change…
        let staged = diff::diff_staged(tmp.path(), "h.txt").unwrap();
        assert_eq!(staged.hunks.len(), 1);
        assert!(staged.hunks[0]
            .raw
            .windows(13)
            .any(|w| w == b"line 1 edited"));
        // …the remaining change is still unstaged…
        let unstaged = diff::diff_unstaged(tmp.path(), "h.txt").unwrap();
        assert_eq!(unstaged.hunks.len(), 1);
        assert!(unstaged.hunks[0]
            .raw
            .windows(14)
            .any(|w| w == b"line 10 edited"));
        // …and the worktree file is untouched by `apply --cached`.
        let on_disk = std::fs::read_to_string(tmp.path().join("h.txt")).unwrap();
        assert!(on_disk.contains("line 1 edited"));
        assert!(on_disk.contains("line 10 edited"));
    }

    #[test]
    fn stale_patch_fails_cleanly_with_git_stderr() {
        let tmp = tempfile::tempdir().unwrap();
        two_hunk_repo(tmp.path());
        let ud = diff::diff_unstaged(tmp.path(), "h.txt").unwrap();
        let mut patch = ud.header_raw.clone();
        patch.extend_from_slice(&ud.hunks[0].raw);
        let expected = ud.index_pre_image.clone();
        mutate::apply_cached(tmp.path(), "h.txt", patch.clone(), expected.as_deref()).unwrap();

        // The index already contains this hunk: its blob no longer matches
        // the diff's post-image, and the explicit check refuses (the UI
        // surfaces this as "press r and try again") — never a silent
        // success, and never a duplicated pure-insertion hunk.
        let err =
            mutate::apply_cached(tmp.path(), "h.txt", patch, expected.as_deref()).unwrap_err();
        assert!(
            matches!(err, mutate::ApplyError::StaleIndex),
            "expected the stale-index refusal, got: {err}"
        );
    }

    #[test]
    fn apply_cached_handles_no_trailing_newline() {
        let tmp = tempfile::tempdir().unwrap();
        fixture_repo(tmp.path());
        std::fs::write(tmp.path().join("n.txt"), "one").unwrap();
        sh(Some(tmp.path()), &["git", "add", "n.txt"]);
        sh(Some(tmp.path()), &["git", "commit", "-qm", "n"]);
        std::fs::write(tmp.path().join("n.txt"), "one!").unwrap();

        let ud = diff::diff_unstaged(tmp.path(), "n.txt").unwrap();
        assert_eq!(ud.hunks.len(), 1);
        let mut patch = ud.header_raw.clone();
        patch.extend_from_slice(&ud.hunks[0].raw);
        let expected = ud.index_pre_image.clone();
        mutate::apply_cached(tmp.path(), "n.txt", patch, expected.as_deref()).unwrap();

        let staged = diff::diff_staged(tmp.path(), "n.txt").unwrap();
        assert_eq!(staged.hunks.len(), 1);
        assert!(staged.hunks[0].raw.windows(4).any(|w| w == b"one!"));
    }

    /// Staging a content hunk of a file whose diff ALSO carries a mode
    /// change must not flip the index entry's mode: the mode lines are
    /// stripped from the reconstructed patch (git add -p asks about the
    /// mode separately; so do we — via the file-level `s`).
    #[cfg(unix)]
    #[test]
    fn content_patch_strips_mode_lines() {
        let tmp = tempfile::tempdir().unwrap();
        fixture_repo(tmp.path());
        let f = tmp.path().join("x.sh");
        std::fs::write(&f, "echo hi\n").unwrap();
        sh(Some(tmp.path()), &["git", "add", "x.sh"]);
        sh(Some(tmp.path()), &["git", "commit", "-qm", "x"]);
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(&f, "echo edited\n").unwrap();

        let ud = diff::diff_unstaged(tmp.path(), "x.sh").unwrap();
        assert!(
            ud.header_raw.windows(9).any(|w| w == b"index 100644") || ud.header.contains("100644")
        );
        let patch = mutate::content_patch(&ud.header_raw, &ud.hunks[0].raw);
        assert!(
            !patch.windows(9).any(|w| w == b"old mode "),
            "mode lines must be stripped: {}",
            String::from_utf8_lossy(&patch)
        );
        mutate::apply_cached(
            tmp.path(),
            "x.sh",
            patch,
            ud.index_pre_image.clone().as_deref(),
        )
        .unwrap();

        // The index entry's mode is untouched; the content hunk is staged.
        let ls = engine::run_trimmed(tmp.path(), &["ls-files", "-s", "--", "x.sh"]);
        assert!(
            ls.as_ref().unwrap().starts_with("100644"),
            "mode unchanged: {ls:?}"
        );
        let staged = diff::diff_staged(tmp.path(), "x.sh").unwrap();
        assert!(staged.hunks[0].raw.windows(6).any(|w| w == b"edited"));
    }

    #[test]
    fn run_bytes_stdin_feeds_the_child_process() {
        let tmp = tempfile::tempdir().unwrap();
        fixture_repo(tmp.path());
        std::fs::write(tmp.path().join("p.bin"), b"payload").unwrap();
        // `git hash-object` of the stdin bytes must equal the hash of a
        // file with identical content — proving stdin reaches git intact.
        let via_stdin =
            engine::run_bytes_stdin(tmp.path(), &["hash-object", "--stdin"], b"payload".to_vec())
                .unwrap();
        let via_file = engine::run_bytes(tmp.path(), &["hash-object", "p.bin"]).unwrap();
        assert_eq!(via_stdin, via_file, "stdin bytes must reach git verbatim");
    }
}
