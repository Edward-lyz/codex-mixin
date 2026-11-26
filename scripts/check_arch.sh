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
    src/gateway src/upstream src/ech src/provider src/protocol src/fusion
    src/catalog src/config src/benchmark
    src/web_search src/images src/application src/clients crates/platform/src src/context
)
PROTO_CONVERT=(
    src/protocol/convert src/protocol/sse.rs src/protocol/openai_chat
    src/protocol/openai_events src/protocol/anthropic_compat.rs
    src/protocol/compaction.rs src/protocol/model_reasoning.rs
)
LOWER=(
    src/upstream src/ech src/provider src/protocol src/catalog
    src/config src/benchmark src/web_search src/images
)
SHELLS=(desktop panel)

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
check_no_match "catalog must not reference fusion" 'crate::fusion\b|crate::\{[^}]*\bfusion\b' src/catalog
check_no_match "clients must not reference application" 'crate::application\b|crate::\{[^}]*\bapplication\b' src/clients
check_no_match "provider must not reference clients" 'crate::clients\b|crate::\{[^}]*\bclients\b' src/provider
check_no_match "server must not reference crate::cli" 'crate::cli\b' src/server
check_no_match "core must not reference concrete shells" 'crate::(tui|panel)\b|path\s*=\s*"\.\./(tui|panel)/' \
    src/lib.rs src/application src/gateway src/upstream src/ech src/provider src/protocol \
    src/fusion src/catalog src/config src/benchmark \
    src/web_search src/images src/clients src/server src/cli
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

# MIT Axiomatic Design: the module dependency matrix must be strictly
# lower-triangular, so the library crate's module graph may hold no cycles.
if ! python3 - "$ROOT" <<'PY'
import os
import re
import sys
from collections import defaultdict

root = sys.argv[1]
src = os.path.join(root, "src")

# `cli` and `main.rs` belong to the binary crate, whose root is `main.rs`.
modules = set()
for entry in sorted(os.listdir(src)):
    path = os.path.join(src, entry)
    if os.path.isdir(path):
        if entry != "cli":
            modules.add(entry)
    elif entry.endswith(".rs") and entry not in ("lib.rs", "main.rs"):
        modules.add(entry[:-3])

pattern = re.compile(r"\bcrate::([A-Za-z_][A-Za-z0-9_]*)")
group_pattern = re.compile(r"\bcrate::\{([^}]*)\}", re.DOTALL)
alias_re = re.compile(
    r"\bpub(?:\([^)]*\))?\s+use\s+([A-Za-z_][A-Za-z0-9_]*)::[^;]*?\bas\s+([A-Za-z_][A-Za-z0-9_]*)\s*;"
)
reexport_re = re.compile(
    r"\bpub(?:\([^)]*\))?\s+use\s+([A-Za-z_][A-Za-z0-9_]*)(?:::[A-Za-z_][A-Za-z0-9_]*)*::([A-Za-z_][A-Za-z0-9_]*)\s*;"
)

def strip_comments_and_strings(text):
    text = re.sub(r"/\*.*?\*/", " ", text, flags=re.DOTALL)
    text = re.sub(r"//[^\n]*", " ", text)
    text = re.sub(r'"(?:\\.|[^"\\])*"', '""', text)
    return text

# Resolve root-level `pub use` aliases in `src/lib.rs` (e.g. `crate::anthropic` -> `protocol`).
aliases = {}
lib_rs = os.path.join(src, "lib.rs")
if os.path.exists(lib_rs):
    with open(lib_rs, encoding="utf-8") as handle:
        lib_text = strip_comments_and_strings(handle.read())
    for mod_name, alias in alias_re.findall(lib_text):
        if mod_name in modules:
            aliases[alias] = mod_name
    for mod_name, item in reexport_re.findall(lib_text):
        if mod_name in modules and item not in aliases:
            aliases[item] = mod_name

def resolve_target(name):
    if name in modules:
        return name
    return aliases.get(name)

edges = defaultdict(set)
evidence = defaultdict(list)

for module in sorted(modules):
    path = os.path.join(src, module)
    if os.path.isdir(path):
        files = sorted(
            os.path.join(dirpath, name)
            for dirpath, _, names in os.walk(path)
            for name in names
            if name.endswith(".rs")
        )
    else:
        files = [path + ".rs"]
    for filename in files:
        with open(filename, encoding="utf-8") as handle:
            cleaned = strip_comments_and_strings(handle.read())
        candidates = list(pattern.findall(cleaned))
        for group in group_pattern.findall(cleaned):
            for part in group.split(","):
                match = re.match(r"\s*([A-Za-z_][A-Za-z0-9_]*)", part)
                if match:
                    candidates.append(match.group(1))
        for name in candidates:
            target = resolve_target(name)
            if target and target != module:
                edges[module].add(target)
                evidence[(module, target)].append(filename)

index, low, stack, on_stack, components = {}, {}, [], set(), []
counter = 0

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
for module in sorted(modules):
    if module not in index:
        strongconnect(module)

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
                print("    " + os.path.relpath(filename, root))
    sys.exit(1)
print(f"lower-triangular dependency matrix OK ({len(modules)} modules, 0 cycles)")
PY
then
    fail=1
fi

if [ "$fail" -ne 0 ]; then
    echo "architecture boundary check failed"
    exit 1
fi
echo "architecture boundaries OK"
