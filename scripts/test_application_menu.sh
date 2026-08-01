#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TEST_BINARY="$(mktemp -d)/application-menu-tests"

xcrun swiftc \
  "$ROOT_DIR/macos/ApplicationMenuSupport.swift" \
  "$ROOT_DIR/macos/tests/ApplicationMenuSupportTests.swift" \
  -framework Cocoa \
  -o "$TEST_BINARY"
"$TEST_BINARY"
