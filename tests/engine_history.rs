//! Engine tests for the history surface: log parsing, graph lanes, commit
//! detail, and the checkout / new-worktree actions.

mod common;

use common::{fixture_repo, sh, sh_allow_fail, sh_out};
use worktree_tool::engine;
use worktree_tool::engine::history::{self, GraphCell};

/// init commit + one follow-up commit editing f.txt. Returns (init_sha,
/// second_sha).
fn two_commit_repo(dir: &std::path::Path) -> (String, String) {
    fixture_repo(dir);
    std::fs::write(dir.join("f.txt"), "two").unwrap();
    sh(Some(dir), &["git", "add", "f.txt"]);
    sh(Some(dir), &["git", "commit", "-qm", "second commit"]);
    let init = sh_out(dir, &["git", "rev-parse", "main~1"]);
    let second = sh_out(dir, &["git", "rev-parse", "main"]);
    (init, second)
}

/// main work commit + side-branch commit + a --no-ff merge of the two.
fn merge_repo(dir: &std::path::Path) {
    fixture_repo(dir);
    sh(Some(dir), &["git", "checkout", "-qb", "side"]);
    std::fs::write(dir.join("side.txt"), "side").unwrap();
    sh(Some(dir), &["git", "add", "side.txt"]);
    sh(Some(dir), &["git", "commit", "-qm", "side work"]);
    sh(Some(dir), &["git", "checkout", "-q", "main"]);
    std::fs::write(dir.join("f.txt"), "main work").unwrap();
    sh(Some(dir), &["git", "commit", "-qam", "main work"]);
    sh_allow_fail(
        Some(dir),
        &["git", "merge", "--no-ff", "-qm", "merge", "side"],
    );
}

#[test]
fn log_lists_commits_newest_first_with_fields() {
    let tmp = tempfile::tempdir().unwrap();
    let (init, second) = two_commit_repo(tmp.path());

    let log = history::log(tmp.path(), 0, 10).unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].subject, "second commit");
    assert_eq!(log[1].subject, "init");
    assert_eq!(log[0].hash, second);
    assert_eq!(log[0].parents, vec![init]);
    assert_eq!(log[1].parents, Vec::<String>::new());
    assert_eq!(log[0].author, "t");
    assert_eq!(log[0].hash.len(), 40, "full hash");
    assert!(
        !log[0].short.is_empty() && log[0].short.len() < 40,
        "short hash: {:?}",
        log[0].short
    );
    // The branch head carries a decoration naming the branch.
    assert!(
        log[0].refs.contains("main"),
        "HEAD branch decoration: {:?}",
        log[0].refs
    );
}

#[test]
fn log_merge_has_both_parents_in_order() {
    let tmp = tempfile::tempdir().unwrap();
    merge_repo(tmp.path());
    let main = sh_out(tmp.path(), &["git", "rev-parse", "main~1"]);
    let side = sh_out(tmp.path(), &["git", "rev-parse", "side"]);

    let log = history::log(tmp.path(), 0, 10).unwrap();
    assert_eq!(log[0].subject, "merge");
    assert_eq!(log[0].parents, vec![main, side], "first parent first");
}

#[test]
fn log_respects_max_count() {
    let tmp = tempfile::tempdir().unwrap();
    two_commit_repo(tmp.path());
    assert_eq!(history::log(tmp.path(), 0, 1).unwrap().len(), 1);
}

#[test]
fn log_on_a_repo_without_commits_is_empty() {
    let tmp = tempfile::tempdir().unwrap();
    sh(Some(tmp.path()), &["git", "init", "-q", "-b", "main"]);
    assert!(history::log(tmp.path(), 0, 10).unwrap().is_empty());
}

#[test]
fn lanes_fork_does_not_double_wire_the_shared_parent() {
    // Two children of A (a fork): A must keep ONE wire. Regression for an
    // i==0 placement that re-wired the shared parent on the second child,
    // leaving an unconsumable phantom wire.
    let mut commits = vec![
        log_commit("c", &[]),
        log_commit("b", &["a"]),
        log_commit("a", &[]),
    ];
    // relabel: c and b both fork from a
    commits[0].parents = vec!["a".into()];
    commits[1].parents = vec!["a".into()];
    commits[2].hash = "a".into();
    commits[2].parents = vec![];
    let rows = history::assign_lanes(&mut commits);
    // The root's row must be exactly its commit cell — no phantom wire.
    assert_eq!(rows[2].cells, vec![GraphCell::Commit]);
}

fn log_commit(hash: &str, parents: &[&str]) -> worktree_tool::engine::history::LogCommit {
    worktree_tool::engine::history::LogCommit {
        hash: hash.to_string(),
        short: hash.to_string(),
        parents: parents.iter().map(|p| p.to_string()).collect(),
        author: "t".into(),
        timestamp: 0,
        refs: String::new(),
        subject: hash.into(),
        lane: 0,
    }
}

