#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ $# -eq 0 ]]; then
    echo "usage: $(basename "$0") <powershell-code...>" >&2
    exit 2
fi

exec "$SCRIPT_DIR/winps.sh" -NoProfile -Command "$*"
