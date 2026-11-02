#!/bin/bash
# nocodegen_leg.sh [--stats] [--apps "<package>..."]: makepad run's no-codegen mode (MAKEPAD_RUN_NO_CODEGEN=1:
# no writable+executable memory, every function runs in the threaded interpreter, the constraint of iOS) on an
# arm64 Mac. Run from the workspace root (the JIT runs through `cargo run --profile host`), under a devhub job.
#   1. diff_fast at the `nocodegen` tier: every program's D lines and exit status equal the rustc-built run's
#   2. agree.sh with MAKEPAD_RUN_NO_CODEGEN=1 in the environment (`makepad test` runs the tests interpreted)
#   3. app boots: each package runs until its first frame (MAKEPAD_HAL_SCRIPT `wait_frame 1`), hidden, muted
# --stats: no gate. The same programs, the compiler tests (libs/relax/compiler/tests, `makepad test <file>`) and
# the apps run with MAKEPAD_RUN_INTERP=all MAKEPAD_RUN_INTERP_STATS=1 (the interpreter where it can, machine code
# where it cannot), and the operations it could not run are summed over all runs, most frequent first.
# Prints "NOCODEGEN-OK" or "NOCODEGEN-FAIL: <what>"; exit 0 / 1.
set -u
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0
root=$PWD
J="cargo run --profile host -q --manifest-path $root/Cargo.toml -p makepad-run --bin makepad --"
export MAKEPAD_RUN_STD=${MAKEPAD_RUN_STD:-$root/libs/relax/compiler/std}
apps="makepad-example-counter makepad-app-calculator"
stats=
while [ $# -gt 0 ]; do
    case $1 in
    --stats) stats=1 ;;
    --apps) shift; apps=$1 ;;
    esac
    shift
done
[ "$(uname -m)" = arm64 ] && [ "$(uname -s)" = Darwin ] || { echo "NOCODEGEN-FAIL: needs an arm64 Mac (uname $(uname -sm))"; exit 75; }
work=$(mktemp -d "${TMPDIR:-/tmp}/nocodegen.XXXXXX") || exit 2
echo "toolchain: $(rustc -V)"
if ! cargo build --profile host -j 8 -p makepad-run --bin makepad --bin makepad-launcher > "$work/build" 2>&1; then
    grep -E '^error' -A6 "$work/build" | head -30
    echo "NOCODEGEN-FAIL: makepad build"
    rm -r -- "${work:?}"; exit 1
