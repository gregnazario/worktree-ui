#!/bin/bash
# Build worktree-tool for any Rust target, picking the best strategy per
# target so you don't have to remember which one needs what.
#
#   scripts/build-target.sh <target>
#
#   scripts/build-target.sh host                      # native build
#   scripts/build-target.sh macos-universal           # .app + DMG (package script)
#   scripts/build-target.sh x86_64-pc-windows-gnu     # cross .exe from any host (zigbuild)
#   scripts/build-target.sh x86_64-unknown-linux-gnu  # via Docker when available
#   scripts/build-target.sh x86_64-unknown-freebsd    # compile-validate via zigbuild
#   scripts/build-target.sh <any other triplet>       # passthrough to cargo
#
# Strategies (see docs/BUILDING.md for the full matrix):
#   *-apple-darwin        cargo + Xcode (native SDK)
#   *-windows-gnu         cargo-zigbuild — produces a full executable
#   *linux*               Docker container with the X11 deps if Docker is
#                         present; else zigbuild (compiles, link needs
#                         target sysroot libs — see BUILDING.md)
#   *freebsd*             zigbuild compiles all crates; the final link needs
#                         native libs — build on FreeBSD or via CI
#   anything else         plain cargo build --release --target

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

TARGET="${1:-host}"
BUILD_CMD="cargo build --release"

have() { command -v "$1" >/dev/null 2>&1; }

say() { printf '==> %s\n' "$*"; }
fail() { printf 'error: %s\n' "$*" >&2; exit 1; }

case "$TARGET" in
    host)
        say "Native release build"
        $BUILD_CMD
        say "Done: target/release/worktree-tool"
        ;;

    macos-universal)
        say "macOS universal .app + DMG"
        exec scripts/package-macos.sh
        ;;

    *-apple-darwin)
        say "Apple target $TARGET (native SDK)"
        rustup target add "$TARGET"
        $BUILD_CMD --target "$TARGET"
        say "Done: target/$TARGET/release/worktree-tool"
        ;;

    *-windows-*)
        if [[ "${OSTYPE:-}" == msys* || "${OSTYPE:-}" == cygwin* ]]; then
            say "Native Windows build ($TARGET)"
            $BUILD_CMD --target "$TARGET"
            say "Done: target/$TARGET/release/worktree-tool.exe"
        else
            # gpui embeds its DirectX shaders via fxc.exe, which only runs
            # when the build script itself executes on Windows — a cross
            # host cannot produce a runnable app. This path is a
            # compile/link smoke check only.
            have cargo-zigbuild || fail "cross-checking Windows needs cargo-zigbuild; for a runnable build use a Windows host or CI (see docs/BUILDING.md)"
            say "Windows cross smoke check via zigbuild ($TARGET) — NOT runnable: gpui shaders need a Windows host (fxc.exe)"
            cargo zigbuild --target "$TARGET" || \
                { echo "note: release-profile cross builds may fail on gpui's generated shader includes — expected off-Windows; use CI"; exit 1; }
        fi
        ;;

    *linux*)
        if have docker; then
            # Match the container arch to the target arch so the build is
            # native INSIDE the container (an arm64 container building an
            # x86_64 target would need a full cross toolchain).
            case "$TARGET" in
                x86_64-*)   PLATFORM="--platform=linux/amd64" ;;
                aarch64-*)  PLATFORM="--platform=linux/arm64" ;;
                *)          PLATFORM="" ;;
            esac
            say "Linux build in Docker ($TARGET) — native X11/xkbcommon libs included"
            docker build -q $PLATFORM -t worktree-tool/builder -f scripts/Dockerfile.linux-build scripts/ >/dev/null
            docker run --rm $PLATFORM -v "$ROOT":/work -w /work -e TARGET="$TARGET" worktree-tool/builder
            say "Done: target/$TARGET/release/worktree-tool (built in container, artifacts on host via volume)"
        else
            have cargo-zigbuild || fail "no Docker; cross-building Linux needs cargo-zigbuild, or build natively per docs/BUILDING.md"
            say "No Docker: zigbuild attempt ($TARGET) — expect a final-link error for missing xcb/xkbcommon unless a target sysroot is configured"
            rustup target add "$TARGET"
            cargo zigbuild --release --target "$TARGET"
        fi
        ;;

    *freebsd*)
        have cargo-zigbuild || fail "cross-building FreeBSD needs cargo-zigbuild; full linking requires a FreeBSD system (or CI) — see docs/BUILDING.md"
        say "FreeBSD via zigbuild ($TARGET) — all crates compile; final link needs native libs"
        rustup target add "$TARGET"
        cargo zigbuild --release --target "$TARGET"
        ;;

    *)
        say "Passthrough build for $TARGET"
        rustup target add "$TARGET"
        $BUILD_CMD --target "$TARGET"
        ;;
esac
