//! GPUI-store tests for `WorkingCopyStore`: grouping, list-bounded
//! selection, async detail loading, and mutation flagging.

use gpui::TestAppContext;
use worktree_tool::engine::working_copy::Group;
use worktree_tool::wc_store::{Pane, WorkingCopyStore};

fn sh(cwd: Option<&std::path::Path>, cmd: &[&str]) {
    let status = std::process::Command::new(cmd[0])
        .args(&cmd[1..])
        .current_dir(cwd.unwrap_or(std::path::Path::new(".")))
        .status()
        .expect("spawn");
    assert!(status.success(), "failed: {cmd:?}");
}

/// fixture repo + one staged mod (f.txt), one unstaged mod (g.txt), one
/// untracked (u.txt)
fn fixture(cx_work: &std::path::Path) {
    sh(Some(cx_work), &["git", "init", "-q", "-b", "main"]);
    sh(Some(cx_work), &["git", "config", "user.email", "t@t.t"]);
    sh(Some(cx_work), &["git", "config", "user.name", "t"]);
    sh(Some(cx_work), &["git", "config", "commit.gpgsign", "false"]);
    std::fs::write(cx_work.join("f.txt"), "one").unwrap();
    std::fs::write(cx_work.join("g.txt"), "one").unwrap();
    sh(Some(cx_work), &["git", "add", "."]);
    sh(Some(cx_work), &["git", "commit", "-qm", "init"]);
    std::fs::write(cx_work.join("f.txt"), "one changed").unwrap();
    sh(Some(cx_work), &["git", "add", "--", "f.txt"]);
    std::fs::write(cx_work.join("g.txt"), "one changed").unwrap();
    std::fs::write(cx_work.join("u.txt"), "new").unwrap();
}

#[gpui::test]
fn refresh_groups_and_selection(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    let store = cx.update(|cx| WorkingCopyStore::new(tmp.path().to_path_buf(), cx));
    cx.run_until_parked();
    store.update(cx, |wc, cx| {
        let rows = wc.rows();
        let groups: Vec<Group> = rows.iter().map(|(g, _)| *g).collect();
        assert_eq!(
            groups,
            vec![Group::Staged, Group::Unstaged, Group::Untracked]
        );
        assert_eq!(wc.staged_count(), 1);
        // first row selected by default, its diff loaded
        assert_eq!(wc.selected, Some(0));
        assert!(matches!(wc.pane, Pane::Files));
        let (group, entry) = wc.selected_row().unwrap();
        assert_eq!(group, Group::Staged);
        assert_eq!(entry.path, "f.txt");
        assert!(
            wc.detail.is_some(),
            "diff should load for the selected file"
        );
        cx.notify();
    });
}

#[gpui::test]
fn selection_moves_within_groups_and_loads_diffs(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    let store = cx.update(|cx| WorkingCopyStore::new(tmp.path().to_path_buf(), cx));
    cx.run_until_parked();
    store.update(cx, |wc, cx| {
        // Staged group has 1 row → select_next parks there, second call
        // moves to Unstaged group (adjacent), third to Untracked.
        wc.select_next(cx);
        assert_eq!(wc.selected, Some(1));
        let (group, entry) = wc.selected_row().unwrap();
        assert_eq!((group, entry.path.as_str()), (Group::Unstaged, "g.txt"));
        wc.select_next(cx);
        let (group, entry) = wc.selected_row().unwrap();
        assert_eq!((group, entry.path.as_str()), (Group::Untracked, "u.txt"));
        // list-bounded: one more step would stay on the last row of the list
        wc.select_next(cx);
        assert_eq!(wc.selected, Some(2));
        wc.select_prev(cx);
        wc.select_prev(cx);
        wc.select_prev(cx);
        assert_eq!(
            wc.selected,
            Some(0),
            "list-bounded: stopped at the first row"
        );
    });
    // The detail view loads off the background executor: jump back to the
    // untracked row and let its preview arrive.
    store.update(cx, |wc, cx| {
        wc.select(Some(2), cx);
    });
    cx.run_until_parked();
    store.update(cx, |wc, _cx| {
        assert_eq!(wc.selected, Some(2));
        let (group, entry) = wc.selected_row().unwrap();
        assert_eq!((group, entry.path.as_str()), (Group::Untracked, "u.txt"));
        // untracked row → preview detail, not diff
        assert!(matches!(
            wc.detail,
            Some(worktree_tool::wc_store::FileDetail::Preview(_))
        ));
    });
}

