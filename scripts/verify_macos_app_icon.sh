#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_DIR="$ROOT_DIR/dist/Codex Mixin.app"
REQUIRE_ADAPTIVE_ICON=0

if [[ "${1:-}" == "--require-adaptive" ]]; then
  REQUIRE_ADAPTIVE_ICON=1
  shift
fi
if [[ $# -gt 1 ]]; then
  echo "usage: $0 [--require-adaptive] [app-path]" >&2
  exit 2
fi
if [[ $# -eq 1 ]]; then
  APP_DIR="$1"
fi

INFO_PLIST="$APP_DIR/Contents/Info.plist"
RESOURCES_DIR="$APP_DIR/Contents/Resources"
ICON_PACKAGE="$ROOT_DIR/macos/CodexMixin.icon"
ICON_JSON="$ICON_PACKAGE/icon.json"

plutil -convert json -o - "$ICON_JSON" >/dev/null
LIGHT_SOURCE="$(plutil -extract \
  'groups.0.layers.0.image-name-specializations.0.value' raw -o - "$ICON_JSON")"
DARK_APPEARANCE="$(plutil -extract \
  'groups.0.layers.0.image-name-specializations.1.appearance' raw -o - "$ICON_JSON")"
DARK_SOURCE="$(plutil -extract \
  'groups.0.layers.0.image-name-specializations.1.value' raw -o - "$ICON_JSON")"

if [[ "$DARK_APPEARANCE" != "dark" ]]; then
  echo "adaptive icon does not declare a dark appearance" >&2
  exit 1
fi
for source_name in "$LIGHT_SOURCE" "$DARK_SOURCE"; do
  if [[ ! -f "$ICON_PACKAGE/Assets/$source_name" ]]; then
    echo "adaptive icon source is missing: $source_name" >&2
    exit 1
  fi
done

FALLBACK_ICON="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIconFile' "$INFO_PLIST")"
if [[ "$FALLBACK_ICON" != "CodexMixin" ]]; then
  echo "unexpected fallback icon name: $FALLBACK_ICON" >&2
  exit 1
fi
for resource_name in CodexMixin.icns CodexMixinDark.icns; do
  if [[ ! -f "$RESOURCES_DIR/$resource_name" ]]; then
    echo "application icon resource is missing: $resource_name" >&2
    exit 1
  fi
done

if [[ -f "$RESOURCES_DIR/Assets.car" ]]; then
  ADAPTIVE_ICON="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIconName' "$INFO_PLIST")"
  if [[ "$ADAPTIVE_ICON" != "CodexMixin" ]]; then
    echo "unexpected adaptive icon name: $ADAPTIVE_ICON" >&2
    exit 1
  fi
elif [[ "$REQUIRE_ADAPTIVE_ICON" == "1" ]]; then
  echo "adaptive icon asset catalog is missing" >&2
  exit 1
elif /usr/libexec/PlistBuddy -c 'Print :CFBundleIconName' "$INFO_PLIST" >/dev/null 2>&1; then
  echo "CFBundleIconName is set without an Assets.car" >&2
  exit 1
fi

echo "macOS application icons verified"
