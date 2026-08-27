#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TEST_BINARY="$(mktemp -d)/install-claude-panel-tests"

"$ROOT_DIR/scripts/swiftgen.sh" config run --config "$ROOT_DIR/swiftgen.yml" >/dev/null

xcrun swiftc \
  -target "$(uname -m)-apple-macosx13.1" \
  "$ROOT_DIR/macos/Generated/L10n.swift" \
  "$ROOT_DIR/macos/Localization.swift" \
  "$ROOT_DIR/macos/ApplicationMenuSupport.swift" \
  "$ROOT_DIR/macos/LiquidGlassSupport.swift" \
  "$ROOT_DIR/macos/InstallClaudePanel.swift" \
  "$ROOT_DIR/macos/tests/InstallClaudePanelTests.swift" \
  -framework Cocoa \
  -framework SwiftUI \
  -o "$TEST_BINARY"
"$TEST_BINARY"
