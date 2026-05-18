#!/usr/bin/env bash
# Generate all icon assets from the committed master PNGs.
#
# Requirements: sips and iconutil (both ship with macOS Xcode tools).
#
# Sources (committed, do NOT gitignore):
#   assets/icon-1024.png       — app icon master raster (1024×1024)
#   assets/menubar-src.png     — menu bar glyph (128×128 monochrome)
#   assets/menubar-active-src.png — active-state variant
#
# To regenerate the master PNGs from the SVG source:
#   rsvg-convert -w 1024 -h 1024 assets/icon.svg -o assets/icon-1024.png
# (rsvg-convert not installed by default; use Inkscape or any SVG renderer)
#
# Outputs (generated, gitignored):
#   assets/AppIcon.icns
#   assets/menubar-template.png / @2x
#   assets/menubar-active.png   / @2x
set -euo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
ASSETS="$REPO/assets"
SRC="$ASSETS/icon-1024.png"
ICONSET="$ASSETS/AppIcon.iconset"

if [[ ! -f "$SRC" ]]; then
    echo "error: $SRC not found. Run from repo root after generating the master PNG." >&2
    exit 1
fi

# --- App icon (AppIcon.icns) ---
rm -rf "$ICONSET"
mkdir -p "$ICONSET"
for s in 16 32 64 128 256 512; do
    sips -z "$s" "$s" "$SRC" --out "$ICONSET/icon_${s}x${s}.png"        >/dev/null
    sips -z $((s*2)) $((s*2)) "$SRC" --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
sips -z 1024 1024 "$SRC" --out "$ICONSET/icon_512x512@2x.png" >/dev/null
iconutil -c icns "$ICONSET" -o "$ASSETS/AppIcon.icns"
rm -rf "$ICONSET"
echo "✓ AppIcon.icns"

# --- Menu bar icons ---
MBSRC="$ASSETS/menubar-src.png"
MBACT="$ASSETS/menubar-active-src.png"
sips -z 18 18 "$MBSRC" --out "$ASSETS/menubar-template.png"    >/dev/null
sips -z 36 36 "$MBSRC" --out "$ASSETS/menubar-template@2x.png" >/dev/null
sips -z 18 18 "$MBACT" --out "$ASSETS/menubar-active.png"      >/dev/null
sips -z 36 36 "$MBACT" --out "$ASSETS/menubar-active@2x.png"   >/dev/null
echo "✓ menubar-template.png + @2x"
echo "✓ menubar-active.png   + @2x"

echo "Done. Assets in $ASSETS/"
