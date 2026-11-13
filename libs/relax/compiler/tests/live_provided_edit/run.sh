#!/bin/bash
# usage: run.sh <makepad binary>: under `makepad test --live`, an edit of a provided crate (one built
# into the makepad binary) always ends with a verdict, never a session stuck in "patching"
# (WORKPOOL's makepad-hal struct edit hung 14 min). The crate is a byte-identical copy of
# tools/run/fixture (relax-provided-fixture), so the copy's source identity is the one built in.
# 1. The binary runs from a copy outside any cargo target directory, as devhub's boxes run theirs:
#    it cannot rebuild itself, and each save is refused at once ("needs a JIT rebuild", then
#    "restart pending").
# 2. The copy sits in a cargo target directory, as on a person's machine (cargo run), and the
#    rebuild's cargo is this fixture's ($CARGO): a failing rebuild ends refused with the build's
#    errors, one that builds nothing new ends refused "rebuild produced no change".
# 3. A relaunch asked while the rebuild runs ("rebuild makepad, then restart"), and the rebuild
#    builds nothing new: the restart still happens (it used to end at "no restart", silently).
set -u
H=$(cd "$(dirname "$0")" && pwd)
R=$(cd "$H/../../../../.." && pwd)
T=$(mktemp -d "${TMPDIR:-/tmp}/live_provided.XXXXXX") || exit 1
: "${T:?}"
pid=""
jit=""
finish() {
    [ -n "$jit" ] && kill "$jit" 2>/dev/null
    [ -n "$pid" ] && kill "$pid" 2>/dev/null
    [ -n "$pid" ] && wait "$pid" 2>/dev/null
    rm -rf "$T"
}
trap finish EXIT
fail() { echo "live_provided_edit: FAILED ($1)"; tail -20 "$S/log"; exit 1; }
# a session in $S running the binary $S/$2 (its own home, workspace and cache-free state)
start() {
    S="$T/$1"
    mkdir -p "$S/ws/fixture" "$S/home" "$S/tmp" "$(dirname "$S/$2")"
    cp "$BIN" "$S/$2" || exit 1
    cp -R "$R/tools/run/fixture/Cargo.toml" "$R/tools/run/fixture/src" "$R/tools/run/fixture/tests" "$S/ws/fixture/"
    cat > "$S/ws/Cargo.toml" <<EOF
[workspace]
members = ["fixture"]
resolver = "2"

[workspace.dependencies]
relax-api = { path = "$R/libs/relax_api" }
EOF
    (cd "$S/ws" && RELAX_CACHE=off RELAX_HOME="$S/home" TMPDIR="$S/tmp" exec "$S/$2" test --live relax-provided-fixture --test relax_api_entries) > "$S/log" 2>&1 &
    pid=$!
    MP="$S/$2"
    for _ in $(seq 1 600); do
        grep -q '^testverdict group=0' "$S/log" && break
        kill -0 "$pid" 2>/dev/null || fail "$1: the session ended before its first verdict"
        sleep 0.5
    done
    grep -q '^testverdict group=0' "$S/log" || fail "$1: no initial verdict in 300 s"
    jit=$(sed -n 's/^makepad-remote: pid \([0-9]*\) .*/\1/p' "$S/log" | head -1)
    [ -n "$jit" ] || fail "$1: no inspect socket line"
    lib="$S/ws/fixture/src/lib.rs"
}
stop() {
    kill "$jit" 2>/dev/null; kill "$pid" 2>/dev/null; wait "$pid" 2>/dev/null
    jit=""; pid=""
}
# the next patch event after the last one in $S/watch (none there: from now), added to it
watch() {
    since=$(sed -n 's/.*"rearm":"watch since=\([0-9]*\)".*/\1/p' "$S/watch" | tail -1)
    "$MP" inspect "$jit" watch on=patchrefused,patchverified timeout_s=15 --json ${since:+since=$since} >> "$S/watch" 2>&1
}
# the patch events up to the edit's final verdict (a refusal "refused"), within 10 s of the edit
# made at $2; $1 names the step, $3 what the refusal says
verdict() {
    for _ in 1 2 3; do
        grep -q '"outcome":"refused"' "$S/watch" && break
        watch
    done
    out=$(cat "$S/watch")
    echo "$out" | grep -q '"outcome":"refused"' || fail "$1: not refused: $out"
    echo "$out" | grep -q "$3" || fail "$1: the reason does not say '$3': $out"
    [ $(($(date +%s) - $2)) -le 10 ] || fail "$1: answered after $(($(date +%s) - $2)) s"
    : > "$S/watch"
}
# an edit: the watch starts before it (from now), then $1 runs
edit() {
    : > "$S/watch"
    watch & w=$!
    sleep 1
    t0=$(date +%s)
    perl -pi -e "$1" "$lib"
    wait $w
}
export BIN="$1"
unset RELAX_STD

