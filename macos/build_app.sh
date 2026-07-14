#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="Codex Mixin.app"
APP_DIR="$ROOT_DIR/dist/$APP_NAME"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
RESOURCES_DIR="$CONTENTS_DIR/Resources"
CARGO_BUILD_ARGS=(--release)
TARGET_DIR="$ROOT_DIR/target/release"

if [[ -n "${CARGO_BUILD_TARGET:-}" ]]; then
  CARGO_BUILD_ARGS+=(--target "$CARGO_BUILD_TARGET")
  TARGET_DIR="$ROOT_DIR/target/$CARGO_BUILD_TARGET/release"
fi

cd "$ROOT_DIR"
cargo build "${CARGO_BUILD_ARGS[@]}"
"$ROOT_DIR/macos/make_icon.sh"
PACKAGE_ID="$(cargo pkgid --package codex-mixin)"
APP_VERSION="${PACKAGE_ID##*#}"
APP_VERSION="${APP_VERSION##*@}"

rm -rf "$APP_DIR"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR"
cp "$ROOT_DIR/macos/Info.plist" "$CONTENTS_DIR/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $APP_VERSION" "$CONTENTS_DIR/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $APP_VERSION" "$CONTENTS_DIR/Info.plist"
cp "$ROOT_DIR/macos/CodexMixin.icns" "$RESOURCES_DIR/CodexMixin.icns"
cp "$TARGET_DIR/codex-mixin" "$RESOURCES_DIR/codex-mixin"
chmod +x "$RESOURCES_DIR/codex-mixin"

swiftc "$ROOT_DIR/macos/MenuBarApp.swift" \
  "$ROOT_DIR/macos/ModelBenchmarkWindow.swift" \
  -framework Cocoa \
  -o "$MACOS_DIR/CodexMixinMenu"
chmod +x "$MACOS_DIR/CodexMixinMenu"

cp "$TARGET_DIR/codex-mixin" "$ROOT_DIR/dist/codex-mixin"
chmod +x "$ROOT_DIR/dist/codex-mixin"

BUILT_VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$CONTENTS_DIR/Info.plist")"
if [[ "$BUILT_VERSION" != "$APP_VERSION" ]]; then
  echo "app version mismatch: expected $APP_VERSION, got $BUILT_VERSION" >&2
  exit 1
fi

echo "$APP_DIR"
