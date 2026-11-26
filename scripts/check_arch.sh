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

# Four layers, strictly lower-triangular: core <- infra <- runtime <- app.
CORE_LAYER=(
    src/core src/infra src/app crates/platform/src
    src/runtime/benchmark src/runtime/catalog src/runtime/context
    src/runtime/fusion src/runtime/gateway src/runtime/upstream
)
PROTO_CONVERT=(
    src/core/protocol/convert src/core/protocol/sse.rs src/core/protocol/openai_chat
    src/core/protocol/openai_events src/core/protocol/anthropic_compat.rs
    src/core/protocol/compaction.rs src/core/protocol/model_reasoning.rs
)
LOWER=(
    src/runtime/upstream src/infra/ech src/core/provider src/core/protocol
    src/runtime/catalog src/infra/config src/runtime/benchmark
    src/infra/web_search src/core/images
)
SHELLS=(desktop panel)

check_paths "core" "${CORE_LAYER[@]}"
check_paths "protocol" "${PROTO_CONVERT[@]}"
check_paths "lower" "${LOWER[@]}"
check_paths "shell" "${SHELLS[@]}"
check_paths "test helper" tests/common/mod.rs
check_no_match "core must not reference crate::server" 'crate::(runtime::)?server\b' "${CORE_LAYER[@]}"
check_no_match "core must not reference crate::cli" 'crate::cli\b' "${CORE_LAYER[@]}"
check_no_match "core must not reference AppState" '\bAppState\b' "${CORE_LAYER[@]}"
check_no_match "protocol conversion must not send over the network" 'reqwest::Client|reqwest::blocking|Client::builder|\.send\(\)|\.post\(|\.execute\(' "${PROTO_CONVERT[@]}"
check_no_match "application must not use clap" '\bclap::' src/app
check_no_match "application must not use terminal/UI crates" '\b(indicatif|ratatui|console|crossterm)::' src/app
check_no_match "application must not print" '\b(print|println|eprint|eprintln)!' src/app
check_no_match "clients must not use clap" '\bclap::' src/infra/clients
check_no_match "clients must not use terminal/UI crates" '\b(indicatif|ratatui|console|crossterm)::' src/infra/clients
check_no_match "clients must not print" '\b(print|println|eprint|eprintln)!' src/infra/clients
check_no_match "lower layers must not reference FusionEngine" '\bFusionEngine\b' "${LOWER[@]}"
check_no_match "provider rules must not reference gateway" 'crate::(runtime::)?gateway::' src/core/provider
check_no_match "protocol rules must not reference gateway or provider runtime" 'crate::(runtime::)?gateway::|crate::(core::)?provider::(ProviderRuntime|ProviderRegistry)' src/core/protocol
check_no_match "protocol must not read or write stored config" 'crate::(infra::)?config::(ensure_|load_stored|mutate_stored|save_stored)|\b(ensure_compaction_secret|ensure_gateway_client_key|(load|mutate|save|delete|import|export)_stored_config\w*|stored_config_path)\b' src/core/protocol
check_no_match "protocol must not depend on the HTTP transport" '\breqwest\b' src/core/protocol
check_no_match "gateway must not reference fusion" 'crate::(runtime::)?fusion\b|crate::(runtime::)?\{[^}]*\bfusion\b' src/runtime/gateway
check_no_match "catalog must not reference fusion" 'crate::(runtime::)?fusion\b|crate::(runtime::)?\{[^}]*\bfusion\b' src/runtime/catalog
check_no_match "clients must not reference application" 'crate::(app|application)\b|crate::\{[^}]*\b(app|application)\b' src/infra/clients
check_no_match "provider must not reference clients" 'crate::(infra::)?clients\b|crate::(infra::)?\{[^}]*\bclients\b' src/core/provider
check_no_match "server must not reference crate::cli" 'crate::cli\b' src/runtime/server
check_no_match "core must not reference concrete shells" 'crate::(tui|panel)\b|path\s*=\s*"\.\./(tui|panel)/' \
    src/lib.rs src/core src/infra src/runtime src/app src/cli
check_no_match "panel shell must not reach CLI internals" 'crate::cli\b|super::super::cli' panel

