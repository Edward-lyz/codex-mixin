#!/usr/bin/env bash
# Static architecture boundary checks for codex-mixin.
#
# These are coarse source-level constraints (rg over explicit imports and
# fully-qualified references). They are NOT Rust type analysis; cargo clippy
# and the test suite remain the authoritative checks. Kept deliberately
# simple so a regression trips a single grep.
set -euo pipefail

cd "$(dirname "$0")/.."

fail=0

check_no_match() {
    local label="$1" pattern="$2"
    shift 2
    local hits
    hits=$(rg -n "$pattern" "$@" 2>/dev/null || true)
    if [ -n "$hits" ]; then
        echo "ARCH FAIL: $label"
        echo "$hits"
        fail=1
    fi
}

# Core request-path modules: they must never depend on the inbound server
# layer, the CLI binary, or the AppState composition.
CORE='src/gateway src/upstream src/provider src/protocol src/fusion src/catalog src/config src/benchmark src/web_search src/images src/application src/clients'
check_no_match "core must not reference crate::server" 'crate::server' $CORE
check_no_match "core must not reference crate::cli" 'crate::cli' $CORE
check_no_match "core must not reference AppState" '\bAppState\b' $CORE

# Protocol conversion and codec modules must not perform network sends;
# request_body.rs is the deliberate low-level transport helper and is exempt.
PROTO_CONVERT='src/protocol/convert src/protocol/sse src/protocol/openai_chat src/protocol/openai_events src/protocol/anthropic_compat src/protocol/compaction src/protocol/model_reasoning'
check_no_match "protocol conversion must not send over the network" 'reqwest::Client|reqwest::blocking|Client::builder|\.send\(\)|\.post\(|\.execute\(' $PROTO_CONVERT

# Library use cases must not drag terminal/UI crates into the library.
check_no_match "application must not use clap" '\bclap::' src/application
check_no_match "application must not use terminal/UI crates" '\b(indicatif|ratatui|console|crossterm)::' src/application
check_no_match "clients must not use clap" '\bclap::' src/clients
check_no_match "clients must not use terminal/UI crates" '\b(indicatif|ratatui|console|crossterm)::' src/clients

# Lower layers must not reference the fusion engine or the gateway executor.
LOWER='src/upstream src/provider src/protocol src/catalog src/config src/benchmark src/web_search src/images'
check_no_match "lower layers must not reference FusionEngine" '\bFusionEngine\b' $LOWER
check_no_match "provider rules must not reference gateway" 'crate::gateway::' src/provider
check_no_match "protocol rules must not reference gateway or provider runtime" 'crate::gateway::|crate::provider::(ProviderRuntime|ProviderRegistry)' src/protocol

# The server layer owns AppState; it must not reach into the CLI binary.
check_no_match "server must not reference crate::cli" 'crate::cli' src/server

if [ "$fail" -ne 0 ]; then
    echo
    echo "architecture boundary check failed"
    exit 1
fi
echo "architecture boundaries OK"
