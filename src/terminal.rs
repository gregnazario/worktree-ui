//! Terminal discovery, selection settings (XDG paths), and launching.
//!
//! Selection precedence: `settings.toml` → `$TERMCMD` → first detected
//! terminal. Each platform registry is ordered by preference, so
//! auto-detect resolves to the first installed entry.
//!
//! Launch semantics: CLI terminals are spawned with the worktree as the
//! process working directory (every supported terminal inherits its cwd);
//! Windows Terminal additionally gets an explicit `-d` flag because its
//! profiles override the inherited directory. macOS app bundles launch via
//! `open -a <app> <path>`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// How to launch a given terminal.
#[derive(Clone, Debug, PartialEq)]
pub enum Launch {
    /// `open -a <app> <path>` — macOS app bundle (macOS only).
    #[cfg(target_os = "macos")]
    OpenApp(&'static str),
    /// Binary plus args; `"{path}"` in args is replaced with the worktree
    /// path. Launched with the worktree as the process working directory.
    Cli(&'static str, &'static [&'static str]),
}

/// One entry in the platform terminal registry.
pub struct TerminalKind {
    pub id: &'static str,
    pub name: &'static str,
    /// macOS app bundle file name (e.g. "iTerm.app"); None on other
    /// platforms, where detection is CLI-based only.
    pub bundle: Option<&'static str>,
    pub cli: Option<(&'static str, &'static [&'static str])>,
}

const fn kind(
    id: &'static str,
    name: &'static str,
    bundle: Option<&'static str>,
    cli: Option<(&'static str, &'static [&'static str])>,
) -> TerminalKind {
    TerminalKind {
        id,
        name,
        bundle,
        cli,
    }
}

#[cfg(target_os = "macos")]
pub const REGISTRY: &[TerminalKind] = &[
    kind("terminal", "Terminal", Some("Terminal.app"), None),
    kind("iterm", "iTerm2", Some("iTerm.app"), None),
    kind(
        "wezterm",
        "WezTerm",
        Some("WezTerm.app"),
        Some(("wezterm", &["start", "--cwd", "{path}"])),
    ),
    kind(
        "ghostty",
        "Ghostty",
        Some("Ghostty.app"),
        Some(("ghostty", &["--working-directory", "{path}"])),
    ),
    kind(
        "alacritty",
        "Alacritty",
        Some("Alacritty.app"),
        Some(("alacritty", &["--working-directory", "{path}"])),
    ),
    kind(
        "kitty",
        "Kitty",
        Some("kitty.app"),
        Some(("kitty", &["--directory", "{path}"])),
    ),
    kind("warp", "Warp", Some("Warp.app"), None),
    kind("hyper", "Hyper", Some("Hyper.app"), None),
];

/// Linux and the BSDs: CLI-only detection, preference-ordered. The
/// freedesktop `xdg-terminal-exec` spec comes first when present, then
/// desktop-environment terminals, then the cross-platform ones, then xterm
/// as the universal fallback.
#[cfg(all(unix, not(target_os = "macos")))]
pub const REGISTRY: &[TerminalKind] = &[
    kind(
        "xdg-terminal-exec",
        "System (xdg-terminal-exec)",
        None,
        Some(("xdg-terminal-exec", &[])),
    ),
    kind(
        "gnome-terminal",
        "GNOME Terminal",
        None,
        Some(("gnome-terminal", &[])),
    ),
    kind("konsole", "Konsole", None, Some(("konsole", &[]))),
    kind(
        "xfce4-terminal",
        "Xfce Terminal",
        None,
        Some(("xfce4-terminal", &[])),
    ),
    kind("foot", "foot", None, Some(("foot", &[]))),
    kind("tilix", "Tilix", None, Some(("tilix", &[]))),
    kind("kitty", "Kitty", None, Some(("kitty", &[]))),
    kind("ghostty", "Ghostty", None, Some(("ghostty", &[]))),
    kind("alacritty", "Alacritty", None, Some(("alacritty", &[]))),
    kind("wezterm", "WezTerm", None, Some(("wezterm", &[]))),
    kind("xterm", "xterm", None, Some(("xterm", &[]))),
];

#[cfg(target_os = "windows")]
pub const REGISTRY: &[TerminalKind] = &[
    kind(
        "wt",
        "Windows Terminal",
        None,
        Some(("wt", &["-d", "{path}"])),
    ),
    kind("pwsh", "PowerShell", None, Some(("pwsh", &[]))),
    kind(
        "powershell",
        "Windows PowerShell",
        None,
        Some(("powershell", &[])),
    ),
    kind("cmd", "Command Prompt", None, Some(("cmd", &[]))),
    kind("alacritty", "Alacritty", None, Some(("alacritty", &[]))),
    kind("wezterm", "WezTerm", None, Some(("wezterm", &[]))),
    kind("ghostty", "Ghostty", None, Some(("ghostty", &[]))),
];

/// A terminal detected on this machine.
#[derive(Clone, Debug)]
pub struct InstalledTerminal {
    pub id: &'static str,
    pub name: &'static str,
    pub launch: Launch,
}

/// Short launch description for the settings dialog, kept here so UI code
/// stays free of platform cfgs.
pub fn describe_launch(launch: &Launch) -> String {
    match launch {
        #[cfg(target_os = "macos")]
        Launch::OpenApp(app) => format!("open -a {app}"),
        Launch::Cli(bin, args) => {
            if args.is_empty() {
                bin.to_string()
            } else {
                format!("{bin} {}", args.join(" "))
            }
        }
    }
}

/// Pure registry resolution so detection is unit-testable without touching
/// the filesystem: `bundle_exists("iTerm.app")` / `cli_on_path("wezterm")`
/// say what their names say.
pub fn detect_with(
    bundle_exists: impl Fn(&str) -> bool,
    cli_on_path: impl Fn(&str) -> bool,
) -> Vec<InstalledTerminal> {
    // Bundles only exist on the macOS registry; reference the probe on the
    // other platforms so it stays in the signature.
    let _ = &bundle_exists;
    REGISTRY
        .iter()
        .filter_map(|kind| {
            // Prefer the CLI form (explicit cwd where flagged, and it works
            // even when the app bundle lives outside the search dirs).
            if let Some((bin, args)) = kind.cli {
                if cli_on_path(bin) {
                    return Some(InstalledTerminal {
                        id: kind.id,
                        name: kind.name,
                        launch: Launch::Cli(bin, args),
                    });
                }
            }
            #[cfg(target_os = "macos")]
            if let Some(bundle) = kind.bundle {
                if bundle_exists(bundle) {
                    let app = bundle.strip_suffix(".app").unwrap_or(bundle);
                    return Some(InstalledTerminal {
                        id: kind.id,
                        name: kind.name,
                        launch: Launch::OpenApp(app),
                    });
                }
            }
            None
        })
        .collect()
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// XDG config dir (explicitly XDG semantics on every platform, per project
/// convention): `$XDG_CONFIG_HOME` when set and absolute, else `~/.config`.
pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
        let dir = PathBuf::from(dir);
        if dir.is_absolute() {
            return dir.join("worktree-tool");
        }
    }
    home_dir().join(".config").join("worktree-tool")
}

pub fn settings_path() -> PathBuf {
    config_dir().join("settings.toml")
}

/// Persisted user preferences. Only `terminal` today; the parser is a
/// deliberately tiny TOML subset (`key = "value"` lines) so the app has no
/// config-format dependency.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    /// Chosen terminal id (a registry id), or None for auto-detect.
    pub terminal: Option<String>,
}

pub fn parse_settings(text: &str) -> Settings {
    let mut terminal = None;
    for raw in text.lines() {
        // Comments run to end of line only when they start the line or
        // follow whitespace; a '#' inside a value is kept.
        let mut line: &str = raw;
        if line.trim_start().starts_with('#') {
            line = "";
        } else if let Some(i) = line.find(" #") {
            line = &line[..i];
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim().trim_matches('"').trim_matches('\'');
            if key == "terminal" && !value.is_empty() {
                terminal = Some(value.to_string());
            }
        }
    }
    Settings { terminal }
}

pub fn load_settings() -> Settings {
    std::fs::read_to_string(settings_path())
        .map(|text| parse_settings(&text))
        .unwrap_or_default()
}

pub fn render_settings(settings: &Settings) -> String {
    let mut out = String::from("# worktree-tool settings\n# terminal: one of ");
    out.push_str(&REGISTRY.iter().map(|k| k.id).collect::<Vec<_>>().join(", "));
    out.push_str(" (or unset for auto-detect)\n");
    if let Some(id) = &settings.terminal {
        out.push_str(&format!("terminal = \"{id}\"\n"));
    }
    out
}

pub fn save_settings(settings: &Settings) -> std::io::Result<PathBuf> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, render_settings(settings))?;
    Ok(path)
}

#[cfg(target_os = "macos")]
fn bundle_search_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/System/Applications"),
        PathBuf::from("/System/Applications/Utilities"),
    ];
    dirs.push(home_dir().join("Applications"));
    dirs
}

pub fn detect_installed() -> Vec<InstalledTerminal> {
    detect_with(
        #[cfg(target_os = "macos")]
        |bundle: &str| {
            bundle_search_dirs()
                .iter()
                .any(|dir| dir.join(bundle).is_dir())
        },
        #[cfg(not(target_os = "macos"))]
        |_bundle: &str| false,
        |bin: &str| binary_on_path(bin),
    )
}

/// PATH lookup that is aware of Windows executable extensions.
fn binary_on_path(bin: &str) -> bool {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path).any(|dir| {
        #[cfg(unix)]
        {
            is_executable(&dir.join(bin))
        }
        #[cfg(not(unix))]
        {
            ["", ".exe", ".cmd", ".bat"]
                .iter()
                .any(|ext| is_executable(&dir.join(format!("{bin}{ext}"))))
        }
    })
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.is_file()
            && std::fs::metadata(path)
                .map(|m| m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

fn launch(launch: &Launch, path: &Path) {
    match launch {
        #[cfg(target_os = "macos")]
        Launch::OpenApp(app) => {
            let _ = Command::new("open").arg("-a").arg(app).arg(path).spawn();
        }
        Launch::Cli(bin, args) => {
            let _ = Command::new(bin)
                .args(args.iter().map(|a| {
                    if *a == "{path}" {
                        path.to_string_lossy().into_owned()
                    } else {
                        a.to_string()
                    }
                }))
                // Every supported terminal opens in its process working
                // directory, so this is the portable way to set the cwd.
                .current_dir(path)
                .spawn();
        }
    }
}

/// Opens `path` in the user's chosen terminal, resolving precedence:
/// settings file → `$TERMCMD` → first detected terminal.
pub fn open_in_terminal(path: &Path) {
    let installed = detect_installed();
    let by_id = |id: &str| installed.iter().find(|t| t.id == id);

    let settings = load_settings();
    if let Some(id) = settings.terminal.as_deref() {
        if let Some(t) = by_id(id) {
            launch(&t.launch, path);
            return;
        }
        // Stale preference (terminal was uninstalled): fall through to auto.
    }

    if let Ok(termcmd) = std::env::var("TERMCMD") {
        if !termcmd.is_empty() {
            if let Some(t) = by_id(&termcmd) {
                launch(&t.launch, path);
                return;
            }
            #[cfg(target_os = "macos")]
            {
                // Zed convention: TERMCMD is an app name usable with `open -a`.
                let _ = Command::new("open")
                    .arg("-a")
                    .arg(&termcmd)
                    .arg(path)
                    .spawn();
                return;
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = Command::new(termcmd).current_dir(path).spawn();
                return;
            }
        }
    }

    if let Some(t) = installed.first() {
        launch(&t.launch, path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_ids_are_unique_and_entries_are_findable() {
        let mut ids: Vec<_> = REGISTRY.iter().map(|k| k.id).collect();
        let total = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), total, "duplicate registry ids");
        for k in REGISTRY {
            assert!(
                k.bundle.is_some() || k.cli.is_some(),
                "{} has neither bundle nor cli",
                k.id
            );
            if let Some((_, args)) = k.cli {
                assert!(
                    args.iter().all(|a| *a == "{path}" || !a.contains("{path}")),
                    "{} uses the placeholder inside a compound arg",
                    k.id
                );
            }
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn detect_prefers_cli_over_bundle() {
        let found = detect_with(|_| false, |bin| bin == "wezterm");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "wezterm");
        assert_eq!(
            found[0].launch,
            Launch::Cli("wezterm", &["start", "--cwd", "{path}"])
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn detect_falls_back_to_bundle() {
        let found = detect_with(|b| b == "iTerm.app", |_| false);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].launch, Launch::OpenApp("iTerm"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn detect_skips_missing_and_keeps_registry_order() {
        let found = detect_with(|b| b == "Terminal.app" || b == "Warp.app", |_| false);
        assert_eq!(
            found.iter().map(|t| t.id).collect::<Vec<_>>(),
            vec!["terminal", "warp"]
        );
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn detect_is_cli_only_in_preference_order() {
        let found = detect_with(|_| false, |bin| bin == "kitty" || bin == "gnome-terminal");
        assert_eq!(
            found.iter().map(|t| t.id).collect::<Vec<_>>(),
            vec!["gnome-terminal", "kitty"]
        );
        assert_eq!(found[0].launch, Launch::Cli("gnome-terminal", &[]));
    }

    #[test]
    fn settings_parse_handles_quotes_comments_and_junk() {
        let s = parse_settings("# comment\nterminal = \"iterm\"\nnoise\nx = 'y'\n");
        assert_eq!(s.terminal.as_deref(), Some("iterm"));
        let s = parse_settings("terminal='ghostty' # trailing\n");
        assert_eq!(s.terminal.as_deref(), Some("ghostty"));
        assert_eq!(parse_settings("").terminal, None);
        assert_eq!(parse_settings("terminal = \"\"").terminal, None);
    }

    #[test]
    fn settings_roundtrip() {
        let s = Settings {
            terminal: Some("kitty".into()),
        };
        assert_eq!(parse_settings(&render_settings(&s)), s);
        assert_eq!(
            parse_settings(&render_settings(&Settings::default())),
            Settings::default()
        );
    }

    #[test]
    fn settings_save_and_load_via_xdg() {
        let tmp = tempfile::tempdir().unwrap();
        // Test-scoped XDG_CONFIG_HOME pointing at a scratch dir; restored after.
        let prev = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", tmp.path());
        let saved = save_settings(&Settings {
            terminal: Some("iterm".into()),
        })
        .unwrap();
        assert_eq!(
            saved,
            tmp.path().join("worktree-tool").join("settings.toml")
        );
        assert_eq!(load_settings().terminal.as_deref(), Some("iterm"));
        match prev {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
    }
}