#[gpui::test]
fn toggle_stage_and_discard_mutate_and_flag(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    let store = cx.update(|cx| WorkingCopyStore::new(tmp.path().to_path_buf(), cx));
    cx.run_until_parked();
    // select the unstaged g.txt row and stage it
    store.update(cx, |wc, cx| {
        wc.select(Some(1), cx);
    });
    cx.run_until_parked();
    store.update(cx, |wc, cx| {
        wc.toggle_stage(cx);
    });
    cx.run_until_parked();
    store.update(cx, |wc, _cx| {
        assert!(wc.take_mutated(), "mutation must set the home-refresh flag");
        assert!(!wc.take_mutated(), "flag is consumed once");
        // g.txt is now staged: groups changed
        assert_eq!(wc.staged_count(), 2);
    });
    cx.run_until_parked();
    // discard the untracked file
    store.update(cx, |wc, cx| {
        wc.select(Some(wc.rows().len() - 1), cx);
    });
    cx.run_until_parked();
    store.update(cx, |wc, cx| {
        assert!(matches!(wc.selected_row(), Some((Group::Untracked, _))));
        wc.discard_path(true, "u.txt".to_string(), cx);
    });
    cx.run_until_parked();
    assert!(!tmp.path().join("u.txt").exists());
    store.update(cx, |wc, _cx| {
        assert!(wc.take_mutated());
    });
}

#[gpui::test]
fn staging_a_modified_rename_row_stages_the_follow_up_edit(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    // Staged rename, then the new path edited again: status v2 emits a
    // `2 RM` record (new path present in BOTH Staged and Unstaged groups).
    sh(Some(tmp.path()), &["git", "mv", "f.txt", "renamed.txt"]);
    std::fs::write(tmp.path().join("renamed.txt"), "moved\nedited\n").unwrap();
    let store = cx.update(|cx| WorkingCopyStore::new(tmp.path().to_path_buf(), cx));
    cx.run_until_parked();
    // Select the Unstaged renamed.txt row and press s.
    store.update(&mut cx.clone(), |wc, cx| {
        let pos = wc
            .rows()
            .iter()
            .position(|(g, i)| {
                *g == Group::Unstaged && wc.wc.as_ref().unwrap().entries[*i].path == "renamed.txt"
            })
            .expect("RM record has an Unstaged row");
        wc.select(Some(pos), cx);
    });
    cx.run_until_parked();
    store.update(&mut cx.clone(), |wc, cx| wc.toggle_stage(cx));
    cx.run_until_parked();
    store.update(&mut cx.clone(), |wc, _cx| {
        // `s` on the Unstaged surface stages the follow-up edit without
        // error (the bug was `git add` also targeting the rename's vanished
        // old path, aborting the whole invocation). Git's result: the
        // rename stays staged with the newer content — f.txt's staged
        // deletion REMAINS, as half of the rename.
        let renamed = wc
            .wc
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .find(|e| e.path == "renamed.txt")
            .expect("renamed.txt still listed");
        assert_eq!(renamed.wt_status, '.', "follow-up edit fully staged");
        assert!(wc.staged_count() >= 1, "g.txt edit staged");
    });
}

