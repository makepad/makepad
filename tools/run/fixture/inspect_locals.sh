#!/bin/sh
# Regression test: makepad inspect `locals` prints the VALUES of each frame's locals at a stop,
# for every inline level, from the code's variable table (no probe, no recompile)
# (relax-provided-fixture's relax-inspect-locals bin, hidden and muted, fresh homes).
# A bounds panic in `pick`, which `outer` inlines in its optimised code; at the panic:
# - outer: `scale` (an integer held across the panic call) = 3001, `p` (a struct a call takes the
#   address of: a stack slot) = Pt { x: 1000, y: 7 }, `n` = 1000, `gone` optimised out;
# - pick (its own inline level, or its own frame where it is not inlined): `i` = 9, `doubled` = 6002.
# Runs at O3 (outer tiers up: pick inlined at O3), O1 (MAKEPAD_RUN_O1=always), the default O2
# pipeline (MAKEPAD_RUN_O3=0), and warm (a second run linking the first run's cached O3 code).
# Also a breakpoint in `sink` (called from outer's O3 code): `locals break=K1` shows outer's `p`
# and `n` in the frame above; and a paused thread (`pause`, `locals thread=main`).
# Run from the workspace root after `cargo build --release -p makepad-run`. TARGET (default
# target/release). Prints one PASS/FAIL line per check and a last line `inspect_locals: N/M passed`.
T=${TARGET:-target/release}
dir=$(mktemp -d "${TMPDIR:-/tmp}/inspect_locals.XXXXXX") || exit 1
mkdir -p "$dir/home" "$dir/t"
pass=0; total=0
check() {
    total=$((total + 1))
    [ -n "$VERBOSE" ] && printf '%s\n' "$2" | cut -c1-1200 | sed 's/^/  | /'
    if printf '%s\n' "$2" | grep -E -q -- "$3"; then pass=$((pass + 1)); echo "PASS $1"; else echo "FAIL $1"; printf '%s\n' "$2" | head -24 | cut -c1-900 | sed 's/^/    /'; fi
}
# start <cache dir> <secs> <env...>: the app in the background (pid in $pid)
start() {
    c=$1; secs=$2; shift 2
    env MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 SANDBOX_MUTE=1 MAKEPAD_HOME="$dir/home" TMPDIR="$dir/t" \
        MAKEPAD_RUN_HOME="$dir/runhome" MAKEPAD_RUN_CACHE_DIR="$dir/$c" MAKEPAD_RUN_PANIC_PARK=600 \
        PROBE_TRIGGER="$dir/trigger" PROBE_SECS=$secs "$@" \
        "$T/makepad" run relax-provided-fixture --bin relax-inspect-locals > "$dir/run-$c.log" 2>&1 &
    pid=$!
    n=0
    until ask ping > /dev/null 2>&1; do
        n=$((n + 1)); [ $n -ge 600 ] && break
        kill -0 $pid 2>/dev/null || break
        sleep 0.2
    done
}
ask() { TMPDIR="$dir/t" "$T/makepad" inspect $pid "$@" as=json 2>&1; }
panic_locals() {
    touch "$dir/trigger"
    n=0
    until ask panics | grep -q '"id":"P'; do n=$((n + 1)); [ $n -ge 150 ] && break; sleep 0.2; done
    ask locals
}
stop() { ask resume P1 panic > /dev/null 2>&1; kill $pid 2>/dev/null; wait $pid 2>/dev/null; }
values() {
    t=$1; r=$2
    check "$t: outer's scale (a register across the call) = 3001" "$r" '"fn":"relax_inspect_locals::outer".*"name":"scale","type":"u32","value":"3001"'
    check "$t: outer's p (a stack slot) = Pt { x: 1000, y: 7 }" "$r" '"fn":"relax_inspect_locals::outer".*"name":"p","type":"[^"]*Pt","value":"Pt \{\\n    x: 1000,\\n    y: 7,\\n\}"'
    check "$t: outer's n = 1000" "$r" '"fn":"relax_inspect_locals::outer".*"name":"n","type":"u32","value":"1000"'
    check "$t: outer's gone is optimised out" "$r" '"fn":"relax_inspect_locals::outer".*"name":"gone","type":"u32","value":null,"why":"[^"]*optimised out'
    check "$t: pick's i = 9" "$r" '"fn":"relax_inspect_locals::pick".*"name":"i","type":"usize","value":"9"'
    check "$t: pick's doubled = 6002" "$r" '"fn":"relax_inspect_locals::pick".*"name":"doubled","type":"u32","value":"6002"'
}

