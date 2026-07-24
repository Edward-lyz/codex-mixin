#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TEST_BINARY="$(mktemp -d)/settings-panel-presentation-tests"

xcrun swiftc \
  "$ROOT_DIR/macos/SettingsPanel.swift" \
  "$ROOT_DIR/macos/tests/SettingsPanelPresentationTests.swift" \
  -framework Cocoa \
  -o "$TEST_BINARY"
"$TEST_BINARY"