#[gpui::test]
fn unstaging_a_pure_rename_resets_both_paths(cx: &mut gpui::TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    // Minimal state with NO other changes, and f.txt committed UNMODIFIED
    // so `git mv` yields a single `2 R` record (the fixture's staged f.txt
    // modification makes git report D + A instead — rename detection fails
    // once content diverges — and those are independent entries).
    sh(Some(tmp.path()), &["git", "init", "-q", "-b", "main"]);
    sh(Some(tmp.path()), &["git", "config", "user.email", "t@t.t"]);
    sh(Some(tmp.path()), &["git", "config", "user.name", "t"]);
    sh(
        Some(tmp.path()),
        &["git", "config", "commit.gpgsign", "false"],
    );
    std::fs::write(tmp.path().join("f.txt"), "one").unwrap();
    sh(Some(tmp.path()), &["git", "add", "."]);
    sh(Some(tmp.path()), &["git", "commit", "-qm", "init"]);
    sh(Some(tmp.path()), &["git", "mv", "f.txt", "moved.txt"]);
    let store = cx.update(|cx| WorkingCopyStore::new(tmp.path().to_path_buf(), cx));
    cx.run_until_parked();
    store.update(&mut cx.clone(), |wc, cx| {
        let pos = wc
            .rows()
            .iter()
            .position(|(g, i)| {
                *g == Group::Staged && wc.wc.as_ref().unwrap().entries[*i].path == "moved.txt"
            })
            .expect("staged rename row present");
        wc.select(Some(pos), cx);
    });
    cx.run_until_parked();
    store.update(&mut cx.clone(), |wc, cx| wc.toggle_stage(cx));
    cx.run_until_parked();
    store.update(&mut cx.clone(), |wc, _cx| {
        let entries = &wc.wc.as_ref().unwrap().entries;
        assert!(
            entries.iter().all(|e| e.index_status == '.' || e.untracked),
            "unstage resets both rename paths (no staged entries): got {:?}",
            entries
        );
        assert!(
            tmp.path().join("moved.txt").exists(),
            "worktree file untouched by the unstage"
        );
        assert!(
            entries.iter().any(|e| e.path == "moved.txt" && e.untracked),
            "moved.txt back to untracked"
        );
    });
}

#[gpui::test]
fn staged_summary_lists_files(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    let store = cx.update(|cx| WorkingCopyStore::new(tmp.path().to_path_buf(), cx));
    cx.run_until_parked();
    store.update(cx, |wc, _cx| {
        let summary = worktree_tool::wc_store::staged_summary(wc.wc.as_ref().unwrap());
        assert_eq!(summary, "1 staged file: f.txt");
    });
}

#[gpui::test]
fn discard_refuses_when_live_tracked_state_contradicts_the_dialog(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    let store = cx.update(|cx| WorkingCopyStore::new(tmp.path().to_path_buf(), cx));
    cx.run_until_parked();
    // The dialog opened on untracked u.txt; an external `git add` flips it
    // to tracked before confirm (the store snapshot is not refreshed, so
    // the dialog's flag no longer describes reality).
    store.update(cx, |wc, cx| {
        let pos = wc
            .rows()
            .iter()
            .position(|(g, i)| {
                *g == Group::Untracked && wc.wc.as_ref().unwrap().entries[*i].path == "u.txt"
            })
            .expect("fixture has an untracked u.txt row");
        wc.select(Some(pos), cx);
    });
    cx.run_until_parked();
    sh(Some(tmp.path()), &["git", "add", "u.txt"]);
    store.update(cx, |wc, cx| {
        wc.discard_path(true, "u.txt".to_string(), cx);
    });
    cx.run_until_parked();
    // Refused: the file and its content are untouched, the snapshot was
    // refreshed to the flipped state, and the user is told to reopen the
    // dialog — acting on either the stale snapshot or the flipped live
    // state alone could delete or restore on wrong information.
    assert!(tmp.path().join("u.txt").exists());
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("u.txt")).unwrap(),
        "new"
    );
    store.update(cx, |wc, _cx| {
        assert!(!wc.take_mutated(), "a refusal is not a mutation");
        assert!(
            wc.message
                .as_deref()
                .unwrap_or_default()
                .contains("state changed"),
            "expected the flip refusal, got {:?}",
            wc.message
        );
        let staged = wc.rows().iter().any(|(g, i)| {
            *g == Group::Staged && wc.wc.as_ref().unwrap().entries[*i].path == "u.txt"
        });
        assert!(
            staged,
            "snapshot refreshed to the flipped state after refusal"
        );
    });
}

