#!/bin/bash
# Regression test: the tests bucket (land_gate.sh testrun) runs the tests the fast bucket would pick for a diff,
# under the tree's JIT (`makepad test <pkg>`), one `test <pkg>` unit each: the changed package and a gate package
# that uses it (affected.py GATE_PACKAGES, here the draw); not a package that only uses it, nor one whose tests use
# it (a dev-dependency), nor one the diff does not reach: every user of a JIT or platform change ran, 547 units,
# and the bucket held most of the pool's boxes for 78-106 min a run (11-08). A change to what the JIT is built from
# runs its own tests and the smoke set's and JIT checks' (the draw); a workspace-wide input, every gate package's;
# GATE_FULL=1 runs every package's whatever the diff (the full bucket, daily). A failing test fails
# its unit alone and says the command that runs it (GATEREPRO); GATE_PROVEN skips a proven unit; GATE_RETRY runs a
# red one whatever the diff reaches. The units run slowest first by their last measures (GATE_TIMES), one not
# measured yet at the median; a package measured past GATE_SPLIT_S runs a unit per test target. With GATE_QUEUE=1 a
# shard takes them from its run's one queue a unit per free slot (a stub of the master's dealer here), and runs none
# of a queue not its own. A unit whose processes do nothing (HUNG_QUIET, short here) is stopped and
# fails HUNG, naming the test it still ran, while the other units go on; one that works past it is no hang (a
# makepad-app-amp test sat in a GStreamer pipeline and its lease idled with hundreds of packages to go, 11-07). The
# gate's own shell sleeps while its units run (it spun a core through every run, 11-07). A
# workspace of stub crates in a temporary git repo, its JIT a stub whose `makepad test` fails a package whose
# source says BROKEN, waits forever on one that says HANGS and works for 6 s on one that says BUSY; needs cargo,
# git and python3, nothing of this tree but the gate's scripts.
here=$(cd "$(dirname "$0")" && pwd)
dir=$(mktemp -d "${TMPDIR:-/tmp}/test_run.XXXXXX") || exit 2
trap 'rm -rf -- "$dir"' EXIT
export CARGO_TARGET_DIR=$dir/target
unset GATE_PROVEN GATE_RETRY GATE_FULL GATE_QUEUE DEVHUB_TOLD DEVHUB_LEASE
g() { git -C "$dir/repo" -c user.name=test-run -c user.email= -c commit.gpgsign=false "$@" > /dev/null; }
put() { mkdir -p "$dir/repo/$(dirname "$1")"; printf '%s\n' "$2" > "$dir/repo/$1"; }
lib() { # lib <dir> <package> <lib.rs> [Cargo.toml dependency lines]
  put "$1/Cargo.toml" "[package]
name = \"$2\"
version = \"0.1.0\"
edition = \"2021\"
$4"
  put "$1/src/lib.rs" "$3"
}
lib libs/shape test-run-shape 'pub fn area(w: u32) -> u32 { w * w }'
lib libs/plan test-run-plan 'pub fn floor(w: u32) -> u32 { test_run_shape::area(w) }' '[dependencies]
test-run-shape = { path = "../shape" }'
lib libs/user test-run-user 'pub fn one() -> u32 { 1 }' '[features]
variant = []

[dev-dependencies]
test-run-shape = { path = "../shape" }

[package.metadata.makepad]
test-variants = ["variant"]'
lib libs/draw makepad-draw 'pub fn fill(w: u32) -> u32 { test_run_shape::area(w) }' '[dependencies]
test-run-shape = { path = "../shape" }'
lib libs/other test-run-other 'pub fn two() -> u32 { 2 }' '[features]
heavy = []

