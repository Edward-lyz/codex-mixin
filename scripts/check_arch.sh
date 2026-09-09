#!/usr/bin/env bash
set -euo pipefail

ROOT="${1:-$(dirname "$0")/..}"
cd "$ROOT"
command -v rg >/dev/null || { echo "ARCH ERROR: rg is required" >&2; exit 2; }
fail=0

check_paths() {
    local label="$1"
    shift
    local path
    for path in "$@"; do
        if [ ! -e "$path" ]; then
            echo "ARCH ERROR: $label path does not exist: $path" >&2
            exit 2
        fi
    done
}

check_no_match() {
    local label="$1" pattern="$2"
    shift 2
    local hits status
    set +e
    hits=$(rg -n "$pattern" "$@" 2>&1)
    status=$?
    set -e
    if [ "$status" -eq 0 ]; then
        echo "ARCH FAIL: $label"
        echo "$hits"
        fail=1
    elif [ "$status" -ne 1 ]; then
        echo "ARCH ERROR: $label check could not run" >&2
        echo "$hits" >&2
        exit 2
    fi
}

CORE=(
    src/gateway src/upstream src/provider src/protocol src/fusion
    src/catalog.rs src/catalog src/config.rs src/config src/benchmark
    src/web_search src/images src/application src/clients
)
PROTO_CONVERT=(
    src/protocol/convert src/protocol/sse.rs src/protocol/openai_chat
    src/protocol/openai_events src/protocol/anthropic_compat.rs
    src/protocol/compaction.rs src/protocol/model_reasoning.rs
)
LOWER=(
    src/upstream src/provider src/protocol src/catalog.rs src/catalog
    src/config.rs src/config src/benchmark src/web_search src/images
)

check_paths "core" "${CORE[@]}"
check_paths "protocol" "${PROTO_CONVERT[@]}"
check_paths "lower" "${LOWER[@]}"
check_no_match "core must not reference crate::server" 'crate::server' "${CORE[@]}"
check_no_match "core must not reference crate::cli" 'crate::cli' "${CORE[@]}"
check_no_match "core must not reference AppState" '\bAppState\b' "${CORE[@]}"
check_no_match "protocol conversion must not send over the network" 'reqwest::Client|reqwest::blocking|Client::builder|\.send\(\)|\.post\(|\.execute\(' "${PROTO_CONVERT[@]}"
check_no_match "application must not use clap" '\bclap::' src/application
check_no_match "application must not use terminal/UI crates" '\b(indicatif|ratatui|console|crossterm)::' src/application
check_no_match "application must not print" '\b(print|println|eprint|eprintln)!' src/application
check_no_match "clients must not use clap" '\bclap::' src/clients
check_no_match "clients must not use terminal/UI crates" '\b(indicatif|ratatui|console|crossterm)::' src/clients
check_no_match "clients must not print" '\b(print|println|eprint|eprintln)!' src/clients
check_no_match "lower layers must not reference FusionEngine" '\bFusionEngine\b' "${LOWER[@]}"
check_no_match "provider rules must not reference gateway" 'crate::gateway::' src/provider
check_no_match "protocol rules must not reference gateway or provider runtime" 'crate::gateway::|crate::provider::(ProviderRuntime|ProviderRegistry)' src/protocol
check_no_match "server must not reference crate::cli" 'crate::cli' src/server

if [ "$fail" -ne 0 ]; then
    echo "architecture boundary check failed"
    exit 1
fi
echo "architecture boundaries OK"
