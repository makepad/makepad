#!/bin/bash
# Regression test: the WM, JIT-compiled under `makepad run`, routes `ui` requests through the HAL's
# rustc-built dispatch (10-30: its Rust-ABI router in the HAL's static was refused, "static
# `makepad_hal::a11y::UI_ROUTERS` is read from JIT-compiled code"). The WM registers a C entry
# (a11y::ui_router); the HAL calls it and the WM's local route (dispatch_local) goes back to the
# HAL. Headless, muted, fresh homes. Run from the workspace root.
cargo build --profile host -p makepad-run --bin makepad || exit 3
T=${CARGO_TARGET_DIR:-target}/host
dir=$(mktemp -d "${TMPDIR:-/tmp}/wm_ui_route.XXXXXX") || exit 1
mkdir -p "$dir/home" "$dir/t"
MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 SANDBOX_MUTE=1 MAKEPAD_HOME="$dir/home" TMPDIR="$dir/t" \
    MAKEPAD_RUN_HOME="$dir/runhome" MAKEPAD_RUN_CACHE_DIR="$dir/cache" MAKEPAD_RUN_TARGET_DIR="$dir/target" \
    "$T/makepad" run --report-to "$dir/report" makepad-app-wm -- --remote > "$dir/run.log" 2>&1 &
pid=$!
for i in $(seq 1 480); do
    grep -q present "$dir/report" 2>/dev/null && break
    kill -0 $pid 2>/dev/null || break
    sleep 0.5
done
sleep 2
ask() { TMPDIR="$dir/t" perl -e 'alarm 60; exec @ARGV' "$T/makepad" inspect $pid "$@" 2>&1; }
fail=""
grep -q present "$dir/report" 2>/dev/null || fail="$fail no-present"
nosuch=$(ask ui app=nosuch-app)
echo "$nosuch" | grep -q "no such hosted app" || fail="$fail wm-route-not-reached"
shell=$(ask ui app=shell)
echo "$shell" | grep -q '"err"' && fail="$fail shell-route-error"
[ -n "$shell" ] || fail="$fail shell-route-empty"
grep -q "lives in a provided crate" "$dir/run.log" && fail="$fail provided-refusal"
kill $pid 2>/dev/null
wait $pid 2>/dev/null
if [ -z "$fail" ]; then
    echo "wm_ui_route: ok"
else
    echo "wm_ui_route: FAILED$fail"
    echo "--- app=nosuch:"; echo "$nosuch" | tail -5; echo "--- app=shell:"; echo "$shell" | tail -5
    echo "--- run.log:"; grep -E "error|lives in|panic" "$dir/run.log" | head -10; tail -15 "$dir/run.log"
    exit 1
fi
