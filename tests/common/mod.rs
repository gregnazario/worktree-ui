use std::path::Path;

pub fn sh(cwd: &Path, cmd: &[&str]) {
    let status = std::process::Command::new(cmd[0])
        .args(&cmd[1..])
        .current_dir(cwd)
        .status()
        .expect("spawn");
    assert!(status.success(), "failed: {cmd:?}");
}

/// Repo with one commit on `main`: file `f.txt` containing "one".
pub fn fixture_repo(dir: &Path) {
    sh(dir, &["git", "init", "-q", "-b", "main"]);
    sh(dir, &["git", "config", "user.email", "t@t.t"]);
    sh(dir, &["git", "config", "user.name", "t"]);
    std::fs::write(dir.join("f.txt"), "one").unwrap();
    sh(dir, &["git", "add", "."]);
    sh(dir, &["git", "commit", "-qm", "init"]);
}
