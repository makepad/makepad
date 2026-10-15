#!/usr/bin/env bash
# The WM starts the AI hub as its own child and stops it when it ends
# (apps/wm/src/hub_child.rs, settings ~/.makepad/wm/ai-hub.conf): restarting the
# WM restarts the hub, and its chat model loads again (about a minute for a 27B).
set -Eeuo pipefail
if test -f /etc/makepad/wm.env; then
    set -a
    source /etc/makepad/wm.env
    set +a
fi
export MAKEPAD=linux_direct
export MAKEPAD_WM_ROOT=${MAKEPAD_WM_ROOT:-/home/arch/makepad}
export CARGO_NET_OFFLINE=true
export XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
cd "$MAKEPAD_WM_ROOT"
if test ! -x target/release/makepad-app-wm || test "${MAKEPAD_WM_BUILD:-0}" = 1; then
    cargo build --offline --release -p makepad-app-wm
fi

# The saved display arrangement (order, per-screen modes, render-on GPU):
# ${XDG_CONFIG_HOME:-$HOME/.config}/makepad/wm/display-layout, migrated
# from the legacy display-gpu file for one release when display-layout
# does not exist yet. See tools/linux/display-layout-env.sh (and
# apps/wm/src/shell/display_layout.rs, the pure model) for the file
# format and the exact resolution rule. Never fatal: a missing or
# malformed file falls back to the renderer's automatic choices, logged
# to stderr.
if test -r "$MAKEPAD_WM_ROOT/tools/linux/display-layout-env.sh"; then
    . "$MAKEPAD_WM_ROOT/tools/linux/display-layout-env.sh"
fi
exec target/release/makepad-app-wm "-scale=${MAKEPAD_WM_SCALE:-1.3}" "$@"