[[test]]
name = "heavy"
required-features = ["heavy"]'
put libs/other/tests/more.rs '#[test] fn more() {}'
put libs/other/tests/heavy.rs '#[test] fn heavy() {}'
put Cargo.toml '[workspace]
members = ["tools/run", "libs/shape", "libs/plan", "libs/user", "libs/other", "libs/draw"]
resolver = "2"'
put tools/run/Cargo.toml '[package]
name = "makepad-run"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "makepad"
path = "src/main.rs"'
put tools/run/src/main.rs '// `makepad test <package> [<target>] [-- --list]` from the workspace root: one test, failing when its source says BROKEN,
// two, the second never ending, when it says HANGS (its pid in TEST_RUN_LOG.pids), working 6 s when it says BUSY;
// each run is said in TEST_RUN_LOG. `makepad inspect <pid> <question>`: a stub answer.
use std::io::Write;
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "inspect") {
        println!("{{\"verdict\":\"blocked\",\"why\":\"main is blocked: the stub waits\"}}");
        return;
    }
    let pkg = args.get(2).cloned().unwrap_or_default();
    let list = args.iter().any(|a| a == "--list");
    let dir = format!("libs/{}", pkg.trim_start_matches("test-run-"));
    let src = std::fs::read_to_string(format!("{dir}/src/lib.rs")).unwrap_or_default();
    if list {
        println!("tests::it: test\ntests::stuck: test\n\n2 tests, 0 benchmarks");
        return;
    }
    let log = std::env::var("TEST_RUN_LOG").ok();
    if let Some(log) = &log {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(log).unwrap();
        let sel: Vec<&str> = args[3..].iter().map(String::as_str).take_while(|a| *a != "--").collect();
        // the line in one write: appended whole, whatever the other units append meanwhile
        // (writeln! writes its pieces one by one, and two runs merged their lines)
        let line = format!("{}\n", std::iter::once(pkg.as_str()).chain(sel).collect::<Vec<_>>().join(" "));
        f.write_all(line.as_bytes()).unwrap();
    }
    if src.contains("BROKEN") {
        // as the JIT fails two tests: the caught panic of a passing test first, a long assertion, a report alone
        let (q, long) = (char::from(39u8), "x".repeat(500));
        let report = |msg: &str, at: &str| format!("  report: {{\"kind\":\"panic\",\"message\":\"{msg}\",\"location\":\"{dir}/src/lib.rs:{at}\",\"backtrace\":[]}}");
        println!("running 3 tests\n{}\ntest tests::caught ... ok", report("expected", "9:9"));
        println!("thread {q}tests::it{q} (7) panicked at {dir}/src/lib.rs:1:1:\nbroken: {long}\nits second line\n{}\ntest tests::it ... FAILED", report(&format!("broken: {long}\\nits second line"), "1:1"));
        println!("{}\ntest tests::other ... FAILED\n\nfailures:\n\n\nfailures:\n    tests::it\n    tests::other\n\ntest result: FAILED. 1 passed; 2 failed; 0 ignored", report("the \\\"quoted\\\" one", "2:2"));
        std::process::exit(101);
    }
    if src.contains("HANGS") {
        println!("running 2 tests\ntest tests::it ... ok");
        if let Some(log) = &log {
            let line = format!("{}\n", std::process::id());
            std::fs::OpenOptions::new().create(true).append(true).open(format!("{log}.pids")).unwrap().write_all(line.as_bytes()).unwrap();
        }
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
    if src.contains("BUSY") {
        let start = std::time::Instant::now();
        let mut n = 0u64;
        while start.elapsed().as_secs() < 6 { n = std::hint::black_box(n.wrapping_mul(31).wrapping_add(1)); }
    }
    println!("running 1 test\ntest tests::it ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored");
}'
g init -q -b base && g add -A && g commit -q -m base
base=$(git -C "$dir/repo" rev-parse HEAD)
(cd "$dir/repo" && cargo build -q --release -p makepad-run --bin makepad) || { echo "test_run FAILED: the stub JIT did not build"; exit 1; }
export MAKEPAD_LOADER=$CARGO_TARGET_DIR/release/makepad
# the shape changes and every test passes
g checkout -q -b shape
lib libs/shape test-run-shape 'pub fn area(w: u32) -> u32 { w.pow(2) }'
g commit -q -am shape
# the plan breaks its own test
g checkout -q -b plan "$base"
lib libs/plan test-run-plan '// BROKEN
pub fn floor(w: u32) -> u32 { test_run_shape::area(w) + 1 }' '[dependencies]
test-run-shape = { path = "../shape" }'
g commit -q -am plan
fail=""
# the master's dealer for one shard (bucket.rs Dealer): each GATEWANT of its output answered in its $DEVHUB_TOLD with
# the queue's next unit, the ones other shards took (DEAL_TAKEN: indexes by ,) skipped, then none; DEAL_DIGEST: the
# dealer's queue is another
dealer='import os, sys, time
out, told, taken, digest = sys.argv[1:5]
taken = {int(x) for x in taken.split(",") if x}
queue, nxt, answered = None, 0, set()
while True:
    for l in (open(out).read().splitlines() if os.path.exists(out) else []):
        w = l.split()
        if w[:1] == ["GATEQUEUE"] and len(w) == 4:
            queue = (int(w[2]), digest or w[3])
        elif w[:1] == ["GATEWANT"] and len(w) == 3 and queue and w[2] not in answered:
            answered.add(w[2])
            while nxt in taken:
                nxt += 1
            said = f"{w[2]} {nxt} {queue[1]}" if nxt < queue[0] else f"{w[2]} -"
            nxt += 1
            with open(told, "a") as f:
                f.write(said + "\n")
    time.sleep(0.1)'
