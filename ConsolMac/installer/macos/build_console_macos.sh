#!/bin/zsh
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../../.." && pwd)"
CRATE_DIR="$ROOT_DIR/ConsolMac/app"
DIST_DIR="$ROOT_DIR/ConsolMac/dist"
TMP_DIR="$ROOT_DIR/ConsolMac/.tmp"
APP_NAME="BK-Console macOS"
APP_DIR="$DIST_DIR/$APP_NAME.app"
DMG_PATH="$DIST_DIR/$APP_NAME.dmg"
ICONSET_DIR="$TMP_DIR/app.iconset"
PLIST_SOURCE="$ROOT_DIR/ConsolMac/installer/macos/Info.plist"
BIN_SOURCE="$ROOT_DIR/target/release/bk-wiver-console-macos"

rm -rf "$TMP_DIR" "$APP_DIR" "$DMG_PATH"
mkdir -p "$TMP_DIR" "$DIST_DIR"

cargo build --release -p bk-wiver-console-macos

mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"

cp "$PLIST_SOURCE" "$APP_DIR/Contents/Info.plist"
cp "$BIN_SOURCE" "$APP_DIR/Contents/MacOS/$APP_NAME"
chmod +x "$APP_DIR/Contents/MacOS/$APP_NAME"

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

iconutil -c icns "$ICONSET_DIR" -o "$ICNS_PATH"
cp "$ICNS_PATH" "$APP_DIR/Contents/Resources/app.icns"

hdiutil create \
  -volname "$APP_NAME" \
  -srcfolder "$APP_DIR" \
  -ov \
  -format UDZO \
  "$DMG_PATH" >/dev/null

echo "Created:"
echo "  $APP_DIR"
echo "  $DMG_PATH"
