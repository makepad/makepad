#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ $# -ne 1 ]]; then
    echo "usage: $(basename "$0") <script.ps1>" >&2
    exit 2
fi

SCRIPT_PATH="$1"
if [[ ! -f "$SCRIPT_PATH" ]]; then
    echo "missing script file: $SCRIPT_PATH" >&2
    exit 1
fi

exec "$SCRIPT_DIR/winps.sh" < "$SCRIPT_PATH"
