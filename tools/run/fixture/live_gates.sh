#!/bin/bash
# Regression test (relax#33): a save the live watcher holds, or one that changes nothing the
# program runs, says so in one line of the session's output naming the file. A save that did
# nothing visible looked like an edit the session ignored. One `makepad run` session of a program
# (app/) that depends on a library under libs/ whose module `later` the file selection defers
# (RELAX_LAZY_DEFER; the completion loader off), and on a copy of the provided crate
# tools/run/fixture. The library is no app crate (reload.rs gandalf_app_roots): the run checks
# its app crates whole before they run, and loads every file of them for it. Its makepad is a
# copy outside any cargo target dir, so it cannot rebuild itself. In turn:
#   outline: later.rs edited while nothing has loaded it: "<file>: loaded only as an outline";
#            then main.rs calls it, and the program runs later.rs's saved text, not the boot's;
#   git:     the save of a library whose `.git` file names a gitdir that is gone applies, and so
#            does one in a repository with a stale index.lock (each waited for a quiet window
#            that never came, silently); a lock while the repository's ref moved holds the save,
#            "waits for the tree to settle (<lock> is there)" after a second, until it goes;
#   restart: an edit of the provided crate needs a JIT rebuild the session cannot make; the next
#            save says "restart pending" in the output (it was an event alone);
#   term:    a SIGTERM to `makepad test --live` (a driver's terminate) ends the run it started
#            and that run's process group too (the run kept watching and testing, relax#88).
#            Before that, its package's derive of its own runs in the macro world: an edit of the
#            file with the derive builds no second world (the load's is kept; relax#144).
# Each step waits on the line it expects. Run from the workspace root; works on copies under the
# tree's target folder. Prints "live_gates: ok" or "live_gates: FAILED <step>"; exit 0 / 1.
ROOT=$(pwd)
cargo build --profile host -q --manifest-path "$ROOT/Cargo.toml" -p makepad-run --bin makepad || exit 1
J=${CARGO_TARGET_DIR:-$ROOT/target}/host/makepad
mkdir -p "$ROOT/target"
S=$(mktemp -d "$ROOT/target/live_gates.XXXXXX") || exit 1
: "${S:?}"
pid=""
tp=""
child=""
finish() {
    [ -n "$pid" ] && kill "$pid" 2>/dev/null && wait "$pid" 2>/dev/null
    [ -n "$tp" ] && kill "$tp" 2>/dev/null && wait "$tp" 2>/dev/null
    # the term step's run, by its recorded pid (and its group, which it leads with the fix)
    [ -n "$child" ] && { kill -- "-$child" || kill "$child"; } 2>/dev/null
    rm -rf -- "$S"
}
trap finish EXIT
unset RELAX_STD
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
mkdir -p "$S/ws/app/src" "$S/ws/fixture" "$S/ws/libs/later/src" "$S/ws/libs/gitfile/src" "$S/ws/libs/gitlock/src" "$S/ws/libs/term/src" "$S/ws/libs/derive/src" "$S/home" "$S/thome" "$S/tmp"
cp "$J" "$S/makepad" || exit 1
cp -R "$ROOT/tools/run/fixture/Cargo.toml" "$ROOT/tools/run/fixture/src" "$ROOT/tools/run/fixture/tests" "$S/ws/fixture/" || exit 1
cat > "$S/ws/Cargo.toml" <<EOF
[workspace]
members = ["app", "fixture", "libs/later", "libs/gitfile", "libs/gitlock", "libs/term", "libs/derive"]
resolver = "2"

[workspace.dependencies]
relax-api = { path = "$ROOT/libs/relax_api" }
EOF
cat > "$S/ws/app/Cargo.toml" <<'EOF'
[package]
name = "relax-live-gates"
version = "0.1.0"
edition = "2021"

[dependencies]
relax-provided-fixture = { path = "../fixture" }
relax-live-later = { path = "../libs/later" }
relax-live-gitfile = { path = "../libs/gitfile" }
relax-live-gitlock = { path = "../libs/gitlock" }
EOF
cat > "$S/ws/libs/later/Cargo.toml" <<'EOF'
[package]
name = "relax-live-later"
version = "0.1.0"
edition = "2021"
EOF
echo 'pub mod later;' > "$S/ws/libs/later/src/lib.rs"
for g in gitfile gitlock; do
    printf '[package]\nname = "relax-live-%s"\nversion = "0.1.0"\nedition = "2021"\n' "$g" > "$S/ws/libs/$g/Cargo.toml"
    printf 'pub fn v() -> u32 {\n    1\n}\n' > "$S/ws/libs/$g/src/lib.rs"
done
printf '[package]\nname = "relax-live-term"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\nrelax-live-derive = { path = "../derive" }\n' > "$S/ws/libs/term/Cargo.toml"
cat > "$S/ws/libs/term/src/lib.rs" <<'EOF'
#[derive(relax_live_derive::Describe)]
pub struct Record;

#[test]
fn adds() {
    assert_eq!(1 + 1, 2);
}

#[test]
fn describes() {
    assert_eq!(Record::describe(), "Record");
}
EOF
printf '[package]\nname = "relax-live-derive"\nversion = "0.1.0"\nedition = "2021"\n\n[lib]\nproc-macro = true\n' > "$S/ws/libs/derive/Cargo.toml"
cat > "$S/ws/libs/derive/src/lib.rs" <<'EOF'
extern crate proc_macro;
use proc_macro::TokenStream;

