#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="Codex Mixin.app"
APP_DIR="$ROOT_DIR/dist/$APP_NAME"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
RESOURCES_DIR="$CONTENTS_DIR/Resources"
FRAMEWORKS_DIR="$CONTENTS_DIR/Frameworks"
CARGO_BUILD_ARGS=(--release)
TARGET_DIR="$ROOT_DIR/target/release"
MACOS_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-13.1}"
export MACOSX_DEPLOYMENT_TARGET="$MACOS_DEPLOYMENT_TARGET"
CODE_SIGN_IDENTITY="${CODE_SIGN_IDENTITY:--}"

if [[ "${CODEX_MIXIN_REFRESH_NASA_WALLPAPERS:-0}" == "1" ]]; then
  # The wallpapers are decorative and a usable set is committed, so a refresh
  # failure must not block a release build.
  if ! "$ROOT_DIR/scripts/sync_nasa_wallpapers.py"; then
    echo "warning: NASA wallpaper refresh failed; using the committed set" >&2
  fi
fi

if [[ -n "${CARGO_BUILD_TARGET:-}" ]]; then
  CARGO_BUILD_ARGS+=(--target "$CARGO_BUILD_TARGET")
  TARGET_DIR="$ROOT_DIR/target/$CARGO_BUILD_TARGET/release"
fi

case "${CARGO_BUILD_TARGET:-}" in
  "") SWIFT_ARCH="$(uname -m)" ;;
  aarch64-apple-darwin) SWIFT_ARCH="arm64" ;;
  x86_64-apple-darwin) SWIFT_ARCH="x86_64" ;;
  *)
    echo "unsupported macOS build target: $CARGO_BUILD_TARGET" >&2
    exit 1
    ;;
esac

cd "$ROOT_DIR"
cargo build "${CARGO_BUILD_ARGS[@]}"
mkdir -p "$ROOT_DIR/macos/Generated"
"$ROOT_DIR/scripts/check_localizations.sh"
"$ROOT_DIR/scripts/swiftgen.sh" config run --config "$ROOT_DIR/swiftgen.yml"
SPARKLE_DIR="$("$ROOT_DIR/scripts/fetch_sparkle.sh")"
"$ROOT_DIR/macos/make_icon.sh"
PACKAGE_ID="$(cargo pkgid --package codex-mixin)"
APP_VERSION="${PACKAGE_ID##*#}"
APP_VERSION="${APP_VERSION##*@}"

