#!/bin/sh
# Regression test: makepad inspect names the code version every frame, probe and pin is about
# (relax-provided-fixture's relax-inspect-tiers bin, hidden and muted, fresh homes): `leaf` is
# inlined into the hot `outer`, which tiers up to O3.
# - versions: outer's O3 code, leaf's inlined copy in it;
# - count leaf: outer is rebuilt at O3 (same tier) without the copy, the count moves, `probes`
#   lists the versions, `rm` says which code is back;
# - tier leaf o1 / auto: outer runs O1, then its O3 code again; tier outer o3 fresh=1: a new O3 build;
# - a bounds panic in `pick` inlined into outer's O3 code: the frame names pick, its inline stack
#   (pick at its line, inlined at outer's call line) and each level's locals.
# Run from the workspace root after `cargo build --release -p makepad-run`. TARGET (default
# target/release). Prints one PASS/FAIL line per check and a last line `inspect_tiers: N/M passed`.
T=${TARGET:-target/release}
dir=$(mktemp -d "${TMPDIR:-/tmp}/inspect_tiers.XXXXXX") || exit 1
mkdir -p "$dir/home" "$dir/t"
MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 SANDBOX_MUTE=1 MAKEPAD_HOME="$dir/home" TMPDIR="$dir/t" \
    MAKEPAD_RUN_HOME="$dir/runhome" MAKEPAD_RUN_CACHE_DIR="$dir/cache" MAKEPAD_RUN_PANIC_PARK=600 \
    PROBE_TRIGGER="$dir/trigger" PROBE_SECS=300 \
    "$T/makepad" run relax-provided-fixture --bin relax-inspect-tiers > "$dir/run.log" 2>&1 &
pid=$!
ask() { TMPDIR="$dir/t" "$T/makepad" inspect $pid "$@" as=json 2>&1; }
n=0
until ask ping > /dev/null 2>&1; do
    n=$((n + 1)); [ $n -ge 600 ] && break
    kill -0 $pid 2>/dev/null || break
    sleep 0.2
done
pass=0; total=0
check() {
    total=$((total + 1))
    [ -n "$VERBOSE" ] && printf '%s\n' "$2" | cut -c1-900 | sed 's/^/  | /'
    if printf '%s\n' "$2" | grep -q -- "$3"; then pass=$((pass + 1)); echo "PASS $1"; else echo "FAIL $1"; printf '%s\n' "$2" | head -12 | cut -c1-600 | sed 's/^/    /'; fi
}
# outer tiers up (hot loop): wait for its O3 code
n=0
until ask versions relax_inspect_tiers::outer | grep -q '"version":{"slot":[0-9]*,"fn":"[^"]*","tier":"O3"'; do
    n=$((n + 1)); [ $n -ge 120 ] && break
    sleep 0.5
