#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TEST_BINARY="$(mktemp -d)/provider-support-tests"

xcrun swiftc \
  "$ROOT_DIR/macos/ProviderSupport.swift" \
  "$ROOT_DIR/macos/tests/ProviderSupportTests.swift" \
  -o "$TEST_BINARY"
"$TEST_BINARY"
