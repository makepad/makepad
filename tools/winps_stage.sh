#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "usage: $(basename "$0") <script.ps1>" >&2
    exit 2
fi

SCRIPT_PATH="$1"
STAGED_PATH="/tmp/codex_win_remote.ps1"

if [[ ! -f "$SCRIPT_PATH" ]]; then
    echo "missing script file: $SCRIPT_PATH" >&2
    exit 1
fi

cp "$SCRIPT_PATH" "$STAGED_PATH"
echo "$STAGED_PATH"