done
r=$(ask versions relax_inspect_tiers::outer); check "versions: outer runs O3 code, installed" "$r" '"tier":"O3","origin":"fresh","code":"0x[0-9a-f]*+0x[0-9a-f]*","state":"installed"'
r=$(ask versions relax_inspect_tiers::leaf); check "versions: leaf's inlined copy in outer's code" "$r" '"inlined_into":{"slot":[0-9]*,"fn":"relax_inspect_tiers::outer","tier":"O3"'
r=$(ask count relax_inspect_tiers::leaf); check "count leaf: outer rebuilt at O3 without the copy" "$r" '"inliner":"relax_inspect_tiers::outer".*"now":{[^}]*"tier":"O3","origin":"inspect"'
check "count leaf: the row names the O3 code it replaced" "$r" '"inliner":"relax_inspect_tiers::outer".*"replaced":{[^}]*"tier":"O3","origin":"fresh"'
sleep 1
r=$(ask probes); check "probes: leaf counts" "$r" '"fn":"relax_inspect_tiers::leaf","hits":[1-9]'
check "probes: lists outer as an inliner on G1" "$r" '"on":"G1".*"role":"inliner"'
r=$(ask rm all); check "rm: outer's O3 code is back" "$r" '"back":{[^}]*"fn":"relax_inspect_tiers::outer","tier":"O3","origin":"fresh"'
r=$(ask tier relax_inspect_tiers::leaf o1); check "tier leaf o1: outer pinned at O1" "$r" '"inliner":"relax_inspect_tiers::outer".*"now":{[^}]*"tier":"O1","origin":"inspect"'
r=$(ask versions relax_inspect_tiers::outer); check "versions: outer's installed code is the O1 pin" "$r" '"tier":"O1","origin":"inspect","code":"[^"]*","state":"installed"'
r=$(ask tier relax_inspect_tiers::leaf auto); check "tier leaf auto: outer's O3 code is back" "$r" '"unpinned":"pin[0-9]*".*"tier":"O3","origin":"fresh"'
r=$(ask tier relax_inspect_tiers::outer o3 fresh=1); check "tier outer o3 fresh=1: a new O3 build in place" "$r" '"o3":"relax_inspect_tiers::outer","slot":[0-9]*,"installed":true,"replaced":{[^}]*"tier":"O3"'
r=$(ask stacks thread=main); check "stacks: JIT frames carry their code version" "$r" '"code":{"slot":[0-9]*,"tier":"'
touch "$dir/trigger"
n=0
until ask panics | grep -q '"id":"P'; do n=$((n + 1)); [ $n -ge 100 ] && break; sleep 0.2; done
r=$(ask events on=panic); check "panic: the frame is pick, inlined at its call site in outer's O3 code" "$r" '"fn":"relax_inspect_tiers::pick","at":"[^"]*inspect_tiers.rs:14:[0-9]*","in":"jit/O3 fresh inlined at [^"]*inspect_tiers.rs:26:[0-9]* in relax_inspect_tiers::outer"'
check "panic: the inline stack, innermost first" "$r" '"inline_stack":\[{"fn":"relax_inspect_tiers::pick","at":"[^"]*inspect_tiers.rs:14:[0-9]*"},{"fn":"relax_inspect_tiers::outer","at":"[^"]*inspect_tiers.rs:26:[0-9]*"}\]'
r=$(ask locals); check "locals: per inline level" "$r" '"inline_level":1,"fn":"relax_inspect_tiers::outer".*"name":"table"'
r1="$r"
r="$r1"; check "locals: the inlined fn's locals" "$r" '"fn":"relax_inspect_tiers::pick".*"inlined_into"'
ask resume P1 panic > /dev/null 2>&1
kill $pid 2>/dev/null; wait $pid 2>/dev/null
# warm: a short run that ends on its own writes its cache (outer's O3 code); the next run of the
# same cache dir links that code, with its inline stack
MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 SANDBOX_MUTE=1 MAKEPAD_HOME="$dir/home" TMPDIR="$dir/t" \
    MAKEPAD_RUN_HOME="$dir/runhome" MAKEPAD_RUN_CACHE_DIR="$dir/wcache" PROBE_SECS=6 \
    "$T/makepad" run relax-provided-fixture --bin relax-inspect-tiers > "$dir/run1.log" 2>&1 &
pid=$!
wait $pid
MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 SANDBOX_MUTE=1 MAKEPAD_HOME="$dir/home" TMPDIR="$dir/t" \
    MAKEPAD_RUN_HOME="$dir/runhome" MAKEPAD_RUN_CACHE_DIR="$dir/wcache" MAKEPAD_RUN_PANIC_PARK=600 \
    PROBE_TRIGGER="$dir/trigger" PROBE_SECS=300 \
    "$T/makepad" run relax-provided-fixture --bin relax-inspect-tiers > "$dir/run2.log" 2>&1 &
pid=$!
n=0
until ask ping > /dev/null 2>&1; do n=$((n + 1)); [ $n -ge 600 ] && break; kill -0 $pid 2>/dev/null || break; sleep 0.2; done
sleep 1
r=$(ask versions relax_inspect_tiers::outer); check "warm: outer runs warm-cache O3 code" "$r" '"tier":"O3","origin":"warm-[a-z]*","code":"[^"]*","state":"installed"'
touch "$dir/trigger"
n=0
until ask panics | grep -q '"id":"P'; do n=$((n + 1)); [ $n -ge 100 ] && break; sleep 0.2; done
r=$(ask events on=panic); check "warm: the inline stack survives the disk cache" "$r" '"in":"jit/O3 warm-[a-z]* inlined at [^"]*inspect_tiers.rs:26:[0-9]* in relax_inspect_tiers::outer"'
ask resume P1 panic > /dev/null 2>&1
kill $pid 2>/dev/null; wait $pid 2>/dev/null
[ -n "$KEEP" ] || rm -rf "$dir"
echo "inspect_tiers: $pass/$total passed"
