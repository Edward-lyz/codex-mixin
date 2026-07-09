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
mkdir -p "$STAGING_DIR/bin" "$OUT_DIR"
cp -R "$APP_PATH" "$STAGING_DIR/Codex Mixin.app"
cp "$BIN_PATH" "$STAGING_DIR/bin/codex-mixin"
cp "$ROOT_DIR/README.md" "$STAGING_DIR/README.md"
ln -s /Applications "$STAGING_DIR/Applications"

hdiutil create \
  -volname "Codex Mixin" \
  -srcfolder "$STAGING_DIR" \
  -ov \
  -format UDZO \
  "$DMG_PATH"
