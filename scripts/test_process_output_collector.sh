#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TEST_BINARY="$(mktemp -d)/process-output-collector-tests"

xcrun swiftc \
  "$ROOT_DIR/macos/ProcessOutputCollector.swift" \
  "$ROOT_DIR/macos/tests/ProcessOutputCollectorTests.swift" \
  -o "$TEST_BINARY"

"$TEST_BINARY"
