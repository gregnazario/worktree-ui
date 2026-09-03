//! Commit authoring through the user's editor — git's own COMMIT_EDITMSG
//! flow. Resolution order mirrors git: $GIT_EDITOR, core.editor, $VISUAL,
//! $EDITOR, platform default. The command is whitespace-split (quoted
//! paths with spaces are a documented Phase 1 limitation).

use crate::engine::{self, GitError, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// Creates the commit-message file with EXCLUSIVE creation and (on unix)
/// owner-only permissions. Both matter in the shared temp dir: `create_new`
/// refuses to follow a pre-planted symlink or clobber an existing file, and
/// 0600 keeps the in-progress message private. Returns the open handle —
/// callers write through it, never by path, so a path swap can't redirect
/// the write.
fn create_msg_file() -> Result<(std::fs::File, PathBuf)> {
    for _ in 0..32 {
        let n = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "worktree-tool-commit-{}-{n}.msg",
            std::process::id()
        ));
        // The restrictive mode is set AT CREATION (not chmod'd afterwards):
        // a post-create chmod leaves a 0644 window in which another local
        // user can open a readable fd and read the message once written.
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            opts.mode(0o600);
        }
        match opts.open(&path) {
            Ok(file) => return Ok((file, path)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(GitError {
                    message: format!("could not create commit message file: {e}"),
                })
            }
        }
    }
    Err(GitError {
        message: "could not create a unique commit message file".into(),
    })
}

pub fn author(worktree: &Path) -> (String, String) {
    let name = engine::run_trimmed(worktree, &["config", "user.name"])
        .unwrap_or_else(|_| "(unset)".into());
    let email = engine::run_trimmed(worktree, &["config", "user.email"])
        .unwrap_or_else(|_| "(unset)".into());
    (name, email)
}

/// Pure so tests can inject env/config lookups. Returns a non-empty argv
/// (the platform default guarantees at least one element, split on
/// whitespace); the message file is appended as the last argument. Every
/// source passes through the same blank-value filter so an exported-empty
/// `$VISUAL`/`$EDITOR` falls through to the platform default instead of
/// producing an empty argv.
pub fn resolve_editor(
    git_config_value: Option<&str>,
    getenv: &dyn Fn(&str) -> Option<String>,
) -> Vec<String> {
    let usable = |v: Option<String>| v.filter(|v| !v.trim().is_empty());
    let source = usable(getenv("GIT_EDITOR"))
        .or_else(|| usable(git_config_value.map(str::to_string)))
        .or_else(|| usable(getenv("VISUAL")))
        .or_else(|| usable(getenv("EDITOR")))
        .unwrap_or_else(|| platform_default_editor().to_string());
    source.split_whitespace().map(str::to_string).collect()
}

#[cfg(all(not(windows), not(target_os = "freebsd")))]
fn platform_default_editor() -> &'static str {
    "vim"
}

