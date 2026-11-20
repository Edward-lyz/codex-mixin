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
cp "$DESKTOP_DIR/macos/CodexMixin.icns" "$RESOURCES_DIR/CodexMixin.icns"
cp "$DESKTOP_DIR/macos/CodexMixinDark.icns" "$RESOURCES_DIR/CodexMixinDark.icns"

# The Liquid Glass light/dark icon is an asset catalog compiled by Xcode 26's
# actool. Command Line Tools ship no actool; a local build can reuse the
# Assets.car of a CI-built release, or fall back to the static ICNS icons.
ACTOOL_VERSION=""
if xcrun --find actool >/dev/null 2>&1; then
  ACTOOL_VERSION="$(xcrun actool --version | awk '
    /<key>short-bundle-version<\/key>/ {
      getline
      sub(/^.*<string>/, "")
      sub(/<\/string>.*$/, "")
      print
      exit
    }
  ')"
fi
if [[ "${ACTOOL_VERSION%%.*}" =~ ^[0-9]+$ ]] && (( ${ACTOOL_VERSION%%.*} >= 26 )); then
  ICON_OUTPUT="$DESKTOP_DIR/target/adaptive-icon"
  rm -rf "$ICON_OUTPUT"
  mkdir -p "$ICON_OUTPUT"
  xcrun actool "$DESKTOP_DIR/macos/CodexMixin.icon" \
    --compile "$ICON_OUTPUT" \
    --output-format human-readable-text \
    --notices \
    --warnings \
    --output-partial-info-plist "$ICON_OUTPUT/assetcatalog_generated_info.plist" \
    --app-icon CodexMixin \
    --include-all-app-icons \
    --accent-color AccentColor \
    --enable-on-demand-resources NO \
    --development-region en \
    --target-device mac \
    --minimum-deployment-target 26.0 \
    --platform macosx
  cp "$ICON_OUTPUT/Assets.car" "$RESOURCES_DIR/Assets.car"
elif [[ -n "${CODEX_MIXIN_PREBUILT_ASSETS_CAR:-}" ]]; then
  cp "$CODEX_MIXIN_PREBUILT_ASSETS_CAR" "$RESOURCES_DIR/Assets.car"
elif [[ "${CODEX_MIXIN_REQUIRE_ADAPTIVE_ICON:-0}" == "1" ]]; then
  echo "the adaptive app icon needs Xcode 26 actool; found ${ACTOOL_VERSION:-none}" >&2
  exit 1
else
  /usr/libexec/PlistBuddy -c "Delete :CFBundleIconName" "$CONTENTS_DIR/Info.plist"
  echo "warning: Xcode 26 actool is unavailable; using the static ICNS icons" >&2
fi
mkdir -p "$RESOURCES_DIR/Attribution"
for group in agents providers nasa-wallpapers; do
  cp "$DESKTOP_DIR/assets/$group/ATTRIBUTION.md" "$RESOURCES_DIR/Attribution/$group.md"
done

cp "$DESKTOP_TARGET_DIR/codex-mixin-desktop" "$MACOS_DIR/codex-mixin-desktop"
cp "$CLI_TARGET_DIR/codex-mixin" "$MACOS_DIR/codex-mixin"
chmod +x "$MACOS_DIR/codex-mixin-desktop" "$MACOS_DIR/codex-mixin"
# Installed menu LaunchAgents from the SwiftUI app still use this name.
ln -s codex-mixin-desktop "$MACOS_DIR/CodexMixinMenu"

APP_IDENTIFIER="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$CONTENTS_DIR/Info.plist")"
CLI_IDENTIFIER="$APP_IDENTIFIER.cli"
if [[ "$CODE_SIGN_IDENTITY" == "-" ]]; then
  # An ad-hoc signature otherwise pins its build hash, so every update would
  # look like a new app to macOS privacy permissions. Pin the identifier.
  SIGN_ARGS=(--timestamp=none)
  CLI_SIGN_ARGS=(--requirements "=designated => identifier \"$CLI_IDENTIFIER\"")
  APP_SIGN_ARGS=(--requirements "=designated => identifier \"$APP_IDENTIFIER\"")
else
  # A Developer ID signature carries its own identity; notarization also
  # needs the hardened runtime and a secure timestamp.
  SIGN_ARGS=(--options runtime --timestamp)
  CLI_SIGN_ARGS=()
  APP_SIGN_ARGS=()
fi
codesign --force --sign "$CODE_SIGN_IDENTITY" "${SIGN_ARGS[@]}" --identifier "$CLI_IDENTIFIER" \
  ${CLI_SIGN_ARGS[@]+"${CLI_SIGN_ARGS[@]}"} "$MACOS_DIR/codex-mixin"
codesign --force --sign "$CODE_SIGN_IDENTITY" "${SIGN_ARGS[@]}" \
  ${APP_SIGN_ARGS[@]+"${APP_SIGN_ARGS[@]}"} "$APP_DIR"
codesign --verify --deep --strict --verbose=2 "$APP_DIR"
for signed in "$APP_DIR" "$MACOS_DIR/codex-mixin"; do
  # Each update must satisfy the identity the previous version was granted.
  requirement="$(codesign -d -r- "$signed" 2>&1)"
  if [[ "$CODE_SIGN_IDENTITY" == "-" ]] && grep -q cdhash <<<"$requirement"; then
    echo "ad-hoc designated requirement still pins the build hash: $signed" >&2
    exit 1
  fi
done

# Notarize when App Store Connect credentials are configured.
if [[ "$CODE_SIGN_IDENTITY" != "-" && -n "${NOTARY_KEY_PATH:-}" ]]; then
  NOTARY_ZIP="$DESKTOP_DIR/target/notarize.zip"
  ditto -c -k --keepParent "$APP_DIR" "$NOTARY_ZIP"
  xcrun notarytool submit "$NOTARY_ZIP" --wait \
    --key "$NOTARY_KEY_PATH" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER_ID"
  xcrun stapler staple "$APP_DIR"
  rm -f "$NOTARY_ZIP"
fi
echo "Built $APP_DIR"
