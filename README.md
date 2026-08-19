# worktree-tool

A native GUI for managing [git worktrees](https://git-scm.com/docs/git-worktree),
built in Rust with [GPUI](https://gpui.rs) (Zed's GPU-accelerated UI framework).

Lists every worktree of a repository with its branch, status (dirty files,
ahead/behind vs upstream), and offers create / remove / prune operations plus
quick actions (open in terminal, reveal, copy path). Keyboard-first.

## Requirements

- macOS (v1 is developed and tested on macOS; Linux/Windows backends exist in
  GPUI but are untested here)
- Stable Rust toolchain
- `git` on your `PATH`
- Xcode command line tools (`xcode-select --install`) for the Metal backend

## Usage

Run from inside any git repository — the app detects the repo from the
current working directory:

```sh
cd ~/git/myrepo
cargo run --release
```

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
# terminal: one of terminal, iterm, wezterm, ghostty, alacritty, kitty, warp,
# hyper (or unset for auto-detect)
terminal = "iterm"
```

Terminal resolution order: `settings.toml` → `$TERMCMD` env var (app name,
[Zed convention](https://zed.dev)) → first detected terminal. Terminal.app
ships with macOS, so auto-detect always resolves there by default. For
terminals with a CLI on `PATH` (wezterm, ghostty, alacritty, kitty), the CLI
form is preferred so the new window opens in the worktree's directory.

## Development

```sh
cargo build        # first build takes a few minutes (shader compilation)
cargo test         # unit + git integration tests
cargo clippy -- -D warnings
```

Note: text inputs accept raw key events; IME/marked-text input is not
supported in v1.
