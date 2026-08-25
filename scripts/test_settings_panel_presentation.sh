#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TEST_BINARY="$(mktemp -d)/settings-panel-presentation-tests"

"$ROOT_DIR/scripts/swiftgen.sh" config run --config "$ROOT_DIR/swiftgen.yml" >/dev/null
LOCALIZATION_BUNDLE="$($ROOT_DIR/scripts/prepare_test_localization.sh)"

xcrun swiftc \
  "$ROOT_DIR/macos/Generated/L10n.swift" \
  "$ROOT_DIR/macos/Localization.swift" \
  "$ROOT_DIR/macos/ApplicationMenuSupport.swift" \
  "$ROOT_DIR/macos/SettingsPanel.swift" \
  "$ROOT_DIR/macos/tests/SettingsPanelPresentationTests.swift" \
  -framework Cocoa \
  -o "$TEST_BINARY"
CODEX_MIXIN_LOCALIZATION_DIR="$LOCALIZATION_BUNDLE" "$TEST_BINARY"
