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

// git's editor semantics: a `%s` in the spec gets the message path
// substituted (instead of appended), and `:` is a no-op success.
#[test]
fn editor_arg_substitution_and_colon_noop() {
    let _guard = ENV_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    common::fixture_repo(tmp.path());
    std::fs::write(tmp.path().join("f.txt"), "changed").unwrap();
    common::sh(Some(tmp.path()), &["git", "add", "--", "f.txt"]);

    // A script whose $2 receives the substituted message path (%s is
    // the first arg after our splitter substitutes it in-place).
    let script = write_subst_editor_script();
    std::env::set_var("GIT_EDITOR", &script);
    match commit::commit_with_editor(tmp.path(), "1 staged file: f.txt", None) {
        Ok(commit::CommitOutcome::Committed) => {}
        other => panic!("expected Committed via %s substitution, got {other:?}"),
    }
    let subject = common::sh_out(tmp.path(), &["git", "log", "-1", "--format=%s"]);
    assert_eq!(subject, "substituted draft");

    // `:` = succeed without launching anything → empty message → abort.
    std::env::set_var("GIT_EDITOR", ":");
    match commit::commit_with_editor(tmp.path(), "1 staged file: f.txt", None) {
        Ok(commit::CommitOutcome::AbortedEmpty { draft: None }) => {}
        other => panic!("expected AbortedEmpty from colon editor, got {other:?}"),
    }
    std::env::remove_var("GIT_EDITOR");
}

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
        (vec!["vi".to_string()], false)
    );
    // then core.editor config
    assert_eq!(
        commit::resolve_editor(Some("nano"), &|_| None),
        (vec!["nano".to_string()], false)
    );
    // then VISUAL (split on whitespace), then EDITOR, then default
    assert_eq!(
        commit::resolve_editor(None, &getenv),
        (vec!["code".to_string(), "-w".to_string()], false)
    );
    assert_eq!(
        commit::resolve_editor(None, &|k| (k == "EDITOR").then(|| "emacs".to_string())),
        (vec!["emacs".to_string()], false)
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
        (vec![expected_default.to_string()], true)
    );
}

#[test]
fn strip_comments_removes_comments_and_trims() {
    assert_eq!(
        commit::strip_comments('#', "\n# comment\nsubject\n\nbody line\n# trailing\n"),
        "subject\n\nbody line"
    );
    assert_eq!(commit::strip_comments('#', "# only comments\n"), "");
    // `core.commentChar = ";"` users get the same treatment.
    assert_eq!(commit::strip_comments(';', "# kept\n; dropped\n"), "# kept");
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
    match commit::commit_with_editor(tmp.path(), "1 staged file: f.txt", None) {
        Ok(commit::CommitOutcome::Committed) => {}
        other => panic!("expected Committed, got {other:?}"),
    }
    let subject = common::sh_out(tmp.path(), &["git", "log", "-1", "--format=%s"]);
    assert_eq!(subject, "committed by test");

    // Empty message (editor exits 0 without writing anything) aborts; the
    // file is still template-identical, so nothing is kept.
    std::env::set_var("GIT_EDITOR", EDITOR_OK);
    std::fs::write(tmp.path().join("f.txt"), "three").unwrap();
    common::sh(Some(tmp.path()), &["git", "add", "--", "f.txt"]);
    match commit::commit_with_editor(tmp.path(), "1 staged file: f.txt", None) {
        Ok(commit::CommitOutcome::AbortedEmpty { draft: None }) => {}
        other => panic!("expected AbortedEmpty, got {other:?}"),
    }
    // Editor failure surfaces as an error.
    std::env::set_var("GIT_EDITOR", EDITOR_FAIL);
    assert!(commit::commit_with_editor(tmp.path(), "1 staged file: f.txt", None).is_err());
    std::env::remove_var("GIT_EDITOR");
}

#[test]
fn commit_via_editor_keeps_a_fully_quoted_draft_on_abort() {
    let _guard = ENV_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    common::fixture_repo(tmp.path());

    // A draft whose every line starts with the comment char strips to an
    // empty message (abort) — but it is typed work: the raw file must be
    // preserved and its path surfaced, not deleted.
    let editor = write_editor_script("# typed thought, quoted");
    std::env::set_var("GIT_EDITOR", &editor);
    std::fs::write(tmp.path().join("f.txt"), "four").unwrap();
    common::sh(Some(tmp.path()), &["git", "add", "--", "f.txt"]);
    match commit::commit_with_editor(tmp.path(), "1 staged file: f.txt", None) {
        Ok(commit::CommitOutcome::AbortedEmpty { draft: Some(p) }) => {
            let kept = std::fs::read_to_string(&p).unwrap();
            assert!(kept.contains("typed thought"), "kept draft: {kept:?}");
        }
        other => panic!("expected AbortedEmpty with a kept draft, got {other:?}"),
    }
    std::env::remove_var("GIT_EDITOR");
}

/// Cross-platform "editor" that writes a fixed message into the file it
/// receives as its last argument.
/// A script (referenced with a `%s` placeholder in GIT_EDITOR) that writes
/// a fixed message INTO the file git passes as the script's first argument.
fn write_subst_editor_script() -> String {
    let dir = tempfile::tempdir().unwrap().keep();
    let path = dir.join("ed");
    #[cfg(unix)]
    {
        let script = path.with_extension("sh");
        std::fs::write(
            &script,
            "#!/bin/sh\nprintf '%s' 'substituted draft' > \"$1\"\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        format!("{} %s", script.display())
    }
    #[cfg(windows)]
    {
        let script = path.with_extension("cmd");
        std::fs::write(&script, "@echo substituted draft>%1\r\n").unwrap();
        format!("cmd /c {} %s", script.display())
    }
}

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
