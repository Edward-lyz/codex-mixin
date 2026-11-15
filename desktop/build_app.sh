#!/usr/bin/env bash
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# CLT's newer SDK may require SwiftUI macros supplied only by Xcode. Keep the
# supported SDK usable locally without changing the system selection.
if [[ -z "${SDKROOT:-}" ]] && [[ "$(xcode-select -p)" == /Library/Developer/CommandLineTools ]] \
    && [[ -d /Library/Developer/CommandLineTools/SDKs/MacOSX26.sdk ]]; then
  export SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.sdk
fi
exec "$ROOT_DIR/macos/build_app.sh" "$@"