/// `impl <Name> { pub fn describe() -> &'static str { "<Name>" } }` for the struct it derives on
#[proc_macro_derive(Describe)]
pub fn describe(input: TokenStream) -> TokenStream {
    let mut tokens = input.into_iter();
    let mut name = String::new();
    while let Some(t) = tokens.next() {
        if t.to_string() == "struct" {
            name = tokens.next().map(|n| n.to_string()).unwrap_or_default();
            break;
        }
    }
    format!("impl {0} {{ pub fn describe() -> &'static str {{ \"{0}\" }} }}", name).parse().unwrap()
}
EOF
# a worktree's `.git` file whose repository is not there; a repository a crashed git left its
# index.lock in (older than every save)
echo "gitdir: $S/nowhere" > "$S/ws/libs/gitfile/.git"
mkdir -p "$S/ws/libs/gitlock/.git/refs/heads"
echo 'ref: refs/heads/main' > "$S/ws/libs/gitlock/.git/HEAD"
echo 0123456789abcdef0123456789abcdef01234567 > "$S/ws/libs/gitlock/.git/refs/heads/main"
: > "$S/ws/libs/gitlock/.git/index.lock"
cat > "$S/ws/app/src/main.rs" <<'EOF'
fn now() -> u32 {
    1
}

fn main() {
    // a value four times a second, until the script ends the session
    loop {
        println!("value {}", now() + relax_provided_fixture::add(0, 0) as u32);
        println!("gates {} {}", relax_live_gitfile::v(), relax_live_gitlock::v());
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
(cd "$S/ws" && RELAX_CACHE=off RELAX_HOME="$S/home" TMPDIR="$S/tmp" MAKEPAD_HIDE_WINDOWS=1 \
    RELAX_LAZY_DEFER=libs/later/src/later.rs RELAX_LAZY_COMPLETE=0 \
    exec "$S/makepad" run relax-live-gates) > "$S/log" 2>&1 &
pid=$!
said start '^value 1$'
perl -pi -e 's/^    2$/    3/' "$S/ws/libs/later/src/later.rs"
said outline '^relax live: .*later\.rs: loaded only as an outline'
# the program reads the deferred module now: its saved text runs
perl -pi -e 's/^    1$/    relax_live_later::later::k()/' "$S/ws/app/src/main.rs"
said outline-use '^value (2|3)$'
grep -q '^value 2$' "$S/log" && fail outline-stale-text
perl -pi -e 's/^    1$/    2/' "$S/ws/libs/gitfile/src/lib.rs"
said gitdir-gone '^gates 2 1$'
perl -pi -e 's/^    1$/    2/' "$S/ws/libs/gitlock/src/lib.rs"
said stale-lock '^gates 2 2$'
# a ref moved since the tree last settled: the lock can be the operation's that moves it
echo 1123456789abcdef0123456789abcdef01234567 > "$S/ws/libs/gitlock/.git/refs/heads/main"
perl -pi -e 's/^    2$/    3/' "$S/ws/libs/gitlock/src/lib.rs"
said lock-said '^relax live: .*gitlock/src/lib\.rs: waits for the tree to settle \(.*index\.lock is there\)'
rm -f -- "$S/ws/libs/gitlock/.git/index.lock"
said lock-gone '^gates 2 3$'
said lock-settled '^relax live: .*gitlock/src/lib\.rs: the tree settled after'
# a field more in the provided crate's `P`: a layout edit makepad must be rebuilt for
perl -pi -e 's/^pub struct P \{$/pub struct P {\n    pub added: u64,/' "$S/ws/fixture/src/lib.rs"
said restart-refused '^relax live: .*needs a JIT rebuild'
perl -pi -e 's/pub added: u64,/pub added: u8,/' "$S/ws/fixture/src/lib.rs"
said restart-pending '^relax live: .*lib\.rs: restart pending'
# a SIGTERM to `makepad test --live` ends the run it started (its child, recorded while the
# command lives) and that run's process group; before, the run kept watching and testing
(cd "$S/ws" && RELAX_CACHE=off RELAX_HOME="$S/thome" TMPDIR="$S/tmp" RELAX_BOOT_TRACE=1 \
    exec "$S/makepad" test --live relax-live-term) > "$S/tlog" 2>&1 &
tp=$!
grep -qE -m1 '^testverdict ' < <(timeout 120 tail --pid="$tp" -n +1 -f "$S/tlog" 2>/dev/null) || { tail -20 "$S/tlog"; fail term-verdict; }
# an edit of the file whose derive runs in the macro world: the load's world runs it again
perl -pi -e 's/1 \+ 1, 2/2 + 0, 2/' "$S/ws/libs/term/src/lib.rs"
[ "$(grep -cE -m2 '^testverdict .*failed=0' < <(timeout 60 tail --pid="$tp" -n +1 -f "$S/tlog" 2>/dev/null))" = 2 ] || { tail -20 "$S/tlog"; fail world-edit-verdict; }
[ "$(grep -c 'boot-trace: macro world: std' "$S/tlog")" = 1 ] || { grep 'macro world' "$S/tlog"; fail world-built-again; }
times="$times world $(date +%s.%N | awk -v t0="$t0" '{printf "%.1f", $1 - t0}')s"
child=$(ps -o pid= --ppid "$tp" | tr -d ' ')
case "$child" in ''|*[!0-9]*) fail term-child-pid;; esac
kill -TERM "$tp"
wait "$tp"
tp=""
for _ in $(seq 50); do
    { kill -0 "$child" || kill -0 -- "-$child"; } 2>/dev/null || break
    sleep 0.1
done
{ kill -0 "$child" || kill -0 -- "-$child"; } 2>/dev/null && fail term-child-left
child=""
times="$times term $(date +%s.%N | awk -v t0="$t0" '{printf "%.1f", $1 - t0}')s"
echo "live_gates: ok (since the session started:$times)"
