#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
STAGED_PATH="/tmp/codex_win_remote.ps1"

if [[ ! -f "$STAGED_PATH" ]]; then
    echo "missing staged script: $STAGED_PATH" >&2
    echo "stage one first with: tools/winps_stage.sh <script.ps1>" >&2
    exit 1
fi

exec "$SCRIPT_DIR/winps.sh" < "$STAGED_PATH"