gate() { # gate <label> <rev> <expect ok|fail> <ran: packages, sorted> <grep -F'd lines, | between>: land_gate.sh testrun
  local out code ran want pid st deals=""
  g checkout -q "$2"
  rm -f "$dir/ran" "$dir/out"
  if [ "${GATE_QUEUE:-}" = 1 ] && [ -n "${DEVHUB_TOLD:-}" ]; then
    : > "$DEVHUB_TOLD"; python3 -I -c "$dealer" "$dir/out" "$DEVHUB_TOLD" "${DEAL_TAKEN:-}" "${DEAL_DIGEST:-}" & deals=$!
  fi
  # the gate's shell's own CPU (utime + stime, not its children's) into gate_ticks, read until it ends
  (cd "$dir/repo" && TEST_RUN_LOG=$dir/ran exec bash "$here/land_gate.sh" testrun "$base" -) > "$dir/out" 2>&1 &
  pid=$!; gate_ticks=0
  while kill -0 "$pid" 2>/dev/null; do
    read -r -a st 2>/dev/null < "/proc/$pid/stat" && gate_ticks=$((st[13] + st[14])); sleep 0.1
  done
  wait "$pid"; code=$?; out=$(cat "$dir/out")
  if [ -n "$deals" ]; then kill "$deals"; wait "$deals" 2>/dev/null; fi
  gate_out=$out
  ran=$(sort "$dir/ran" 2>/dev/null | paste -sd' ' -)
  case "$3:$code" in ok:0|fail:1) ;; *) fail="$fail $1(exit $code)"; echo "$out" | tail -20;; esac
  [ "$ran" = "$4" ] || { fail="$fail $1(ran: ${ran:-none}; want $4)"; }
  local IFS='|'
  for want in $5; do
    echo "$out" | grep -qF -- "$want" || { fail="$fail $1(no \`$want\`)"; echo "$out" | tail -20; }
  done
}
gate shape shape ok "makepad-draw test-run-shape" "GATEUNIT ok test test-run-shape|GATEUNIT ok test makepad-draw|NODEGATE-OK testrun 2 packages|GATETIME test test-run-shape |GATESTART test makepad-draw "
# each unit's start and end are said live, before the units' own output (it comes once they all ended): the
# master's check-in lists one running far over its measure by them (bucket.rs slow_units)
for p in test-run-shape makepad-draw; do
  said=$(echo "$gate_out" | grep -E "^(GATESTART test $p [0-9]+|GATEEND test $p|GATEUNIT ok test $p)$" | cut -d' ' -f1 | paste -sd' ' -)
  [ "$said" = "GATESTART GATEEND GATEUNIT" ] || fail="$fail shape($p said live: $said)"