# 1. outside a cargo target dir: refused at once
start box makepad
# a layout edit of a provided crate: a field more in `P`
edit 's/^pub struct P \{$/pub struct P {\n    pub added: u64,/'
grep -q 'pub added: u64' "$lib" || fail "the edit did not apply"
verdict layout "$t0" 'needs a JIT rebuild'
# a save after it: answered too (the restart it waits for is pending)
edit 's/pub added: u64,/pub added: u32,/'
verdict later "$t0" 'restart pending'
kill -0 "$pid" 2>/dev/null || fail "the session ended"
stop

# 2. in a cargo target dir: the rebuild runs, and its end is the verdict
mkdir -p "$T/mac/target/host/.fingerprint"
touch "$T/mac/target/CACHEDIR.TAG"
cat > "$T/cargo" <<EOF
#!/bin/bash
# the rebuild's cargo: fails, or builds nothing new (the binary stays as it is)
# (slow: a build that takes a while, which a relaunch can reach)
[ "\$(cat "$T/cargo_mode")" = slow ] && sleep 6
if [ "\$(cat "$T/cargo_mode")" = fail ]; then
    echo "error[E0599]: no method named forced found (live_provided_edit's failing rebuild)" >&2
    echo "  --> tools/run/src/main.rs:1:1" >&2
    exit 101
fi
exit 0
EOF
chmod +x "$T/cargo"
echo fail > "$T/cargo_mode"
export CARGO="$T/cargo"
start mac target/host/makepad
edit 's/^pub struct P \{$/pub struct P {\n    pub added: u64,/'
verdict "failing rebuild" "$t0" 'rebuilding makepad FAILED'
grep -q 'forced' "$S/log" || fail "the build's error is not shown"
echo noop > "$T/cargo_mode"
edit 's/pub added: u64,/pub added: u32,/'
verdict "unchanged rebuild" "$t0" 'rebuild produced no change'
kill -0 "$pid" 2>/dev/null || fail "the session ended"

# 3. the relaunch asked during a rebuild that builds nothing new restarts
echo slow > "$T/cargo_mode"
builds=$(grep -c 'it builds first in the background' "$S/log")
perl -pi -e 's/pub added: u32,/pub added: u16,/' "$lib"
for _ in $(seq 1 60); do
    [ "$(grep -c 'it builds first in the background' "$S/log")" -gt "$builds" ] && break
    sleep 0.5
done
[ "$(grep -c 'it builds first in the background' "$S/log")" -gt "$builds" ] || fail "relaunch: the save started no rebuild"
asked=$("$MP" inspect "$jit" session relaunch 2>&1)
echo "$asked" | grep -q 'rebuild makepad, then restart' || fail "relaunch: not taken by the rebuild: $asked"
for _ in $(seq 1 120); do
    grep -q '^relax live: restarting (' "$S/log" && break
    sleep 0.5
done
grep -q 'restarting the app as asked' "$S/log" || fail "relaunch: the unchanged rebuild did not go on to the restart"
grep -q '^relax live: restarting (' "$S/log" || fail "relaunch: no restart in 60 s"
echo "live_provided_edit: ok"
