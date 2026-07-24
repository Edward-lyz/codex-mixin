#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TEST_BINARY="$(mktemp -d)/menu-view-update-tests"

xcrun swiftc \
  "$ROOT_DIR/macos/MenuViewUpdateSupport.swift" \
  "$ROOT_DIR/macos/tests/MenuViewUpdateSupportTests.swift" \
  -framework Cocoa \
  -o "$TEST_BINARY"
"$TEST_BINARY"
