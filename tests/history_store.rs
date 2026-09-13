//! GPUI-store tests for `HistoryStore`: log loading, selection-driven
//! detail loads, load-more, and the checkout / worktree actions.

use gpui::TestAppContext;
use worktree_tool::history_store::HistoryStore;

fn sh(cwd: Option<&std::path::Path>, cmd: &[&str]) {
    let status = std::process::Command::new(cmd[0])
        .args(&cmd[1..])
        .current_dir(cwd.unwrap_or(std::path::Path::new(".")))
        .status()
        .expect("spawn");
    assert!(status.success(), "failed: {cmd:?}");
}

fn sh_out(cwd: &std::path::Path, cmd: &[&str]) -> String {
    let out = std::process::Command::new(cmd[0])
        .args(&cmd[1..])
        .current_dir(cwd)
        .output()
        .expect("spawn");
    assert!(out.status.success(), "failed: {cmd:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Four commits on main: init, a, b, c (each editing its own file).
fn four_commit_repo(dir: &std::path::Path) {
    sh(Some(dir), &["git", "init", "-q", "-b", "main"]);
    sh(Some(dir), &["git", "config", "user.email", "t@t.t"]);
    sh(Some(dir), &["git", "config", "user.name", "t"]);
    sh(Some(dir), &["git", "config", "commit.gpgsign", "false"]);
    for name in ["init", "a", "b", "c"] {
        std::fs::write(dir.join(format!("{name}.txt")), name).unwrap();
        sh(Some(dir), &["git", "add", "."]);
        let msg = format!("commit {name}");
        sh(Some(dir), &["git", "commit", "-qm", &msg]);
    }
}

#[gpui::test]
fn log_loads_commits_and_lanes(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    four_commit_repo(tmp.path());
    let store = cx.update(|cx| HistoryStore::new(tmp.path().to_path_buf(), cx));
    cx.run_until_parked();
    store.update(cx, |hs, _cx| {
        assert_eq!(hs.commits.len(), 4);
        assert_eq!(hs.rows.len(), 4);
        assert_eq!(hs.commits[0].subject, "commit c");
        assert!(hs.rows.iter().all(|r| r.lane == 0), "linear history");
        assert!(hs.selected.is_some(), "first commit pre-selected");
    });
}

#[gpui::test]
fn selecting_a_commit_loads_files_and_the_first_diff(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    four_commit_repo(tmp.path());
    let store = cx.update(|cx| HistoryStore::new(tmp.path().to_path_buf(), cx));
    cx.run_until_parked();
    // Select the "commit b" row (b.txt was added, f.txt/init untouched).
    store.update(cx, |hs, cx| {
        let pos = hs
            .commits
            .iter()
            .position(|c| c.subject == "commit b")
            .expect("commit b row");
        hs.select(Some(pos), cx);
    });
    cx.run_until_parked();
    store.update(cx, |hs, _cx| {
        let files = hs.files.as_ref().expect("files loaded");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].letter, 'A');
        assert_eq!(files[0].path, "b.txt");
        assert_eq!(hs.selected_file, Some(0));
        // The first file's diff auto-loaded.
        let diff = hs.file_diff.as_ref().expect("diff loaded");
        assert!(!diff.binary);
        assert!(diff.hunks.iter().any(|h| {
            h.lines.iter().any(|l| {
                l.kind == worktree_tool::engine::diff::DiffLineKind::Add && l.content == "b"
            })
        }));
    });
}

#[gpui::test]
fn refresh_keeps_the_selection_by_hash_and_reload_files(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    four_commit_repo(tmp.path());
    let store = cx.update(|cx| HistoryStore::new(tmp.path().to_path_buf(), cx));
    cx.run_until_parked();
    store.update(cx, |hs, cx| {
        let pos = hs
            .commits
            .iter()
            .position(|c| c.subject == "commit b")
            .unwrap();
        hs.select(Some(pos), cx);
    });
    cx.run_until_parked();
    store.update(cx, |hs, cx| hs.refresh(cx));
    cx.run_until_parked();
    store.update(cx, |hs, _cx| {
        let sel = hs.selected.expect("selection kept");
        assert_eq!(hs.commits[sel].subject, "commit b");
        assert!(hs.files.is_some(), "detail reloaded for the kept selection");
    });
}

#[gpui::test]
fn has_more_tracks_truncation_and_load_more_fills(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    four_commit_repo(tmp.path());
    let store = cx.update(|cx| HistoryStore::new_with_batch(tmp.path().to_path_buf(), 2, cx));
    cx.run_until_parked();
    store.update(cx, |hs, _cx| {
        assert_eq!(hs.commits.len(), 2, "batch honored");
        assert!(hs.has_more, "4 commits > batch of 2");
        assert!(hs.selected.is_some());
    });
    store.update(cx, |hs, cx| {
        hs.load_more(cx);
    });
    cx.run_until_parked();
    store.update(cx, |hs, _cx| {
        assert_eq!(hs.commits.len(), 4, "load_more fetched the rest");
        assert!(!hs.has_more, "exhausted: no more to load");
        assert_eq!(hs.max_count, 4, "loaded depth synced for refreshes");
    });
}

