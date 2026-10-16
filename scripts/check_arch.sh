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
    src/gateway src/upstream src/ech.rs src/ech src/provider src/protocol src/fusion
    src/catalog.rs src/catalog src/config.rs src/config src/benchmark
    src/web_search src/images src/application src/clients src/platform
)
PROTO_CONVERT=(
    src/protocol/convert src/protocol/sse.rs src/protocol/openai_chat
    src/protocol/openai_events src/protocol/anthropic_compat.rs
    src/protocol/compaction.rs src/protocol/model_reasoning.rs
)
LOWER=(
    src/upstream src/ech.rs src/ech src/provider src/protocol src/catalog.rs src/catalog
    src/config.rs src/config src/benchmark src/web_search src/images
)
SHELLS=(macos panel windows)

check_paths "core" "${CORE[@]}"
check_paths "protocol" "${PROTO_CONVERT[@]}"
check_paths "lower" "${LOWER[@]}"
check_paths "shell" "${SHELLS[@]}"
check_paths "test helper" tests/common/mod.rs
check_no_match "core must not reference crate::server" 'crate::server' "${CORE[@]}"
check_no_match "core must not reference crate::cli" 'crate::cli\b' "${CORE[@]}"
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
check_no_match "protocol must not read or write stored config" 'crate::config::(ensure_|load_stored|mutate_stored|save_stored)|\b(ensure_compaction_secret|ensure_gateway_client_key|(load|mutate|save|delete|import|export)_stored_config\w*|stored_config_path)\b' src/protocol
check_no_match "protocol must not depend on the HTTP transport" '\breqwest\b' src/protocol
check_no_match "gateway must not reference fusion" 'crate::fusion\b|crate::\{[^}]*\bfusion\b' src/gateway
check_no_match "catalog must not reference fusion" 'crate::fusion\b|crate::\{[^}]*\bfusion\b' src/catalog.rs src/catalog
check_no_match "clients must not reference application" 'crate::application\b|crate::\{[^}]*\bapplication\b' src/clients
check_no_match "provider must not reference clients" 'crate::clients\b|crate::\{[^}]*\bclients\b' src/provider
check_no_match "server must not reference crate::cli" 'crate::cli\b' src/server
check_no_match "core must not reference concrete shells" 'crate::(tui|panel)\b|path\s*=\s*"\.\./(tui|panel)/' \
    src/lib.rs src/application src/gateway src/upstream src/ech.rs src/ech src/provider src/protocol \
    src/fusion src/catalog.rs src/catalog src/config.rs src/config src/benchmark \
    src/web_search src/images src/clients src/server src/cli
check_no_match "panel shell must not reach CLI internals" 'crate::cli\b|super::super::cli' panel

# Tests that run the real binary must use the hermetic launcher; a bare
# spawn would stop the developer's real gateway through the shared service.
check_no_match "tests must spawn codex-mixin through tests/common" 'CARGO_BIN_EXE' --glob '*.rs' --glob '!tests/common/**' tests
if ! rg -q 'CODEX_GATEWAY_SERVICE_ID' tests/common/mod.rs; then
    echo "ARCH FAIL: tests/common/mod.rs must set CODEX_GATEWAY_SERVICE_ID"
    fail=1
fi

if [ "$fail" -ne 0 ]; then
    echo "architecture boundary check failed"
    exit 1
fi
echo "architecture boundaries OK"
