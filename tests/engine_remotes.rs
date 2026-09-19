//! Engine tests for remote management: list, add, remove.

mod common;

use common::{fixture_repo, sh, sh_out};
use worktree_tool::engine::remotes;

fn bare_remote(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let remote = dir.join(format!("{name}.git"));
    std::fs::create_dir(&remote).unwrap();
    sh(
        None,
        &[
            "git",
            "init",
            "-q",
            "--bare",
            "--initial-branch=main",
            remote.to_str().unwrap(),
        ],
    );
    remote
}

#[test]
fn add_list_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    let origin = bare_remote(tmp.path(), "origin");
    let upstream = bare_remote(tmp.path(), "upstream");

    remotes::add(tmp.path(), "origin", origin.to_str().unwrap()).unwrap();
    remotes::add(tmp.path(), "upstream", upstream.to_str().unwrap()).unwrap();

    let list = remotes::list(tmp.path()).unwrap();
    assert_eq!(list.len(), 2);
    let origin_entry = list.iter().find(|r| r.name == "origin").unwrap();
    assert_eq!(origin_entry.fetch_url, origin.to_str().unwrap());
    assert_eq!(
        origin_entry.push_url, origin_entry.fetch_url,
        "no split push url"
    );
    assert!(list.iter().any(|r| r.name == "upstream"));
}

#[test]
fn list_reports_a_split_push_url() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    let origin = bare_remote(tmp.path(), "origin");
    let push = bare_remote(tmp.path(), "push-mirror");
    remotes::add(tmp.path(), "origin", origin.to_str().unwrap()).unwrap();
    sh(
        Some(tmp.path()),
        &[
            "git",
            "remote",
            "set-url",
            "--push",
            "origin",
            push.to_str().unwrap(),
        ],
    );

    let list = remotes::list(tmp.path()).unwrap();
    assert_eq!(list.len(), 1);
    assert_ne!(
        list[0].push_url, list[0].fetch_url,
        "split push URL surfaced"
    );
}

#[test]
fn remove_deletes_the_remote() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    let origin = bare_remote(tmp.path(), "origin");
    remotes::add(tmp.path(), "origin", origin.to_str().unwrap()).unwrap();
    remotes::add(tmp.path(), "keep", origin.to_str().unwrap()).unwrap();

    remotes::remove(tmp.path(), "origin").unwrap();
    let list = remotes::list(tmp.path()).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "keep");
}

#[test]
fn add_refuses_duplicate_and_bad_names() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    let origin = bare_remote(tmp.path(), "origin");
    remotes::add(tmp.path(), "origin", origin.to_str().unwrap()).unwrap();

    // Duplicate name: git refuses; the engine propagates it.
    assert!(remotes::add(tmp.path(), "origin", origin.to_str().unwrap()).is_err());
    // Injection-shaped and malformed names: refused before git runs.
    for bad in ["-oProxy", "a b", "", "a\nb", "sub/dir"] {
        assert!(
            remotes::add(tmp.path(), bad, "https://example.com/x.git").is_err(),
            "name {bad:?} must be refused"
        );
    }
    // Control-character URLs are refused too.
    assert!(remotes::add(tmp.path(), "ok", "https://x/\n").is_err());
}

#[test]
fn remove_refuses_bad_names() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_repo(tmp.path());
    for bad in ["-oProxy", "", "a b"] {
        assert!(remotes::remove(tmp.path(), bad).is_err());
    }
    assert!(remotes::remove(tmp.path(), "nonexistent").is_err());
    let out = sh_out(tmp.path(), &["git", "remote"]);
    assert!(out.is_empty(), "no remote was created by the refusals");
}
