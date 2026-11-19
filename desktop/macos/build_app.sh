#!/usr/bin/env bash
# Builds dist/Codex Mixin.app: the GPUI desktop shell with the Rust CLI
# beside it in Contents/MacOS, where the shell looks for it.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DESKTOP_DIR="$ROOT_DIR/desktop"
APP_DIR="$ROOT_DIR/dist/Codex Mixin.app"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
RESOURCES_DIR="$CONTENTS_DIR/Resources"
CODE_SIGN_IDENTITY="${CODE_SIGN_IDENTITY:--}"
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-13.1}"

CLI_ARGS=(--release --locked)
DESKTOP_ARGS=(--release)
CLI_TARGET_DIR="$ROOT_DIR/target/release"
DESKTOP_TARGET_DIR="$DESKTOP_DIR/target/release"
if [[ -n "${CARGO_BUILD_TARGET:-}" ]]; then
  CLI_ARGS+=(--target "$CARGO_BUILD_TARGET")
  DESKTOP_ARGS+=(--target "$CARGO_BUILD_TARGET")
  CLI_TARGET_DIR="$ROOT_DIR/target/$CARGO_BUILD_TARGET/release"
  DESKTOP_TARGET_DIR="$DESKTOP_DIR/target/$CARGO_BUILD_TARGET/release"
fi
if [[ -n "${CARGO_BUILD_FEATURES:-}" ]]; then
  CLI_ARGS+=(--features "$CARGO_BUILD_FEATURES")
fi

cd "$ROOT_DIR"
cargo build "${CLI_ARGS[@]}" --bin codex-mixin
cargo build "${DESKTOP_ARGS[@]}" --manifest-path "$DESKTOP_DIR/Cargo.toml"
"$ROOT_DIR/scripts/check_localizations.sh"

PACKAGE_ID="$(cargo pkgid --package codex-mixin)"
APP_VERSION="${PACKAGE_ID##*#}"
APP_VERSION="${APP_VERSION##*@}"

rm -rf "$APP_DIR"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR"
cp "$DESKTOP_DIR/macos/Info.plist" "$CONTENTS_DIR/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $APP_VERSION" "$CONTENTS_DIR/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $APP_VERSION" "$CONTENTS_DIR/Info.plist"
/usr/libexec/PlistBuddy -c "Set :LSMinimumSystemVersion $MACOSX_DEPLOYMENT_TARGET" "$CONTENTS_DIR/Info.plist"
# Static icons only; the adaptive Assets.car needs Xcode's actool.
/usr/libexec/PlistBuddy -c "Delete :CFBundleIconName" "$CONTENTS_DIR/Info.plist"
cp "$DESKTOP_DIR/macos/CodexMixin.icns" "$RESOURCES_DIR/CodexMixin.icns"
cp "$DESKTOP_DIR/macos/CodexMixinDark.icns" "$RESOURCES_DIR/CodexMixinDark.icns"
mkdir -p "$RESOURCES_DIR/Attribution"
for group in agents providers nasa-wallpapers; do
  cp "$DESKTOP_DIR/assets/$group/ATTRIBUTION.md" "$RESOURCES_DIR/Attribution/$group.md"
done

cp "$DESKTOP_TARGET_DIR/codex-mixin-desktop" "$MACOS_DIR/codex-mixin-desktop"
cp "$CLI_TARGET_DIR/codex-mixin" "$MACOS_DIR/codex-mixin"
chmod +x "$MACOS_DIR/codex-mixin-desktop" "$MACOS_DIR/codex-mixin"

codesign --force --sign "$CODE_SIGN_IDENTITY" --timestamp=none "$MACOS_DIR/codex-mixin"
codesign --force --sign "$CODE_SIGN_IDENTITY" --timestamp=none "$APP_DIR"
codesign --verify --deep --strict "$APP_DIR"
echo "Built $APP_DIR"