fi
# one app run until its first frame; $1 package, $2 label, the rest: env
boot() {
    pkg=$1; label=$2; shift 2
    d=$work/boot.$label.$pkg; mkdir -p "$d/home"
    # a short TMPDIR: makepad-remote's socket lives there (sun_path holds 104 bytes)
    t=$(mktemp -d /tmp/ncb.XXXXXX) || return
    printf 'wait_frame 1\n' > "$d/quit.hal"
    t0=$(date +%s)
    env "$@" MAKEPAD_HAL_BACKEND=offscreen MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 SANDBOX_MUTE=1 MUSIC_MUTE=1 \
        MAKEPAD_HOME="$d/home" TMPDIR="$t" MAKEPAD_RUN_HOME="$d/runhome" MAKEPAD_RUN_CACHE_DIR="$d/cache" \
        MAKEPAD_HAL_SCRIPT="$d/quit.hal" perl -e 'alarm 900; exec @ARGV' $J run "$pkg" > "$d/out" 2>&1
    rc=$?
    # the run's session log (its own lines) lives in TMPDIR: kept beside the output
    for l in "$t"/makepad-run-session-*.log; do [ -f "$l" ] && cp "$l" "$d/session.log"; done
    rm -r -- "${t:?}"
    echo "$rc $(( $(date +%s) - t0 ))" > "$d/rc"
}
if [ -n "$stats" ]; then
    # every interp summary line: "makepad-run interp: N fns interpreted (...), M stayed native; <why> xK; ..."
    sweep() { # sweep <label> <file>: makepad test <file> with the interpreter's counts, then without it
        d=$work/s.$1; mkdir -p "$d/home" "$d/tmp"
        env MAKEPAD_RUN_INTERP=all MAKEPAD_RUN_INTERP_STATS=1 MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 SANDBOX_MUTE=1 \
            MAKEPAD_HOME="$d/home" TMPDIR="$d/tmp" MAKEPAD_RUN_HOME="$d/runhome" MAKEPAD_RUN_CACHE_DIR="$d/cache" \
            perl -e 'alarm 600; exec @ARGV' $J test "$2" > "$d/out" 2>&1
        echo $? > "$d/rc"
        # the JIT's own verdict: a file it fails too (a reject test, a gap of its own) is not the interpreter's
        [ "$(cat "$d/rc")" = 0 ] && return
        env MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 SANDBOX_MUTE=1 \
            MAKEPAD_HOME="$d/home" TMPDIR="$d/tmp" MAKEPAD_RUN_HOME="$d/runhome" MAKEPAD_RUN_CACHE_DIR="$d/cache.jit" \
            perl -e 'alarm 600; exec @ARGV' $J test "$2" > "$d/out.jit" 2>&1
        echo $? > "$d/rc.jit"
    }
    n=0
    for f in tools/run/fixture/diff/*.rs libs/relax/compiler/tests/*.rs; do
        n=$((n + 1)); sweep "$n.$(basename "$f" .rs)" "$f" &
        [ $((n % 6)) = 0 ] && wait
    done
    wait
    for pkg in $apps; do boot "$pkg" stats MAKEPAD_RUN_INTERP=all MAKEPAD_RUN_INTERP_STATS=1; done
    # the last summary of each run (the one printed at exit), summed
    for o in "$work"/s.*/out "$work"/boot.stats.*/out; do grep -a 'makepad-run interp:' "$o" | tail -1; done > "$work/lines"
    echo "interp stats: $(wc -l < "$work/lines" | tr -d ' ') runs with a summary of $n test files + $(echo $apps | wc -w | tr -d ' ') apps"
    awk '{
        if (match($0, /: [0-9]+ fns interpreted/)) { split(substr($0, RSTART + 2), a, " "); fi += a[1] }
        if (match($0, /, [0-9]+ stayed native/)) { split(substr($0, RSTART + 2), a, " "); fn += a[1] }
        n = split($0, parts, "; ")
        for (i = 2; i <= n; i++) { k = parts[i]; c = k; sub(/ x[0-9]+$/, "", k); sub(/.* x/, "", c); why[k] += c }
    } END {
        printf "interpreted %d fns, stayed native %d\n", fi, fn
        for (k in why) printf "%8d  %s\n", why[k], k | "sort -rn"
    }' "$work/lines"
    for pkg in $apps; do
        l=$(grep -a 'makepad-run interp:' "$work/boot.stats.$pkg/out" | tail -1)
        echo "app $pkg (rc/s $(cat "$work/boot.stats.$pkg/rc")): ${l:-no summary: $(grep -a -m2 -i 'panic\|error' "$work/boot.stats.$pkg/out" | cut -c1-200)}"
    done
    echo "functions refused (the first of each reason per run):"
    cat "$work"/s.*/out "$work"/boot.stats.*/out | grep -a 'makepad-run interp refused:' | sed 's/^.*interp refused: //' | sort | uniq -c | sort -rn | head -60
    both=0; only=0
    for d in "$work"/s.*; do
        [ "$(cat "$d/rc")" = 0 ] && continue
        if [ "$(cat "$d/rc.jit")" != 0 ]; then both=$((both + 1)); continue; fi
        only=$((only + 1))
        echo "  interpreter only: ${d##*/s.} (rc $(cat "$d/rc"))"
        grep -a -A3 -m1 'panicked\|error\|FAILED\|Abort\|cannot run' "$d/out" | cut -c1-240 | sed 's/^/      /'
    done
    echo "test files failing with the interpreter on: $only (and $both that fail without it too)"
    rm -r -- "${work:?}"
    exit 0
fi
fail=
# inline assembly runs only as machine code: in this mode a function with an asm! block is a
# compile error naming it, so the programs and cases that exercise asm are left out
progs=$(cd tools/run/fixture/diff && ls *.rs | sed 's/\.rs$//' | grep -v '^asm_')
DIFF_PROGRAMS="$progs" DIFF_TIERS=nocodegen DIFF_PKG_DIRS=none tools/run/fixture/diff_fast.sh > "$work/diff" 2>&1
grep -a -A4 'at tier nocodegen\|HARNESS' "$work/diff" | cut -c1-240 | head -80
d=$(grep '^diff: ' "$work/diff" | tail -1)
echo "DIFF: $d"
echo "$d" | grep -qE '^diff: ([0-9]+)/\1 equal' || fail="$fail diff_fast"
AGREE_SKIP="asm_*" MAKEPAD_RUN_NO_CODEGEN=1 AGREE_DIR=$work/agree bash tools/run/fixture/agree.sh > "$work/agree.log" 2>&1 || fail="$fail agree"
grep -a -B12 'agree FAILED\|agree: ok' "$work/agree.log" | cut -c1-240
for pkg in $apps; do
    boot "$pkg" nc MAKEPAD_RUN_NO_CODEGEN=1
    read -r rc secs < "$work/boot.nc.$pkg/rc"
    echo "boot $pkg: rc $rc in ${secs}s"
    if [ "$rc" != 0 ]; then
        grep -a -v '^\s*$' "$work/boot.nc.$pkg/out" | tail -15 | cut -c1-240
        # the run's own lines (makepad run writes them to its session log)
        [ -f "$work/boot.nc.$pkg/session.log" ] && { echo "--- session log"; tail -25 "$work/boot.nc.$pkg/session.log" | cut -c1-240; }
        fail="$fail boot:$pkg"
    fi
done
rm -r -- "${work:?}"
if [ -n "$fail" ]; then echo "NOCODEGEN-FAIL:$fail"; exit 1; fi
echo "NOCODEGEN-OK"
