#!/bin/sh
# A HAL-only change keeps the stored builds (JITCOLD): makepad-hal is a provided crate (the loader
# compiles it in), so a HAL edit rebuilds the loader, and the code above HAL must stay stored.
#   session 1 (cold): a fresh run home: launch hidden + muted to the first present and JIT idle, quit;
#   edit:             a comment into the body of one HAL fn (platform/hal/src/lib.rs, Hal::backend_inits),
#                     rebuild the loader the way the caller built it (BUILD);
#   session 2:        the same launch on the same run home: its first-frame line must say "cache warm"
#                     (every pack accepted, the startup manifest valid) and the JIT spinner never shows;
#   then the file is restored and the loader rebuilt (also on failure or interrupt).
# Before the fix every crate that could reach HAL had its code keyed by HAL's whole-source identity
# (cache/items.rs apply_bound), so this edit recompiled nearly the whole program cold.
# Run from the workspace root after the loader is built. PACKAGE (default Calculator standalone; a
# `makepad run` argument list with ',' for spaces), WAIT (seconds, default 300), TARGET (default
# target/release), BUILD (default: cargo build --release -p makepad-run --bin makepad; give the
# same -p set as the build that made the binary, or cargo rebuilds for other features).
# Prints "warm_after_hal: OK ..." or "warm_after_hal: FAIL ..."; exit 1 on failure.
T=${TARGET:-target/release}
B=$(cd "$(dirname "$T/makepad")" && pwd)/makepad
PACKAGE=${PACKAGE:-makepad-app-calculator,--no-default-features,--features,standalone}
WAIT=${WAIT:-300}
BUILD=${BUILD:-cargo build --release -p makepad-run --bin makepad}
F=platform/hal/src/lib.rs
grep -q '^    pub fn backend_inits(&self) -> u32 {$' "$F" || { echo "warm_after_hal: FAIL: $F has no Hal::backend_inits to edit"; exit 1; }
dir=$(mktemp -d "${TMPDIR:-/tmp}/warm_after_hal.XXXXXX") || exit 1
mkdir -p "$dir/home" "$dir/t"
cp "$F" "$dir/orig.rs" || exit 1
rebuild() { # (word splitting of $BUILD is the point: one cargo argument per word)
    CARGO_INCREMENTAL=0 nice -n 10 $BUILD > "$dir/build.$1" 2>&1 || { echo "warm_after_hal: FAIL: rebuild ($1), see $dir/build.$1"; return 1; }
}
restore() {
    if ! cmp -s "$dir/orig.rs" "$F"; then
        cp "$dir/orig.rs" "$F.restore" && mv "$F.restore" "$F" && rebuild restore
    fi
}
trap 'restore; exit 1' INT TERM
session() { # session <label>: one launch to idle and quit; prints its first-frame line and its `jit` session line
    label=$1
    MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 SANDBOX_MUTE=1 MUSIC_MUTE=1 MAKEPAD_REMOTE=1 TMPDIR="$dir/t" \
        MAKEPAD_HOME="$dir/home" MAKEPAD_RUN_HOME="$dir/run" MAKEPAD_RUN_CACHE_DIR="$dir/run/cache" MAKEPAD_RUN_TARGET_DIR="$dir/run/target" \
        MAKEPAD_RUN_VERBOSE=1 "$B" run --report-to "$dir/report.$label" $(echo "$PACKAGE" | tr ',' ' ') > "$dir/log.$label" 2>&1 &
    pid=$!
    i=0
    while [ $i -lt $((WAIT * 5)) ] && kill -0 $pid 2>/dev/null && ! grep -qx present "$dir/report.$label" 2>/dev/null; do
        sleep 0.2; i=$((i + 1))
    done
    idle=0; i=0; last=""
    while [ $idle -lt 3 ] && [ $i -lt $WAIT ] && kill -0 $pid 2>/dev/null; do
        last=$(TMPDIR="$dir/t" "$B" inspect $pid jit ms=1000 2>&1)
        if echo "$last" | grep -q '^idle'; then idle=$((idle + 1)); else idle=0; fi
        i=$((i + 1))
    done
    kill -TERM $pid 2>/dev/null
    wait $pid
    grep -m1 ' first frame at ' "$dir/log.$label" | sed 's/^.* first frame/first frame/'
    echo "$last" | grep '^session:'
}
cold=$(session cold)
awk '{ print } /^    pub fn backend_inits\(&self\) -> u32 \{$/ { print "        // warm_after_hal probe: a body-only HAL edit" }' "$dir/orig.rs" > "$F.probe" && mv "$F.probe" "$F" || { restore; exit 1; }
rebuild edit || { restore; exit 1; }
hal=$(session hal)
restore || exit 1
first=$(echo "$hal" | grep '^first frame')
spin=$(echo "$hal" | sed -n 's/.*spinner \([0-9]*\).*/\1/p')
what="after the HAL edit: $(echo "$hal" | tr '\n' ' ')(cold: $(echo "$cold" | tr '\n' ' '))"
if echo "$first" | grep -q 'cache warm' && [ "$spin" = 0 ]; then
    echo "warm_after_hal: OK $what"
    [ "${KEEP:-0}" != 1 ] && rm -rf "$dir"
    exit 0
fi
echo "warm_after_hal: FAIL $what; logs in $dir"
exit 1
