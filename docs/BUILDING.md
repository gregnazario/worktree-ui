# Building worktree-tool

The app is a Rust binary on top of [GPUI](https://gpui.rs), which renders
via Metal (macOS), Vulkan (Linux/FreeBSD), or Direct3D (Windows). All four
platforms are built and tested in CI on every PR
(`.github/workflows/ci.yml`).

The short version for any of them:

```sh
cargo build --release    # binary at target/release/worktree-tool
```

The helper `scripts/build-target.sh <target>` wraps all of the strategies
below, so you rarely need this document's details — but here is what it
does and why, per target.

## Native builds

### macOS

Requirements: stable Rust, Xcode command line tools
(`xcode-select --install`).

```sh
cargo build --release
```

For the distributable universal (arm64 + x86_64) `.app` and DMG:

```sh
scripts/package-macos.sh            # dist/Worktree Tool.app + .dmg + .zip
```

### Linux (X11/Wayland)

Requirements beyond Rust (same set CI installs on Ubuntu 24.04):

```sh
sudo apt-get install -y build-essential g++ \
    libxcb1-dev libxkbcommon-dev libxkbcommon-x11-dev
```

Runtime also needs a Vulkan-capable driver stack (any modern desktop has
one). Then:

```sh
cargo build --release
```

### Windows

Requirements: stable Rust with the MSVC toolchain (Visual Studio Build
Tools, which the Rust `x86_64-pc-windows-msvc` target expects) — or use the
cross-build below, which needs nothing Windows-local.

```sh
cargo build --release    # target\release\worktree-tool.exe
```

### FreeBSD

Requirements: `rust` (or rustup) and the X11 libraries, which ports install
under `/usr/local/lib` — outside the base linker/runtime search path:

```sh
pkg install -y git rust libxcb libxkbcommon gcc
export LIBRARY_PATH=/usr/local/lib      # link-time search
export LD_LIBRARY_PATH=/usr/local/lib   # run-time search (tests spawn the binary)
cargo test && cargo build --release
```

## Cross-building from any host

`scripts/build-target.sh` picks a strategy per target. What actually works,
measured on a macOS host (the same conclusions hold elsewhere):

| Target | Strategy | Result |
| --- | --- | --- |
| `x86_64-unknown-linux-gnu` (+aarch64) | Docker container (`scripts/Dockerfile.linux-build`), arch matched to the target so the build is native inside the container | **full binary** — container carries the X11/xkbcommon dev libs; artifacts land in `target/` via a volume mount (x86_64 on Apple Silicon runs emulated, so it is slower there) |
| `x86_64-pc-windows-gnu` | `cargo zigbuild` (smoke check) | all crates compile and the debug profile links — but GPUI embeds its DirectX **shaders via `fxc.exe`, which only runs when building on Windows**, so a cross-host binary is not runnable. Build natively on Windows or use CI for the real thing |
| `x86_64-unknown-freebsd` | `cargo zigbuild` | every crate compiles (full type-check); the **final link** needs FreeBSD's `xcb`/`xkbcommon` libraries — build natively on FreeBSD or let CI do it |
| `*-apple-darwin` | plain `cargo` | full binary (needs Apple SDK, i.e. build on macOS) |

Set up the zig toolchain once if you want the zigbuild paths:

```sh
brew install zig            # or: download from ziglang.org
cargo install cargo-zigbuild
```

Examples:

```sh
scripts/build-target.sh x86_64-unknown-linux-gnu  # -> binary via Docker
scripts/build-target.sh x86_64-pc-windows-gnu     # -> compile smoke check off-Windows
scripts/build-target.sh x86_64-unknown-freebsd    # -> compile smoke check off-FreeBSD
scripts/build-target.sh host                      # native build
scripts/build-target.sh macos-universal           # .app + DMG
scripts/build-target.sh riscv64gc-unknown-linux-gnu   # passthrough for any triplet
```

Why not zigbuild for the Linux/FreeBSD links? zig supplies the C library
(glibc) for cross targets, but not the third-party X11 libraries the Linux
backend links against; wiring a foreign sysroot is fiddlier and less
reproducible than a 30-second Docker build, which is why Docker is the
default there. For FreeBSD, CI's VM build is the practical path.

## CI

Every PR to `main` runs: fmt + clippy + all tests + a release build on
macOS, Linux, Windows, and FreeBSD, the universal macOS package build, and
a docs sanity check — all six are required before merge (branch
protection). See `.github/workflows/` for the definitions.

Tagging `v*` publishes a GitHub Release with a package for **every
platform**: the macOS universal DMG + zip, `linux-x86_64` and
`linux-aarch64` tarballs (built on native runners, including the arm64
one), a `windows-x86_64` zip built on a Windows host (so gpui's DirectX
shaders are compiled by fxc.exe), and a `freebsd-x86_64` tarball from the
FreeBSD VM job.
