#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TEST_BINARY="$(mktemp -d)/application-icon-tests"

xcrun swiftc \
  "$ROOT_DIR/macos/ApplicationIconSupport.swift" \
  "$ROOT_DIR/macos/tests/ApplicationIconSupportTests.swift" \
  -framework Cocoa \
  -o "$TEST_BINARY"
"$TEST_BINARY"
