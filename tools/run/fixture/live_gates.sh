#!/bin/bash
# Regression test (relax#33): a save the live watcher holds, or one that changes nothing the
# program runs, says so in one line of the session's output naming the file. A save that did
# nothing visible looked like an edit the session ignored. One `makepad run` session of a program
# (app/) that depends on a library under libs/ whose module `later` the file selection defers
# (MAKEPAD_RUN_LAZY_DEFER; the completion loader off), and on a copy of the provided crate
# tools/run/fixture. The library is no app crate (reload.rs gandalf_app_roots): the run checks
# its app crates whole before they run, and loads every file of them for it. Its makepad is a
# copy outside any cargo target dir, so it cannot rebuild itself. In turn:
#   outline: later.rs edited while nothing has loaded it: "<file>: loaded only as an outline";
#            then main.rs calls it, and the program runs later.rs's saved text, not the boot's;
#   restart: an edit of the provided crate needs a JIT rebuild the session cannot make; the next
#            save says "restart pending" in the output (it was an event alone).
# Each step waits on the line it expects. Run from the workspace root; works on copies under the
# tree's target folder. Prints "live_gates: ok" or "live_gates: FAILED <step>"; exit 0 / 1.
ROOT=$(pwd)
cargo build --profile host -q --manifest-path "$ROOT/Cargo.toml" -p makepad-run --bin makepad || exit 1
J=${CARGO_TARGET_DIR:-$ROOT/target}/host/makepad
mkdir -p "$ROOT/target"
S=$(mktemp -d "$ROOT/target/live_gates.XXXXXX") || exit 1
: "${S:?}"
pid=""
finish() {
    [ -n "$pid" ] && kill "$pid" 2>/dev/null && wait "$pid" 2>/dev/null
    rm -rf -- "$S"
}
trap finish EXIT
unset MAKEPAD_RUN_STD
fail() {
    echo "live_gates: FAILED $1"
    grep -E '^(value|makepad)' "$S/log" | uniq | tail -20
    exit 1
}
# said <step> <pattern>: the first line of the session's output matching <pattern>, as it is
# written (60 s at most; the follow ends with the session)
said() {
    grep -qE -m1 "$2" < <(timeout 60 tail --pid="$pid" -n +1 -f "$S/log" 2>/dev/null) || fail "$1"
    times="$times $1 $(date +%s.%N | awk -v t0="$t0" '{printf "%.1f", $1 - t0}')s"
}
mkdir -p "$S/ws/app/src" "$S/ws/fixture" "$S/ws/libs/later/src" "$S/home" "$S/tmp"
cp "$J" "$S/makepad" || exit 1
cp -R "$ROOT/tools/run/fixture/Cargo.toml" "$ROOT/tools/run/fixture/src" "$ROOT/tools/run/fixture/tests" "$S/ws/fixture/" || exit 1
cat > "$S/ws/Cargo.toml" <<EOF
[workspace]
members = ["app", "fixture", "libs/later"]
resolver = "2"

[workspace.dependencies]
makepad-relax-api = { path = "$ROOT/libs/relax_api" }
EOF
cat > "$S/ws/app/Cargo.toml" <<'EOF'
[package]
name = "relax-live-gates"
version = "0.1.0"
edition = "2021"

[dependencies]
relax-provided-fixture = { path = "../fixture" }
relax-live-later = { path = "../libs/later" }
EOF
cat > "$S/ws/libs/later/Cargo.toml" <<'EOF'
[package]
name = "relax-live-later"
version = "0.1.0"
edition = "2021"
EOF
echo 'pub mod later;' > "$S/ws/libs/later/src/lib.rs"
cat > "$S/ws/app/src/main.rs" <<'EOF'
fn now() -> u32 {
    1
}

fn main() {
    // a value four times a second, until the script ends the session
    loop {
        println!("value {}", now() + relax_provided_fixture::add(0, 0) as u32);
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}
EOF
cat > "$S/ws/libs/later/src/later.rs" <<'EOF'
pub fn k() -> u32 {
    2
}
EOF
: > "$S/log"
t0=$(date +%s.%N)
times=""
(cd "$S/ws" && MAKEPAD_RUN_CACHE=off MAKEPAD_RUN_HOME="$S/home" TMPDIR="$S/tmp" MAKEPAD_HIDE_WINDOWS=1 \
    MAKEPAD_RUN_LAZY_DEFER=libs/later/src/later.rs MAKEPAD_RUN_LAZY_COMPLETE=0 \
    exec "$S/makepad" run relax-live-gates) > "$S/log" 2>&1 &
pid=$!
said start '^value 1$'
perl -pi -e 's/^    2$/    3/' "$S/ws/libs/later/src/later.rs"
said outline '^makepad live: .*later\.rs: loaded only as an outline'
# the program reads the deferred module now: its saved text runs
perl -pi -e 's/^    1$/    relax_live_later::later::k()/' "$S/ws/app/src/main.rs"
said outline-use '^value (2|3)$'
grep -q '^value 2$' "$S/log" && fail outline-stale-text
# a field more in the provided crate's `P`: a layout edit makepad must be rebuilt for
perl -pi -e 's/^pub struct P \{$/pub struct P {\n    pub added: u64,/' "$S/ws/fixture/src/lib.rs"
said restart-refused '^makepad live: .*needs a JIT rebuild'
perl -pi -e 's/pub added: u64,/pub added: u8,/' "$S/ws/fixture/src/lib.rs"
said restart-pending '^makepad live: .*lib\.rs: restart pending'
echo "live_gates: ok (since the session started:$times)"
