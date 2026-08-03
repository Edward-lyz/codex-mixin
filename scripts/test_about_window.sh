#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TEST_BINARY="$(mktemp -d)/about-window-tests"
SWIFT_ARCH="$(uname -m)"

"$ROOT_DIR/scripts/swiftgen.sh" config run --config "$ROOT_DIR/swiftgen.yml" >/dev/null
LOCALIZATION_BUNDLE="$($ROOT_DIR/scripts/prepare_test_localization.sh)"

xcrun swiftc \
  "$ROOT_DIR/macos/Generated/L10n.swift" \
  "$ROOT_DIR/macos/Localization.swift" \
  -target "$SWIFT_ARCH-apple-macosx13.1" \
  "$ROOT_DIR/macos/InstallCard.swift" \
  "$ROOT_DIR/macos/AboutWindow.swift" \
  "$ROOT_DIR/macos/tests/AboutWindowTests.swift" \
  -framework Cocoa \
  -framework CryptoKit \
  -framework SwiftUI \
  -o "$TEST_BINARY"
CODEX_MIXIN_LOCALIZATION_DIR="$LOCALIZATION_BUNDLE" \
CODEX_MIXIN_WALLPAPER_ASSET_DIR="$ROOT_DIR/macos/assets/nasa-wallpapers" "$TEST_BINARY"
