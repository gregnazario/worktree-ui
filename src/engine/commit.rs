//! Commit authoring through the user's editor — git's own COMMIT_EDITMSG
//! flow. Resolution order mirrors git: $GIT_EDITOR, core.editor, $VISUAL,
//! $EDITOR, platform default. The command is whitespace-split (quoted
//! paths with spaces are a documented Phase 1 limitation).

use crate::engine::{self, GitError, Result};
use std::path::Path;
use std::process::Command;

pub fn author(worktree: &Path) -> (String, String) {
    let name = engine::run_trimmed(worktree, &["config", "user.name"])
        .unwrap_or_else(|_| "(unset)".into());
    let email = engine::run_trimmed(worktree, &["config", "user.email"])
        .unwrap_or_else(|_| "(unset)".into());
    (name, email)
}

/// Pure so tests can inject env/config lookups. Returns argv (split on
/// whitespace); the message file is appended as the last argument.
pub fn resolve_editor(
    git_config_value: Option<&str>,
    getenv: &dyn Fn(&str) -> Option<String>,
) -> Vec<String> {
    let source = getenv("GIT_EDITOR")
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            git_config_value
                .map(str::to_string)
                .filter(|v| !v.trim().is_empty())
        })
        .or_else(|| getenv("VISUAL"))
        .or_else(|| getenv("EDITOR"))
        .unwrap_or_else(|| platform_default_editor().to_string());
    source.split_whitespace().map(str::to_string).collect()
}

#[cfg(not(windows))]
fn platform_default_editor() -> &'static str {
    "vim"
}

#[cfg(windows)]
fn platform_default_editor() -> &'static str {
    "notepad"
}

#[derive(Debug)]
pub enum CommitOutcome {
    Committed,
    AbortedEmpty,
}

pub fn commit_with_editor(worktree: &Path, staged_summary: &str) -> Result<CommitOutcome> {
    let config_editor = engine::run_trimmed(worktree, &["config", "--get", "core.editor"]).ok();
    let argv = resolve_editor(config_editor.as_deref(), &|k| {
        std::env::var(k).ok().filter(|v| !v.is_empty())
    });
    let msg_path =
        std::env::temp_dir().join(format!("worktree-tool-commit-{}.msg", std::process::id()));
    std::fs::write(&msg_path, template(staged_summary)).map_err(|e| GitError {
        message: format!("could not write commit template: {e}"),
    })?;

    let run_result = (|| -> std::io::Result<()> {
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..]).arg(&msg_path).current_dir(worktree);
        let status = cmd.status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!(
                "editor exited with {status}"
            )))
        }
    })();
    let raw = match run_result {
        Ok(()) => std::fs::read_to_string(&msg_path).unwrap_or_default(),
        Err(e) => {
            let _ = std::fs::remove_file(&msg_path);
            return Err(GitError {
                message: format!("could not run editor {}: {e}", argv.join(" ")),
            });
        }
    };
    let _ = std::fs::remove_file(&msg_path);

    let message = strip_comments(&raw);
    if message.is_empty() {
        return Ok(CommitOutcome::AbortedEmpty);
    }
    commit(worktree, &message)?;
    Ok(CommitOutcome::Committed)
}

fn template(staged_summary: &str) -> String {
    format!(
        "\n# Please enter the commit message for your changes. Lines starting\n\
         # with '#' are ignored, and an empty message aborts the commit.\n#\n\
         # {staged_summary}\n"
    )
}

/// Drops `#` comment lines and trims the outside whitespace. Empty result
/// means the user aborted.
pub fn strip_comments(raw: &str) -> String {
    let kept: Vec<&str> = raw.lines().filter(|l| !l.starts_with('#')).collect();
    kept.join("\n").trim().to_string()
}

/// `git commit -q -F <file>` — `-F` avoids every quoting/length issue of
/// `-m`. User hooks run normally.
pub fn commit(worktree: &Path, message: &str) -> Result<()> {
    let msg_path =
        std::env::temp_dir().join(format!("worktree-tool-commit-{}.msg", std::process::id()));
    std::fs::write(&msg_path, message).map_err(|e| GitError {
        message: format!("could not write commit message: {e}"),
    })?;
    let msg_arg = msg_path.to_string_lossy().into_owned();
    let res = engine::run_trimmed(worktree, &["commit", "-q", "-F", &msg_arg]);
    let _ = std::fs::remove_file(&msg_path);
    res.map(|_| ())
}