# Every OS difference lives behind crates/platform; the rest of the code only
# expresses intent through the platform API.
check_no_match "platform-specific code must live in crates/platform" \
    '^\s*[^/\s].*(cfg!?\((not\()?(unix|windows)|any\((unix|windows)|target_os|target_family|std::os::(unix|windows)|consts::(OS|EXE_SUFFIX)|\.exe"|USERPROFILE|LOCALAPPDATA|"APPDATA|var(_os)?\("HOME"\)|env\("HOME"|launchctl|osascript|codesign|plutil|/Applications/|Library/|systemctl|schtasks|taskkill|tasklist|powershell|cmd\.exe|icacls|"/usr/|"/bin/|process_group\(0|kill_process_group|PermissionsExt|from_mode\()' \
    --glob '*.rs' src panel

# Tests that run the real binary must use the hermetic launcher; a bare
# spawn would stop the developer's real gateway through the shared service.
check_no_match "tests must spawn codex-mixin through tests/common" 'CARGO_BIN_EXE' --glob '*.rs' --glob '!tests/common/**' tests
if ! rg -q 'CODEX_GATEWAY_SERVICE_ID' tests/common/mod.rs; then
    echo "ARCH FAIL: tests/common/mod.rs must set CODEX_GATEWAY_SERVICE_ID"
    fail=1
fi

# MIT Axiomatic Design: the macro 4-layer matrix (core <- infra <- runtime <-
# app) and the micro 19-module matrix must both be strictly lower-triangular.
if ! python3 - "$ROOT" <<'PY'
import os
import re
import sys
from collections import defaultdict

root = sys.argv[1]
src = os.path.join(root, "src")

LAYERS = [
    ("core", ["access", "error", "images", "protocol", "provider", "usage"]),
    ("infra", ["clients", "config", "ech", "pi_plugins", "web_search"]),
    (
        "runtime",
        ["benchmark", "catalog", "context", "fusion", "gateway", "server", "upstream"],
    ),
    ("app", ["application"]),
]
LAYER_INDEX = {name: index for index, (name, _) in enumerate(LAYERS)}

# Directory prefixes that belong to each leaf module.
LEAF_OF_PREFIX = {
    "core/access.rs": "access",
    "core/error.rs": "error",
    "core/usage.rs": "usage",
    "core/images": "images",
    "core/protocol": "protocol",
    "core/provider": "provider",
    "infra/clients": "clients",
    "infra/config": "config",
    "infra/ech": "ech",
    "infra/pi_plugins": "pi_plugins",
    "infra/web_search": "web_search",
    "runtime/benchmark": "benchmark",
    "runtime/catalog": "catalog",
    "runtime/context": "context",
    "runtime/fusion": "fusion",
    "runtime/gateway": "gateway",
    "runtime/server": "server",
    "runtime/upstream": "upstream",
    "app": "application",
}
LEAF_LAYER = {}
for layer, leaves in LAYERS:
    for leaf in leaves:
        LEAF_LAYER[leaf] = LAYER_INDEX[layer]

# `src/lib.rs` re-exports a leaf module under its historic root name.
ROOT_ALIAS = {
    "app": "application",
    "application": "application",
    "gateway_access": "access",
    "error": "error",
    "protocol": "protocol",
    "anthropic": "protocol",
    "provider": "provider",
    "ducx_auth_carrier": "provider",
    "clients": "clients",
    "config": "config",
    "web_search": "web_search",
    "images": "images",
    "usage": "usage",
    "ech": "ech",
    "pi_plugins": "pi_plugins",
    "benchmark": "benchmark",
    "catalog": "catalog",
    "context": "context",
    "fusion": "fusion",
    "gateway": "gateway",
    "server": "server",
    "upstream": "upstream",
}

def strip_comments_and_strings(text):
    text = re.sub(r"/\*.*?\*/", " ", text, flags=re.DOTALL)
    text = re.sub(r"//[^\n]*", " ", text)
    text = re.sub(r'"(?:\\.|[^"\\])*"', '""', text)
    return text

def leaf_of_file(rel):
    for prefix, leaf in sorted(LEAF_OF_PREFIX.items(), key=lambda kv: -len(kv[0])):
        if rel == prefix or rel.startswith(prefix + "/"):
            return leaf
    return None

single = re.compile(r"\bcrate::([A-Za-z_][A-Za-z0-9_]*)")
grouped = re.compile(r"\bcrate::\{([^}]*)\}", re.DOTALL)
layered = re.compile(
    r"\bcrate::(core|infra|runtime|app)::([A-Za-z_][A-Za-z0-9_]*)"
)
layered_grouped = re.compile(
    r"\bcrate::(core|infra|runtime|app)::\{([^}]*)\}", re.DOTALL
)

