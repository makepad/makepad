#!/bin/sh
# diff_full.sh: the Relax-vs-rustc differential net, FULL tier. On demand or nightly on node 165 (Linux) only;
# never in the landing gate (use diff_fast.sh there). Hidden, headless runs; every run has its own deleted home.
#   corpus   every libs/relax/compiler/tests/*.rs (the files that rustc --test -O builds) runs natively and under
#            `makepad test` at the baseline and O3 tiers; stdout (minus harness lines, sorted: test order differs) must match. Tests that
#            print timings or addresses show up as "differs": triage by reading the diff, the list is the net.
#   replay   tools/run/fixture/replay/calculator.txt (clicks, text, frames) drives the Calculator built natively
#            and under `makepad run` (O3 forced on with call threshold 1); the state dumps (`snap all=1` after each
#            checkpoint: display, tape, widget tree with rects and text) must be identical.
# Usage: tools/run/fixture/diff_full.sh [corpus|replay]      (default both; replay-only on a machine without rustc deps)
# Env: TARGET (target/release), DIFF_CORPUS (dir of *.rs tests, default libs/relax/compiler/tests), DIFF_CACHE, DIFF_JOBS (default 8), DIFF_NATIVE_CALC (prebuilt native calculator;
#      else cargo build --release -p makepad-app-calculator -j4 runs first), DIFF_FULL_ANYWAY=1 (allow a non-Linux host).
# Output: `full: corpus E/N equal (S skipped), replay equal|DIFFERS`; the diffs follow; exit 1 on any difference.
if [ "$(uname)" != Linux ] && [ -z "$DIFF_FULL_ANYWAY" ]; then echo "full: node 165 (Linux) only; DIFF_FULL_ANYWAY=1 overrides"; exit 2; fi
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
T=${TARGET:-$root/target/release}
case $T in /*) ;; *) T=$PWD/$T ;; esac
M=$T/makepad
[ -x "$M" ] || { echo "full: no $M (cargo build --release -p makepad-run)"; exit 2; }
cache=${DIFF_CACHE:-${TMPDIR:-/tmp}/diff_full_cache}
mkdir -p "$cache"
what=${1:-all}
corpus=${DIFF_CORPUS:-$root/libs/relax/compiler/tests}
export M here root cache corpus
if [ "$1" = "--corpus-job" ]; then
    # $2 work dir, $3 test file: native build (cached), native run, two tiers
    work=$2; src=$3; n=$(basename "$src" .rs)
    h=$(printf '%s' "$(rustc -V)" | cat - "$src" | cksum | cut -d' ' -f1)
    bin=$cache/$n.$h
    if [ ! -x "$bin" ]; then
        nice -n 10 rustc --edition 2021 -O --test "$src" -o "$bin.tmp" > /dev/null 2>&1 || { echo skip > "$work/$n.skip"; exit 0; }
        mv "$bin.tmp" "$bin"
    fi
    norm='s/^test [A-Za-z0-9_:]* \.\.\. //; /^running [0-9]* tests*$/d; /^test result:/d; /^boot:/d; /^  report:/d; /^inlined call sites/d; /^$/d; /^\[/d; /^thread /d; /^note: /d; s/ \([0-9.]+ ms\)$//'
    TO=; command -v timeout > /dev/null && TO="timeout 300"
    d=$(mktemp -d "${TMPDIR:-/tmp}/dfc.XXXXXX"); mkdir -p "$d/home" "$d/tmp"
    ( cd "$(dirname "$src")" && "$bin" --nocapture --test-threads=1 2>/dev/null | sed -E "$norm" | sort > "$work/$n.native" )
    for k in base o3; do
        if [ $k = base ]; then tier="MAKEPAD_RUN_O3=0"; else tier="MAKEPAD_RUN_O3=1"; fi
        ( cd "$(dirname "$src")" && env $tier MAKEPAD_RUN_O3_CALLS=1 MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 MAKEPAD_HOME="$d/home" TMPDIR="$d/tmp" \
            MAKEPAD_RUN_HOME="$d/runhome" MAKEPAD_RUN_CACHE_DIR="$d/cache" MAKEPAD_RUN_STD="$root/libs/relax/compiler/std" \
            nice -n 10 $TO "$M" test "$src" 2>/dev/null | sed -E "$norm" | sort > "$work/$n.$k" )
    done
    rm -r "$d"
    exit 0
fi
corpus_ok=0; corpus_n=0; corpus_skip=0; msgs=; replay=skipped
if [ $what = all ] || [ $what = corpus ]; then
    work=$(mktemp -d "${TMPDIR:-/tmp}/diff_full.XXXXXX")
    ls "$corpus"/*.rs | xargs -P "${DIFF_JOBS:-8}" -n 1 "$0" --corpus-job "$work"
    for src in "$corpus"/*.rs; do
        n=$(basename "$src" .rs)
        if [ -f "$work/$n.skip" ]; then corpus_skip=$((corpus_skip + 1)); continue; fi
        corpus_n=$((corpus_n + 1)); bad=
        for k in base o3; do
            cmp -s "$work/$n.native" "$work/$n.$k" || bad="$bad
$n at tier $k:
$(diff "$work/$n.native" "$work/$n.$k" | head -6 | cut -c1-200)"
        done
        if [ -z "$bad" ]; then corpus_ok=$((corpus_ok + 1)); else msgs="$msgs$bad"; fi
    done
    rm -r "$work"
fi
if [ $what = all ] || [ $what = replay ]; then
    nat=${DIFF_NATIVE_CALC:-$T/makepad-app-calculator}
    # always built from this checkout (a no-op when current): a stale native binary shows up as widget-tree noise
    [ -n "$DIFF_NATIVE_CALC" ] || (cd "$root" && CARGO_INCREMENTAL=0 nice -n 10 cargo build --release -p makepad-app-calculator -j4 > /dev/null 2>&1)
    out=$(mktemp -d /tmp/dfr.XXXXXX)
    for side in native jit; do
        d=$out/$side; mkdir -p "$d/home" "$d/tmp"
        if [ $side = native ]; then cmd="$nat"; set -- "$nat"; else set -- "$M" run makepad-app-calculator; fi
        MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 SANDBOX_MUTE=1 MUSIC_MUTE=1 MAKEPAD_REMOTE=1 MAKEPAD_HOME="$d/home" TMPDIR="$d/tmp" \
            MAKEPAD_RUN_HOME="$d/runhome" MAKEPAD_RUN_CACHE_DIR="$d/cache" MAKEPAD_WINDOW=1280x800@1 MAKEPAD_RUN_O3=1 MAKEPAD_RUN_O3_CALLS=1 \
            nice -n 10 "$@" > "$d/log" 2>&1 &
        pid=$!
        n=0
        until TMPDIR="$d/tmp" "$M" inspect $pid settled > /dev/null 2>&1; do
            n=$((n + 1)); [ $n -ge 600 ] && break
            kill -0 $pid 2>/dev/null || break
            sleep 0.2
        done
        TMPDIR="$d/tmp" "$M" inspect $pid dump > "$d/tree0" 2>&1
        : > "$d/state"
        while read -r verb arg; do
            case $verb in
            '#'*|'') ;;
            step) TMPDIR="$d/tmp" "$M" inspect $pid step frames=$arg wait_loads=1 > /dev/null 2>&1 ;;
            text) TMPDIR="$d/tmp" "$M" inspect $pid key t="$arg" wait=1 > /dev/null 2>&1 ;;
            click)
                xy=$(awk -v n="$arg" '$3 == n { printf "x=%d y=%d", $5 + $7 / 2, $6 + $8 / 2 }' "$d/tree0")
                TMPDIR="$d/tmp" "$M" inspect $pid click $xy wait=1 > /dev/null 2>&1 ;;
            snap) echo "-- snap" >> "$d/state"; TMPDIR="$d/tmp" "$M" inspect $pid snap all=1 2>&1 | sed 's/},{/\
/g' >> "$d/state" ;;
            esac
        done < "$here/replay/calculator.txt"
        TMPDIR="$d/tmp" "$M" inspect $pid quit > /dev/null 2>&1
        sleep 1; kill $pid 2>/dev/null; wait $pid 2>/dev/null
    done
    if [ -s "$out/native/state" ] && cmp -s "$out/native/state" "$out/jit/state"; then replay=equal
    else
        replay=DIFFERS
        msgs="$msgs
replay: Calculator state dumps differ (native vs makepad run):
$(diff "$out/native/state" "$out/jit/state" | head -10 | cut -c1-300)"
    fi
    keep=$out; [ $replay = equal ] && rm -r "$out" && keep=
    [ -n "$keep" ] && msgs="$msgs
replay logs kept in $keep"
fi
printf '%s\n' "$msgs"
echo "full: corpus $corpus_ok/$corpus_n equal ($corpus_skip skipped), replay $replay"
[ $corpus_ok = $corpus_n ] && [ $replay != DIFFERS ]
