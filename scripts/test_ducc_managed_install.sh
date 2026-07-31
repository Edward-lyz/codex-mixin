#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TEST_BINARY="$(mktemp -d)/ducc-managed-install-tests"

xcrun swiftc \
  "$ROOT_DIR/macos/UpdateSupport.swift" \
  "$ROOT_DIR/macos/AppSupport.swift" \
  "$ROOT_DIR/macos/SettingsPanel.swift" \
  "$ROOT_DIR/macos/ProviderSupport.swift" \
  "$ROOT_DIR/macos/ProviderWindowLayoutSupport.swift" \
  "$ROOT_DIR/macos/AppOperationLogging.swift" \
  "$ROOT_DIR/macos/ProviderSettingsWindow.swift" \
  "$ROOT_DIR/macos/tests/DuccManagedInstallTests.swift" \
  -framework Cocoa \
  -o "$TEST_BINARY"
"$TEST_BINARY"