/// A refresh (r, or the auto-refresh after checkout) supersedes an
/// in-flight load-more: the flag must be released so `L` still works
/// afterwards — otherwise the stranded flag permanently disables it.
#[gpui::test]
fn refresh_releases_a_superseded_load_more(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    four_commit_repo(tmp.path());
    let store = cx.update(|cx| HistoryStore::new_with_batch(tmp.path().to_path_buf(), 2, cx));
    cx.run_until_parked();
    // Start a load-more and supersede it with a refresh before it lands.
    store.update(cx, |hs, cx| hs.load_more(cx));
    store.update(cx, |hs, cx| hs.refresh(cx));
    cx.run_until_parked();
    store.update(cx, |hs, cx| {
        assert!(hs.has_more, "refresh kept has_more (2 of 4 commits)");
        hs.load_more(cx);
    });
    cx.run_until_parked();
    store.update(cx, |hs, _cx| {
        assert_eq!(hs.commits.len(), 4, "L works after the superseding refresh");
    });
}

#[gpui::test]
fn checkout_flags_a_home_mutation_and_moves_head(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    four_commit_repo(tmp.path());
    let store = cx.update(|cx| HistoryStore::new(tmp.path().to_path_buf(), cx));
    cx.run_until_parked();
    store.update(cx, |hs, cx| {
        let pos = hs
            .commits
            .iter()
            .position(|c| c.subject == "commit a")
            .unwrap();
        hs.select(Some(pos), cx);
        hs.checkout(cx);
    });
    cx.run_until_parked();
    store.update(cx, |hs, _cx| {
        assert!(hs.take_mutated(), "checkout flags the home refresh");
        assert!(
            hs.message
                .as_deref()
                .unwrap_or_default()
                .contains("detached HEAD"),
            "got {:?}",
            hs.message
        );
    });
    let head = sh_out(tmp.path(), &["git", "rev-parse", "HEAD"]);
    let a = sh_out(tmp.path(), &["git", "rev-parse", "main~2"]);
    assert_eq!(head, a, "HEAD detached onto commit a");
}

#[gpui::test]
fn checkout_refuses_when_dirty(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    four_commit_repo(tmp.path());
    std::fs::write(tmp.path().join("c.txt"), "dirty").unwrap();
    let store = cx.update(|cx| HistoryStore::new(tmp.path().to_path_buf(), cx));
    cx.run_until_parked();
    store.update(cx, |hs, cx| {
        hs.checkout(cx);
    });
    cx.run_until_parked();
    store.update(cx, |hs, _cx| {
        assert!(
            hs.message
                .as_deref()
                .unwrap_or_default()
                .contains("tracked changes"),
            "expected the dirty refusal, got {:?}",
            hs.message
        );
        assert!(!hs.take_mutated(), "a refusal is not a mutation");
    });
}

#[gpui::test]
fn open_worktree_flags_a_home_mutation_and_registers_the_worktree(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    four_commit_repo(tmp.path());
    let store = cx.update(|cx| HistoryStore::new(tmp.path().to_path_buf(), cx));
    cx.run_until_parked();
    store.update(cx, |hs, cx| {
        hs.open_worktree(cx);
    });
    cx.run_until_parked();
    store.update(cx, |hs, _cx| {
        assert!(hs.take_mutated(), "worktree add flags the home refresh");
        assert!(
            hs.message
                .as_deref()
                .unwrap_or_default()
                .contains("Worktree created"),
            "got {:?}",
            hs.message
        );
    });
    let list = sh_out(tmp.path(), &["git", "worktree", "list"]);
    assert_eq!(
        list.lines().count(),
        2,
        "main worktree + the created one: {list}"
    );
}

#[gpui::test]
fn actions_are_gated_while_one_is_in_flight(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    four_commit_repo(tmp.path());
    let store = cx.update(|cx| HistoryStore::new(tmp.path().to_path_buf(), cx));
    cx.run_until_parked();
    // Kick off a checkout WITHOUT letting it land: the action must gate.
    store.update(cx, |hs, cx| {
        hs.checkout(cx);
        assert!(hs.busy(), "in-flight action sets the busy flag");
    });
    store.update(cx, |hs, cx| {
        // A second action while busy is refused with an explanation, not a
        // silent second worktree.
        hs.open_worktree(cx);
        assert!(hs.busy(), "still the first action");
        assert!(
            hs.message.as_deref().unwrap_or_default().contains("Busy"),
            "expected the busy hint, got {:?}",
            hs.message
        );
    });
    cx.run_until_parked();
    store.update(cx, |hs, _cx| {
        assert!(!hs.busy(), "busy clears when the action lands");
    });
    // The gated `w` never ran: no worktree was created (main only), and
    // the checkout itself doesn't create one either.
    let list = sh_out(tmp.path(), &["git", "worktree", "list"]);
    assert_eq!(
        list.lines().count(),
        1,
        "the gated second action must not run: {list}"
    );
}
