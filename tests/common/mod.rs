use std::path::Path;

pub fn sh(cwd: Option<&Path>, cmd: &[&str]) {
    let status = sh_allow_fail(cwd, cmd);
    assert!(status.success(), "failed: {cmd:?}");
}

/// Like [`sh`] but tolerates a non-zero exit (e.g. an intentional merge
/// conflict) and returns the exit status for callers that inspect it.
pub fn sh_allow_fail(cwd: Option<&Path>, cmd: &[&str]) -> std::process::ExitStatus {
    std::process::Command::new(cmd[0])
        .args(&cmd[1..])
        .current_dir(cwd.unwrap_or(Path::new(".")))
        .status()
        .expect("spawn")
}

/// Repo with one commit on `main`: file `f.txt` containing "one".
pub fn fixture_repo(dir: &Path) {
    sh(Some(dir), &["git", "init", "-q", "-b", "main"]);
    sh(Some(dir), &["git", "config", "user.email", "t@t.t"]);
    sh(Some(dir), &["git", "config", "user.name", "t"]);
    std::fs::write(dir.join("f.txt"), "one").unwrap();
    sh(Some(dir), &["git", "add", "."]);
    sh(Some(dir), &["git", "commit", "-qm", "init"]);
}
