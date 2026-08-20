#!/bin/bash
# Builds a universal (arm64 + x86_64) macOS .app bundle and zips it.
# Used by CI (release workflow) and works locally.
#
#   scripts/package-macos.sh [version]
#
# Output: dist/Worktree Tool.app and dist/worktree-tool-<version>-macos-universal.zip

set -euo pipefail

VERSION="${1:-$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)}"
[ -n "$VERSION" ] || VERSION="0.0.0"
APP_NAME="Worktree Tool"
BUNDLE_ID="com.gregnazario.worktree-tool"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIST="$ROOT/dist"

cd "$ROOT"

echo "==> Building universal binary (aarch64 + x86_64 apple-darwin)"
rustup target add aarch64-apple-darwin x86_64-apple-darwin
cargo build --release --target aarch64-apple-darwin --target x86_64-apple-darwin

rm -rf "$DIST"
mkdir -p "$DIST/$APP_NAME.app/Contents/MacOS" "$DIST/$APP_NAME.app/Contents/Resources"

echo "==> Lipo into universal binary"
lipo -create \
    "target/aarch64-apple-darwin/release/worktree-tool" \
    "target/x86_64-apple-darwin/release/worktree-tool" \
    -output "$DIST/$APP_NAME.app/Contents/MacOS/worktree-tool"
chmod +x "$DIST/$APP_NAME.app/Contents/MacOS/worktree-tool"

echo "==> Icon"
cp assets/AppIcon.icns "$DIST/$APP_NAME.app/Contents/Resources/AppIcon.icns"

echo "==> Info.plist"
cat > "$DIST/$APP_NAME.app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Worktree Tool</string>
    <key>CFBundleDisplayName</key><string>Worktree Tool</string>
    <key>CFBundleIdentifier</key><string>${BUNDLE_ID}</string>
    <key>CFBundleExecutable</key><string>worktree-tool</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundleVersion</key><string>${VERSION}</string>
    <key>CFBundleShortVersionString</key><string>${VERSION}</string>
    <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
    <key>LSMinimumSystemVersion</key><string>13.0</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
PLIST

echo "==> Code signature (adhoc, satisfies Gatekeeper's structural check)"
codesign --force --deep --sign - "$DIST/$APP_NAME.app" >/dev/null 2>&1 || \
    echo "    (codesign unavailable; skipping)"

echo "==> Zipping"
ZIP="$DIST/worktree-tool-${VERSION}-macos-universal.zip"
ditto -c -k --keepParent "$DIST/$APP_NAME.app" "$ZIP"

echo "==> Verifying"
lipo -info "$DIST/$APP_NAME.app/Contents/MacOS/worktree-tool"
plutil -lint "$DIST/$APP_NAME.app/Contents/Info.plist"
ls -la "$DIST"

echo "Done: $ZIP"
