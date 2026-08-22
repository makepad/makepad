#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BIN="${MAKEPAD_TUNNEL_BIN:-$REPO_ROOT/target/release/cargo-makepad}"
ADDR="${WIN_TUNNEL_ADDR:-10.0.0.217:8384}"

if [[ ! -x "$BIN" ]]; then
    echo "missing tunnel binary: $BIN" >&2
    echo "build it with: cargo build -p cargo-makepad --release" >&2
    exit 1
fi

if [[ $# -eq 0 ]]; then
    exec "$BIN" tunnel "$ADDR" --no-sync shell powershell -NoProfile -Command -
else
    exec "$BIN" tunnel "$ADDR" --no-sync shell powershell "$@"
fi
