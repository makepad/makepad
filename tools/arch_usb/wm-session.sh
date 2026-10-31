#!/usr/bin/env bash
# The WM starts the AI Hub app (cargo run) at session start and keeps it running
# (apps/wm/src/hub_app.rs, settings ~/.makepad/wm/ai-hub.conf): a WM restart keeps the app,
# its loaded models and its tray, which comes back on the next WM.
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
# The WM's own flags (which apps run under the JIT); the prebuild below reads the same ones.
wm_flags=(--jit sandbox)
# The AI Hub app and the JIT tool are built the way the WM starts them: `makepad-wm-build` runs
# the WM's own start command of each (`cargo run --release -p makepad-app-aihub --features
# <features>`, hub_app.rs; `cargo run --release -p makepad-run --bin makepad`) with `build`, in
# the launcher's environment, so the WM's starts compile nothing.
if test ! -x target/release/makepad-app-aihub || test "${MAKEPAD_WM_BUILD:-0}" = 1; then
    cargo run --offline --release -p makepad-app-wm --bin makepad-wm-build -- "${wm_flags[@]}" -- hub sandbox
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
# Sandbox runs under the JIT (`makepad run makepad-sandbox`, apps/wm/src/apps.rs `--jit`), so
# every node's Sandbox can be inspected and live-patched from the dev machine. Its session
# transcript and inspect socket live on disk (~/.makepad-run/tmp, the TMPDIR the remote hosts
# entries use), not on the /tmp tmpfs other jobs fill: ~/.makepad/wm/jit.env, seeded once.
jit_env="$HOME/.makepad/wm/jit.env"
if test ! -e "$jit_env"; then
    mkdir -p "$HOME/.makepad/wm" "$HOME/.makepad-run/tmp"
    printf 'MAKEPAD_RUN_VERBOSE=1\nTMPDIR=%s/.makepad-run/tmp\n' "$HOME" > "$jit_env"
fi
exec target/release/makepad-app-wm "-scale=${MAKEPAD_WM_SCALE:-1.3}" "${wm_flags[@]}" "$@"
