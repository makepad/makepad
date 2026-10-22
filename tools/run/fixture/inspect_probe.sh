#!/bin/sh
# Regression test: makepad inspect's probes, eval, static and locals against a live program
# (relax-provided-fixture's relax-inspect-probe bin, hidden and muted, fresh homes).
# - a probe on an inherent method with a lifetime parameter, found as `Face::parse`, whose
#   `data: &[u8]` matches the hooked `data: &'a [u8]`, naming the module-private static `FONT`
#   unqualified and as `crate::vector::FONT` (the probe is a child module of the hooked fn's);
# - `eval` in a module's scope, the read-only refusal, `static` with its target allocation;
# - a parked panic: `eval` in the panic's scope by default, `locals`, then the panic resumes.
# Run from the workspace root after `cargo build --release -p makepad-run`. TARGET (default
# target/release). Prints one PASS/FAIL line per check and a last line `inspect_probe: N/M passed`.
T=${TARGET:-target/release}
dir=$(mktemp -d "${TMPDIR:-/tmp}/inspect_probe.XXXXXX") || exit 1
mkdir -p "$dir/home" "$dir/t"
MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 SANDBOX_MUTE=1 MAKEPAD_HOME="$dir/home" TMPDIR="$dir/t" \
    MAKEPAD_RUN_HOME="$dir/runhome" MAKEPAD_RUN_CACHE_DIR="$dir/cache" MAKEPAD_RUN_PANIC_PARK=600 \
    PROBE_TRIGGER="$dir/trigger" PROBE_SECS=300 \
    "$T/makepad" run relax-provided-fixture --bin relax-inspect-probe > "$dir/run.log" 2>&1 &
pid=$!
ask() { TMPDIR="$dir/t" "$T/makepad" inspect $pid "$@" as=json 2>&1; }
n=0
until ask ping > /dev/null 2>&1; do
    n=$((n + 1)); [ $n -ge 600 ] && break
    kill -0 $pid 2>/dev/null || break
    sleep 0.2
done
# the hooked fns have code once the loop ran a few times
sleep 2
pass=0; total=0
check() {
    total=$((total + 1))
    [ -n "$VERBOSE" ] && printf '%s\n' "$2" | cut -c1-600 | sed 's/^/  | /'
    if printf '%s\n' "$2" | grep -q -- "$3"; then pass=$((pass + 1)); echo "PASS $1"; else echo "FAIL $1"; printf '%s\n' "$2" | head -8 | sed 's/^/    /'; fi
}
P='fn probe(data: &[u8], index: u32) { std::relax::probe::emit((data.as_ptr() == FONT.as_ptr(), data.len() == FONT.len(), index)); }'
r=$(ask probe add Face::parse "$P"); check "probe: Face::parse, &[u8] for &'a [u8], private FONT in scope" "$r" '"hooked":1'
sleep 1
r=$(ask events on=probe); check "probe: emitted (true, true, 0)" "$r" '(true, true, 0)'
ask probe rm all > /dev/null
P2='fn probe(data: &[u8], _i: u32) { std::relax::probe::count(if data.len() == crate::vector::FONT.len() { "same" } else { "other" }); }'
r=$(ask probe add relax_inspect_probe::vector::Face::parse "$P2"); check "probe: full path, crate::vector::FONT" "$r" '"hooked":1'
sleep 1
r=$(ask probe read P2); check "probe: counted through crate:: path" "$r" '"count":"same"'
ask probe rm all > /dev/null
r=$(ask count crate::vector::Face::parse); check "count: crate::vector::Face::parse" "$r" 'Face::parse'
ask rm all > /dev/null
r=$(ask eval --in relax_inspect_probe::vector "(FONT.len() > 100, &FONT[..2])"); check "eval: private static in --in scope" "$r" '"value"'
r=$(ask eval in=relax_inspect_probe::vector "{ let mut x = FONT.len(); x += 1; x }"); check "eval: compound assignment refused without mutate=1" "$r" 'mutate=1'
r=$(ask static relax_inspect_probe::vector::FONT); check "static: FONT points into an allocation it fits" "$r" '"fits":"yes"'
touch "$dir/trigger"
n=0
until ask panics | grep -q '"id"'; do n=$((n + 1)); [ $n -ge 50 ] && break; sleep 0.2; done
r=$(ask panics); check "panic parked" "$r" 'boom'
r=$(ask eval "FONT.len() > 100"); check "eval: the parked panic's scope by default" "$r" 'true'
r=$(ask locals); check "locals: the panicking frame's named locals" "$r" '"numbers"'
id=$(ask panics | grep -o '"id":"P[0-9]*"' | head -1 | cut -d'"' -f4)
[ -n "$id" ] && ask resume "$id" panic > /dev/null
sleep 1
kill $pid 2>/dev/null; wait $pid 2>/dev/null
if [ $pass -ne $total ]; then echo "run log: $dir/run.log"; tail -5 "$dir/run.log"; else rm -r "$dir"; fi
echo "inspect_probe: $pass/$total passed"
