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