#[test]
fn lanes_linear_history_stays_on_lane_zero() {
    let tmp = tempfile::tempdir().unwrap();
    two_commit_repo(tmp.path());
    let mut log = history::log(tmp.path(), 0, 10).unwrap();
    let rows = history::assign_lanes(&mut log);
    assert!(rows.iter().all(|r| r.lane == 0), "linear: {rows:?}");
    // Row cells: a commit cell on its lane, no trailing padding.
    assert_eq!(rows[0].cells, vec![GraphCell::Commit]);
    for (i, c) in log.iter().enumerate() {
        assert_eq!(c.lane, 0, "row {i}");
    }
}

#[test]
fn lanes_open_for_a_side_branch_and_close_at_the_merge() {
    let tmp = tempfile::tempdir().unwrap();
    merge_repo(tmp.path());

    let mut log = history::log(tmp.path(), 0, 10).unwrap();
    let rows = history::assign_lanes(&mut log);
    // The merge and the first-parent line stay on lane 0; exactly the side
    // commit opens lane 1.
    let side_lanes: Vec<usize> = log
        .iter()
        .filter(|c| c.subject == "side work")
        .map(|c| c.lane)
        .collect();
    assert_eq!(side_lanes, vec![1], "side work on lane 1");
    assert_eq!(rows.iter().map(|r| r.lane).max(), Some(1));
    for (row, commit) in rows.iter().zip(log.iter()) {
        assert_eq!(row.cells[row.lane], GraphCell::Commit, "{}", commit.subject);
        assert_ne!(
            row.cells.last(),
            Some(&GraphCell::Empty),
            "trailing cells trimmed"
        );
    }
}

#[test]
fn lanes_side_branches_reuse_lanes_after_their_merge() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    for name in ["a", "b"] {
        sh(Some(tmp.path()), &["git", "checkout", "-qb", name]);
        std::fs::write(tmp.path().join(format!("{name}.txt")), name).unwrap();
        sh(Some(tmp.path()), &["git", "add", "."]);
        let msg = format!("{name} work");
        sh(Some(tmp.path()), &["git", "commit", "-qm", &msg]);
        sh(Some(tmp.path()), &["git", "checkout", "-q", "main"]);
    }
    sh_allow_fail(
        Some(tmp.path()),
        &["git", "merge", "--no-ff", "-qm", "m1", "a"],
    );
    sh_allow_fail(
        Some(tmp.path()),
        &["git", "merge", "--no-ff", "-qm", "m2", "b"],
    );

    let mut log = history::log(tmp.path(), 0, 20).unwrap();
    let rows = history::assign_lanes(&mut log);
    // Branch lanes are reused after their merges close them: the max lane
    // stays small even with two side branches.
    assert!(
        rows.iter().map(|r| r.lane).max().unwrap() <= 2,
        "lanes reused"
    );
    assert!(rows.iter().all(|r| r.cells[r.lane] == GraphCell::Commit));
}

#[test]
fn commit_files_cover_add_modify_delete_and_root() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    let root_sha = sh_out(tmp.path(), &["git", "rev-parse", "main"]);

    // An intermediate commit so the next one can DELETE a tracked file.
    std::fs::write(tmp.path().join("g.txt"), "doomed").unwrap();
    sh(Some(tmp.path()), &["git", "add", "g.txt"]);
    sh(Some(tmp.path()), &["git", "commit", "-qm", "g"]);

    std::fs::write(tmp.path().join("added.txt"), "new").unwrap();
    std::fs::write(tmp.path().join("f.txt"), "edited").unwrap();
    sh(Some(tmp.path()), &["git", "rm", "-q", "g.txt"]);
    sh(Some(tmp.path()), &["git", "add", "."]);
    sh(Some(tmp.path()), &["git", "commit", "-qm", "three changes"]);
    let sha = sh_out(tmp.path(), &["git", "rev-parse", "main"]);

    let files = history::commit_files(tmp.path(), &sha, false).unwrap();
    let letters: Vec<(char, &str)> = files.iter().map(|f| (f.letter, f.path.as_str())).collect();
    assert!(letters.contains(&('A', "added.txt")), "{letters:?}");
    assert!(letters.contains(&('M', "f.txt")), "{letters:?}");
    assert!(letters.contains(&('D', "g.txt")), "{letters:?}");

    // Root commit: --root makes diff-tree list the initial files.
    let root_files = history::commit_files(tmp.path(), &root_sha, false).unwrap();
    assert!(root_files
        .iter()
        .any(|f| f.letter == 'A' && f.path == "f.txt"));
}

#[test]
fn commit_files_on_a_merge_diffs_against_the_first_parent() {
    let tmp = tempfile::tempdir().unwrap();
    merge_repo(tmp.path());
    let merge = sh_out(tmp.path(), &["git", "rev-parse", "main"]);

    // Plain diff-tree emits NOTHING for merges; first-parent mode lists
    // the incoming side changes — matching what commit_diff renders.
    assert!(
        history::commit_files(tmp.path(), &merge, false)
            .unwrap()
            .is_empty(),
        "plain diff-tree on a merge is empty"
    );
    let files = history::commit_files(tmp.path(), &merge, true).unwrap();
    // The first-parent diff shows what the MERGE brought in: only the
    // side branch's file (f.txt's "main work" was already in parent 1).
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["side.txt"], "{paths:?}");
}