done
# a shard of a queued run: the queue is those three (the plan red since) by their last measures, the slowest first
# (the shape, the plan, the draw); another shard took the shape, so this one asks for a unit per free slot (two),
# gets the plan and the draw, and at its third ask none is left
times="test test-run-shape=50;test test-run-plan=10;test makepad-draw=5"
GATE_RETRY="test test-run-plan" GATE_QUEUE=1 DEVHUB_TOLD=$dir/told DEVHUB_LEASE=L7 DEAL_TAKEN=0 GATE_TIMES=$times gate queue shape ok "makepad-draw test-run-plan" "testrun: 3 units, 65 s of their last measures, the slowest first|GATEQUEUE L7 3 |GATEWANT L7 1|GATEWANT L7 3|GATEUNIT ok test test-run-plan|GATEUNIT ok test makepad-draw|NODEGATE-OK testrun 2 packages"
order=$(echo "$gate_out" | grep -oE '^GATEUNIT ok test [a-z-]+' | paste -sd' ' -)
[ "$order" = "GATEUNIT ok test test-run-plan GATEUNIT ok test makepad-draw" ] || fail="$fail queue(order: $order)"
! echo "$gate_out" | grep -q "^GATEWANT L7 4" || fail="$fail queue(asked past none left)"
# a dealer whose queue is another's: the shard runs none of it and fails, saying so; no lease telling it: it fails
GATE_QUEUE=1 DEVHUB_TOLD=$dir/told DEVHUB_LEASE=L7 DEAL_DIGEST=000000000000 GATE_TIMES=$times gate stale shape fail "" "testrun: the dealer's queue 000000000000 is not this shard's|NODEGATE-FAIL: queue"
GATE_QUEUE=1 gate untold shape fail "" "NODEGATE-FAIL: queue (no \$DEVHUB_TOLD"
gate plan plan fail "test-run-plan" "GATEUNIT fail test test-run-plan|GATEREPRO test test-run-plan :: makepad test test-run-plan|NODEGATE-FAIL: test:test-run-plan|FAILED tests::it|  panicked at libs/plan/src/lib.rs:1:1 -- broken: $(printf 'x%.0s' {1..500})|FAILED tests::other|  panicked at libs/plan/src/lib.rs:2:2 -- the \"quoted\" one"
# every failing test named, each with its own panic whole (a passing test's caught one is no failure's)
! echo "$gate_out" | grep -qE 'lib.rs:9:9|its second line' || fail="$fail plan(a passing test's panic or a second line said)"
GATE_PROVEN="test makepad-draw;check test-run-shape" gate proven shape ok "test-run-shape" "GATEUNIT proven test makepad-draw|GATEUNIT ok test test-run-shape"
GATE_RETRY="test test-run-other" gate retry plan fail "test-run-other test-run-plan" "GATEUNIT ok test test-run-other|GATEUNIT fail test test-run-plan"
# every test runs under the JIT: a change to it runs its own tests and those of the smoke set and the JIT checks
# (the draw), as the fast bucket checks them, not every package's; a change to the root Cargo.toml runs every gate
# package's
g checkout -q -b jit "$base"
printf '// a JIT change\n' >> "$dir/repo/tools/run/src/main.rs"
g commit -q -am jit
gate jit jit ok "makepad-draw makepad-run" "GATEUNIT ok test makepad-run|GATEUNIT ok test makepad-draw|NODEGATE-OK testrun 2 packages"
g checkout -q -b wide "$base"
printf '# a workspace-wide change\n' >> "$dir/repo/Cargo.toml"
g commit -q -am wide
gate wide wide ok "makepad-draw" "NODEGATE-OK testrun 1 packages"
all="makepad-draw makepad-run test-run-other test-run-plan test-run-shape test-run-user test-run-user --features variant"
# the full bucket: every package's tests on a diff that reaches only the plan, its red among them; the user's
# test variant (a feature that swaps what it builds) a unit of its own beside it
GATE_FULL=1 gate full plan fail "$all" "GATEUNIT fail test test-run-plan|GATEUNIT ok test test-run-other|GATEUNIT ok test makepad-run|GATEUNIT ok test test-run-user|GATEUNIT ok test test-run-user --features variant|NODEGATE-FAIL: test:test-run-plan"
# a unit not measured yet is queued at the median measure: the draw (255 s) after the shape and before the plan
# (red since), not (as the slowest measured) before the shape's 500 s
GATE_RETRY="test test-run-plan" GATE_TIMES="test test-run-shape=500;test test-run-plan=10" gate median shape ok "makepad-draw test-run-plan test-run-shape" "testrun: 3 units, 765 s of their last measures, the slowest first"
order=$(echo "$gate_out" | grep -oE '^GATESTART test [a-z-]+' | cut -d' ' -f3 | paste -sd' ' -)
[ "$order" = "test-run-shape makepad-draw test-run-plan" ] || fail="$fail median(order: $order)"
# a package measured past GATE_SPLIT_S (600 s) runs as a unit per test target, each proven alone: the other's lib
# runs and its tests/more.rs was proven; tests/heavy.rs needs a feature its defaults lack, so `makepad test` of the
# package skips it, and named alone it is an error: no unit
GATE_RETRY="test test-run-other" GATE_TIMES="test test-run-other=700" GATE_PROVEN="test test-run-other --test more" gate split shape ok "makepad-draw test-run-other --lib test-run-shape" "GATEUNIT ok test test-run-other --lib|GATEUNIT proven test test-run-other --test more|GATETIME test test-run-other --lib |GATESTART test test-run-other --lib |NODEGATE-OK testrun 3 packages"
# the plan hangs in its second test while the shape works past the quiet: the plan alone is stopped, HUNG. Four
# units two at a time (in this order by their measures): the other ends first, and the draw waits on the plan and
# the shape, the other's pid no longer among those it waits on (a `wait -n` given an ended job's pid again returns at once: the gate's shell
# spun a core while they ran, 11-07)
g checkout -q -b hang "$base"
lib libs/shape test-run-shape '// BUSY
pub fn area(w: u32) -> u32 { w.pow(2) }'
lib libs/plan test-run-plan '// HANGS
pub fn floor(w: u32) -> u32 { test_run_shape::area(w) }' '[dependencies]
test-run-shape = { path = "../shape" }'
g commit -q -am hang
GATE_RETRY="test test-run-other" GATE_TIMES="test test-run-other=40;test test-run-plan=30;test test-run-shape=20;test makepad-draw=10" HUNG_QUIET=3 HUNG_LOOK=1 gate hang hang fail "makepad-draw test-run-other test-run-plan test-run-shape" "GATEUNIT ok test test-run-other|GATEUNIT fail test test-run-plan|HUNG: tests::stuck; its stacks: testrun-test-run-plan.hung|main is blocked: the stub waits|GATEREPRO test test-run-plan :: makepad test test-run-plan -- tests::stuck --exact|GATEUNIT ok test test-run-shape|GATEUNIT ok test makepad-draw|NODEGATE-FAIL: test:test-run-plan"
[ "$gate_ticks" -lt 100 ] || fail="$fail hang(the gate's shell took $gate_ticks ticks of CPU: it spun while its units ran)"
! echo "$gate_out" | grep -q "^GATETIME test test-run-plan " || fail="$fail hang(a hung unit's seconds are no measure)"
echo "$gate_out" | grep -q "^GATETIME test test-run-shape " || fail="$fail hang(no measure of the busy unit)"
while read -r pid; do
  ! kill -0 "$pid" 2>/dev/null || { fail="$fail hang(its process $pid still runs)"; kill -KILL "$pid"; }
done < "$dir/ran.pids"
# a unit whose watch ends while it runs is stopped at once and fails HUNG, saying so: it never runs unbounded (the
# watch, called by a path from the tree's root, never started in this stub repo, and the hang above sat 37 min,
# 11-08)
printf 'sleep 1; echo "the watch broke" >&2; exit 3\n' > "$dir/broken_watch.sh"
rm -f "$dir/ran.pids"
UNIT_WATCH=$dir/broken_watch.sh GATE_PROVEN="test test-run-shape;test makepad-draw" gate nowatch hang fail "test-run-plan" "GATEUNIT fail test test-run-plan|HUNG (its unit watch ended (exit 3) while it ran, which leaves it unbounded: the watch broke|NODEGATE-FAIL: test:test-run-plan"
while read -r pid; do
  ! kill -0 "$pid" 2>/dev/null || { fail="$fail nowatch(its process $pid still runs)"; kill -KILL "$pid"; }
done < "$dir/ran.pids"
if [ -n "$fail" ]; then echo "test_run FAILED:$fail"; exit 1; fi
echo "test_run: ok"
