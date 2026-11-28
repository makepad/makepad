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
# measured yet at the median; a package measured past GATE_SPLIT_S runs a unit per test target, and so does one whose
# Cargo.toml runs a target under cargo (native-tests), that target's unit under `cargo test`. With GATE_QUEUE=1 a
# shard takes them from its run's one queue a unit per free slot (a stub of the master's dealer here), and runs none
# of a queue not its own. A unit whose processes do nothing (HUNG_QUIET, short here) is stopped and
# fails HUNG, naming the test it still ran (after a child's own run of a test inside its target's), while the
# other units go on; one that works past it is no hang (a
# makepad-app-amp test sat in a GStreamer pipeline and its lease idled with hundreds of packages to go, 11-07), until
# main stops it by hand (`devhub bucket stop`: its lease told `stop <unit> :: <by> :: <why>`), when it fails HUNG,
# saying who stopped it and why, and the shard goes on. The
# gate's own shell sleeps while its units run (it spun a core through every run, 11-07). A
# workspace of stub crates in a temporary git repo, its JIT a stub whose `makepad test` fails a package whose
# source says BROKEN, waits forever on one that says HANGS, works for 6 s on one that says BUSY and for 60 s in its
# second test on one that says SPINS; needs cargo,
# git and python3, nothing of this tree but the gate's scripts.
# A nested gate must not publish into the outer lease's keep.
unset DEVHUB_KEEP
here=$(cd "$(dirname "$0")" && pwd)
dir=$(mktemp -d "${TMPDIR:-/tmp}/test_run.XXXXXX") || exit 2
trap 'rm -rf -- "$dir"' EXIT
export CARGO_TARGET_DIR=$dir/target
unset GATE_PROVEN GATE_RETRY GATE_FULL GATE_QUEUE GATE_TEST_TIMES DEVHUB_TOLD DEVHUB_LEASE
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
// two, the second never ending, when it says HANGS (the first run again in a child of its own before it, as the
// harness self-exec says it: its own `running 1 test` to `test result:` inside the target; its pid in
// TEST_RUN_LOG.pids), working 6 s when it says BUSY, 60 s in the second of two when it says SPINS (its pid in
// TEST_RUN_LOG.pids), its build failing before any test when it says UNBUILT;
// each run is said in TEST_RUN_LOG; with `-Z unstable-options --report-time` and RUSTC_BOOTSTRAP (the run of a part), its
// result line ends in its time. `makepad inspect <pid> <question>`: a stub answer.
use std::io::Write;
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "inspect") {
        println!("{{\"verdict\":\"blocked\",\"why\":\"main is blocked: the stub waits\"}}");
        return;
    }
    // the gate tests under the profile devhub test runs: release with the checks (land_gate.sh TP)
    assert_eq!(args.get(3..5), Some(&["--profile".to_string(), "release-checks".to_string()][..]), "not the checked release profile: {args:?}");
    let args: Vec<String> = args[..3].iter().chain(&args[5..]).cloned().collect();
    let pkg = args.get(2).cloned().unwrap_or_default();
    let list = args.iter().any(|a| a == "--list");
    let dir = format!("libs/{}", pkg.trim_start_matches("test-run-"));
    let src = std::fs::read_to_string(format!("{dir}/src/lib.rs")).unwrap_or_default();
    if list {
        println!("tests::it: test\ntests::stuck: test\n\n2 tests, 0 benchmarks");
        return;
    }
    // as makepad_test writes the artifacts of a test: <root>/<package>/<test>/, its app home inside
    if let Some(root) = std::env::var_os("MAKEPAD_TEST_ARTIFACTS") {
        let test = std::path::Path::new(&root).join(&pkg).join("tests_it");
        std::fs::create_dir_all(test.join("home")).unwrap();
        std::fs::write(test.join("run.log"), "ran\n").unwrap();
        std::fs::write(test.join("home").join("state"), "an app home\n").unwrap();
    }
    let log = std::env::var("TEST_RUN_LOG").ok();
    if let Some(log) = &log {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(log).unwrap();
        let sel: Vec<&str> = args[3..].iter().map(String::as_str).take_while(|a| *a != "--").collect();
        // the line in one write: appended whole, whatever the other units append meanwhile
        // (writeln! writes its pieces one by one, and two runs merged their lines)
        let line = format!("{}\n", std::iter::once(pkg.as_str()).chain(sel).collect::<Vec<_>>().join(" "));
        f.write_all(line.as_bytes()).unwrap();
        // the queue values of a run stay in the shell of its gate: a unit that inherits one is said (land_gate.sh QUEUE_VARS)
        let inherited: Vec<&str> = ["GATE_PROVEN", "GATE_TIMES", "GATE_PEAKS", "GATE_JOBS", "GATE_APPS", "GATE_TEST_TIMES", "GATE_QUEUE_DIR"].into_iter().filter(|v| std::env::var_os(v).is_some()).collect();
        if !inherited.is_empty() {
            let line = format!("{}\n", inherited.join(" "));
            std::fs::OpenOptions::new().create(true).append(true).open(format!("{log}.env")).unwrap().write_all(line.as_bytes()).unwrap();
        }
    }
    if src.contains("UNBUILT") {
        // (a build takes its time: the watch of the unit looks at it, its peak said)
        std::thread::sleep(std::time::Duration::from_secs(2));
        println!("error[E0063]: missing field `x` in initializer of `Plan`\n  --> {dir}/src/lib.rs:1:1");
        std::process::exit(1);
    }
    if src.contains("BROKEN") {
        // as the JIT fails two tests: the caught panic of a passing test first, a long assertion, a report alone
        let (q, long) = (char::from(39u8), "x".repeat(500));
        let report = |msg: &str, at: &str| format!("  report: {{\"kind\":\"panic\",\"message\":\"{msg}\",\"location\":\"{dir}/src/lib.rs:{at}\",\"backtrace\":[]}}");
        println!("running 4 tests\n{}\ntest tests::caught ... ok", report("expected", "9:9"));
        println!("thread {q}tests::it{q} (7) panicked at {dir}/src/lib.rs:1:1:\nbroken: {long}\nits second line\n{}\ntest tests::it ... FAILED", report(&format!("broken: {long}\\nits second line"), "1:1"));
        println!("{}\ntest tests::other ... FAILED", report("the \\\"quoted\\\" one", "2:2"));
        // a harness whose message announces the output of its child: what follows its colon is said too
        println!("thread {q}tests::harness{q} (8) panicked at {dir}/src/lib.rs:3:3:\nrun.sh failed:\ncase a: ok\n\ncase b: wrong output\nnote: run with RUST_BACKTRACE=1 for a backtrace\ntest tests::harness ... FAILED\n\nfailures:\n\n\nfailures:\n    tests::it\n    tests::other\n    tests::harness\n\ntest result: FAILED. 1 passed; 3 failed; 0 ignored");
        std::process::exit(101);
    }
    if src.contains("HANGS") {
        println!("running 2 tests\n\nrunning 1 test\ntest tests::it ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out\n\ntest tests::it ... ok");
        if let Some(log) = &log {
            let line = format!("{}\n", std::process::id());
            std::fs::OpenOptions::new().create(true).append(true).open(format!("{log}.pids")).unwrap().write_all(line.as_bytes()).unwrap();
        }
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
    if src.contains("SPINS") {
        println!("running 2 tests\ntest tests::it ... ok");
        if let Some(log) = &log {
            let line = format!("{}\n", std::process::id());
            std::fs::OpenOptions::new().create(true).append(true).open(format!("{log}.pids")).unwrap().write_all(line.as_bytes()).unwrap();
        }
        let start = std::time::Instant::now();
        let mut n = 0u64;
        while start.elapsed().as_secs() < 60 { n = std::hint::black_box(n.wrapping_mul(31).wrapping_add(1)); }
        println!("test tests::stuck ... ok\n\ntest result: ok. 2 passed; 0 failed; 0 ignored");
        return;
    }
    if src.contains("BUSY") {
        let start = std::time::Instant::now();
        let mut n = 0u64;
        while start.elapsed().as_secs() < 6 { n = std::hint::black_box(n.wrapping_mul(31).wrapping_add(1)); }
    }
    let timed = args.windows(2).any(|w| w[0] == "-Z" && w[1] == "unstable-options") && args.iter().any(|a| a == "--report-time") && std::env::var_os("RUSTC_BOOTSTRAP").is_some();
    println!("running 1 test\ntest tests::it ... ok{}\n\ntest result: ok. 1 passed; 0 failed; 0 ignored", if timed { " <0.012s>" } else { "" });
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
# dealer's queue is another; DEAL_NONE=1: it tells this shard of another queue none of its own (`<n> - <digest>`);
# DEAL_STOP=<unit> :: <by> :: <why>: once that unit started (GATESTART), main's stop of it (`devhub bucket stop`)
dealer='import os, sys, time
out, told, taken, digest, none, stop = sys.argv[1:7]
taken = {int(x) for x in taken.split(",") if x}
queue, nxt, answered = None, 0, set()
while True:
    for l in (open(out).read().splitlines() if os.path.exists(out) else []):
        w = l.split()
        if stop and l.startswith("GATESTART " + stop.split(" :: ")[0] + " "):
            with open(told, "a") as f:
                f.write("stop " + stop + "\n")
            stop = ""
        if w[:1] == ["GATEQUEUE"] and len(w) == 4:
            queue = (int(w[2]), digest or w[3])
        elif w[:1] == ["GATEWANT"] and len(w) == 3 and queue and w[2] not in answered:
            answered.add(w[2])
            while nxt in taken:
                nxt += 1
            said = f"{w[2]} - {queue[1]}" if none else f"{w[2]} {nxt} {queue[1]}" if nxt < queue[0] else f"{w[2]} -"
            nxt += 1
            with open(told, "a") as f:
                f.write(said + "\n")
    time.sleep(0.1)'
gate() { # gate <label> <rev> <expect ok|fail|lost> <ran: packages, sorted> <grep -F'd lines, | between>: land_gate.sh testrun
  local out code ran want pid st deals=""
  g checkout -q "$2"
  rm -f "$dir/ran" "$dir/ran.env" "$dir/out"
  if [ "${GATE_QUEUE:-}" = 1 ] && [ -n "${DEVHUB_TOLD:-}" ]; then
    : > "$DEVHUB_TOLD"; python3 -I -c "$dealer" "$dir/out" "$DEVHUB_TOLD" "${DEAL_TAKEN:-}" "${DEAL_DIGEST:-}" "${DEAL_NONE:-}" "${DEAL_STOP:-}" & deals=$!
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
  case "$3:$code" in ok:0|fail:1|lost:4) ;; *) fail="$fail $1(exit $code)"; echo "$out" | tail -20;; esac
  [ "$ran" = "$4" ] || { fail="$fail $1(ran: ${ran:-none}; want $4)"; }
  [ ! -s "$dir/ran.env" ] || fail="$fail $1(its units inherited $(tr ' ' '\n' < "$dir/ran.env" | sort -u | paste -sd' ' -))"
  local IFS='|'
  for want in $5; do
    echo "$out" | grep -qF -- "$want" || { fail="$fail $1(no \`$want\`)"; echo "$out" | tail -20; }
  done
}
mkdir -p "$dir/shape-keep"
DEVHUB_KEEP=$dir/shape-keep gate shape shape ok "makepad-draw test-run-shape" "GATEUNIT ok test test-run-shape|GATEUNIT ok test makepad-draw|NODEGATE-OK testrun 2 packages|GATETIME test test-run-shape |GATESTART test makepad-draw "
[ -z "$(find "$dir/shape-keep" -mindepth 1 -print -quit)" ] || fail="$fail shape(green gate published a keep)"
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
# a dealer whose queue is another's (a run's shard asked after a master restart, 11-17): told a unit of it, the shard
# runs none and says no verdict, as a lost lease's run (exit 4, its bucket runs it once more), never a red; told none
# of it, it runs none and passes empty. No lease telling it: it fails
GATE_QUEUE=1 DEVHUB_TOLD=$dir/told DEVHUB_LEASE=L7 DEAL_DIGEST=000000000000 GATE_TIMES=$times gate stale shape lost "" "testrun: the dealer's queue 000000000000 is not this shard's|testrun: no verdict: told a unit of another queue"
! echo "$gate_out" | grep -qE '^(NODEGATE-|GATESTEP fail)' || fail="$fail stale(a verdict said)"
GATE_QUEUE=1 DEVHUB_TOLD=$dir/told DEVHUB_LEASE=L7 DEAL_DIGEST=000000000000 DEAL_NONE=1 GATE_TIMES=$times gate other shape ok "" "is not the run's 000000000000 (made from other measures): it takes none of it|NODEGATE-OK testrun 0 packages"
GATE_QUEUE=1 gate untold shape fail "" "NODEGATE-FAIL: queue (no \$DEVHUB_TOLD"
gate plan plan fail "test-run-plan" "GATEUNIT fail test test-run-plan|GATEREPRO test test-run-plan :: makepad test test-run-plan --profile release-checks|NODEGATE-FAIL: test:test-run-plan|FAILED tests::it|  panicked at libs/plan/src/lib.rs:1:1 -- broken: $(printf 'x%.0s' {1..500})|FAILED tests::other|  panicked at libs/plan/src/lib.rs:2:2 -- the \"quoted\" one|FAILED tests::harness|  panicked at libs/plan/src/lib.rs:3:3 -- run.sh failed:|    case a: ok|    case b: wrong output"
! echo "$gate_out" | grep -q 'RUST_BACKTRACE' || fail="$fail plan(the runner's own note said as the harness's output)"
# every failing test named, each with its own panic whole (a passing test's caught one is no failure's)
! echo "$gate_out" | grep -qE 'lib.rs:9:9|its second line' || fail="$fail plan(a passing test's panic or a second line said)"
# a failed unit's test artifacts stay in the lease's keep, its app home aside; a passing unit's are not kept
keep=$dir/keep
DEVHUB_KEEP=$keep MAKEPAD_TEST_ARTIFACTS=$dir/artifacts GATE_RETRY="test test-run-shape" gate kept plan fail "test-run-plan test-run-shape" "GATEUNIT fail test test-run-plan|GATEUNIT ok test test-run-shape|test test-run-plan: its test artifacts kept as artifacts-test-run-plan (1 files)"
[ "$(cat "$keep/artifacts-test-run-plan/tests_it/run.log" 2>/dev/null)" = ran ] || fail="$fail kept(the failed unit's artifacts)"
[ ! -e "$keep/artifacts-test-run-plan/tests_it/home" ] || fail="$fail kept(its app home)"
[ ! -e "$keep/artifacts-test-run-shape" ] || fail="$fail kept(a passing unit's)"
# a red unit whose tests ran is a measure; one whose build failed before its tests started is none, its seconds nor
# its peak (a failed build's time stood for a 20-38 min unit's, and the next run's watch read it against that, 11-16)
echo "$gate_out" | grep -q "^GATETIME test test-run-plan " || fail="$fail kept(a red unit's seconds are no measure)"
g checkout -q -b unbuilt "$base"
lib libs/plan test-run-plan '// UNBUILT
pub fn floor(w: u32) -> u32 { test_run_shape::area(w) }' '[dependencies]
test-run-shape = { path = "../shape" }'
g commit -q -am unbuilt
HUNG_LOOK=1 gate unbuilt unbuilt fail "test-run-plan" "GATEUNIT fail test test-run-plan|missing field|NODEGATE-FAIL: test:test-run-plan"
! echo "$gate_out" | grep -qE "^GATE(TIME|PEAK) test test-run-plan " || fail="$fail unbuilt(a failed build's seconds or peak are a measure)"
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
# a target measured past GATE_SPLIT_S runs in parts, k = one per GATE_SPLIT_S of its measure: the other's lib at
# 1300 s in 3, each its share of the target's tests by sorted name (the stub lists two: the third part has none and
# runs nothing), each measured alone, each test's time said (GATETESTS); the next run's k follows the
# parts' measures (500 + 400 s: 2 parts), and its parts' shares its tests' (GATE_TEST_TIMES, deal_part): the slow
# tests::stuck alone in part 1, where by sorted name it went to part 2
GATE_RETRY="test test-run-other" GATE_TIMES="test test-run-other=700;test test-run-other --lib=1300" GATE_PROVEN="test test-run-other --test more" gate parts shape ok "makepad-draw test-run-other --lib test-run-other --lib test-run-shape" "testrun: test test-run-other --lib: 1300 s of its last measure, in 3 parts of its tests|test test-run-other --lib part 1/3: 1 of its target's tests|GATEUNIT ok test test-run-other --lib part 1/3|GATEUNIT ok test test-run-other --lib part 2/3|test test-run-other --lib part 3/3: no test of its target in its share|GATEUNIT ok test test-run-other --lib part 3/3|GATETIME test test-run-other --lib part 2/3 |GATETESTS test test-run-other --lib part 1/3 :: tests::it=0.0/"
# (its values as a devhub bucket hands them over: files of $GATE_QUEUE_DIR, one per variable, bucket.rs QueueInput)
mkdir -p "$dir/queue2"
printf '%s' "test test-run-other --lib part 1/3=500;test test-run-other --lib part 2/3=400;test test-run-other --lib part 3/3=0" > "$dir/queue2/GATE_TIMES"
printf '%s' "test test-run-other --test more :: tests::it=900;test test-run-other --lib :: tests::stuck=300 tests::it=1" > "$dir/queue2/GATE_TEST_TIMES"
printf '%s' "test test-run-other --test more" > "$dir/queue2/GATE_PROVEN"
GATE_RETRY="test test-run-other" GATE_QUEUE_DIR=$dir/queue2 gate parts2 shape ok "makepad-draw test-run-other --lib test-run-other --lib test-run-shape" "testrun: test test-run-other --lib: 900 s of its last measure, in 2 parts of its tests|test test-run-other --lib part 1/2: 1 of its target's tests, 300 s of their last measures (the parts 1-300 s; 2 of the target's 2 measured)|test test-run-other --lib part 2/2: 1 of its target's tests, 1 s of their last measures|GATEUNIT ok test test-run-other --lib part 2/2"
# the other's Cargo.toml runs its tests/more.rs under cargo (native-tests): the package runs a unit per target, its lib
# under the JIT and more under `cargo test`, never the JIT, whose red says the cargo command that shows it; a
# declared target the package lacks is said
g checkout -q -b native "$base"
printf '\n[package.metadata.makepad.native-tests]\n"test:more" = "it runs its own binary"\n"test:gone" = "no such target"\n' >> "$dir/repo/libs/other/Cargo.toml"
put libs/other/tests/more.rs '#[test] fn more() { panic!("red under cargo") }'
g commit -q -am native
gate native native fail "test-run-other --lib" "test test-run-other --test more: under cargo, as its Cargo.toml's native-tests says: it runs its own binary|GATEUNIT ok test test-run-other --lib|GATEUNIT fail test test-run-other --test more|red under cargo|GATEREPRO test test-run-other --test more :: cargo test --release -p test-run-other --test more|testrun: test-run-other's native-tests names test:gone, which is none of its test targets|NODEGATE-FAIL: test:test-run-other---test-more"
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
DEVHUB_KEEP=$keep DEVHUB_LEASE=L41 GATE_RETRY="test test-run-other" GATE_TIMES="test test-run-other=40;test test-run-plan=30;test test-run-shape=20;test makepad-draw=10" HUNG_QUIET=3 HUNG_LOOK=1 gate hang hang fail "makepad-draw test-run-other test-run-plan test-run-shape" "GATEUNIT ok test test-run-other|GATEUNIT fail test test-run-plan|HUNG: tests::stuck; its stacks: testrun-test-run-plan.hung|main is blocked: the stub waits|GATEREPRO test test-run-plan :: makepad test test-run-plan --profile release-checks -- tests::stuck --exact|GATEUNIT ok test test-run-shape|GATEUNIT ok test makepad-draw|NODEGATE-FAIL: test:test-run-plan"
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
# a unit at work in its second test, which its watch never stops by itself, stopped by main (`devhub bucket stop`:
# its lease told so beside the dealer's lines): it fails HUNG at its watch's next look, saying who stopped it and
# why first, and the test it was in (its --list after it, which the same stop does not stop), its processes gone;
# the shard's other units pass. (An in-process hub's tick kept a Fab test's wait going 40 min beside its app at
# 0%, and nothing but a cancel of the whole run could end it, 11-27)
g checkout -q -b stop "$base"
lib libs/shape test-run-shape '// SPINS
pub fn area(w: u32) -> u32 { w.pow(2) }'
g commit -q -am stop
rm -f "$dir/ran.pids"
DEVHUB_KEEP=$keep GATE_RETRY="test test-run-other" GATE_QUEUE=1 DEVHUB_TOLD=$dir/told DEVHUB_LEASE=L7 DEAL_STOP="test test-run-shape :: main :: stuck spinning" GATE_TIMES="test test-run-shape=40;test test-run-other=20;test makepad-draw=10" HUNG_QUIET=600 HUNG_LOOK=1 gate stop stop fail "makepad-draw test-run-other test-run-shape" "GATEUNIT fail test test-run-shape|test test-run-shape: HUNG (stopped by main: stuck spinning, |HUNG: stopped by main: stuck spinning, |s into the unit; tests::stuck; its stacks: testrun-test-run-shape.hung in the run's kept logs|GATEREPRO test test-run-shape :: makepad test test-run-shape --profile release-checks -- tests::stuck --exact|GATEUNIT ok test test-run-other|GATEUNIT ok test makepad-draw|NODEGATE-FAIL: test:test-run-shape"
! echo "$gate_out" | grep -q "^GATETIME test test-run-shape " || fail="$fail stop(a stopped unit's seconds are no measure)"
while read -r pid; do
  ! kill -0 "$pid" 2>/dev/null || { fail="$fail stop(its process $pid still runs)"; kill -KILL "$pid"; }
