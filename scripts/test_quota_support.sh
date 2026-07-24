#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TEST_BINARY="$(mktemp -d)/quota-support-tests"

xcrun swiftc \
  "$ROOT_DIR/macos/QuotaSupport.swift" \
  "$ROOT_DIR/macos/tests/QuotaSupportTests.swift" \
  -o "$TEST_BINARY"
"$TEST_BINARY"
