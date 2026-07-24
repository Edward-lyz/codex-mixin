#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: verify-linux-static.sh <binary>" >&2
  exit 2
fi

binary="$1"
if [[ ! -x "$binary" ]]; then
  echo "missing executable Linux binary: $binary" >&2
  exit 1
fi

if ! file "$binary" | grep -Eq 'ELF .* (static-pie linked|statically linked)'; then
  echo "Linux release binary is not statically linked:" >&2
  file "$binary" >&2
  exit 1
fi

if readelf --version-info "$binary" 2>/dev/null | grep -q 'GLIBC_'; then
  echo "Linux release binary unexpectedly requires glibc:" >&2
  readelf --version-info "$binary" | grep 'GLIBC_' >&2
  exit 1
fi

"$binary" --version
