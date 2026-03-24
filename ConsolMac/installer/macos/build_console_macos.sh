#!/bin/zsh
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../../.." && pwd)"
CRATE_DIR="$ROOT_DIR/ConsolMac/app"
DIST_DIR="$ROOT_DIR/ConsolMac/dist"
TMP_DIR="$ROOT_DIR/ConsolMac/.tmp"
TARGET_DIR="${CARGO_TARGET_DIR:-$CRATE_DIR/target}"
APP_NAME="BK-Console macOS"
APP_DIR="$DIST_DIR/$APP_NAME.app"
DMG_PATH="$DIST_DIR/$APP_NAME.dmg"
ICONSET_DIR="$TMP_DIR/app.iconset"
PLIST_SOURCE="$ROOT_DIR/ConsolMac/installer/macos/Info.plist"
BIN_SOURCE="$TARGET_DIR/release/bk-wiver-console-macos"
FFMPEG_SOURCE="$(command -v ffmpeg || true)"

rm -rf "$TMP_DIR" "$APP_DIR" "$DMG_PATH"
mkdir -p "$TMP_DIR" "$DIST_DIR"

cargo build --release --manifest-path "$CRATE_DIR/Cargo.toml"

mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"

cp "$PLIST_SOURCE" "$APP_DIR/Contents/Info.plist"
cp "$BIN_SOURCE" "$APP_DIR/Contents/MacOS/$APP_NAME"
chmod +x "$APP_DIR/Contents/MacOS/$APP_NAME"

if [[ -n "$FFMPEG_SOURCE" ]]; then
  cp -L "$FFMPEG_SOURCE" "$APP_DIR/Contents/Resources/ffmpeg"
  chmod +x "$APP_DIR/Contents/Resources/ffmpeg"
else
  echo "Warning: ffmpeg not found in PATH, macOS app will rely on runtime PATH lookup"
fi

mkdir -p "$ICONSET_DIR"
LOGO_SOURCE="$ROOT_DIR/branding/logo.png"
ICNS_PATH="$TMP_DIR/app.icns"

sips -z 16 16     "$LOGO_SOURCE" --out "$ICONSET_DIR/icon_16x16.png" >/dev/null
sips -z 32 32     "$LOGO_SOURCE" --out "$ICONSET_DIR/icon_16x16@2x.png" >/dev/null
sips -z 32 32     "$LOGO_SOURCE" --out "$ICONSET_DIR/icon_32x32.png" >/dev/null
sips -z 64 64     "$LOGO_SOURCE" --out "$ICONSET_DIR/icon_32x32@2x.png" >/dev/null
sips -z 128 128   "$LOGO_SOURCE" --out "$ICONSET_DIR/icon_128x128.png" >/dev/null
sips -z 256 256   "$LOGO_SOURCE" --out "$ICONSET_DIR/icon_128x128@2x.png" >/dev/null
sips -z 256 256   "$LOGO_SOURCE" --out "$ICONSET_DIR/icon_256x256.png" >/dev/null
sips -z 512 512   "$LOGO_SOURCE" --out "$ICONSET_DIR/icon_256x256@2x.png" >/dev/null
sips -z 512 512   "$LOGO_SOURCE" --out "$ICONSET_DIR/icon_512x512.png" >/dev/null
sips -z 1024 1024 "$LOGO_SOURCE" --out "$ICONSET_DIR/icon_512x512@2x.png" >/dev/null

if iconutil -c icns "$ICONSET_DIR" -o "$ICNS_PATH"; then
  cp "$ICNS_PATH" "$APP_DIR/Contents/Resources/app.icns"
else
  echo "Warning: iconutil failed to build app.icns, continuing without bundled app icon"
fi

hdiutil create \
  -volname "$APP_NAME" \
  -srcfolder "$APP_DIR" \
  -ov \
  -format UDZO \
  "$DMG_PATH" >/dev/null

echo "Created:"
echo "  $APP_DIR"
echo "  $DMG_PATH"