done < "$dir/ran.pids"
if [ -n "$fail" ]; then echo "test_run FAILED:$fail"; exit 1; fi

# A second workspace for selection inside product subtrees. Reuse the built stub for the gate run.
set -e
(
  dir=$dir/products
  sandbox=apps/commercial/sandbox
  sim_dep='makepad-game-sim = { path = "../sim" }'
  lib "$sandbox/libs/sim" makepad-game-sim ''
  lib "$sandbox/libs/script" makepad-game-script '' "[dependencies]
$sim_dep"
  lib "$sandbox/libs/gen" makepad-game-gen '' "[dependencies]
$sim_dep"
  lib "$sandbox/libs/indirect" sandbox-indirect '' "[dependencies]
makepad-game-script = { path = \"../script\" }
[build-dependencies]
$sim_dep
[target.'cfg(any())'.dependencies]
$sim_dep"
  lib "$sandbox" makepad-sandbox '' '[dependencies]
makepad-game-sim = { path = "libs/sim" }
makepad-platform = { path = "../../../platform" }'
  lib apps/commercial/other/tests relax-commercial-tests '' '[dependencies]
makepad-game-sim = { path = "../../sandbox/libs/sim" }'
  lib apps/apps/create/core makepad-town-core ''
  lib apps/apps/create/app makepad-town '' '[dev-dependencies]
makepad-town-core = { path = "../core" }'
  lib platform makepad-platform ''
  lib libs/platform-user platform-user '' '[dependencies]
makepad-platform = { path = "../../platform" }'
  put Cargo.toml '[workspace]
members = ["apps/commercial/sandbox", "apps/commercial/sandbox/libs/*", "apps/commercial/other/tests",
           "apps/apps/create/*", "platform", "libs/platform-user"]
exclude = ["libs/stitch"]
resolver = "2"'
  g init -q -b products
  g add -A
  g commit -q -m products
  base=$(git -C "$dir/repo" rev-parse HEAD)

  expect_runs() { # name, source to touch, exact --runs output
    local got
    printf '\n' >> "$dir/repo/$2"
    g commit -q -am "$1"
    got=$(cd "$dir/repo" && python3 -I "$here/affected.py" --runs HEAD^ -)
    [ "$got" = "$3" ] || { printf '%s FAILED\nexpected:\n%s\ngot:\n%s\n' "$1" "$3" "$got"; return 1; }
  }
  # The script's split measure plus an unmeasured user fits exactly. The sandbox already runs.
  GATE_TIMES="test makepad-game-script=500;test makepad-game-script --lib=270;test makepad-sandbox=500" \
    expect_runs test_small_group_direct_user_sandbox_miss "$sandbox/libs/sim/src/lib.rs" \
"tests makepad-game-gen $sandbox/libs/gen
tests makepad-game-script $sandbox/libs/script
tests makepad-game-sim $sandbox/libs/sim
tests makepad-sandbox $sandbox"
  sim_rev=$(git -C "$dir/repo" rev-parse HEAD)
  expect_runs test_small_group_dev_dep_direct_user apps/apps/create/core/src/lib.rs \
'tests makepad-town apps/apps/create/app
tests makepad-town-core apps/apps/create/core'
  expect_runs test_shared_framework_crate_does_not_expand_in_slice1 platform/src/lib.rs \
"tests makepad-platform platform
tests makepad-sandbox $sandbox"

  mkdir -p "$dir/queue"
  printf '%s' 'test makepad-game-script=10;test makepad-game-script --lib=200;test makepad-game-gen=200' > "$dir/queue/GATE_TIMES"
  GATE_QUEUE_DIR=$dir/queue gate test_large_direct_user_wall_over_budget_defers_explicitly "$sim_rev" ok \
    "makepad-game-sim makepad-sandbox" "direct-users-deferred:$sandbox(2 pkgs,400s>300s)|NODEGATE-OK testrun 2 packages" || exit 1
  [ -z "$fail" ] || { echo "test_run FAILED:$fail"; exit 1; }

  put libs/stitch/Cargo.toml '[workspace]
members = ["core", "user"]
resolver = "2"'
  lib libs/stitch/core stitch-core ''
  lib libs/stitch/user stitch-user '' '[dependencies]
stitch-core = { path = "../core" }'
  python3 -I - "$here/affected.py" "$dir/repo" <<'PY'
import os, runpy, sys
from pathlib import Path

def test_load_gate_times_variants_and_parts():
    affected = runpy.run_path(sys.argv[1])
    cases = [
        ("test pkg=500;test pkg --lib=270", 270.0),
        ("test pkg=250;test pkg --features web=15", 265.0),
        ("test pkg --features web=15", 45.0),
        ("test pkg --lib=700;test pkg --lib part 1/2=350;test pkg --lib part 2/2=350", 700.0),
        ("test pkg --lib part 1/2=200;test pkg --lib part 2/2=200;"
         "test pkg --lib part 1/3=100;test pkg --lib part 2/3=100;test pkg --lib part 3/3=150", 400.0),
    ]
    previous = os.environ.get("GATE_TIMES")
    try:
        for times, expected in cases:
            os.environ["GATE_TIMES"] = times
            got = affected["load_gate_times"](None)
            assert got == {"pkg": expected}, (times, expected, got)
    finally:
        if previous is None:
            os.environ.pop("GATE_TIMES", None)
        else:
            os.environ["GATE_TIMES"] = previous

def test_nested_workspace_direct_users_included():
    affected = runpy.run_path(sys.argv[1])
    root = Path(sys.argv[2]).resolve()
    workspace = root / "libs/stitch"
    core, user = workspace / "core", workspace / "user"
    packages = {}
    members = affected["local"](root, affected["metadata"](workspace / "Cargo.toml"), packages)
    roots = affected["workspace_roots"](root, packages)
    assert roots == {core: workspace, user: workspace}, roots
    assert affected["package_group"](root, core, roots) == "libs/stitch"
    # The nested workspace's members come from its own Cargo metadata.
    added = affected["small_group_direct_users"](root, packages, members, {core}, {}, {core})
    assert added == {user}, added

test_load_gate_times_variants_and_parts()
test_nested_workspace_direct_users_included()
PY
)
# The three real failing gates published records only for files in their own directories.
python3 -I - "$keep" "$here/test_failures.py" "$dir/manifest-cli" <<'PY'
import json
from pathlib import Path
import subprocess
import sys

keep, helper, scratch = map(Path, sys.argv[1:])
for lease, pkg, hung in [("local", "test-run-plan", False), ("L41", "test-run-plan", True),
                         ("L7", "test-run-shape", True)]:
    run = (keep / ("gate-run." + lease)).read_text().splitlines()
    assert len(run) == 1 and run[0].startswith("landgate.")
    folder = keep / run[0]
    records = [json.loads(line) for line in (folder / "gate-manifest.jsonl").read_text().splitlines()]
    assert any(r["kind"] == "unit" and r["unit"] == "test " + pkg and "testrun-" + pkg in r["files"] for r in records)
    if hung:
        assert any(r["kind"] == "hung" and r["unit"] == "test " + pkg and r["test"] == "tests::stuck" for r in records)
    for r in records:
        files = r.get("files", []) + [r[k] for k in ("file", "unit_log", "names") if k in r]
        assert all((folder / f).is_file() for f in files), r

def cli(*args, check=True):
    return subprocess.run([sys.executable, "-I", helper, *args], check=check)

staged, dest, ptr = (scratch / f for f in ("staged", "manifest", "gate-run.local"))
cli("--manifest", dest, "--kind", "busy", "--unit", "test u", "state.busy")
for kind in ["test", "hung"]:
    cli("--manifest", staged, "--kind", kind, "--unit", "test u", "--test", "t", "--unit-log", "output", "state." + kind)
cli("--manifest", staged, "--kind", "unit", "--unit", "test u", "output", "part.names")
cli("--manifest", staged, "--kind", "busy_clear", "--unit", "test u")
cli("--flush-manifest", staged, dest, "--run-pointer", ptr, "landgate.demo")
records = [json.loads(line) for line in dest.read_text().splitlines()]
assert [r["kind"] for r in records] == ["busy", "test", "hung", "unit", "busy_clear"]
assert all(r["v"] == 1 and r["unit"] == "test u" for r in records)
assert records[3]["files"] == ["output", "part.names"]
assert cli("--run-pointer", ptr, "../point outside", check=False).returncode != 0
assert ptr.read_text() == "landgate.demo\n"
PY

# The fixture verdict may mention another directory. Only the gate-owned logs are collected.
(
  base=$dir/reg-said; export DEVHUB_KEEP=$base/parent/keep
  KEEP_LOGS=$here/keep_logs.sh; run_dir=landgate.reg1
  fx=$base/log/fixture-demo
  mkdir -p "$fx" "$DEVHUB_KEEP" "$base/parent/sibling"
  echo DEMO_LOG > "$fx/demo.log"
  echo SENTINEL > "$base/parent/sibling/sentinel.log"
  echo SIBLING_LOG > "$base/parent/sibling.log"
  if bash "$KEEP_LOGS" "$fx"; then exit 1; fi
  for seg in '..' '.' 'a/b' 'a\\b' '-bad'; do
    if bash "$KEEP_LOGS" "$fx" "$seg"; then exit 1; fi
  done
  if bash "$KEEP_LOGS" "$fx" "$run_dir" ..; then exit 1; fi
  [ -z "$(find "$DEVHUB_KEEP" -mindepth 1 -print -quit)" ]
  . <(sed -n '/^said()/,/^}/p' "$here/land_gate.sh")
  rc=0
  said "demo: OK" 400 "demo: FAIL; logs in .."$'\n'"demo: FAIL; logs in $base/parent/sibling" "$fx" fixture-demo || rc=$?
  [ "$rc" = 1 ] && [ ! -e "$fx" ] && [ "$(cat "$DEVHUB_KEEP/$run_dir/fixture-demo/demo.log")" = DEMO_LOG ] || exit 1
  [ -z "$(find "$DEVHUB_KEEP" \( -name sibling.log -o -name sentinel.log \) -print -quit)" ]
  [ ! -e "$base/parent/demo.log" ] && [ ! -e "$base/parent/sentinel.log" ] || exit 1
  mkdir -p "$fx"; echo PASS > "$fx/pass.log"
  said "demo: OK" 400 "demo: OK" "$fx" fixture-pass
  [ ! -e "$fx" ] && [ ! -e "$DEVHUB_KEEP/$run_dir/fixture-pass" ]
)

# The JIT session is taken from this launch's TMPDIR, including when another path was printed.
(
  cd "$here/../../.."
  base=$dir/reg-present; export DEVHUB_KEEP=$base/keep
  log=$base/log; run_dir=landgate.reg2; KEEP_LOGS=$here/keep_logs.sh
  mkdir -p "$log" "$DEVHUB_KEEP"
  echo UNRELATED_SESSION_LOG > "$base/other.log"
  export present_other=$base/other.log
  cat > "$base/stub_present.sh" <<'STUB'
echo OWN_SESSION_LOG > "$TMPDIR/relax-session-99999-1.log"
ln -s "$present_other" "$TMPDIR/relax-session-99999-link.log"
echo "relax: session log kept at $present_other"
exit 1
STUB
  B="bash $base/stub_present.sh"; PRESENT_WARM=1; PRESENT_WAIT=2; SETTLE=0; fail=""
  . <(sed -n '/^present()/,/^}/p; /^present_keep()/,/^}/p; /^settle_errors()/p' "$here/land_gate.sh")
  rc=0; present demo . demo || rc=$?
  [ "$rc" = 1 ]
  output=$DEVHUB_KEEP/$run_dir/present-demo
  grep -qF "relax: session log kept at $present_other" "$output/out.log"
  [ "$(cat "$output/relax-session-99999-1.log")" = OWN_SESSION_LOG ]
  [ ! -e "$output/other.log" ] && [ ! -e "$output/relax-session-99999-link.log" ] && [ ! -e "$DEVHUB_KEEP/present-demo" ] || exit 1
  if grep -rFq UNRELATED_SESSION_LOG "$DEVHUB_KEEP"; then exit 1; fi
)

# A manual stop captures a non-unit step, ends its process, and leaves the gate running.
(
  log=$dir/step-stop; mkdir -p "$log"
  export DEVHUB_TOLD=$log/told HUNG_LOOK=1
  printf '%s\n' 'stop step demo :: main :: stuck' > "$DEVHUB_TOLD"
  B=true fail=""
  . <(sed -n '/^watched()/,/^}/p; /^watched_step()/,/^}/p; /^gate_mark()/,/^}/p; /^watch_hung()/,/^}/p; /^stage_manifest()/,/^}/p; /^step_fail()/p' "$here/land_gate.sh")
  if watched_step demo "$log/demo" sh -c 'echo $$ > "$1"; exec sleep 20' _ "$log/pid" > "$log/verdict"; then exit 1; fi
  [ "$fail" = " demo" ] && [ -s "$log/demo.hung" ] || exit 1
  grep -q '^HUNG: stopped by main: stuck' "$log/verdict"
  grep -qx 'GATESTEP fail demo' "$log/verdict"
  if kill -0 "$(cat "$log/pid")" 2>/dev/null; then exit 1; fi
  watched_step next "$log/next" echo continued 4>"$log/markers" || exit 1
  grep -qx continued "$log/next"
  grep -q '^GATESTEPSTART next ' "$log/markers"
  grep -qx 'GATESTEPEND next' "$log/markers"
)

echo "test_run: ok"
