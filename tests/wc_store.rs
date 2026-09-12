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
