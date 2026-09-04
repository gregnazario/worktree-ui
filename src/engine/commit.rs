//! Commit authoring through the user's editor — git's own COMMIT_EDITMSG
//! flow. Resolution order mirrors git: $GIT_EDITOR, core.editor, $VISUAL,
//! $EDITOR, platform default. The command is whitespace-split (quoted
//! paths with spaces are a documented Phase 1 limitation).

use crate::engine::{self, GitError, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// Shared handle to the running commit editor's process. The UI keeps a
/// clone for the length of the editor session: `request_abandon` is the
/// escape hatch for a wedged or forgotten editor (a `subl -w` tab left
/// open, an editor blocked on a network mount) — it kills the child so the
/// pending commit unwinds instead of keyboard-locking the detail view
/// until the app quits.
#[derive(Clone, Default)]
pub struct EditorHandle {
    child: Arc<Mutex<Option<std::process::Child>>>,
    abandon: Arc<AtomicBool>,
}

impl EditorHandle {
    pub fn new() -> Self {
        Self::default()
    }

    /// True once abandon has been requested. The child may not exist yet —
    /// the waiting side re-checks this flag on every poll tick and kills
    /// the editor itself if it spawned after the request landed.
    pub fn abandon_requested(&self) -> bool {
        self.abandon.load(Ordering::SeqCst)
    }

    pub fn request_abandon(&self) {
        self.abandon.store(true, Ordering::SeqCst);
        // Fast path: the child is already parked in the slot — kill and reap
        // it here (SIGKILL/TerminateProcess, so the reap is immediate).
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// What became of the launched editor process.
enum EditorExit {
    Ok,
    Failed(std::io::Error),
    /// Killed via `EditorHandle::request_abandon`.
    Abandoned,
}

/// Spawns the editor and waits, keeping the child in the shared slot so the
/// UI can kill it (see `EditorHandle`). The wait polls `try_wait` instead of
/// blocking in `wait()`: the slot must stay free for `request_abandon` to
/// take the child, and the flag must be re-checked every tick to cover an
/// abandon request that landed before the spawn. 40ms is far below any
/// human interaction with the editor.
fn run_editor_process(mut cmd: Command, handle: &EditorHandle) -> EditorExit {
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return EditorExit::Failed(e),
    };
    *handle.child.lock().unwrap() = Some(child);
    loop {
        if handle.abandon_requested() {
            if let Some(mut child) = handle.child.lock().unwrap().take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            return EditorExit::Abandoned;
        }
        let tick = {
            let mut guard = handle.child.lock().unwrap();
            // The abandoner takes the child between the flag check and this
            // lock — a None slot means it is dead either way.
            guard.as_mut().map(|child| child.try_wait())
        };
        match tick {
            None => return EditorExit::Abandoned,
            Some(Ok(Some(status))) => {
                handle.child.lock().unwrap().take();
                return if status.success() {
                    EditorExit::Ok
                } else {
                    EditorExit::Failed(std::io::Error::other(format!(
                        "editor exited with {status}"
                    )))
                };
            }
            Some(Ok(None)) => {}
            Some(Err(e)) => return EditorExit::Failed(e),
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

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
) -> (Vec<String>, bool) {
    let usable = |v: Option<String>| v.filter(|v| !v.trim().is_empty());
    let (source, used_default) = usable(getenv("GIT_EDITOR"))
        .map(|v| (v, false))
        .or_else(|| {
            git_config_value
                .map(str::to_string)
                .filter(|v| !v.trim().is_empty())
                .map(|v| (v, false))
        })
        .or_else(|| usable(getenv("VISUAL")).map(|v| (v, false)))
        .or_else(|| usable(getenv("EDITOR")).map(|v| (v, false)))
        .unwrap_or_else(|| (platform_default_editor().to_string(), true));
    (
        source.split_whitespace().map(str::to_string).collect(),
        used_default,
    )
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
    /// The editor produced an empty message. `draft` points at the kept
    /// raw file when the user changed it at all (e.g. every line quoted
    /// with the comment char) — a file still byte-identical to the
    /// template holds nothing typed and is dropped.
    AbortedEmpty {
        draft: Option<PathBuf>,
    },
    /// The user abandoned the session: the editor was killed via
    /// `EditorHandle` before committing. `draft` points at saved,
    /// non-comment message content, kept on disk for recovery.
    Abandoned {
        draft: Option<PathBuf>,
    },
}

pub fn commit_with_editor(
    worktree: &Path,
    staged_summary: &str,
    editor: Option<&EditorHandle>,
) -> Result<CommitOutcome> {
    let config_editor = engine::run_trimmed(worktree, &["config", "--get", "core.editor"]).ok();
    // git also honors `core.commentChar`; the "auto" mode is approximated
    // with '#' (the common case for messages this short).
    let comment_char = engine::run_trimmed(worktree, &["config", "--get", "core.commentChar"])
        .ok()
        .filter(|v| v.trim() != "auto")
        .and_then(|v| v.trim().chars().next())
        .unwrap_or('#');
    let (argv, used_default) = resolve_editor(config_editor.as_deref(), &|k| {
        std::env::var(k).ok().filter(|v| !v.trim().is_empty())
    });
    // Launched from Finder/Dock the app has no TTY, and the unix *default*
    // editor (vim) cannot run without one — the failure would be opaque.
    // Only the platform DEFAULT on unix is guarded: an explicitly
    // configured editor (even one equal to the default) is the user's own
    // choice and may well cope, and the Windows default (notepad) is a GUI
    // app that needs no terminal.
    #[cfg(not(windows))]
    if used_default && !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return Err(GitError {
            message: format!(
                "no terminal available for the default editor '{}' — set $GIT_EDITOR \
                 or run: git config --global core.editor <editor>",
                argv[0]
            ),
        });
    }
    #[cfg(windows)]
    let _ = used_default;
    let (mut file, msg_path) = create_msg_file()?;
    use std::io::Write as _;
    let template_text = template(comment_char, staged_summary);
    file.write_all(template_text.as_bytes()).map_err(|e| {
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
            spawn_argv.push(a.replacen("%s", &msg_arg, 1));
            substituted = true;
        } else {
            spawn_argv.push(a.clone());
        }
    }
    if !substituted {
        spawn_argv.push(msg_arg.clone());
    }

    let run_result = if argv.len() == 1 && argv[0] == ":" {
        EditorExit::Ok // ":" = succeed without launching
    } else {
        let mut cmd = Command::new(&spawn_argv[0]);
        cmd.args(&spawn_argv[1..]).current_dir(worktree);
        match editor {
            Some(handle) => run_editor_process(cmd, handle),
            None => match cmd.status() {
                Ok(status) if status.success() => EditorExit::Ok,
                Ok(status) => EditorExit::Failed(std::io::Error::other(format!(
                    "editor exited with {status}"
                ))),
                Err(e) => EditorExit::Failed(e),
            },
        }
    };
    match run_result {
        EditorExit::Ok => {}
        EditorExit::Failed(e) => {
            // The editor may have SAVED the draft before exiting non-zero:
            // keep the STRIPPED message (comment lines removed) so the
            // recovery command below works verbatim, and tell the user
            // where it is instead of destroying typed work.
            let draft = preserve_draft(comment_char, &msg_path);
            let hint = draft
                .map(|p| {
                    format!(
                        " — your draft is preserved at {} (recommit with: git commit -F \"{}\")",
                        p.display(),
                        p.display()
                    )
                })
                .unwrap_or_default();
            return Err(GitError {
                message: format!("could not run editor {}{}: {e}", argv.join(" "), hint),
            });
        }
        EditorExit::Abandoned => {
            return Ok(CommitOutcome::Abandoned {
                draft: preserve_draft(comment_char, &msg_path),
            });
        }
    }

    let raw = std::fs::read_to_string(&msg_path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::InvalidData {
            // A non-UTF-8 save still holds the user's draft: keep the
            // file (convertible with iconv) and point at it.
            GitError {
                message: format!(
                    "the editor saved the message in a non-UTF-8 encoding — \
                     your draft is preserved at {}: {e}",
                    msg_path.display()
                ),
            }
        } else {
            // Any other read error (file deleted by the editor, transient
            // IO failure) leaves the file — and possibly typed work — on
            // disk: name it like every other failure path instead of
            // leaking an unnamed 0600 temp file.
            GitError {
                message: format!(
                    "could not read the commit message file at {} — \
                     whatever the editor saved is preserved there: {e}",
                    msg_path.display()
                ),
            }
        }
    })?;

    let message = strip_comments(comment_char, &raw);
    if message.is_empty() {
        // An empty stripped message can still hold typed work: a draft
        // whose every line starts with the comment char (or whitespace)
        // strips to nothing. Keep the raw file whenever the user changed
        // it at all; drop a file still byte-identical to the template
        // (opened and quit with nothing typed).
        let draft = if raw != template_text {
            Some(msg_path.clone())
        } else {
            let _ = std::fs::remove_file(&msg_path);
            None
        };
        return Ok(CommitOutcome::AbortedEmpty { draft });
    }
    match commit(worktree, &message) {
        Ok(()) => {
            let _ = std::fs::remove_file(&msg_path);
            Ok(CommitOutcome::Committed)
        }
        Err(e) => {
            // commit() kept its own message file and the error names it;
            // this editor draft is a byte-for-byte duplicate — remove it so
            // no stray second copy lingers in the temp dir.
            let _ = std::fs::remove_file(&msg_path);
            Err(e)
        }
    }
}

/// After an editor failure or abandon: keep saved, non-comment message
/// content on disk (the caller surfaces its path) and drop a file that
/// holds nothing recoverable.
fn preserve_draft(comment_char: char, msg_path: &Path) -> Option<PathBuf> {
    let stripped = std::fs::read_to_string(msg_path)
        .ok()
        .map(|raw| strip_comments(comment_char, &raw))
        .filter(|m| !m.is_empty());
    match stripped {
        Some(m) => {
            let _ = std::fs::write(msg_path, m);
            Some(msg_path.to_path_buf())
        }
        None => {
            let _ = std::fs::remove_file(msg_path);
            None
        }
    }
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
