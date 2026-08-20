use std::path::Path;
use worktree_tool::git;
use worktree_tool::model::WorktreeStatus;

fn sh(cwd: Option<&Path>, cmd: &[&str]) {
    let status = std::process::Command::new(cmd[0])
        .args(&cmd[1..])
        .current_dir(cwd.unwrap_or(Path::new(".")))
        .status()
        .expect("spawn");
    assert!(status.success(), "failed: {cmd:?}");
}

fn fixture_repo(dir: &Path) {
    sh(Some(dir), &["git", "init", "-q", "-b", "main"]);
    sh(Some(dir), &["git", "config", "user.email", "t@t.t"]);
    sh(Some(dir), &["git", "config", "user.name", "t"]);
    std::fs::write(dir.join("f.txt"), "one").unwrap();
    sh(Some(dir), &["git", "add", "."]);
    sh(Some(dir), &["git", "commit", "-qm", "init"]);
}

#[test]
fn detects_repo_root_and_lists_worktrees() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    let sub = tmp.path().join("nested");
    std::fs::create_dir(&sub).unwrap();
    let root = git::repo_root(&sub).unwrap();
    assert_eq!(
        root.canonicalize().unwrap(),
        tmp.path().canonicalize().unwrap()
    );
    let entries = git::list_worktrees(&root).unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].is_main);
    assert_eq!(entries[0].branch.as_deref(), Some("main"));
}

#[test]
fn status_pass_marks_dirty_and_unavailable() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    std::fs::write(tmp.path().join("f.txt"), "changed").unwrap();
    std::fs::write(tmp.path().join("new.txt"), "untracked").unwrap();
    let mut entries = git::list_worktrees(tmp.path()).unwrap();
    entries.push(worktree_tool::model::WorktreeEntry {
        path: tmp.path().join("nope"),
        head: None,
        branch: None,
        is_main: false,
        status: WorktreeStatus::Pending,
    });
    let done = git::status_pass(entries);
    assert!(matches!(
        done[0].status,
        WorktreeStatus::Dirty {
            unstaged: 1,
            untracked: 1,
            ..
        }
    ));
    assert!(matches!(done[1].status, WorktreeStatus::Unavailable(_)));
}

#[test]
fn run_git_reports_stderr_on_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let err = git::run_git(Some(tmp.path()), &["rev-parse", "--show-toplevel"]).unwrap_err();
    assert!(!err.message.is_empty());
}

#[test]
fn add_remove_prune_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());

    // new-branch mode
    let wt = tmp.path().join("wt-a");
    git::add_worktree(tmp.path(), &wt, Some("feat-a"), "main").unwrap();
    assert!(wt.join("f.txt").exists());
    assert_eq!(git::list_worktrees(tmp.path()).unwrap().len(), 2);
    assert!(git::local_branches(tmp.path())
        .unwrap()
        .contains(&"feat-a".to_string()));

    // dirty removal is refused without force, succeeds with it
    std::fs::write(wt.join("new.txt"), "x").unwrap();
    assert!(git::remove_worktree(tmp.path(), &wt, false).is_err());
    git::remove_worktree(tmp.path(), &wt, true).unwrap();
    assert_eq!(git::list_worktrees(tmp.path()).unwrap().len(), 1);

    // existing-branch mode + prune of a deleted directory
    git::add_worktree(tmp.path(), &tmp.path().join("wt-b"), Some("feat-b"), "main").unwrap();
    std::fs::remove_dir_all(tmp.path().join("wt-b")).unwrap();
    assert_eq!(git::list_worktrees(tmp.path()).unwrap().len(), 2);
    git::prune(tmp.path()).unwrap();
    assert_eq!(git::list_worktrees(tmp.path()).unwrap().len(), 1);

    assert_eq!(git::default_branch(tmp.path()), "main");
}

#[test]
fn status_pass_parallel_preserves_order() {
    let tmp = tempfile::tempdir().unwrap();
    // repo gets its own dir so worktrees can live OUTSIDE it — otherwise
    // the main worktree sees them as untracked files.
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    fixture_repo(&repo);
    // enough entries to engage the parallel path (threads >= 2)
    for i in 0u32..10 {
        let wt = tmp.path().join("wts").join(format!("wt-{i:02}"));
        git::add_worktree(&repo, &wt, Some(&format!("b-{i:02}")), "main").unwrap();
        if i.is_multiple_of(2) {
            std::fs::create_dir_all(&wt).unwrap();
            std::fs::write(wt.join("dirty.txt"), "x").unwrap();
        }
    }
    let entries = git::list_worktrees(&repo).unwrap();
    assert_eq!(entries.len(), 11);
    let done = git::status_pass(entries);
    // statuses must align with the entry they belong to
    for (i, e) in done.iter().enumerate() {
        let name = e.path.file_name().unwrap().to_string_lossy().into_owned();
        if i == 0 {
            assert!(
                matches!(e.status, WorktreeStatus::Clean { .. }),
                "main dirty"
            );
        } else {
            let n: u32 = name.split('-').nth(1).unwrap().parse().unwrap();
            let is_dirty = matches!(e.status, WorktreeStatus::Dirty { .. });
            assert_eq!(is_dirty, n.is_multiple_of(2), "status mismatch for {name}");
        }
    }
}

#[test]
fn dash_prefixed_names_are_not_interpreted_as_options() {
    let tmp = tempfile::tempdir().unwrap();
    // repo in its own dir; dash-named worktrees outside it
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    fixture_repo(&repo);
    let wts = tmp.path().join("wts");
    std::fs::create_dir(&wts).unwrap();

    // A destination literally named "-force" must be a path, not an option:
    // with the `--` separator git either creates it or refuses with a clean
    // error — it must never succeed by treating it as --force.
    let dash_dest = wts.join("-force");
    match git::add_worktree(&repo, &dash_dest, Some("dash-dest"), "main") {
        Ok(()) => {
            assert!(dash_dest.is_dir(), "worktree dir should exist");
            let list = git::list_worktrees(&repo).unwrap();
            assert_eq!(list.len(), 2);
        }
        Err(e) => assert!(
            !e.message.contains("usage:") && !e.message.contains("--force"),
            "looked like option interpretation: {}",
            e.message
        ),
    }
    // Removing a dash-named worktree must also stay positional.
    let _ = git::remove_worktree(&repo, &dash_dest, true);
    assert_eq!(git::list_worktrees(&repo).unwrap().len(), 1);

    // A dash-prefixed branch name: with `--` it is never option-parsed.
    let wt = wts.join("b");
    let res = git::add_worktree(&repo, &wt, Some("-b"), "main");
    match res {
        Ok(()) => {
            assert!(git::local_branches(&repo)
                .unwrap()
                .contains(&"-b".to_string()));
        }
        Err(e) => assert!(
            !e.message.contains("usage:"),
            "option interpretation suspected: {}",
            e.message
        ),
    }
}
