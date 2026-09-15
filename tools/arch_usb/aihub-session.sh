#!/usr/bin/env bash
set -Eeuo pipefail
if test -f /etc/profile.d/makepad-cuda.sh; then
    source /etc/profile.d/makepad-cuda.sh
fi
if test -f /etc/makepad/aihub.env; then
    set -a
    source /etc/makepad/aihub.env
    set +a
fi
MAKEPAD_AI_HUB_ROOT=${MAKEPAD_AI_HUB_ROOT:-/home/arch/makepad}
cd "$MAKEPAD_AI_HUB_ROOT"
exec target/release/makepad-ai-hub --host "${MAKEPAD_AI_HUB_HOST:-0.0.0.0}" --port "${MAKEPAD_ASSET_AI_PORT:-8785}" --cache-dir "${MAKEPAD_ASSET_AI_CACHE:-$HOME/.makepad/weights}" "$@"