/// The escape hatch is platform-neutral (std `Child::kill`), but a killable
/// sleeping editor is trivial to script only on unix — the cmd.exe child
/// tree on Windows would outlive the kill and add nothing to the coverage.
#[cfg(unix)]
#[gpui::test]
fn abandon_commit_kills_a_wedged_editor_and_unwinds(cx: &mut TestAppContext) {
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());
    let _env = ENV_LOCK.lock().unwrap();

    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    let script = tmp.path().join("slow-editor.sh");
    std::fs::write(&script, "#!/bin/sh\nsleep 30\n").unwrap();
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::env::set_var("GIT_EDITOR", &script);

    let store = cx.update(|cx| WorkingCopyStore::new(tmp.path().to_path_buf(), cx));
    cx.run_until_parked();
    // Stage the unstaged g.txt so there is something to commit.
    store.update(cx, |wc, cx| {
        let pos = wc
            .rows()
            .iter()
            .position(|(g, i)| {
                *g == Group::Unstaged && wc.wc.as_ref().unwrap().entries[*i].path == "g.txt"
            })
            .expect("fixture has an unstaged g.txt row");
        wc.select(Some(pos), cx);
    });
    cx.run_until_parked();
    store.update(cx, |wc, cx| wc.toggle_stage(cx));
    cx.run_until_parked();
    store.update(cx, |wc, _cx| {
        // Consume the stage's home-refresh flag so the abandon assertions
        // below observe the abandon itself, not the earlier stage.
        assert!(wc.take_mutated());
    });
    store.update(cx, |wc, cx| {
        wc.commit_with_editor(cx);
        assert!(wc.commit_editor_active(), "editor session is in flight");
    });
    // The "editor" is a 30s sleep — wedged from the test's perspective.
    // Abandon it whether or not the child has spawned yet: the hatch sets a
    // flag that the wait loop checks before and during its poll ticks.
    store.update(cx, |wc, cx| wc.abandon_commit(cx));
    cx.run_until_parked();
    store.update(cx, |wc, _cx| {
        assert!(!wc.commit_editor_active(), "session unwound");
        assert!(
            wc.message
                .as_deref()
                .unwrap_or_default()
                .starts_with("Commit abandoned"),
            "expected the abandoned notice, got {:?}",
            wc.message
        );
        assert!(!wc.take_mutated(), "an abandoned commit is not a mutation");
        assert_eq!(wc.staged_count(), 2, "staged set untouched by the abandon");
    });
    std::env::remove_var("GIT_EDITOR");
}

/// A committed 12-line file edited on lines 1 and 10 → two unstaged hunks.
fn two_hunk_file(dir: &std::path::Path) {
    let lines: Vec<String> = (1..=12).map(|i| format!("line {i}")).collect();
    std::fs::write(dir.join("h.txt"), lines.join("\n") + "\n").unwrap();
    sh(Some(dir), &["git", "add", "h.txt"]);
    sh(Some(dir), &["git", "commit", "-qm", "h"]);
    let mut edited = lines.clone();
    edited[0] = "line 1 edited".into();
    edited[9] = "line 10 edited".into();
    std::fs::write(dir.join("h.txt"), edited.join("\n") + "\n").unwrap();
}

#[gpui::test]
fn stage_hunk_stages_only_the_hovered_hunk(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    two_hunk_file(tmp.path());
    let store = cx.update(|cx| WorkingCopyStore::new(tmp.path().to_path_buf(), cx));
    cx.run_until_parked();
    // Select the unstaged h.txt row.
    store.update(cx, |wc, cx| {
        let pos = wc
            .rows()
            .iter()
            .position(|(g, i)| {
                *g == Group::Unstaged && wc.wc.as_ref().unwrap().entries[*i].path == "h.txt"
            })
            .expect("fixture has an unstaged h.txt row");
        wc.select(Some(pos), cx);
    });
    cx.run_until_parked();
    store.update(cx, |wc, _cx| {
        assert_eq!(wc.hunk_count(), Some(2));
        assert_eq!(wc.hunk_cursor(), 0, "cursor starts on the first hunk");
    });
    // Hover the second hunk, then stage it.
    store.update(cx, |wc, cx| wc.hunk_next(cx));
    store.update(cx, |wc, _cx| {
        assert_eq!(wc.hunk_cursor(), 1);
    });
    store.update(cx, |wc, cx| wc.stage_hunk(cx));
    cx.run_until_parked();
    store.update(cx, |wc, _cx| {
        assert!(wc.take_mutated(), "staging a hunk flags the home refresh");
        // The staged diff now holds exactly the hovered (second) hunk…
        assert_eq!(wc.hunk_count(), Some(1), "unstaged diff shrank to one hunk");
        assert_eq!(wc.hunk_cursor(), 0, "cursor clamped to the shrunken diff");
    });
    // …and git agrees: the index holds only the line-10 edit.
    let staged = worktree_tool::engine::diff::diff_staged(tmp.path(), "h.txt").unwrap();
    assert_eq!(staged.hunks.len(), 1);
    assert!(staged.hunks[0]
        .raw
        .windows(14)
        .any(|w| w == b"line 10 edited"));
    // The worktree still holds both edits.
    let on_disk = std::fs::read_to_string(tmp.path().join("h.txt")).unwrap();
    assert!(on_disk.contains("line 1 edited"));
    assert!(on_disk.contains("line 10 edited"));
}

