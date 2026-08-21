#!/bin/bash
# Builds a universal (arm64 + x86_64) macOS .app bundle, a drag-to-install
# DMG, and a zip. Used by CI (release workflow) and works locally.
#
#   scripts/package-macos.sh [version]
#
# Output:
#   dist/Worktree Tool.app
#   dist/worktree-tool-<version>-macos-universal.dmg
#   dist/worktree-tool-<version>-macos-universal.zip

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

echo "==> DMG staging"
STAGING="$DIST/dmg-staging"
mkdir -p "$STAGING/.background"
cp -R "$DIST/$APP_NAME.app" "$STAGING/"
ln -s /Applications "$STAGING/Applications"
cp assets/dmg-background.png "$STAGING/.background/background.png"

echo "==> Creating read-write DMG and styling"
RW_DMG="$DIST/rw.dmg"
hdiutil create -volname "$APP_NAME" \
    -srcfolder "$STAGING" \
    -fs HFS+ -fsargs "-c c=64,a=16,e=16" \
    -format UDRW -ov "$RW_DMG" >/dev/null
MOUNT="/Volumes/$APP_NAME"
hdiutil attach "$RW_DMG" -readwrite -noverify -noautoopen -mountpoint "$MOUNT" >/dev/null

# Position the app and the Applications symlink over the branded background.
# Best effort: Finder automation can be unavailable (e.g. headless CI); a
# plain-but-functional DMG is still produced.
osascript <<APPLESCRIPT || echo "    (Finder styling unavailable; DMG left unstyled)"
tell application "Finder"
    tell disk "$APP_NAME"
        open
        set current view of container window to icon view
        set toolbar visible of container window to false
        set statusbar visible of container window to false
        set the bounds of container window to {200, 120, 860, 520}
        set theViewOptions to the icon view options of container window
        set arrangement of theViewOptions to not arranged
        set icon size of theViewOptions to 128
        set background picture of theViewOptions to file ".background:background.png"
        set position of item "$APP_NAME" of it to {180, 200}
        set position of item "Applications" of it to {480, 200}
        update without registering applications
        delay 2
        close
    end tell
end tell
APPLESCRIPT

# Detach, tolerating Finder briefly holding the volume open.
DETACHED=0
for _ in 1 2 3 4 5; do
    if hdiutil detach "$MOUNT" -force >/dev/null 2>&1; then DETACHED=1; break; fi
    sleep 2
done
[ "$DETACHED" = "1" ] || { echo "failed to detach $MOUNT"; exit 1; }

echo "==> Converting to compressed DMG"
DMG="$DIST/worktree-tool-${VERSION}-macos-universal.dmg"
hdiutil convert "$RW_DMG" -format UDZO -imagekey zlib-level=9 -o "$DMG" >/dev/null
rm -f "$RW_DMG"
rm -rf "$STAGING"

echo "==> Verifying"
lipo -info "$DIST/$APP_NAME.app/Contents/MacOS/worktree-tool"
plutil -lint "$DIST/$APP_NAME.app/Contents/Info.plist"
hdiutil verify "$DMG" >/dev/null && echo "dmg: verified ($DMG)"

echo "Done: $DMG"
