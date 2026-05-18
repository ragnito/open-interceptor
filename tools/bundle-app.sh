#!/usr/bin/env bash
# Build and bundle Open Interceptor.app for macOS.
#
# Usage:
#   tools/bundle-app.sh [--target <triple>]
#
# Defaults to the host architecture. Common values:
#   aarch64-apple-darwin   (Apple Silicon)
#   x86_64-apple-darwin    (Intel)
#
# After running, drag "Open Interceptor.app" to /Applications.
# If macOS quarantines the app after download:
#   xattr -dr com.apple.quarantine "/Applications/Open Interceptor.app"
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="${1:-}"
TARGET_FLAG=""
TARGET_DIR="$REPO/target"

if [[ "$TARGET" == "--target" ]]; then
    TARGET="$2"
    TARGET_FLAG="--target $TARGET"
    TARGET_DIR="$REPO/target/$TARGET"
fi

RELEASE_DIR="$TARGET_DIR/release"
ASSETS="$REPO/assets"
APP_NAME="Open Interceptor"
APP_BUNDLE="$REPO/$APP_NAME.app"
CONTENTS="$APP_BUNDLE/Contents"

echo "==> Generating icons…"
bash "$REPO/tools/make-icons.sh"

echo "==> Building open-interceptor (headless)…"
cargo build --release $TARGET_FLAG --bin open-interceptor

echo "==> Building open-interceptor-menubar…"
cargo build --release $TARGET_FLAG --features menubar --bin open-interceptor-menubar

echo "==> Assembling $APP_NAME.app…"
rm -rf "$APP_BUNDLE"
mkdir -p "$CONTENTS/MacOS" "$CONTENTS/Resources"

cp "$RELEASE_DIR/open-interceptor"          "$CONTENTS/MacOS/open-interceptor"
cp "$RELEASE_DIR/open-interceptor-menubar"  "$CONTENTS/MacOS/open-interceptor-menubar"
cp "$ASSETS/AppIcon.icns"                   "$CONTENTS/Resources/AppIcon.icns"
cp "$ASSETS/menubar-template.png"           "$CONTENTS/Resources/menubar-template.png"
cp "$ASSETS/menubar-template@2x.png"        "$CONTENTS/Resources/menubar-template@2x.png"
cp "$ASSETS/menubar-active.png"             "$CONTENTS/Resources/menubar-active.png"
cp "$ASSETS/menubar-active@2x.png"          "$CONTENTS/Resources/menubar-active@2x.png"

# Read version from Cargo.toml
VERSION=$(grep '^version' "$REPO/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')

cat > "$CONTENTS/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>Open Interceptor</string>
    <key>CFBundleDisplayName</key>
    <string>Open Interceptor</string>
    <key>CFBundleIdentifier</key>
    <string>com.open-interceptor.menubar</string>
    <key>CFBundleExecutable</key>
    <string>open-interceptor-menubar</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>LSUIElement</key>
    <true/>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSHumanReadableCopyright</key>
    <string>MIT License</string>
</dict>
</plist>
PLIST

echo "==> Ad-hoc signing (for local use)…"
codesign --force --deep --sign - "$APP_BUNDLE"

echo ""
echo "Done: $APP_BUNDLE"
echo ""
echo "Install: drag '$APP_NAME.app' to /Applications"
echo ""
echo "If macOS blocks the app after download:"
echo "  xattr -dr com.apple.quarantine \"/Applications/$APP_NAME.app\""
echo ""
echo "Note: for public distribution you'll need Developer ID + notarization."