edges = defaultdict(set)
evidence = defaultdict(list)
violations = []
files_scanned = 0

for directory, _, names in os.walk(src):
    for name in sorted(names):
        if not name.endswith(".rs"):
            continue
        filename = os.path.join(directory, name)
        rel = os.path.relpath(filename, src).replace(os.sep, "/")
        if rel.startswith("cli/") or rel in ("lib.rs", "main.rs"):
            continue
        source_leaf = leaf_of_file(rel)
        if source_leaf is None:
            continue
        files_scanned += 1
        with open(filename, encoding="utf-8") as handle:
            cleaned = strip_comments_and_strings(handle.read())

        targets = []
        for layer_name, leaf in layered.findall(cleaned):
            if layer_name == "app":
                targets.append("application")
            elif layer_name in LAYER_INDEX and leaf in LEAF_LAYER:
                targets.append(leaf)
        for layer_name, body in layered_grouped.findall(cleaned):
            if layer_name == "app":
                targets.append("application")
            elif layer_name in LAYER_INDEX:
                for part in body.split(","):
                    match = re.match(r"\s*([A-Za-z_][A-Za-z0-9_]*)", part)
                    if match and match.group(1) in LEAF_LAYER:
                        targets.append(match.group(1))
        for name_hit in single.findall(cleaned):
            leaf = ROOT_ALIAS.get(name_hit, name_hit if name_hit in LEAF_LAYER else None)
            if leaf:
                targets.append(leaf)
        for body in grouped.findall(cleaned):
            for part in body.split(","):
                match = re.match(r"\s*([A-Za-z_][A-Za-z0-9_]*)", part)
                if match:
                    hit = match.group(1)
                    leaf = ROOT_ALIAS.get(hit, hit if hit in LEAF_LAYER else None)
                    if leaf:
                        targets.append(leaf)

        for target in targets:
            if target == source_leaf:
                continue
            edges[source_leaf].add(target)
            evidence[(source_leaf, target)].append(rel)
            if LEAF_LAYER[source_leaf] < LEAF_LAYER[target]:
                violations.append((rel, source_leaf, target))

if violations:
    print(
        "ARCH FAIL: upward layer dependency detected "
        "(violates Axiomatic Design lower-triangular matrix):"
    )
    for rel, source_leaf, target in violations:
        print(
            f"  {rel}: {source_leaf} (L{LEAF_LAYER[source_leaf]}) -> "
            f"{target} (L{LEAF_LAYER[target]})"
        )
    sys.exit(1)

index, low, stack, on_stack, components = {}, {}, [], set(), []
counter = 0
nodes = sorted(LEAF_LAYER)

def strongconnect(node):
    global counter
    index[node] = low[node] = counter
    counter += 1
    stack.append(node)
    on_stack.add(node)
    for neighbor in sorted(edges[node]):
        if neighbor not in index:
            strongconnect(neighbor)
            low[node] = min(low[node], low[neighbor])
        elif neighbor in on_stack:
            low[node] = min(low[node], index[neighbor])
    if low[node] == index[node]:
        component = []
        while True:
            member = stack.pop()
            on_stack.discard(member)
            component.append(member)
            if member == node:
                break
        components.append(sorted(component))

sys.setrecursionlimit(10000)
for node in nodes:
    if node not in index:
        strongconnect(node)

cycles = [component for component in components if len(component) > 1]
if cycles:
    print(
        "ARCH FAIL: cyclic module dependency detected "
        "(violates Axiomatic Design lower-triangular matrix):"
    )
    for component in cycles:
        print("  cycle: " + " -> ".join(component + [component[0]]))
        for position, member in enumerate(component):
            target = component[(position + 1) % len(component)]
            for filename in evidence.get((member, target), []):
                print("    src/" + filename)
    sys.exit(1)

print(
    f"lower-triangular dependency matrix OK "
    f"({len(LAYERS)} layers, {len(nodes)} modules, {files_scanned} files, 0 cycles)"
)
PY
then
    fail=1
fi

if [ "$fail" -ne 0 ]; then
    echo "architecture boundary check failed"
    exit 1
fi
echo "architecture boundaries OK"