#[test]
fn commit_diff_renders_the_file_against_the_first_parent() {
    let tmp = tempfile::tempdir().unwrap();
    two_commit_repo(tmp.path());
    let second = sh_out(tmp.path(), &["git", "rev-parse", "main"]);

    let ud = history::commit_diff(tmp.path(), &second, "f.txt").unwrap();
    assert!(!ud.binary);
    assert_eq!(ud.hunks.len(), 1);
    assert!(ud.hunks[0]
        .lines
        .iter()
        .any(|l| l.kind == engine::diff::DiffLineKind::Add && l.content == "two"));
}

#[test]
fn log_propagates_a_broken_head_instead_of_empty() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    // Corrupt HEAD: rev-parse HEAD fails AND symbolic-ref fails — the log
    // must surface an error, not masquerade as "No commits yet".
    std::fs::write(tmp.path().join(".git/HEAD"), "garbage").unwrap();
    let err = history::log(tmp.path(), 0, 10).unwrap_err();
    assert!(!err.message.is_empty());
}

#[test]
fn checkout_refuses_when_the_worktree_is_dirty() {
    let tmp = tempfile::tempdir().unwrap();
    let (init, _) = two_commit_repo(tmp.path());
    std::fs::write(tmp.path().join("f.txt"), "dirty").unwrap();

    let err = history::checkout(tmp.path(), &init).unwrap_err();
    assert!(
        err.message.contains("tracked changes"),
        "expected the dirty refusal, got: {err}"
    );
    // HEAD untouched.
    let head = sh_out(tmp.path(), &["git", "rev-parse", "HEAD"]);
    assert_ne!(head, init);
}

#[test]
fn checkout_detaches_onto_the_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let (init, _) = two_commit_repo(tmp.path());
    history::checkout(tmp.path(), &init).unwrap();
    let head = sh_out(tmp.path(), &["git", "rev-parse", "HEAD"]);
    assert_eq!(head, init);
}

#[test]
fn open_worktree_at_creates_a_registered_detached_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    let (init, _) = two_commit_repo(tmp.path());
    let short = sh_out(tmp.path(), &["git", "rev-parse", "--short", "main"]);

    let created = history::open_worktree_at(tmp.path(), &init, &short).unwrap();
    assert!(created.is_dir());
    assert!(
        created
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains(&short),
        "path names the commit: {created:?}"
    );
    let list = sh_out(tmp.path(), &["git", "worktree", "list", "--porcelain"]);
    // Compare by DIRECTORY NAME: git registers the path with symlinks and
    // 8.3 short-name components resolved (RUNNER~1 vs runneradmin), so a
    // full-path comparison is environment-dependent.
    let name = created.file_name().unwrap().to_string_lossy().into_owned();
    assert!(list.contains(&name), "list: {list}");
}

#[test]
fn open_worktree_at_suffixes_collisions() {
    let tmp = tempfile::tempdir().unwrap();
    let (init, _) = two_commit_repo(tmp.path());
    let short = sh_out(tmp.path(), &["git", "rev-parse", "--short", "main"]);
    let first = history::open_worktree_at(tmp.path(), &init, &short).unwrap();
    let second = history::open_worktree_at(tmp.path(), &init, &short).unwrap();
    assert_ne!(first, second, "collision must not clobber");
    assert!(second.is_dir());
}

#[test]
fn commit_files_detects_renames_with_both_paths() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    // A PURE rename (no content edit) is detected as R100 — any content
    // rewrite would need similarity above git's 50% threshold to count.
    sh(Some(tmp.path()), &["git", "mv", "f.txt", "renamed.txt"]);
    sh(Some(tmp.path()), &["git", "commit", "-qm", "renamed"]);
    let sha = sh_out(tmp.path(), &["git", "rev-parse", "main"]);

    let files = history::commit_files(tmp.path(), &sha, false).unwrap();
    assert_eq!(files.len(), 1, "{files:?}");
    assert_eq!(files[0].letter, 'R');
    assert_eq!(files[0].orig_path.as_deref(), Some("f.txt"), "pre-image");
    assert_eq!(files[0].path, "renamed.txt", "post-image");

    // The detail diff is queried with the POST-rename path and parses
    // cleanly (a pure rename under a pathspec renders as an addition —
    // git pairs rename sides only when both match the pathspec; the
    // meaningful old → new display lives in the file chips' name-status).
    let ud = history::commit_diff(tmp.path(), &sha, "renamed.txt").unwrap();
    assert!(!ud.binary);
    assert!(ud.header.contains("renamed.txt"), "{:?}", ud.header);
}
