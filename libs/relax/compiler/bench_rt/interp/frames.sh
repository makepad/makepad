#!/bin/bash
# frames.sh [<package>...]: an app's start and UI frame times under the JIT and in no-codegen mode
# (every function interpreted, the iOS constraint), from the workspace root on a quiet arm64 Mac (a
# devhub timed job on the mini). Per package and mode: a cold run (an empty code cache) and a warm
# one (the same cache again), each to its first frame and then through resize30.hal (10 idle
# frames, 30 resizes, each followed by a frame), hidden on the offscreen backend. Prints the
# seconds each run took and, of the warm run, the Cx build time of the idle and the resize frames
# (median / p90 / max, MAKEPAD_HAL_FRAME_LOG). Default package: makepad-app-calculator.
# FRAMES_PHASES=1 also prints the warm run's boot phases; FRAMES_PROFILE=1 samples its main thread.
set -u
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0
pkgs=${*:-makepad-app-calculator}
here=libs/relax/compiler/bench_rt/interp
cargo build --profile host -q -p makepad-run --bin makepad || { echo "FRAMES-FAIL: makepad build"; exit 1; }
T=${CARGO_TARGET_DIR:-$PWD/target}
work=$(mktemp -d /tmp/frames.XXXXXX) || exit 2
stats() { # stats <label> <file of build_us values>
    sort -n "$2" | awk -v l="$1" '{ v[NR] = $1 } END {
        if (NR == 0) { printf "  %s: no frames\n", l; exit }
        printf "  %s: %d frames, median %.0f us, p90 %.0f us, max %.0f us\n", l, NR, v[int((NR + 1) / 2)], v[int(NR * 0.9 + 0.5) > 0 ? int(NR * 0.9 + 0.5) : 1], v[NR]
    }'
}
for pkg in $pkgs; do
    for mode in jit nocodegen; do
        env=(MAKEPAD_HAL_BACKEND=offscreen MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 SANDBOX_MUTE=1 MAKEPAD_HAL_FRAME_LOG=1 MAKEPAD_HAL_SCRIPT=$PWD/$here/resize30.hal)
        [ $mode = nocodegen ] && env+=(MAKEPAD_RUN_NO_CODEGEN=1)
        # FRAMES_PHASES=1: the warm run's boot phases (MAKEPAD_RUN_VERBOSE's timed lines)
        [ "${FRAMES_PHASES:-}" = 1 ] && env+=(MAKEPAD_RUN_VERBOSE=1)
        d=$work/$pkg.$mode; mkdir -p "$d/home" "$d/tmp"
        for run in cold warm; do
            t0=$(perl -MTime::HiRes=time -e 'printf "%.3f", time')
            prof=()
            # FRAMES_PROFILE=1: the warm run's main thread sampled (mainprof.rs), its leaves summed
            [ "${FRAMES_PROFILE:-}" = 1 ] && [ $run = warm ] && prof=(MAKEPAD_RUN_PROFILE_MAIN=$d/prof.tsv MAKEPAD_RUN_PROFILE_MAIN_MS=8000)
            env "${env[@]}" ${prof[@]+"${prof[@]}"} MAKEPAD_HOME="$d/home" TMPDIR="$d/tmp" MAKEPAD_RUN_HOME="$d/runhome" MAKEPAD_RUN_CACHE_DIR="$d/cache" \
                perl -e 'alarm 900; exec @ARGV' "$T/host/makepad" run "$pkg" > "$d/$run.out" 2>&1
            rc=$?
            t1=$(perl -MTime::HiRes=time -e 'printf "%.3f", time')
            echo "$pkg $mode $run: rc $rc, $(echo "$t1 - $t0" | bc) s"
        done
        # the warm run: the 10 idle frames after the first, then the resize frames
        sed -n 's/^hal-frame \([0-9]*\) build_us \([0-9.]*\)$/\1 \2/p' "$d/warm.out" > "$d/frames"
        awk 'NR >= 2 && NR <= 11 { print $2 }' "$d/frames" > "$d/idle"
        awk 'NR > 11 { print $2 }' "$d/frames" > "$d/resize"
        stats "$mode idle" "$d/idle"
        stats "$mode resize" "$d/resize"
        [ "${FRAMES_PHASES:-}" = 1 ] && grep -a -E 'makepad-run: .*[0-9.]+ ms|boot-trace|first (frame|present)' "$d/warm.out" | head -40 | cut -c1-200
        if [ -s "$d/prof.tsv" ]; then
            echo "  $mode main thread, warm: $(wc -l < "$d/prof.tsv" | tr -d ' ') samples; by role: $(cut -f4 "$d/prof.tsv" | sort | uniq -c | sort -rn | tr '\n' ' ')"
            cut -f2 "$d/prof.tsv" | sort | uniq -c | sort -rn | head -25 | cut -c1-180
            echo "  by first JIT frame:"; cut -f3 "$d/prof.tsv" | sort | uniq -c | sort -rn | head -15 | cut -c1-180
        fi
        [ -s "$d/frames" ] || grep -a -v '^\s*$' "$d/warm.out" | tail -5 | cut -c1-200
    done
done
rm -r -- "${work:?}"
echo "FRAMES-DONE"
