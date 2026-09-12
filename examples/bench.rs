//! Performance harness for the git/parsing layer. Run with:
//! `cargo run --release --example bench`
//! The fixture repos are built in a tempdir and thrown away.

use std::path::{Path, PathBuf};
use std::time::Instant;
use worktree_tool::git;
use worktree_tool::model::{self, WorktreeEntry, WorktreeStatus};

fn sh(dir: &Path, cmd: &[&str]) {
    let status = std::process::Command::new(cmd[0])
        .args(&cmd[1..])
        .current_dir(dir)
        .status()
        .expect("spawn");
    assert!(status.success(), "failed: {cmd:?}");
}

fn fixture_repo(dir: &Path, worktrees: usize) {
    println!("building fixture with {worktrees} worktrees…");
    let t = Instant::now();
    sh(dir, &["git", "init", "-q", "-b", "main"]);
    sh(dir, &["git", "config", "user.email", "t@t.t"]);
    sh(dir, &["git", "config", "user.name", "t"]);
    std::fs::write(dir.join("f.txt"), "one").unwrap();
    sh(dir, &["git", "add", "."]);
    sh(dir, &["git", "commit", "-qm", "init"]);
    let base = dir.with_file_name(format!(
        "{}-wts",
        dir.file_name().unwrap().to_string_lossy()
    ));
    std::fs::create_dir_all(&base).unwrap();
    for i in 0..worktrees {
        let path = base.join(format!("branch-{i:03}"));
        sh(
            dir,
            &[
                "git",
                "worktree",
                "add",
                "-q",
                &path.display().to_string(),
                "-b",
                &format!("branch-{i:03}"),
                "main",
            ],
        );
        // every third worktree is dirty, to exercise both status paths
        if i % 3 == 0 {
            std::fs::write(path.join("dirty.txt"), format!("dirty {i}")).unwrap();
        }
    }
    println!("  fixture built in {:.1}s", t.elapsed().as_secs_f32());
}

fn bench_parser(worktrees: usize) {
    let mut blob = String::new();
    blob.push_str("worktree /repo/main\nHEAD abc123\nbranch refs/heads/main\n\n");
    for i in 0..worktrees {
        blob.push_str(&format!(
            "worktree /repo/wts/b{i}\nHEAD def{i:04}\nbranch refs/heads/b{i}\n\n"
        ));
    }
    let iterations = 2000;
    let t = Instant::now();
    for _ in 0..iterations {
        let entries = model::parse_worktree_porcelain(&blob);
        assert_eq!(entries.len(), worktrees + 1);
    }
    let per = t.elapsed().as_micros() as f64 / iterations as f64;
    println!(
        "parse_worktree_porcelain  {:4} entries: {:8.1} µs/op ({:6.0}k entries/s)",
        worktrees + 1,
        per,
        (worktrees + 1) as f64 / per * 1000.0
    );

    let status = "# branch.oid abc\n# branch.head main\n# branch.ab +2 -1\n1 .M N... 1 2 3 a b f.txt\n? new.txt\n";
    let iterations = 100_000;
    let t = Instant::now();
    for _ in 0..iterations {
        let st = model::parse_status_porcelain_v2(status);
        assert!(matches!(st, WorktreeStatus::Dirty { .. }));
    }
    let per = t.elapsed().as_micros() as f64 / iterations as f64;
    println!("parse_status_porcelain_v2             {:8.2} µs/op", per);
}

fn bench_filter(entries: usize) {
    let list: Vec<WorktreeEntry> = (0..entries)
        .map(|i| WorktreeEntry {
            path: PathBuf::from(format!("/repo/wts/branch-{i:04}")),
            head: None,
            branch: Some(format!("branch-{i:04}")),
            is_main: i == 0,
            status: WorktreeStatus::Pending,
        })
        .collect();
    let iterations = 1000;
    let t = Instant::now();
    for _ in 0..iterations {
        let (idx, sel) = worktree_tool::store::apply_filter(&list, "branch-0042", None);
        assert_eq!(idx.len(), 1);
        assert_eq!(sel, Some(0));
    }
    let per = t.elapsed().as_micros() as f64 / iterations as f64;
    println!("apply_filter             {entries} entries: {per:8.1} µs/op");
}

