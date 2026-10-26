#!/bin/sh
# Relax's stored builds are keyed by the codegen identity (relax-compiler src/codegen_id.rs), not by the
# whole binary. On one run home and one app home of PACKAGE:
#   1. baseline: a cold session, then a warm one;
#   2. a change outside the code generator (a devhub string, libs/devhub): rebuild makepad. The binary differs,
#      the codegen id does not, and the next warm session links the stored code: no stored entry missed
#      (cache lookups linked + startup image slots placed >= 90% of the baseline warm session's; no more entries
#      missed as recorded under another key or gone than the baseline warm session (MAKEPAD_RUN_WARM_STRICT
#      ledger); first compiles under a quarter of the cold session's);
#   3. a change of an RIR pass (libs/relax/codegen version.rs MAX_LOOPS): rebuild. The codegen id changes, the
#      next session uses no stored code of the old code generator (no lookup linked, no startup image slot; first
#      compiles at least 3/4 of the cold session's) and the app presents.
# The edits are restored (and makepad rebuilt) on exit. Run from the workspace root on a pool node (it rebuilds
# makepad three times; never on the user's Mac). Each session: launch hidden + muted, first present, JIT idle
# (3 idle `inspect jit` looks in a row), `inspect cache`, quit (SIGTERM).
# PACKAGE (default "makepad-app-calculator,--no-default-features,--features,standalone"), WAIT (default 300).
# Prints "codegen_id: OK ..." or "codegen_id: FAIL ..."; exit 1 on failure.
T=${CARGO_TARGET_DIR:-$PWD/target}/release
B=$T/makepad
PACKAGE=${PACKAGE:-makepad-app-calculator,--no-default-features,--features,standalone}
WAIT=${WAIT:-300}
DEVHUB=libs/devhub/src/cli.rs
PASS=libs/relax/codegen/src/version.rs
dir=$(mktemp -d "${TMPDIR:-/tmp}/codegen_id.XXXXXX") || exit 1
cp "$DEVHUB" "$dir/devhub.orig"; cp "$PASS" "$dir/pass.orig"
build() { CARGO_INCREMENTAL=0 cargo build --release -p makepad-run --bin makepad -j8 > "$dir/build.log" 2>&1 || { tail -20 "$dir/build.log"; echo "codegen_id: FAIL build"; exit 1; }; }
restore() {
    cmp -s "$dir/devhub.orig" "$DEVHUB" || cp "$dir/devhub.orig" "$DEVHUB"
    cmp -s "$dir/pass.orig" "$PASS" || { cp "$dir/pass.orig" "$PASS"; build; }
}
trap restore EXIT
id() { MAKEPAD_RUN_CACHE_FP=1 MAKEPAD_RUN_VERBOSE=1 "$B" check makepad-app-calculator 2>&1 | sed -n 's/.*codegen id \([0-9a-f]*\).*/\1/p' | head -1; }
args=$(echo "$PACKAGE" | tr ',' ' ')
mkdir -p "$dir/home" "$dir/t"
session() { # session <label>: one launch to idle and quit; prints its `jit` session line and its `cache` line
    label=$1
    MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 SANDBOX_MUTE=1 MUSIC_MUTE=1 MAKEPAD_REMOTE=1 TMPDIR="$dir/t" MAKEPAD_RUN_WARM_STRICT=1 \
        MAKEPAD_HOME="$dir/home" MAKEPAD_RUN_HOME="$dir/run" MAKEPAD_RUN_CACHE_DIR="$dir/run/cache" MAKEPAD_RUN_TARGET_DIR="$dir/run/target" \
        MAKEPAD_RUN_VERBOSE=1 "$B" run --report-to "$dir/report.$label" $args > "$dir/log.$label" 2>&1 &
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
    cache=$(TMPDIR="$dir/t" "$B" inspect $pid cache 2>&1 | grep '^warm cache:')
    kill -TERM $pid 2>/dev/null
    wait $pid
    grep -qx present "$dir/report.$label" 2>/dev/null && echo "present" || echo "NOT PRESENT"
    echo "$last" | grep '^session:'
    echo "$cache"
}
num() { sed 's/\.\([0-9]\)k$/\100/; s/k$/000/'; }
# stored code a session used: cache lookups linked + startup image slots placed
hits() { l=$(sed -n 's/^warm cache: \([0-9.k]*\) lookups linked.*/\1/p' | num); echo "$1" | sed -n 's/.*startup image: \([0-9.k]*\) slots placed.*/\1/p' | num | { read p; echo $(( ${l:-0} + ${p:-0} )); }; }
# stored entries missed because they were recorded under another key or are gone (strict ledger)
ledger() { sed -n 's/.*recorded under another key \([0-9.k]*\), recorded but gone \([0-9.k]*\),.*/\1 \2/p' | { read a b; echo $(( $(echo ${a:-0} | num) + $(echo ${b:-0} | num) )); }; }
first() { sed -n 's/.*session: compiled \([0-9]*\) first.*/\1/p'; }
fail=""
build
b0=$(sha256sum "$B" | cut -c1-16); id0=$(id)
cold=$(session cold); warm=$(session warm)
echo "baseline: codegen id $id0, binary $b0"; echo "  cold: $cold" | tr '\n' ' '; echo; echo "  warm: $warm" | tr '\n' ' '; echo
# 2. outside the code generator
sed 's/this machine.s JITs only/this machine'"'"'s JITs only (codegen_id fixture)/' "$dir/devhub.orig" > "$DEVHUB"
cmp -s "$dir/devhub.orig" "$DEVHUB" && fail="$fail devhub-edit-not-applied"
build
b1=$(sha256sum "$B" | cut -c1-16); id1=$(id)
[ "$b1" != "$b0" ] || fail="$fail devhub-edit-left-the-binary-unchanged"
[ -n "$id1" ] && [ "$id1" = "$id0" ] || fail="$fail codegen-id-changed-by-devhub($id0->$id1)"
dev=$(session after-devhub)
echo "after a devhub change: codegen id $id1, binary $b1"; echo "  warm: $dev" | tr '\n' ' '; echo
echo "$dev" | grep -qx present || fail="$fail devhub-not-present"
h0=$(echo "$warm" | hits "$warm"); h1=$(echo "$dev" | hits "$dev")
[ $((h1 * 10)) -ge $((h0 * 9)) ] || fail="$fail devhub-stored-code-used($h1<90%of$h0)"
l0=$(echo "$warm" | ledger); l=$(echo "$dev" | ledger)
[ "$l" -le "$l0" ] || fail="$fail devhub-recompiled-stored(key-changed+gone $l > baseline warm $l0)"
f0=$(echo "$warm" | first); f1=$(echo "$dev" | first); fc=$(echo "$cold" | first)
[ -n "$f1" ] && [ $((f1 * 4)) -lt $((fc))  ] || fail="$fail devhub-compiled-like-cold($f1 of cold $fc)"
cp "$dir/devhub.orig" "$DEVHUB"
# 3. an RIR pass
sed 's/^const MAX_LOOPS: usize = 4;/const MAX_LOOPS: usize = 3;/' "$dir/pass.orig" > "$PASS"
cmp -s "$dir/pass.orig" "$PASS" && fail="$fail pass-edit-not-applied"
build
id2=$(id)
[ -n "$id2" ] && [ "$id2" != "$id0" ] || fail="$fail codegen-id-unchanged-by-pass-edit"
cg=$(session after-pass)
echo "after an RIR pass change: codegen id $id2"; echo "  warm: $cg" | tr '\n' ' '; echo
echo "$cg" | grep -qx present || fail="$fail pass-not-present"
h2=$(echo "$cg" | hits "$cg"); f2=$(echo "$cg" | first)
[ "$h2" = 0 ] || fail="$fail pass-change-reused-old-code($h2 stored fns used)"
[ -n "$f2" ] && [ $((f2 * 4)) -ge $((fc * 3)) ] || fail="$fail pass-change-not-a-full-compile($f2 of cold $fc)"
if [ -z "$fail" ]; then
    echo "codegen_id: OK (devhub change: id kept, stored code used $h1 vs baseline warm $h0, first compiles $f1 vs warm $f0 / cold $fc, key-changed+gone $l vs $l0; pass change: new id, 0 stored fns used, $f2 first compiles)"
    [ "${KEEP:-0}" = 1 ] || rm -rf "$dir"
else
    echo "codegen_id: FAIL$fail (logs in $dir)"
    exit 1
fi
