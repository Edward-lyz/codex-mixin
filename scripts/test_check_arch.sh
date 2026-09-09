#!/usr/bin/env bash
set -euo pipefail

ROOT="$(dirname "$0")/.."
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
cp -R "$ROOT/src" "$TMP/src"
mkdir -p "$TMP/scripts"
cp "$ROOT/scripts/check_arch.sh" "$TMP/scripts/check_arch.sh"

expect_rejected() {
    local file="$1" violation="$2"
    rm -rf "$TMP/src"
    cp -R "$ROOT/src" "$TMP/src"
    printf '\n%s\n' "$violation" >> "$TMP/$file"
    if bash "$TMP/scripts/check_arch.sh" "$TMP" >/dev/null 2>&1; then
        echo "negative architecture check unexpectedly passed: $file" >&2
        exit 1
    fi
}

bash "$TMP/scripts/check_arch.sh" "$TMP" >/dev/null
expect_rejected src/protocol/compaction.rs 'fn violation() { reqwest::Client::new().post("http://invalid").send(); }'
expect_rejected src/config.rs 'use crate::server::AppState;'
expect_rejected src/application/mod.rs 'fn violation() { println!("bad"); }'
echo "architecture negative checks OK"
