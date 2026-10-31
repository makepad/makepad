#!/bin/bash
# provided_sweep.sh: every app that runs under the JIT reaches only the provided crates' statics and
# fns it may (lower.rs check_provided_*: "lives in a provided crate"). Per app: the boundary audit
# (`makepad check --boundary all`, every body JIT code would compile) and a headless run kept alive
# SECS seconds doing its startup work. Prints each distinct refusal once, then
# "provided_sweep: ok" or "provided_sweep: FAILED <apps>". Run from the workspace root on a node.
# APPS overrides the list; SECS (default 20); AUDIT=0 skips the audits, RUNS=0 the runs; SWEEP_OUT the log dir.
T=${CARGO_TARGET_DIR:-$PWD/target}
APPS=${APPS:-"makepad-fab makepad-stage makepad-scope makepad-sandbox makepad-app-synth makepad-app-calculator makepad-app-wm"}
SECS=${SECS:-20}
export MAKEPAD_RUN_STD=$PWD/libs/relax/compiler/std
nice cargo build --profile host -p makepad-run --bin makepad -j8 > "${TMPDIR:-/tmp}/sweep-build.log" 2>&1 || { tail -20 "${TMPDIR:-/tmp}/sweep-build.log"; echo "BUILD FAILED"; exit 3; }
M="$T/host/makepad"
# the logs stay in the target (`devhub get <lane> target/provided_sweep.out <dir>`); each app's files are rewritten
out=${SWEEP_OUT:-$T/provided_sweep.out}; mkdir -p "$out" || exit 1
failed=""
root=$PWD
# a commercial app runs from apps/commercial (its packages are members there), Sandbox from its own workspace
dir_of() { case "$1" in makepad-sandbox) echo apps/commercial/sandbox;; makepad-fab|makepad-stage|makepad-scope) echo apps/commercial;; *) echo .;; esac; }
for app in $APPS; do
    # a tree without apps/commercial (allapps of work alone) skips the commercial apps, saying so
    [ -f "$root/$(dir_of "$app")/Cargo.toml" ] || { echo "skip $app: no $(dir_of "$app") in this tree"; continue; }
    cd "$root/$(dir_of "$app")" || { failed="$failed $app"; continue; }
    if [ "${AUDIT:-1}" != 0 ]; then
        t0=$SECONDS
        nice "$M" check --boundary all "$app" > "$out/$app.audit" 2>&1
        rc=$?
        echo "audit $app: rc=$rc $(grep -c '^violation' "$out/$app.audit") violation(s) in $((SECONDS - t0)) s"
        [ $rc = 0 ] || failed="$failed $app"
    fi
    if [ "${RUNS:-1}" != 0 ]; then
        mkdir -p "$out/$app.home" "$out/$app.tmp"
        MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 SANDBOX_MUTE=1 MAKEPAD_HOME="$out/$app.home" TMPDIR="$out/$app.tmp" \
            nice "$M" run "$app" > "$out/$app.run" 2>&1 &
        pid=$!
        for _ in $(seq 1 "$SECS"); do kill -0 $pid 2>/dev/null || break; sleep 1; done
        if kill -0 $pid 2>/dev/null; then alive=alive; kill $pid; else alive=exited; fi
        wait $pid 2>/dev/null
        echo "run $app: $alive after ${SECS}s, $(grep -c 'lives in a provided crate' "$out/$app.run") refusal line(s), $(grep -ci 'panicked' "$out/$app.run") panic line(s)"
    fi
    cd "$root"
    if cat "$out/$app".* 2>/dev/null | grep -q "lives in a provided crate"; then failed="$failed $app"; fi
done
echo "--- distinct refusals:"
cat "$out"/*.audit "$out"/*.run 2>/dev/null | grep -o "static \`[^\`]*\` is read from JIT-compiled code\|\`[^\`]*\` calls \`[^\`]*\` in JIT-compiled code\|a field of \`[^\`]*\` is [a-z]*\|\`[^\`]*\` is built in JIT-compiled code\|\`[^\`]*\` is [a-z]* by variant" | sort | uniq -c | sort -rn
echo "--- violation rows (audit) with the way in:"
grep -h -A1 "^violation" "$out"/*.audit 2>/dev/null | cut -c1-400 | head -80
echo "--- panics:"
grep -h -i -A2 "panicked" "$out"/*.run 2>/dev/null | head -40
if [ -z "$failed" ]; then echo "provided_sweep: ok"; else echo "provided_sweep: FAILED$failed"; echo "logs: $out"; exit 1; fi