#[gpui::test]
fn hunk_cursor_resets_on_selection_change(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    two_hunk_file(tmp.path());
    let store = cx.update(|cx| WorkingCopyStore::new(tmp.path().to_path_buf(), cx));
    cx.run_until_parked();
    let unstaged_h = |wc: &WorkingCopyStore| {
        wc.rows()
            .iter()
            .position(|(g, i)| {
                *g == Group::Unstaged && wc.wc.as_ref().unwrap().entries[*i].path == "h.txt"
            })
            .expect("unstaged h.txt row")
    };
    store.update(cx, |wc, cx| {
        let pos = unstaged_h(wc);
        wc.select(Some(pos), cx);
    });
    cx.run_until_parked();
    store.update(cx, |wc, cx| {
        wc.hunk_next(cx);
        wc.hunk_next(cx); // saturates at the last hunk
        assert_eq!(wc.hunk_cursor(), 1, "next stops at the last hunk");
        wc.hunk_prev(cx);
        assert_eq!(wc.hunk_cursor(), 0);
        wc.hunk_next(cx);
    });
    // Selecting a different row (then back) resets the cursor to 0.
    store.update(cx, |wc, cx| {
        let other = wc
            .rows()
            .iter()
            .position(|(g, i)| {
                *g == Group::Unstaged && wc.wc.as_ref().unwrap().entries[*i].path == "g.txt"
            })
            .expect("unstaged g.txt row");
        wc.select(Some(other), cx);
    });
    cx.run_until_parked();
    store.update(cx, |wc, cx| {
        assert_eq!(wc.hunk_cursor(), 0, "selection change resets the cursor");
        let back = unstaged_h(wc);
        wc.select(Some(back), cx);
    });
    cx.run_until_parked();
    store.update(cx, |wc, _cx| {
        assert_eq!(wc.hunk_cursor(), 0);
    });
}

#[gpui::test]
fn stage_hunk_hints_on_ineligible_rows(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    two_hunk_file(tmp.path());
    // A modified binary file: unstaged row whose diff is binary. (Committed
    // BEFORE staging g.txt — a later commit would sweep the staged edit.)
    std::fs::write(tmp.path().join("b.bin"), [0u8, 1, 2, 3]).unwrap();
    sh(Some(tmp.path()), &["git", "add", "b.bin"]);
    sh(Some(tmp.path()), &["git", "commit", "-qm", "b"]);
    std::fs::write(tmp.path().join("b.bin"), [0u8, 1, 2, 4]).unwrap();
    // two_hunk_file's commit swept the fixture's staged f.txt, so stage a
    // fresh edit LAST to keep a Staged row.
    std::fs::write(tmp.path().join("g.txt"), "staged edit").unwrap();
    sh(Some(tmp.path()), &["git", "add", "g.txt"]);

    let store = cx.update(|cx| WorkingCopyStore::new(tmp.path().to_path_buf(), cx));
    cx.run_until_parked();
    let row_of = |wc: &WorkingCopyStore, path: &str, group: Group| {
        wc.rows()
            .iter()
            .position(|(g, i)| *g == group && wc.wc.as_ref().unwrap().entries[*i].path == path)
            .unwrap_or_else(|| panic!("{group:?} row for {path}"))
    };
    // Staged row: hunk staging only speaks unstaged.
    store.update(cx, |wc, cx| {
        wc.select(Some(row_of(wc, "g.txt", Group::Staged)), cx);
    });
    cx.run_until_parked();
    store.update(cx, |wc, cx| {
        wc.stage_hunk(cx);
        assert!(
            wc.message
                .as_deref()
                .unwrap_or_default()
                .contains("unstaged"),
            "expected the staged-row hint, got {:?}",
            wc.message
        );
    });
    // Binary diff: whole-file only.
    store.update(cx, |wc, cx| {
        wc.select(Some(row_of(wc, "b.bin", Group::Unstaged)), cx);
    });
    cx.run_until_parked();
    store.update(cx, |wc, cx| {
        wc.stage_hunk(cx);
        assert!(
            wc.message.as_deref().unwrap_or_default().contains("binary"),
            "expected the binary hint, got {:?}",
            wc.message
        );
        assert!(!wc.take_mutated(), "a hint is not a mutation");
    });
    // Untracked row: no hunks, file-level only.
    store.update(cx, |wc, cx| {
        wc.select(Some(row_of(wc, "u.txt", Group::Untracked)), cx);
    });
    cx.run_until_parked();
    store.update(cx, |wc, cx| {
        wc.stage_hunk(cx);
        assert!(
            wc.message
                .as_deref()
                .unwrap_or_default()
                .contains("file row"),
            "expected the whole-file hint, got {:?}",
            wc.message
        );
    });
    // Untracked DIRECTORY row: same whole-file hint — never a silent dead
    // key (dirs hit the group match, not an early return).
    std::fs::create_dir(tmp.path().join("newdir")).unwrap();
    std::fs::write(tmp.path().join("newdir/x.txt"), "x").unwrap();
    sh(Some(tmp.path()), &["git", "status", "--porcelain=v2"]);
    store.update(cx, |wc, cx| {
        wc.refresh(cx);
    });
    cx.run_until_parked();
    store.update(cx, |wc, cx| {
        wc.select(Some(row_of(wc, "newdir/", Group::Untracked)), cx);
    });
    cx.run_until_parked();
    store.update(cx, |wc, cx| {
        wc.stage_hunk(cx);
        assert!(
            wc.message
                .as_deref()
                .unwrap_or_default()
                .contains("file row"),
            "expected the whole-file hint for a dir row, got {:?}",
            wc.message
        );
    });
}