// FreeBSD's base system ships `ee`, not vim.
#[cfg(target_os = "freebsd")]
fn platform_default_editor() -> &'static str {
    "ee"
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
    // git also honors `core.commentChar`; the "auto" mode is approximated
    // with '#' (the common case for messages this short).
    let comment_char = engine::run_trimmed(worktree, &["config", "--get", "core.commentChar"])
        .ok()
        .filter(|v| v.trim() != "auto")
        .and_then(|v| v.trim().chars().next())
        .unwrap_or('#');
    let argv = resolve_editor(config_editor.as_deref(), &|k| {
        std::env::var(k).ok().filter(|v| !v.trim().is_empty())
    });
    // Launched from Finder/Dock the app has no TTY, and the unix *default*
    // editor (vim) cannot run without one — the failure would be opaque.
    // Only the DEFAULT is guarded, and only on unix: the Windows default
    // (notepad) is a GUI app that needs no terminal. A configured editor is
    // the user's own choice and may well cope (e.g. a GUI editor).
    #[cfg(not(windows))]
    {
        use std::io::IsTerminal as _;
        if argv.len() == 1
            && argv[0] == platform_default_editor()
            && !std::io::stdin().is_terminal()
        {
            return Err(GitError {
                message: format!(
                    "no terminal available for the default editor '{}' — set $GIT_EDITOR \
                     or run: git config --global core.editor <editor>",
                    argv[0]
                ),
            });
        }
    }
    let (mut file, msg_path) = create_msg_file()?;
    use std::io::Write as _;
    file.write_all(template(comment_char, staged_summary).as_bytes())
        .map_err(|e| {
            let _ = std::fs::remove_file(&msg_path);
            GitError {
                message: format!("could not write commit template: {e}"),
            }
        })?;
    drop(file);

    // git semantics: an editor spec containing `%s` gets the message file
    // substituted at that spot (only the first `%s` per git's editor.c);
    // otherwise the path is appended. A spec of exactly `:` succeeds
    // without launching anything.
    let msg_arg = msg_path.to_string_lossy().into_owned();
    let mut spawn_argv: Vec<String> = Vec::new();
    let mut substituted = false;
    for a in &argv {
        if !substituted && a.contains("%s") {
            spawn_argv.push(a.replace("%s", &msg_arg));
            substituted = true;
        } else {
            spawn_argv.push(a.clone());
        }
    }
    if !substituted {
        spawn_argv.push(msg_arg.clone());
    }

    let run_result = (|| -> std::io::Result<()> {
        if argv.len() == 1 && argv[0] == ":" {
            return Ok(()); // ":" = succeed without launching
        }
        let mut cmd = Command::new(&spawn_argv[0]);
        cmd.args(&spawn_argv[1..]).current_dir(worktree);
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
        Ok(()) => std::fs::read_to_string(&msg_path).map_err(|e| {
            // A non-UTF-8 save still holds the user's draft: keep the file
            // (convertible with iconv) and point at it, instead of deleting.
            GitError {
                message: format!(
                    "the editor saved the message in a non-UTF-8 encoding — \
                     your draft is preserved at {}: {e}",
                    msg_path.display()
                ),
            }
        })?,
        Err(e) => {
            // The editor may have SAVED the draft before exiting non-zero
            // (e.g. save-then-fail hooks): keep the file and tell the user
            // where it is, instead of destroying typed work.
            let draft = std::fs::read_to_string(&msg_path)
                .ok()
                .map(|raw| strip_comments(comment_char, &raw))
                .filter(|m| !m.is_empty());
            let hint = match draft {
                Some(_) => format!(
                    " — your draft is preserved at {} (recommit with: git commit -F \"{}\")",
                    msg_path.display(),
                    msg_path.display()
                ),
                None => {
                    let _ = std::fs::remove_file(&msg_path);
                    String::new()
                }
            };
            return Err(GitError {
                message: format!("could not run editor {}{}: {e}", argv.join(" "), hint),
            });
        }
    };
    let _ = std::fs::remove_file(&msg_path);

    let message = strip_comments(comment_char, &raw);
    if message.is_empty() {
        return Ok(CommitOutcome::AbortedEmpty);
    }
    commit(worktree, &message)?;
    Ok(CommitOutcome::Committed)
}

fn template(comment_char: char, staged_summary: &str) -> String {
    format!(
        "\n{c} Please enter the commit message for your changes. Lines starting\n\
         {c} with '{c}' are ignored, and an empty message aborts the commit.\n{c}\n\
         {c} {staged_summary}\n",
        c = comment_char
    )
}

/// Drops `#` comment lines and trims the outside whitespace. Empty result
/// means the user aborted.
pub fn strip_comments(comment_char: char, raw: &str) -> String {
    let kept: Vec<&str> = raw
        .lines()
        .filter(|l| !l.starts_with(comment_char))
        .collect();
    kept.join("\n").trim().to_string()
}

/// `git commit -q -F <file>` — `-F` avoids every quoting/length issue of
/// `-m`. User hooks run normally. On failure the message file is KEPT (the
/// error names its path): git itself preserves COMMIT_EDITMSG on failed
/// commits, and the typed message is the user's work — a rejecting
/// pre-commit hook must not destroy it.
pub fn commit(worktree: &Path, message: &str) -> Result<()> {
    let (mut file, msg_path) = create_msg_file()?;
    use std::io::Write as _;
    file.write_all(message.as_bytes()).map_err(|e| {
        let _ = std::fs::remove_file(&msg_path);
        GitError {
            message: format!("could not write commit message: {e}"),
        }
    })?;
    drop(file);
    let msg_arg = msg_path.to_string_lossy().into_owned();
    let res = engine::run_trimmed(worktree, &["commit", "-q", "-F", &msg_arg]);
    if let Err(e) = &res {
        // Keep the file so the message survives; the error tells the user
        // where it is.
        return Err(GitError {
            message: format!(
                "{} — your commit message is preserved at {}",
                e.message, msg_arg
            ),
        });
    }
    let _ = std::fs::remove_file(&msg_path);
    Ok(())
}
