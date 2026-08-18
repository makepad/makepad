#!/usr/bin/env bash
# Stage a local script on the Windows box and execute it, one connection,
# no stdin piping, no quoting hazards: tools/winrun.sh <script.(ps1|py|bat)> [args...]
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BIN="${MAKEPAD_TUNNEL_BIN:-$REPO_ROOT/target/release/cargo-makepad}"
ADDR="${WIN_TUNNEL_ADDR:-10.0.0.217:8384}"
exec "$BIN" tunnel "$ADDR" run "$@"
