//! Best-effort OS integration helpers. Fire-and-forget: failures to spawn
//! (e.g. missing terminal app) are silently ignored in v1.

use std::path::Path;

#[cfg(target_os = "macos")]
pub fn open_in_terminal(path: &Path) {
    let _ = std::process::Command::new("open")
        .arg("-a")
        .arg("Terminal")
        .arg(path)
        .spawn();
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn open_in_terminal(path: &Path) {
    for term in ["xdg-terminal-exec", "gnome-terminal", "konsole", "xterm"] {
        if std::process::Command::new(term)
            .arg("--working-directory")
            .arg(path)
            .spawn()
            .or_else(|_| std::process::Command::new(term).arg(path).spawn())
            .is_ok()
        {
            return;
        }
    }
}

#[cfg(target_os = "windows")]
pub fn open_in_terminal(path: &Path) {
    let spawn = |exe: &str, pre: &[&str]| {
        std::process::Command::new("cmd")
            .arg("/C")
            .arg("start")
            .arg(exe)
            .args(pre)
            .arg(path)
            .spawn()
    };
    let _ = spawn("wt", &["-d"]).or_else(|_| spawn("cmd", &["/K", "cd", "/D"]));
}

#[cfg(target_os = "macos")]
pub fn reveal_in_file_manager(path: &Path) {
    let _ = std::process::Command::new("open").arg(path).spawn();
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn reveal_in_file_manager(path: &Path) {
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

#[cfg(target_os = "windows")]
pub fn reveal_in_file_manager(path: &Path) {
    let _ = std::process::Command::new("explorer").arg(path).spawn();
}
