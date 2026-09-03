mod common;

use std::sync::Mutex;
use worktree_tool::engine::commit;

/// Tests mutate process-global env vars; serialize them.
static ENV_LOCK: Mutex<()> = Mutex::new(());

#[cfg(windows)]
const EDITOR_OK: &str = "cmd /c exit 0";
#[cfg(windows)]
const EDITOR_FAIL: &str = "cmd /c exit 1";
#[cfg(not(windows))]
const EDITOR_OK: &str = "true";
#[cfg(not(windows))]
const EDITOR_FAIL: &str = "false";

#[test]
fn resolve_editor_order_and_splitting() {
    let getenv = |k: &str| match k {
        "VISUAL" => Some("code -w".to_string()),
        _ => None,
    };
    // GIT_EDITOR wins
    assert_eq!(
        commit::resolve_editor(Some("nano"), &|k| (k == "GIT_EDITOR")
            .then(|| "vi".to_string())),
        vec!["vi"]
    );
    // then core.editor config
    assert_eq!(
        commit::resolve_editor(Some("nano"), &|_| None),
        vec!["nano"]
    );
    // then VISUAL (split on whitespace), then EDITOR, then default
    assert_eq!(commit::resolve_editor(None, &getenv), vec!["code", "-w"]);
    assert_eq!(
        commit::resolve_editor(None, &|k| (k == "EDITOR").then(|| "emacs".to_string())),
        vec!["emacs"]
    );
    // Exported-empty values fall through exactly like unset ones.
    let getenv = |k: &str| match k {
        "VISUAL" => Some("  ".to_string()),
        "EDITOR" => Some(String::new()),
        _ => None,
    };
    let expected_default = if cfg!(windows) {
        "notepad"
    } else if cfg!(target_os = "freebsd") {
        "ee"
    } else {
        "vim"
    };
    assert_eq!(
        commit::resolve_editor(None, &getenv),
        vec![expected_default]
    );
    assert_eq!(
        commit::resolve_editor(None, &|_| None),
        vec![expected_default]
    );
}

#[test]
fn strip_comments_removes_comments_and_trims() {
    assert_eq!(
        commit::strip_comments("\n# comment\nsubject\n\nbody line\n# trailing\n"),
        "subject\n\nbody line"
    );
    assert_eq!(commit::strip_comments("# only comments\n"), "");
}

#[test]
fn commit_via_editor_round_trip_and_abort() {
    let _guard = ENV_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    common::fixture_repo(tmp.path());

    // Editor that writes a message into the file we pass it.
    let editor = write_editor_script("committed by test");
    std::env::set_var("GIT_EDITOR", &editor);
    std::fs::write(tmp.path().join("f.txt"), "two").unwrap();
    common::sh(Some(tmp.path()), &["git", "add", "--", "f.txt"]);
    match commit::commit_with_editor(tmp.path(), "1 staged file: f.txt") {
        Ok(commit::CommitOutcome::Committed) => {}
        other => panic!("expected Committed, got {other:?}"),
    }
    let subject = common::sh_out(tmp.path(), &["git", "log", "-1", "--format=%s"]);
    assert_eq!(subject, "committed by test");

    // Empty message (editor exits 0 without writing anything) aborts.
    std::env::set_var("GIT_EDITOR", EDITOR_OK);
    std::fs::write(tmp.path().join("f.txt"), "three").unwrap();
    common::sh(Some(tmp.path()), &["git", "add", "--", "f.txt"]);
    match commit::commit_with_editor(tmp.path(), "1 staged file: f.txt") {
        Ok(commit::CommitOutcome::AbortedEmpty) => {}
        other => panic!("expected AbortedEmpty, got {other:?}"),
    }
    // Editor failure surfaces as an error.
    std::env::set_var("GIT_EDITOR", EDITOR_FAIL);
    assert!(commit::commit_with_editor(tmp.path(), "1 staged file: f.txt").is_err());
    std::env::remove_var("GIT_EDITOR");
}

/// Cross-platform "editor" that writes a fixed message into the file it
/// receives as its last argument.
fn write_editor_script(message: &str) -> String {
    let path = tempfile::tempdir().unwrap().keep().join("ed");
    #[cfg(unix)]
    {
        let script = path.with_extension("sh");
        std::fs::write(
            &script,
            format!("#!/bin/sh\nprintf '%s' '{message}' > \"$1\"\n"),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script.display().to_string()
    }
    #[cfg(windows)]
    {
        // std refuses to spawn .cmd/.bat directly (BatBadBut mitigation), so
        // the GIT_EDITOR string routes through `cmd /c`; resolve_editor's
        // whitespace split yields ["cmd", "/c", script] and the message file
        // is appended as the script's %1.
        let script = path.with_extension("cmd");
        std::fs::write(
            &script,
            format!("@echo off\r\n(echo {message})> \"%~1\"\r\n"),
        )
        .unwrap();
        format!("cmd /c {}", script.display())
    }
}