fn bench_git_layer(repo: &Path, label: &str) {
    let t = Instant::now();
    let entries = git::list_worktrees(repo).expect("list");
    let list_ms = t.elapsed().as_secs_f64() * 1000.0;

    let t = Instant::now();
    let done = git::status_pass(entries);
    let status_ms = t.elapsed().as_secs_f64() * 1000.0;
    let dirty = done
        .iter()
        .filter(|e| matches!(e.status, WorktreeStatus::Dirty { .. }))
        .count();

    println!(
        "{label}: list = {list_ms:7.1} ms, status_pass = {status_ms:7.1} ms ({} worktrees, {dirty} dirty)"
        , done.len()
    );
}

fn bench_working_copy(repo: &Path) {
    // 2000 untracked files in one directory: exercises the untracked-dir
    // collapse in the status parse.
    let changes = repo.join("bench-changes");
    std::fs::create_dir_all(&changes).unwrap();
    for i in 0..2000u32 {
        std::fs::write(changes.join(format!("f{i:04}.txt")), format!("content {i}")).unwrap();
    }
    // 200 tracked files, committed then modified: gives the single-file
    // diff something real to chew on (untracked files never appear in
    // `git diff`).
    let tracked = repo.join("bench-tracked");
    std::fs::create_dir_all(&tracked).unwrap();
    for i in 0..200u32 {
        std::fs::write(
            tracked.join(format!("t{i:03}.txt")),
            format!("original {i}"),
        )
        .unwrap();
    }
    sh(repo, &["git", "add", "--", "bench-tracked"]);
    sh(repo, &["git", "commit", "-qm", "bench tracked files"]);
    std::fs::write(tracked.join("t000.txt"), "original 0\nmodified line\n").unwrap();

    use worktree_tool::engine::{diff, working_copy};
    let t = Instant::now();
    let wc = working_copy::status(repo).expect("status");
    let status_ms = t.elapsed().as_secs_f64() * 1000.0;
    let t = Instant::now();
    let d = diff::diff_unstaged(repo, "bench-tracked/t000.txt").expect("diff");
    let diff_ms = t.elapsed().as_secs_f64() * 1000.0;
    let hunks: usize = d.hunks.iter().map(|h| h.lines.len()).sum();
    println!(
        "working copy: status = {status_ms:7.1} ms ({} entries), single-file diff = {diff_ms:5.1} ms ({} diff lines)",
        wc.entries.len(),
        hunks
    );
    assert!(hunks > 0, "bench diff must measure a real diff");
}

fn main() {
    println!("== parsers ==");
    bench_parser(10);
    bench_parser(100);

    println!("== filter ==");
    bench_filter(100);
    bench_filter(1000);

    println!("== git layer (end to end) ==");
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo20 = tmp.path().join("repo20");
    std::fs::create_dir(&repo20).unwrap();
    fixture_repo(&repo20, 20);
    bench_git_layer(&repo20, " 20 worktrees");
    bench_git_layer(&repo20, " 20 worktrees (warm)");

    let repo50 = tmp.path().join("repo50");
    std::fs::create_dir(&repo50).unwrap();
    fixture_repo(&repo50, 50);
    bench_git_layer(&repo50, " 50 worktrees");

    // A typical refresh is list + status_pass back to back.
    let t = Instant::now();
    let entries = git::list_worktrees(&repo50).expect("list");
    git::status_pass(entries);
    println!(
        " 50 worktrees full refresh: {:.1} ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    println!("== working copy ==");
    bench_working_copy(&repo20);
}
