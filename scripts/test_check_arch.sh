#!/usr/bin/env bash
set -euo pipefail

ROOT="$(dirname "$0")/.."
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
cp -R "$ROOT/src" "$TMP/src"
mkdir -p "$TMP/scripts"
mkdir -p "$TMP/macos" "$TMP/panel" "$TMP/windows"
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
expect_rejected src/ech.rs 'use crate::server::AppState;'
expect_rejected src/application/mod.rs 'fn violation() { println!("bad"); }'
expect_rejected src/application/mod.rs 'use crate::panel;'
expect_rejected src/protocol/compaction.rs 'use crate::config::ensure_compaction_secret;'
expect_rejected src/protocol/compaction.rs 'use crate::config::{ensure_compaction_secret};'
expect_rejected src/protocol/sse.rs 'fn violation() { let _ = crate::config::load_stored_config(); }'
expect_rejected src/protocol/openai_events/gemini.rs 'fn violation(_: reqwest::Error) {}'
expect_rejected src/gateway/router.rs 'use crate::fusion::FusionProfile;'
expect_rejected src/gateway/router.rs 'use crate::{fusion, error};'
expect_rejected src/catalog/service.rs 'use crate::fusion::FusionMode;'
expect_rejected src/clients/frameworks/mod.rs 'use crate::application::agents::IntegrationStatus;'
expect_rejected src/provider/resolver.rs 'use crate::clients::files::write_atomic_if_changed;'
echo "architecture negative checks OK"