# O3: outer tiers up (hot loop); pick is inlined into its O3 code
start cache 300
n=0
until ask versions relax_inspect_locals::outer | grep -q '"version":{"slot":[0-9]*,"fn":"[^"]*","tier":"O3"'; do
    n=$((n + 1)); [ $n -ge 120 ] && break
    sleep 0.5
done
# a paused thread: its frames' locals where they are in memory (frame slots) -- its registers
# are host code's while it sleeps
ask pause > /dev/null 2>&1
r=$(ask locals thread=main)
check "pause: main's trigger (a frame slot) read from the paused thread" "$r" '"thread":"T[0-9]*","frame":[0-9]*,"inline_level":0,"fn":"relax_inspect_locals::main".*"name":"trigger","type":"String","value":"\\"[^"]*trigger\\""'
ask resume > /dev/null 2>&1
# a breakpoint in sink, called from outer's O3 code
r=$(ask break relax_inspect_locals::sink)
n=0
until ask events on=break | grep -q '"id":"K'; do n=$((n + 1)); [ $n -ge 100 ] && break; sleep 0.2; done
r=$(ask locals break=K1)
check "break: the frame above sink is outer's O3 code" "$r" '"frame":1,"fn":"relax_inspect_locals::outer","at":"[^"]*","code":\{[^}]*"tier":"O3'
check "break: outer's p = Pt { x: 1000, y: 7 }" "$r" '"break":"K1","frame":1,"inline_level":0,"fn":"relax_inspect_locals::outer".*"name":"p","type":"[^"]*Pt","value":"Pt \{\\n    x: 1000,\\n    y: 7,\\n\}"'
check "break: outer's n = 1000" "$r" '"break":"K1","frame":1,"inline_level":0,"fn":"relax_inspect_locals::outer".*"name":"n","type":"u32","value":"1000"'
ask rm all > /dev/null 2>&1
ask continue K1 > /dev/null 2>&1
sleep 1
r=$(panic_locals)
check "o3: pick is an inline level of outer's O3 code" "$r" '"panic":"P1","frame":0,"inline_level":0,"fn":"relax_inspect_locals::pick","at":"[^"]*inspect_locals.rs:[23][0-9]:[0-9]*","inlined_into":"relax_inspect_locals::outer"'
check "o3: the frame runs O3 code" "$r" '"frame":0,"fn":"relax_inspect_locals::pick","at":"[^"]*","code":\{[^}]*"tier":"O3'
values o3 "$r"
stop

# O1 and the default (O2) pipeline
for tier in o1 base; do
    case $tier in o1) e="MAKEPAD_RUN_O1=always MAKEPAD_RUN_O3=0" ;; *) e="MAKEPAD_RUN_O3=0" ;; esac
    start "cache-$tier" 300 $e
    sleep 1
    r=$(panic_locals)
    values $tier "$r"
    stop
done

# warm: a short run that ends on its own writes its cache (outer's O3 code); the next run of the
# same cache dir links that code with its variable table
start wcache 8
wait $pid
start wcache 300
sleep 1
r=$(ask versions relax_inspect_locals::outer); check "warm: outer runs warm-cache O3 code" "$r" '"tier":"O3","origin":"warm-[a-z]*","code":"[^"]*","state":"installed"'
r=$(panic_locals)
values warm "$r"
stop
[ -n "$KEEP" ] || rm -rf "$dir"
echo "inspect_locals: $pass/$total passed"
