# worktree-tool

<p align="left">
  <img src="assets/logo.svg" width="96" alt="worktree-tool logo">
</p>

[![CI](https://github.com/gregnazario/worktree-ui/actions/workflows/ci.yml/badge.svg)](https://github.com/gregnazario/worktree-ui/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)

A native GUI for managing [git worktrees](https://git-scm.com/docs/git-worktree),
built in Rust with [GPUI](https://gpui.rs) (Zed's GPU-accelerated UI framework).

**[Website, tutorials, and examples →](https://gregnazario.github.io/worktree-ui/)**

Lists every worktree of a repository with its branch, status (dirty files,
ahead/behind vs upstream), and offers create / remove / prune operations plus
quick actions (open in terminal, show in file manager, copy path).
Keyboard-first.

<img src="docs/assets/screenshot.png" width="860" alt="worktree-tool showing three worktrees with clean, dirty, and ahead statuses">

## Install (macOS)

Grab `worktree-tool-<version>-macos-universal.dmg` from the latest
[GitHub Release](https://github.com/gregnazario/worktree-ui/releases)
(built automatically by CI: universal arm64 + x86_64), open it, and drag
**Worktree Tool** onto the **Applications** shortcut. A plain `.zip` of the
`.app` bundle is attached alongside for tooling that prefers it.

The release build is ad-hoc signed, not notarized — on first launch
macOS Gatekeeper may ask you to confirm via right-click → Open. When launched
from Finder the app starts at the path picker (there is no terminal cwd to
detect); when run from a terminal it auto-detects the repository from the
working directory.

To package locally: `scripts/package-macos.sh 0.1.0` produces
`dist/Worktree Tool.app` and the zip.

## Platform support

| Platform | Status |
| --- | --- |
| macOS (Apple silicon / Intel) | built, tested, and manually verified |
| Linux (X11 / Wayland) | compiles and unit-tested via CI; rendering uses Vulkan through blade-graphics, linking needs `libxcb`, `libxkbcommon`, `libxkbcommon-x11`, `libstdc++` |
| Windows | compiles and unit-tested via CI (MSVC toolchain) |
| FreeBSD | compiles via CI (GPUI gates its X11/Wayland backend to `linux` + `freebsd`); least-tested platform |

CI (`.github/workflows/ci.yml`) runs clippy, tests, and a release build on
macOS, Linux, Windows, and FreeBSD on every push once a remote is added.
For local cross-checks without target hardware:
`cargo zigbuild --target x86_64-unknown-linux-gnu --lib` (also
`x86_64-pc-windows-gnu`, `x86_64-unknown-freebsd`) — every crate compiles
cross-platform; only the final Linux binary link needs native X11 libraries.

## Requirements

- Stable Rust toolchain
- `git` on your `PATH`
- macOS: Xcode command line tools (`xcode-select --install`) for the Metal backend
- Linux: the X11/xkbcommon dev packages above plus a Vulkan-capable driver stack

## Usage

Run from inside any git repository — the app detects the repo from the
current working directory:

```sh
cd ~/git/myrepo
cargo run --release
```

For step-by-step tutorials (first worktree, terminal setup, cleanup) and
example workflows (hotfix mid-feature, PR review checkouts, parallel test
runs), see the [website](https://gregnazario.github.io/worktree-ui/#tutorials).

### Shortcuts

| Key | Action |
| --- | --- |
| `n` / `cmd-n` | New worktree |
| `/` | Focus search |
| `r` / `cmd-r` | Refresh |
| `up` / `down` | Move selection |
| `enter` | Open selection in terminal |
| `backspace` / `delete` | Remove selected worktree |
| `esc` | Clear search / close dialog |

## Settings

The **Settings** button (toolbar) lists the terminals detected on this
machine; clicking one persists the choice immediately. The config file lives
at an XDG path on every platform:

```sh
$XDG_CONFIG_HOME/worktree-tool/settings.toml   # e.g. ~/.config/worktree-tool/settings.toml
```

```toml
# worktree-tool settings
# terminal: one of the ids below (or unset for auto-detect)
terminal = "iterm"
```

Terminal resolution order: `settings.toml` → `$TERMCMD` env var (app name,
[Zed convention](https://zed.dev)) → first detected terminal.

**Report a bug** (Settings → *Report a bug*) opens a prefilled GitHub issue
in your browser. It includes only the app version and platform — nothing
else is collected, and you review it before submitting. CLI terminals
launch with the worktree as their working directory; Windows Terminal gets an
explicit `-d` flag because its profiles override the inherited directory.

Supported terminals (auto-detected; the settings dialog only lists installed
ones):

| Platform | Terminals, in auto-detect preference order |
| --- | --- |
| macOS | Terminal, iTerm2, WezTerm, Ghostty, Alacritty, Kitty, Warp, Hyper |
| Linux / BSD | xdg-terminal-exec, GNOME Terminal, Konsole, Xfce Terminal, foot, Tilix, Kitty, Ghostty, Alacritty, WezTerm, xterm |
| Windows | Windows Terminal, PowerShell (7), Windows PowerShell, Command Prompt, Alacritty, WezTerm, Ghostty |

## Development

```sh
cargo build        # first build takes a few minutes (shader compilation)
cargo test         # unit + git integration tests
cargo clippy -- -D warnings
```

Note: text inputs accept raw key events; IME/marked-text input is not
supported in v1.

## License

Apache-2.0 — see [LICENSE](LICENSE).
