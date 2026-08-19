//! Best-effort OS integration helpers. Fire-and-forget: failures to spawn
//! are silently ignored in v1. Terminal launching lives in `terminal.rs`.

use std::path::Path;

/// Button label matching the platform's file-manager vocabulary.
#[cfg(target_os = "macos")]
pub const SHOW_IN_FILE_MANAGER_LABEL: &str = "Show in Finder";
#[cfg(target_os = "windows")]
pub const SHOW_IN_FILE_MANAGER_LABEL: &str = "Show in File Explorer";
#[cfg(all(unix, not(target_os = "macos")))]
pub const SHOW_IN_FILE_MANAGER_LABEL: &str = "Show in Files";

/// Reveals `path` in the platform file manager, selecting it in its parent.
#[cfg(target_os = "macos")]
pub fn reveal_in_file_manager(path: &Path) {
    let _ = std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn();
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn reveal_in_file_manager(path: &Path) {
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

#[cfg(target_os = "windows")]
pub fn reveal_in_file_manager(path: &Path) {
    let _ = std::process::Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn();
}