/// A mode-only change (`chmod +x`) renders a header-only diff: non-binary
/// with ZERO hunks. The cursor clamp must not underflow (regression:
/// `n - 1` wrapped to usize::MAX / panicked in debug). Windows ignores
/// filemode, so the fixture is unix-only.
#[cfg(unix)]
#[gpui::test]
fn zero_hunk_diff_keeps_cursor_clamped_and_hints(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    std::fs::write(tmp.path().join("m.sh"), "#!/bin/sh\necho hi\n").unwrap();
    sh(Some(tmp.path()), &["git", "add", "m.sh"]);
    sh(Some(tmp.path()), &["git", "commit", "-qm", "m"]);
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(
        tmp.path().join("m.sh"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();

    let store = cx.update(|cx| WorkingCopyStore::new(tmp.path().to_path_buf(), cx));
    cx.run_until_parked();
    store.update(cx, |wc, cx| {
        let pos = wc
            .rows()
            .iter()
            .position(|(g, i)| {
                *g == Group::Unstaged && wc.wc.as_ref().unwrap().entries[*i].path == "m.sh"
            })
            .expect("mode-only change has an unstaged row");
        wc.select(Some(pos), cx);
    });
    cx.run_until_parked();
    store.update(cx, |wc, _cx| {
        assert_eq!(wc.hunk_count(), Some(0), "header-only diff has no hunks");
        assert_eq!(wc.hunk_cursor(), 0, "clamp saturates — no underflow");
    });
    // `s` in the diff pane explains instead of dying silently.
    store.update(cx, |wc, cx| {
        wc.stage_hunk(cx);
        assert!(
            wc.message
                .as_deref()
                .unwrap_or_default()
                .contains("no hunks"),
            "expected the no-hunks hint, got {:?}",
            wc.message
        );
        assert!(!wc.take_mutated(), "a hint is not a mutation");
    });
}

/// The cursor and `stage_hunk` are bounded by the RENDERED hunks: a
/// 5000+ line diff truncates, and staging a hunk whose header the pane
/// never drew would act on content the user cannot see (the
/// MAX_VISIBLE_ROWS bug, diff-pane edition).
#[gpui::test]
fn hunk_cursor_never_leaves_the_rendered_range(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    // 12000-line file: rewriting the first 2000 lines makes hunk 1 (~4000
    // diff lines) fit entirely under the 5000-line cap; the 600-line edit
    // near the far end is hunk 2 (~1200 diff lines) — cumulatively past
    // the cap, so the pane truncates BEFORE it.
    let mut lines: Vec<String> = (1..=12000).map(|i| format!("line {i}")).collect();
    std::fs::write(tmp.path().join("big.txt"), lines.join("\n") + "\n").unwrap();
    sh(Some(tmp.path()), &["git", "add", "big.txt"]);
    sh(Some(tmp.path()), &["git", "commit", "-qm", "big"]);
    for (i, l) in lines.iter_mut().enumerate() {
        if i < 2000 {
            *l = format!("edited {i}");
        } else if (11000..11600).contains(&i) {
            *l = format!("tail edit {i}");
        }
    }
    std::fs::write(tmp.path().join("big.txt"), lines.join("\n") + "\n").unwrap();

    let store = cx.update(|cx| WorkingCopyStore::new(tmp.path().to_path_buf(), cx));
    cx.run_until_parked();
    store.update(cx, |wc, cx| {
        let pos = wc
            .rows()
            .iter()
            .position(|(g, i)| {
                *g == Group::Unstaged && wc.wc.as_ref().unwrap().entries[*i].path == "big.txt"
            })
            .expect("unstaged big.txt row");
        wc.select(Some(pos), cx);
    });
    cx.run_until_parked();
    store.update(cx, |wc, _cx| {
        assert_eq!(wc.hunk_count(), Some(2), "git produced two hunks");
        assert_eq!(wc.hunk_bound(), 1, "hunk 2 exceeds the cumulative cap");
    });
    store.update(cx, |wc, cx| {
        wc.hunk_next(cx);
        wc.hunk_next(cx);
        assert_eq!(
            wc.hunk_cursor(),
            0,
            "cursor saturates at the rendered range"
        );
        wc.stage_hunk(cx);
    });
    cx.run_until_parked();
    // The staged change comes from the VISIBLE hunk only.
    let staged = worktree_tool::engine::diff::diff_staged(tmp.path(), "big.txt").unwrap();
    assert!(!staged.binary);
    assert!(staged.hunks.len() == 1, "one hunk staged");
    assert!(staged.hunks[0].raw.windows(8).any(|w| w == b"edited 0"));
    // The truncated hunk's content must not appear anywhere in the index.
    assert!(
        !staged
            .hunks
            .iter()
            .any(|h| String::from_utf8_lossy(&h.raw).contains("tail edit")),
        "the truncated hunk must not be staged"
    );
    assert!(!String::from_utf8_lossy(&staged.hunks[0].raw).contains("line 12000 edited"));
}

/// The detail lags the selection: between `select()` and the async detail
/// load landing, `detail` still describes the PREVIOUS file. Staging in
/// that window would apply file A's hunk under file B's passed eligibility
/// checks — git's preimage check cannot catch pure-insertion hunks. The
/// mismatch must refuse until the new diff arrives.
#[gpui::test]
fn stage_hunk_refuses_while_the_detail_lags_the_selection(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    two_hunk_file(tmp.path());
    let store = cx.update(|cx| WorkingCopyStore::new(tmp.path().to_path_buf(), cx));
    cx.run_until_parked();
    let row_of = |wc: &WorkingCopyStore, path: &str| {
        wc.rows()
            .iter()
            .position(|(g, i)| {
                *g == Group::Unstaged && wc.wc.as_ref().unwrap().entries[*i].path == path
            })
            .unwrap_or_else(|| panic!("unstaged row for {path}"))
    };
    store.update(cx, |wc, cx| {
        wc.select(Some(row_of(wc, "h.txt")), cx);
    });
    cx.run_until_parked();
    // Select g.txt and press s in the SAME update: h.txt's diff is still
    // loaded, g.txt's is still in flight.
    store.update(cx, |wc, cx| {
        wc.select(Some(row_of(wc, "g.txt")), cx);
        wc.stage_hunk(cx);
        assert!(
            wc.message
                .as_deref()
                .unwrap_or_default()
                .contains("loading"),
            "expected the detail-lag refusal, got {:?}",
            wc.message
        );
        assert!(!wc.take_mutated(), "a refusal is not a mutation");
    });
    cx.run_until_parked();
    // Nothing was staged for either file — no wrong-file hunk landed.
    let staged = worktree_tool::engine::diff::diff_staged(tmp.path(), "g.txt").unwrap();
    assert!(staged.hunks.is_empty(), "g.txt must have no staged hunks");
    let staged_h = worktree_tool::engine::diff::diff_staged(tmp.path(), "h.txt").unwrap();
    assert!(staged_h.hunks.is_empty(), "h.txt must have no staged hunks");
    // Once the load lands, staging works normally.
    store.update(cx, |wc, cx| {
        wc.stage_hunk(cx);
    });
    cx.run_until_parked();
    let staged = worktree_tool::engine::diff::diff_staged(tmp.path(), "g.txt").unwrap();
    assert_eq!(staged.hunks.len(), 1, "g.txt staged whole (single hunk)");
}

/// Same file, OTHER surface: selecting the Staged row loads the staged
/// diff; switching to the same path's Unstaged row in the detail-lag
/// window must still refuse — a (path, kind) match is required, or the
/// staged diff's hunks would be applied against the unstaged row's
/// expectations (pure insertions would duplicate in the index).
#[gpui::test]
fn stage_hunk_refuses_when_the_detail_lags_the_surface(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    two_hunk_file(tmp.path());
    // Give h.txt BOTH surfaces: stage the two-hunk edit, then edit again.
    sh(Some(tmp.path()), &["git", "add", "h.txt"]);
    std::fs::write(tmp.path().join("h.txt"), "first\n\n10th\nline 12 edited\n").unwrap();

    let store = cx.update(|cx| WorkingCopyStore::new(tmp.path().to_path_buf(), cx));
    cx.run_until_parked();
    let row_of = |wc: &WorkingCopyStore, group: Group| {
        wc.rows()
            .iter()
            .position(|(g, i)| *g == group && wc.wc.as_ref().unwrap().entries[*i].path == "h.txt")
            .unwrap_or_else(|| panic!("{group:?} row for h.txt"))
    };
    store.update(cx, |wc, cx| {
        wc.select(Some(row_of(wc, Group::Staged)), cx);
    });
    cx.run_until_parked();
    // Switch to the same path's Unstaged row and press s in the SAME
    // update: the staged diff is still what `detail` holds.
    store.update(cx, |wc, cx| {
        wc.select(Some(row_of(wc, Group::Unstaged)), cx);
        wc.stage_hunk(cx);
        assert!(
            wc.message
                .as_deref()
                .unwrap_or_default()
                .contains("loading"),
            "expected the surface-lag refusal, got {:?}",
            wc.message
        );
        assert!(!wc.take_mutated(), "a refusal is not a mutation");
    });
    cx.run_until_parked();
    // Once the unstaged diff lands, staging works on the right surface.
    store.update(cx, |wc, cx| {
        wc.stage_hunk(cx);
    });
    cx.run_until_parked();
    let staged = worktree_tool::engine::diff::diff_staged(tmp.path(), "h.txt").unwrap();
    let all = staged
        .hunks
        .iter()
        .map(|h| String::from_utf8_lossy(&h.raw).into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        all.contains("line 12 edited"),
        "unstaged hunk staged: {all}"
    );
    let unstaged = worktree_tool::engine::diff::diff_unstaged(tmp.path(), "h.txt").unwrap();
    assert!(unstaged.hunks.is_empty(), "nothing left unstaged");
}

/// A file with BOTH a staged and an unstaged record (two `1 M` entries,
/// same path) must keep an unstaged selection on its Unstaged row across a
/// refresh. Regression: resolving the ENTRY first always picked the staged
/// record, snapping the selection onto the Staged row after staging a hunk
/// — which broke the stage-successive-hunks flow.
#[gpui::test]
fn refresh_keeps_an_unstaged_selection_on_a_dual_group_file(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    std::fs::write(tmp.path().join("f.txt"), "one changed").unwrap();
    sh(Some(tmp.path()), &["git", "add", "f.txt"]);
    std::fs::write(tmp.path().join("f.txt"), "one changed again").unwrap();

    let store = cx.update(|cx| WorkingCopyStore::new(tmp.path().to_path_buf(), cx));
    cx.run_until_parked();
    let unstaged_f = |wc: &WorkingCopyStore| {
        wc.rows()
            .iter()
            .position(|(g, i)| {
                *g == Group::Unstaged && wc.wc.as_ref().unwrap().entries[*i].path == "f.txt"
            })
            .expect("unstaged f.txt row")
    };
    store.update(cx, |wc, cx| {
        wc.select(Some(unstaged_f(wc)), cx);
    });
    cx.run_until_parked();
    store.update(cx, |wc, cx| wc.refresh(cx));
    cx.run_until_parked();
    store.update(cx, |wc, _cx| {
        let (group, entry) = wc.selected_row().expect("selection survived the refresh");
        assert_eq!(group, Group::Unstaged, "selection must stay unstaged");
        assert_eq!(entry.path, "f.txt");
    });
}
