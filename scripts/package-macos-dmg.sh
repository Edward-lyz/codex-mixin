#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 3 ]]; then
  echo "usage: package-macos-dmg.sh <target-triple> <version> <out-dir>" >&2
  exit 2
fi

TARGET="$1"
VERSION="$2"
OUT_DIR="$3"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_PATH="$ROOT_DIR/dist/Codex Mixin.app"
BIN_PATH="$ROOT_DIR/target/$TARGET/release/codex-mixin"
STAGING_DIR="$ROOT_DIR/target/package/codex-mixin-dmg-$TARGET"
TEMP_DMG="$ROOT_DIR/target/package/codex-mixin-$VERSION-$TARGET.rw.dmg"
MOUNT_DIR="$ROOT_DIR/target/package/codex-mixin-dmg-mount-$TARGET"
DMG_PATH="$OUT_DIR/codex-mixin-$VERSION-$TARGET.dmg"

if [[ ! -d "$APP_PATH" ]]; then
  echo "missing app bundle: $APP_PATH" >&2
  exit 1
fi
if [[ ! -x "$BIN_PATH" ]]; then
  echo "missing binary: $BIN_PATH" >&2
  exit 1
fi

rm -rf "$STAGING_DIR"
mkdir -p "$STAGING_DIR/bin" "$STAGING_DIR/.background" "$OUT_DIR"
cp -R "$APP_PATH" "$STAGING_DIR/Codex Mixin.app"
cp "$BIN_PATH" "$STAGING_DIR/bin/codex-mixin"
cp "$ROOT_DIR/README.md" "$STAGING_DIR/README.md"
cp "$ROOT_DIR/LICENSE" "$STAGING_DIR/LICENSE"
cp "$ROOT_DIR/NOTICE" "$STAGING_DIR/NOTICE"
ln -s /Applications "$STAGING_DIR/Applications"

swift "$ROOT_DIR/macos/make_dmg_background.swift" "$STAGING_DIR/.background/background.png"

rm -f "$TEMP_DMG" "$DMG_PATH"
rm -rf "$MOUNT_DIR"
hdiutil create \
  -volname "Codex Mixin" \
  -srcfolder "$STAGING_DIR" \
  -ov \
  -format UDRW \
  "$TEMP_DMG"

mkdir -p "$MOUNT_DIR"
cleanup() {
  status=$?
  hdiutil detach "$MOUNT_DIR" -force >/dev/null 2>&1 || true
  rm -rf "$MOUNT_DIR"
  exit "$status"
}
trap cleanup EXIT

hdiutil attach "$TEMP_DMG" -readwrite -noverify -nobrowse -mountpoint "$MOUNT_DIR"

osascript <<APPLESCRIPT
set dmgFolder to POSIX file "$MOUNT_DIR" as alias
tell application "Finder"
  open dmgFolder
  set itemsReady to false
  repeat with attempt from 1 to 20
    try
      set appItem to item "Codex Mixin.app" of dmgFolder
      set applicationsItem to item "Applications" of dmgFolder
      set binItem to item "bin" of dmgFolder
      set readmeItem to item "README.md" of dmgFolder
      set licenseItem to item "LICENSE" of dmgFolder
      set noticeItem to item "NOTICE" of dmgFolder
      set itemsReady to true
      exit repeat
    on error
      update dmgFolder
      delay 0.5
    end try
  end repeat
  if itemsReady is false then error "DMG staging items did not become visible in Finder"
  set current view of container window of dmgFolder to icon view
  set toolbar visible of container window of dmgFolder to false
  set statusbar visible of container window of dmgFolder to false
  set bounds of container window of dmgFolder to {120, 120, 780, 640}
  set viewOptions to icon view options of container window of dmgFolder
  set arrangement of viewOptions to not arranged
  set icon size of viewOptions to 88
  set background picture of viewOptions to POSIX file "$MOUNT_DIR/.background/background.png"
  set position of appItem to {170, 205}
  set position of applicationsItem to {490, 205}
  set position of binItem to {170, 335}
  set position of readmeItem to {490, 335}
  set position of licenseItem to {170, 430}
  set position of noticeItem to {490, 430}
  close container window of dmgFolder
  update dmgFolder without registering applications
end tell
APPLESCRIPT

sync
for attempt in 1 2 3 4 5; do
  if hdiutil detach "$MOUNT_DIR"; then
    break
  fi
  if [[ "$attempt" -eq 5 ]]; then
    hdiutil detach "$MOUNT_DIR" -force
    break
  fi
  sleep "$attempt"
done
trap - EXIT
rm -rf "$MOUNT_DIR"

hdiutil convert "$TEMP_DMG" \
  -format UDZO \
  -o "$DMG_PATH"
rm -f "$TEMP_DMG"
echo "$DMG_PATH"