rm -rf "$APP_DIR"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR" "$FRAMEWORKS_DIR"
cp "$ROOT_DIR/macos/Info.plist" "$CONTENTS_DIR/Info.plist"
for localization_dir in "$ROOT_DIR"/macos/*.lproj; do
  [[ -d "$localization_dir" ]] || continue
  cp -R "$localization_dir" "$RESOURCES_DIR/"
done
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $APP_VERSION" "$CONTENTS_DIR/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $APP_VERSION" "$CONTENTS_DIR/Info.plist"
/usr/libexec/PlistBuddy -c "Set :LSMinimumSystemVersion $MACOS_DEPLOYMENT_TARGET" "$CONTENTS_DIR/Info.plist"
cp "$ROOT_DIR/macos/CodexMixin.icns" "$RESOURCES_DIR/CodexMixin.icns"
mkdir -p "$RESOURCES_DIR/Wallpapers"
cp "$ROOT_DIR/macos/assets/nasa-wallpapers/"*.png "$RESOURCES_DIR/Wallpapers/"
cp "$ROOT_DIR/macos/assets/nasa-wallpapers/manifest.json" "$RESOURCES_DIR/Wallpapers/"
cp "$ROOT_DIR/macos/assets/nasa-wallpapers/ATTRIBUTION.md" "$RESOURCES_DIR/Wallpapers/"
mkdir -p "$RESOURCES_DIR/ProviderLogos"
cp "$ROOT_DIR/macos/assets/providers/"*.svg "$RESOURCES_DIR/ProviderLogos/"
cp "$ROOT_DIR/macos/assets/providers/ATTRIBUTION.md" "$RESOURCES_DIR/ProviderLogos/"
mkdir -p "$RESOURCES_DIR/ThirdPartyLicenses"
cp "$SPARKLE_DIR/LICENSE" "$RESOURCES_DIR/ThirdPartyLicenses/Sparkle.txt"
cp "$TARGET_DIR/codex-mixin" "$RESOURCES_DIR/codex-mixin"
chmod +x "$RESOURCES_DIR/codex-mixin"
cp -R "$SPARKLE_DIR/Sparkle.framework" "$FRAMEWORKS_DIR/Sparkle.framework"

xcrun swiftc \
  -target "$SWIFT_ARCH-apple-macosx$MACOS_DEPLOYMENT_TARGET" \
  -F "$SPARKLE_DIR" \
  -Xlinker -rpath \
  -Xlinker '@executable_path/../Frameworks' \
  "$ROOT_DIR/macos/ApplicationMenuSupport.swift" \
  "$ROOT_DIR/macos/MenuBarApp.swift" \
  "$ROOT_DIR/macos/StatusRefreshCoordinator.swift" \
  "$ROOT_DIR/macos/ProcessOutputCollector.swift" \
  "$ROOT_DIR/macos/GatewayService.swift" \
  "$ROOT_DIR/macos/GatewayStatusSupport.swift" \
  "$ROOT_DIR/macos/GatewayLifecycleSupport.swift" \
  "$ROOT_DIR/macos/LaunchAgentBootstrapSupport.swift" \
  "$ROOT_DIR/macos/GatewayProcessSupport.swift" \
  "$ROOT_DIR/macos/GatewayLaunchAgentSupport.swift" \
  "$ROOT_DIR/macos/SettingsActions.swift" \
  "$ROOT_DIR/macos/CodexActions.swift" \
  "$ROOT_DIR/macos/UpdateController.swift" \
  "$ROOT_DIR/macos/UpdateSupport.swift" \
  "$ROOT_DIR/macos/UpdateWatchdog.swift" \
  "$ROOT_DIR/macos/SettingsPanel.swift" \
  "$ROOT_DIR/macos/ProviderSupport.swift" \
  "$ROOT_DIR/macos/ProviderIconCache.swift" \
  "$ROOT_DIR/macos/ProviderWindowLayoutSupport.swift" \
  "$ROOT_DIR/macos/ProviderSettingsWindow.swift" \
  "$ROOT_DIR/macos/BaiduAuthBridgeSupport.swift" \
  "$ROOT_DIR/macos/ProviderSettingsPresentationSupport.swift" \
  "$ROOT_DIR/macos/MenuViewUpdateSupport.swift" \
  "$ROOT_DIR/macos/AppOperationLogging.swift" \
  "$ROOT_DIR/macos/ServiceMenuViews.swift" \
  "$ROOT_DIR/macos/ProviderUsageDashboardView.swift" \
  "$ROOT_DIR/macos/MenuVisualSupport.swift" \
  "$ROOT_DIR/macos/LiquidGlassSupport.swift" \
  "$ROOT_DIR/macos/ProviderSettingsView.swift" \
  "$ROOT_DIR/macos/AboutWindow.swift" \
  "$ROOT_DIR/macos/InstallCard.swift" \
  "$ROOT_DIR/macos/InstallCodexPanel.swift" \
  "$ROOT_DIR/macos/InstallClaudePanel.swift" \
  "$ROOT_DIR/macos/InstallProgressWindow.swift" \
  "$ROOT_DIR/macos/QuotaSupport.swift" \
  "$ROOT_DIR/macos/Localization.swift" \
  "$ROOT_DIR/macos/Generated/L10n.swift" \
  "$ROOT_DIR/macos/AppSupport.swift" \
  "$ROOT_DIR/macos/ModelBenchmarkDataSupport.swift" \
  "$ROOT_DIR/macos/ModelBenchmarkWindow.swift" \
  "$ROOT_DIR/macos/FusionSettingsLogic.swift" \
  "$ROOT_DIR/macos/FusionSettingsWindow.swift" \
  -framework Cocoa \
  -framework CryptoKit \
  -framework Sparkle \
  -framework SwiftUI \
  -o "$MACOS_DIR/CodexMixinMenu"
chmod +x "$MACOS_DIR/CodexMixinMenu"

cp "$TARGET_DIR/codex-mixin" "$ROOT_DIR/dist/codex-mixin"
chmod +x "$ROOT_DIR/dist/codex-mixin"

MENU_MIN_VERSION="$(xcrun vtool -show-build "$MACOS_DIR/CodexMixinMenu" | awk '$1 == "minos" { print $2; exit }')"
if [[ "$MENU_MIN_VERSION" != "$MACOS_DEPLOYMENT_TARGET" ]]; then
  echo "menu executable deployment target mismatch: expected $MACOS_DEPLOYMENT_TARGET, got ${MENU_MIN_VERSION:-missing}" >&2
  exit 1
fi

SPARKLE_FRAMEWORK="$FRAMEWORKS_DIR/Sparkle.framework/Versions/B"
codesign --force --sign "$CODE_SIGN_IDENTITY" "$SPARKLE_FRAMEWORK/XPCServices/Downloader.xpc"
codesign --force --sign "$CODE_SIGN_IDENTITY" "$SPARKLE_FRAMEWORK/XPCServices/Installer.xpc"
codesign --force --sign "$CODE_SIGN_IDENTITY" "$SPARKLE_FRAMEWORK/Updater.app"
codesign --force --sign "$CODE_SIGN_IDENTITY" "$SPARKLE_FRAMEWORK/Autoupdate"
codesign --force --sign "$CODE_SIGN_IDENTITY" "$FRAMEWORKS_DIR/Sparkle.framework"
codesign --force --sign "$CODE_SIGN_IDENTITY" "$RESOURCES_DIR/codex-mixin"
codesign --force --sign "$CODE_SIGN_IDENTITY" "$MACOS_DIR/CodexMixinMenu"
codesign --force --sign "$CODE_SIGN_IDENTITY" "$APP_DIR"
codesign --verify --deep --strict --verbose=2 "$APP_DIR"

BUILT_VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$CONTENTS_DIR/Info.plist")"
if [[ "$BUILT_VERSION" != "$APP_VERSION" ]]; then
  echo "app version mismatch: expected $APP_VERSION, got $BUILT_VERSION" >&2
  exit 1
fi

echo "$APP_DIR"
