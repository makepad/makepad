#!/bin/bash
# usage: run.sh <makepad binary>: a provided crate (built into the makepad binary) whose sources sit
# in the application's own tree keeps rustc's layouts in a live session. Live sessions once gave the
# structs of the application's crates that were not Copy a hidden extension word (where the fields a
# live edit added lived); a provided crate in that scope took it too, so each such plain type was 8
# bytes larger than rustc's ("provided type ..: rustc lays it out in 32 bytes .., makepad run in
# 40"), became opaque, and a program that builds one parked before its first frame ("a field of
# `ScriptLoc` (`file: String`) is reached from JIT-compiled code"). A handle laid out as rustc lays
# it out (a Note of Strings, Vecs and Options) stayed opaque the same way.
# The program is relax-provided-fixture's, from a byte-identical copy of the crate in a workspace of
# its own (so the crate is in the application's scope), run as a live session: it builds a Loc (a
# plain struct with a String field) and reads it, and builds Notes by literals and hands them to
# the crate's rustc code.
# Prints "provided_live_layout: ok" or "provided_live_layout: FAILED (...)"; exit 0 / 1.
set -u
H=$(cd "$(dirname "$0")" && pwd)
R=$(cd "$H/../../../../.." && pwd)
BIN=$1
T=$(mktemp -d "${TMPDIR:-/tmp}/provided_live_layout.XXXXXX") || exit 1
: "${T:?}"
pid=""
finish() {
    [ -n "$pid" ] && kill "$pid" 2>/dev/null
    [ -n "$pid" ] && wait "$pid" 2>/dev/null
    rm -rf "$T"
}
trap finish EXIT
unset RELAX_STD RELAX_LOADER
mkdir -p "$T/ws/fixture" "$T/home" "$T/tmp" "$T/reg" "$T/box"
# the binary runs from a copy outside any cargo target directory (as devhub's boxes run theirs)
cp "$BIN" "$T/box/makepad" || exit 1
cp -R "$R/tools/run/fixture/Cargo.toml" "$R/tools/run/fixture/src" "$R/tools/run/fixture/tests" "$T/ws/fixture/"
cat > "$T/ws/Cargo.toml" <<EOF
[workspace]
members = ["fixture"]
resolver = "2"

[workspace.dependencies]
relax-api = { path = "$R/libs/relax_api" }
EOF
log="$T/log"
# a live session (no --no-live), hidden; a call of code that did not compile parks it, so the run
# is watched until it ends or parks (the panic the program catches does not park: PANIC_PARK=0)
(cd "$T/ws" && MAKEPAD_HIDE_WINDOWS=1 RELAX_PANIC_PARK=0 RELAX_CACHE=off RELAX_HOME="$T/home" MAKEPAD_REMOTE_REGISTRY="$T/reg" TMPDIR="$T/tmp" \
    exec "$T/box/makepad" run relax-provided-fixture --bin relax-provided-fixture) > "$log" 2>&1 &
pid=$!
end=""
for _ in $(seq 1 600); do
    if ! kill -0 "$pid" 2>/dev/null; then wait "$pid"; end="exit $?"; pid=""; break; fi
    if grep -q 'compile error parked' "$log"; then end="parked"; break; fi
    sleep 0.5
done
[ -n "$end" ] || end="no end in 300 s"
fails=""
want() { grep -q -F -x -- "$1" "$log" || fails="$fails missing:'$1'"; }
[ "$end" = "exit 0" ] || fails="$fails run:$end"
grep -q 'relax: provided type' "$log" && fails="$fails demoted"
grep -q 'lives in a provided crate' "$log" && fails="$fails refused"
want "bound true"
want 'loc Loc { file: "src/app.rs", col: 5, line: 42 } 43 1 false'
grep -q '^note digest built here|a,bb|3|Some("al")|Some(9)|B|7$' "$log" || fails="$fails note-digest"
grep -q '^note round trips [0-9]* growth under 96 MiB: true$' "$log" || fails="$fails note-round-trips"
want "asm 42"
if [ -z "$fails" ]; then
    echo "provided_live_layout: ok"
else
    echo "---- the run ($end):"
    grep 'relax: provided type' "$log" | head -8 | cut -c1-400
    grep -v 'relax: provided type' "$log" | tail -24 | cut -c1-400
    echo "provided_live_layout: FAILED ($fails )"
    exit 1
fi
