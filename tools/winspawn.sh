#!/usr/bin/env bash
# Hidden detached command on a Windows remote. Survives disconnect; no console.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BIN="${MAKEPAD_TUNNEL_BIN:-$REPO_ROOT/target/release/cargo-makepad}"
ADDR="${WIN_TUNNEL_ADDR:-10.0.0.169:8384}"
if [[ $# -eq 0 ]]; then
    echo "usage: $(basename "$0") <command...>" >&2
    exit 2
fi
exec "$BIN" tunnel "$ADDR" --no-sync spawn "$@"
