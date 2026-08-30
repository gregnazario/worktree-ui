//! GPUI-store tests for `WorkingCopyStore`: grouping, group-bounded
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
        // group-bounded: one more step stays on the last row of Untracked
        wc.select_next(cx);
        assert_eq!(wc.selected, Some(2));
        wc.select_prev(cx);
        wc.select_prev(cx);
        wc.select_prev(cx);
        assert_eq!(wc.selected, Some(0), "group-bounded at the top");
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
        wc.discard_selected(cx);
    });
    cx.run_until_parked();
    assert!(!tmp.path().join("u.txt").exists());
    store.update(cx, |wc, _cx| {
        assert!(wc.take_mutated());
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
