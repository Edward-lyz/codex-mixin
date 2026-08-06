#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TEST_BINARY="$(mktemp -d)/install-progress-window-tests"

"$ROOT_DIR/scripts/swiftgen.sh" config run --config "$ROOT_DIR/swiftgen.yml" >/dev/null
LOCALIZATION_BUNDLE="$($ROOT_DIR/scripts/prepare_test_localization.sh)"

xcrun swiftc \
  -sdk "$(xcrun --show-sdk-path --sdk macosx)" \
  -target "$(uname -m)-apple-macos13.1" \
  "$ROOT_DIR/macos/Generated/L10n.swift" \
  "$ROOT_DIR/macos/Localization.swift" \
  "$ROOT_DIR/macos/UpdateSupport.swift" \
  "$ROOT_DIR/macos/AppSupport.swift" \
  "$ROOT_DIR/macos/InstallProgressWindow.swift" \
  "$ROOT_DIR/macos/tests/InstallProgressWindowTests.swift" \
  -framework Cocoa \
  -o "$TEST_BINARY"

CODEX_MIXIN_LOCALIZATION_DIR="$LOCALIZATION_BUNDLE" "$TEST_BINARY"
