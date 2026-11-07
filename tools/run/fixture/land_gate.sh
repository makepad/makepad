#!/bin/bash
# land_gate.sh <fast <base> <cbase>|hot <base> <cbase>|testrun <base> <cbase>|tests <base> <cbase>|checks <base> <cbase>|full|full-a|full-b|full-t|stage>: checks under a devhub lease.
# The fast and hot buckets check a landed work/main tip against their previous tip. They never block landing.
#   full  : build (0 warnings), rustc-check, relax-compiler libtest build, x64 relax tests, tier fuzzer (fixed
#           seeds block, random batch never), makepad check of the 7 gate packages + Stage, headless Calc+Stage
#           present, boundary (provided_sweep.sh audits of the six smoke apps), warm_zero (Calculator: the warm launch shows no JIT spinner), warm_after_app_edit (Sandbox: a
#           new item in an app file stays warm), warm_after_hal (a HAL body edit keeps the stored builds), inspect_cold
#           (inspect during a cold start never stalls the app), relaunch (in-place relaunches keep the app alive,
#           show new first frames with the window kept and state handed over, and have no faults), diff_fast, zone leg
#   full-a/full-b/full-t: full in three (STEPS_A: cargo legs, checks, presents; STEPS_B: the other JIT runtime legs;
#           STEPS_T: the legs whose verdicts are times), A and B on two shared nodes at once, T on a timed lease's
#           box; together exactly the steps of full
#   fast  : the JIT binary of the landed tree (kept or built here, 0 warnings), devhub's binary when changed,
#           checks and warm presents of the affected set across the bucket's landings, plus affected JIT suites,
#           and one cargo check of the test targets the landings reach (both repos; step_testcheck).
#           GATE_PROVEN skips units whose inputs already passed. GATE_RETRY (fast and hot; units joined by ;) names
#           what failed earlier and has not passed since: gated whatever the diff reaches, until it passes.
#   hot   : boot the directly touched apps and always Calculator (one small drawing app: a broken draw path
#           fails every app) on a Linux node's warm JIT, then settle for 3 s; then the GPU guards of the packages
#           the landing reaches (affected.py GUARDS, step_guards).
#   stage : build, makepad check makepad-stage, headless Stage present (commercial-only branches)
#   testrun: the tests bucket: the test targets the landings reach (affected.py's `tests` lines: the changed
#           packages and their users, dev-dependents too, both repos) run under the tree's JIT (`makepad test
#           <pkg>`), one unit `test <pkg>` each, slowest first (GATE_TIMES); with GATE_QUEUE=1 a shard of the
#           run (one lease and box each) takes them a unit at a time from the queue its shards share (next_told).
#           GATE_PROVEN skips a package whose inputs passed; GATE_RETRY runs what failed until it passes;
#           GATE_FULL=1 runs every package's (the full bucket). After landing; it gates nothing.
#   tests : the fast bucket's test-target check alone (step_testcheck), in any cargo workspace (merge_break.sh)
#   checks: the fast bucket's JIT checks alone (affected.py's check lines, step_qcheck) with the tree's own JIT, in
#           any cargo workspace holding the gate's packages (jit_break.sh)
# Every mode runs in place: a devhub rev lease runs in the node's ONE tree (its synced checkout, moved to the gated
# commits: only changed files get new mtimes), with the node's one target, JIT binary and JIT cache, so it is warm.
# Prints "NODEGATE-OK <summary>" or "NODEGATE-FAIL: <steps>"; exit 0 / 1. Every step is a function named step_*;
# the zone leg is the ONE function step_zone (settle_stress.sh).
# Fast bucket proofs: GATE_PROVEN in (units joined by ;), GATEUNIT verdicts out.
mode=${1:-full}
proven() { case ";${GATE_PROVEN:-};" in *";$1;"*) return 0;; *) return 1;; esac; }
unit_line() { echo "GATEUNIT $1 $2"; }
unit_of() { echo "$1 $2${4:+ }${*:4}"; }
T=${CARGO_TARGET_DIR:-$PWD/target}
here=$(cd "$(dirname "$0")" && pwd)
# the node's one JIT through cargo (rebuilt when its sources changed, a ~0.2 s no-op otherwise)
B="cargo run --profile host -q --manifest-path $PWD/Cargo.toml -p makepad-run --bin makepad --"
log=$(mktemp -d "${TMPDIR:-/tmp}/landgate.XXXXXX") || exit 2
# tmpfs /tmp on the nodes: orphaned gate dirs filled it and crashed a hosted app. A failed gate's step logs
# (proofs, presents, depcheck) go to the lease's keep dir first, which its holder copies to the master
KEEP_LOGS=$here/keep_logs.sh
trap '[ -z "$fail" ] || bash "$KEEP_LOGS" "$log"; rm -rf -- "$log"' EXIT
fail=""; sum=""
# The master classifies before acquiring either lease; the shell only carries its verdict.
if [ -n "${DEVHUB_ARM64_VERDICT:-}" ]; then
  echo "$DEVHUB_ARM64_VERDICT"
  sum="$sum $DEVHUB_ARM64_VERDICT"
fi
echo "GATE $mode: steps before (a pool box, 10-30, in series 932 s): build 82 s, rustc_check 17, libtest_build 38, x64_tests 25, fuzz 164, checks 29, presents 27 (calc 8 + stage 19), warm_zero 30, warm_hal 49, warm_app_edit 159, inspect_cold 41, relaunch 44, diff 204, zone 18; after: the STEP lines"
export MAKEPAD_RUN_STD=$PWD/libs/relax/compiler/std

step_build() {
  CARGO_INCREMENTAL=0 cargo build --profile host -p makepad-run -p makepad-remote --bin makepad --bin makepad-launcher --bin makepad-remote -j8 > "$log/build" 2>&1 || { fail="$fail build"; tail -20 "$log/build"; return 1; }
  if grep -E "^(warning|error)" "$log/build" | grep -qv sandbox; then grep -E "^(warning|error)" "$log/build" | grep -v sandbox | head -20; fail="$fail build-warnings"; return 1; fi
}
# a tracked file naming a private address, path or host: except hot, it costs a small build
step_private() {
  cargo run -q --profile host -p makepad-private-check > "$log/private" 2>&1 || { head -20 "$log/private"; fail="$fail private"; }
}
step_rustc_check() {
  CARGO_INCREMENTAL=0 cargo check --release -p makepad-widgets -j8 > "$log/rustc" 2>&1 || { grep -E "^error" -A6 "$log/rustc" | head -20; fail="$fail rustc-check"; }
}
step_libtest_build() {
  CARGO_INCREMENTAL=0 cargo test --release -p relax-compiler --lib --no-run -j8 > "$log/libtest" 2>&1 || { grep -E "^error" -A6 "$log/libtest" | head -20; fail="$fail relax-compiler-libtest-build"; }
}
check_pkg() { # check_pkg <label> <pkg> <dir> [check options: --also=<libs> from affected.py's feature flips]
  # on CHECK_JOBS threads when step_qcheck runs it beside others, else on the lease's
  full=$(cd "$3" && ${CHECK_JOBS:+env MAKEPAD_RUN_JOBS=$CHECK_JOBS} $B check "${@:4}" "$2" 2>&1)
  # a flip's lib the program build does not compile (a provided crate behind the HAL, e.g. makepad-render
  # in makepad-run-android): no build of <pkg> compiles it under the flip; the lib's own check stands
  if echo "$full" | grep -qE "^makepad check: --also [^ ]+: not in [^ ]+'s build$"; then
    echo "$1: $(echo "$full" | grep -oE -- "--also [^ ]+: not in [^ ]+'s build" | head -1) (not compiled there)"; return 0
  fi
  line=$(echo "$full" | grep -E " errors$|load error" | sed "s/^.*all, /$1: /")
  # the errors themselves (bare `path:line:col: message` lines and their notes), not the stats lines
  echo "$line" | grep -qE "^$1: 0 errors$" || echo "$full" | grep -vE "^makepad(-run)? check:|^makepad-run |^ *$" | head -60
  echo "$line"; echo "$line" | grep -qE "^$1: 0 errors$" || fail="$fail $1"
  # a warning fails it too (0 warnings; devhub ci's Judge::Check the same): only errors counted here, and two
  # dead-code warnings of the JIT build stood from 10-31
  warn=$(echo "$full" | grep -E ':[0-9]+:[0-9]+: warning: ')
  [ -z "$warn" ] || { echo "$warn" | head -20; echo "$1: $(echo "$warn" | wc -l | tr -d ' ') warnings"; fail="$fail $1-warnings"; }
}
step_checks() {
  for p in makepad-example-counter makepad-app-calculator makepad-platform makepad-draw makepad-widgets makepad-widgets-core; do check_pkg $p $p .; done
  step_check_stage
  cr=$(bash tools/run/fixture/check_reject.sh 2>&1); echo "$cr"; case "$cr" in *"check_reject: ok"*) ;; *) fail="$fail check-reject";; esac
  mb=$(bash tools/run/fixture/merge_break.sh 2>&1); echo "$mb"; case "$mb" in *"merge_break: ok"*) ;; *) fail="$fail merge-break";; esac
  jb=$(bash tools/run/fixture/jit_break.sh 2>&1); echo "$jb"; case "$jb" in *"jit_break: ok"*) ;; *) fail="$fail jit-break";; esac
  lt=$(bash tools/run/fixture/lazy_typeck.sh 2>&1); echo "$lt"; case "$lt" in *"lazy_typeck: ok"*) ;; *) fail="$fail lazy-typeck";; esac
  ag=$(bash tools/run/fixture/agree.sh 2>&1); echo "$ag"; case "$ag" in *"agree: ok"*) ;; *) fail="$fail agree";; esac
  ct=$(bash tools/run/fixture/check_targets.sh 2>&1); echo "$ct"; case "$ct" in *"check_targets: ok"*) ;; *) fail="$fail check-targets";; esac
  tr=$(bash tools/run/fixture/test_run.sh 2>&1); echo "$tr"; case "$tr" in *"test_run: ok"*) ;; *) fail="$fail test-run";; esac
  gu=$(bash tools/run/fixture/guard_units.sh 2>&1); echo "$gu"; case "$gu" in *"guard_units: ok"*) ;; *) fail="$fail guard-units";; esac
  cu=$(bash tools/run/fixture/cargo_units.sh 2>&1); echo "$cu"; case "$cu" in *"cargo_units: ok"*) ;; *) fail="$fail cargo-units";; esac
  # (the checks above built this tree's JIT)
  jit=${CARGO_TARGET_DIR:-$PWD/target}/host/makepad
  se=$(bash libs/relax/compiler/tests/strict_errors/run.sh "$jit" 2>&1); echo "$se" | grep -E 'FAILED|^strict_errors'; case "$se" in *"strict_errors: ok"*) ;; *) fail="$fail strict-errors";; esac
  # `makepad check --json`'s rows: codes, phases and suggestions as the producers recorded them
  cj=$(bash libs/relax/compiler/tests/check_json/run.sh "$jit" 2>&1); echo "$cj" | grep -E '^[-+]\{|^unexpected|^check_json' | cut -c1-230; case "$cj" in *"check_json: ok"*) ;; *) fail="$fail check-json";; esac
  lc=$(python3 libs/relax/compiler/tests/live/run_cases.py --makepad "$jit" 2>&1) || fail="$fail live-cases"; echo "$lc" | grep -E '^(FAIL|XPASS|live cases)|^         v[0-9]' | cut -c1-230
  # the real-time case beside two more copies of itself: their compiles stay off its thread's cores
  # (its failure names the copies that failed and each one's longest call: `rt-lock-copies[1:5289us,3:5056us]`)
  rl=$(python3 libs/relax/compiler/tests/live/run_cases.py --makepad "$jit" --only rt_lock --copies 3 2>&1) || fail="$fail rt-lock-copies[$(echo "$rl" | sed -nE -e 's/^ *copy ([0-9]+): .*stall ([0-9]+) us.*/\1:\2us/p' -e t -e 's/^ *copy ([0-9]+): .*/\1/p' | paste -sd, -)]"; echo "$rl" | grep -E '^(FAIL|XPASS|live cases)|^         v[0-9]|copy [0-9]' | cut -c1-230
}
step_check_stage() { check_pkg stage makepad-stage apps/commercial; }
present() { # present <name> <dir> <pkg args...>: headless cold run from a fresh home; 1 when the app reports `present`
  name=$1; dir=$2; shift 2
  s=$log/hrun-$name; mkdir -p "$s"
  # its own empty app data and home (gone with $log): nothing an earlier run left in the tree or the box (a
  # draft another synth left open in the tree's local/synth reopened with 9 script errors); the same list as
  # tools/ci_leg/src/smoke.rs fresh_data
  d=$s/data; mkdir -p "$d"/{data,config,state,storage,home,synth,amp,aichat}
  fresh=(XDG_DATA_HOME="$d/data" XDG_CONFIG_HOME="$d/config" XDG_STATE_HOME="$d/state" MAKEPAD_STORAGE_DIR="$d/storage"
    MAKEPAD_TEST_HOME="$d/home" SYNTH_DATA_DIR="$d/synth" AMP_DATA_DIR="$d/amp" MAKEPAD_AICHAT_DIR="$d/aichat")
  if [ -n "$PRESENT_WARM" ]; then  # fast/hot: the node's one warm JIT cache (the tree's; keyed by the loader build id)
    (cd "$dir" && exec env "${fresh[@]}" ${PRESENT_JOBS:+MAKEPAD_RUN_JOBS=$PRESENT_JOBS MAKEPAD_RUN_O3_THREADS=$(( (PRESENT_JOBS + 1) / 2 ))} MAKEPAD_HIDE_WINDOWS=1 SANDBOX_MUTE=1 MAKEPAD_TRACE=startup $B run --report-to $s/report "$@" -- --remote > $s/out.log 2>&1) &
  else  # full: cold, from a fresh home
    rh=$s/home
    (cd "$dir" && exec env "${fresh[@]}" MAKEPAD_RUN_HOME=$rh MAKEPAD_RUN_CACHE_DIR=$rh/cache \
      MAKEPAD_HIDE_WINDOWS=1 SANDBOX_MUTE=1 MAKEPAD_TRACE=startup $B run --report-to $s/report "$@" -- --remote > $s/out.log 2>&1) &
  fi
  pid=$!
  for i in $(seq 1 $(( ${PRESENT_WAIT:-120} * 2 ))); do
    grep -q present $s/report 2>/dev/null && break
    kill -0 $pid 2>/dev/null || break
    # a JIT compile error or panic before the first frame fails the present now, not after the wait
    [ -n "$(settle_errors "$s")" ] && break
    sleep 0.5
  done
  # the settle window: the first frame comes before the app's worker threads do their work (Fab's model thread
  # panicked 6.4 s after a passing present), so the app lives on until the JIT is idle (two quiet looks in a row)
  # or SETTLE s (15); a panic (parked or not), a JIT compile error or an exit in that window fails the present
  # hot waits the full settle window
  settled=""
  if grep -q present $s/report 2>/dev/null; then
    app=$(pgrep -P $pid | head -1)  # the JIT under cargo run: the app's own pid, for inspect
    t0=$(date +%s); idle=0
    while [ $(( $(date +%s) - t0 )) -lt ${SETTLE:-15} ] && { [ "$mode" = hot ] || [ $idle -lt 2 ]; } && kill -0 $pid 2>/dev/null; do
      if [ "$mode" = hot ]; then sleep 0.5
      elif [ -n "$app" ] && $B inspect $app jit ms=500 2>/dev/null | grep -q '^idle'; then idle=$((idle + 1)); else idle=0; sleep 0.5; fi
    done
    kill -0 $pid 2>/dev/null || settled="exited during the settle window"
    bad=$(settle_errors "$s")
    [ -n "$bad" ] && settled="$settled${settled:+; }$bad"
  fi
  kill -0 $pid 2>/dev/null && kill $pid
  wait $pid 2>/dev/null
  n=$(grep -cx present $s/report 2>/dev/null)
  echo "$name present: $n${settled:+ (settle: $settled)}"
  [ "$n" = 1 ] || { fail="$fail $name-present"; bad=$(settle_errors "$s"); echo "$name: no present ${bad:+(failed before its first frame: $bad)}${bad:-within ${PRESENT_WAIT:-120} s}; its last log lines:"; tail -30 "$s/out.log"; return 1; }
  [ -z "$settled" ] || { fail="$fail $name-settle"; echo "$name: failed in the settle window after its first frame; its last log lines:"; tail -30 "$s/out.log"; return 1; }
}
settle_errors() { # settle_errors <run dir>: the panics and JIT compile errors of a run (its log and its report), one line
  { grep -E 'panic parked|stale access parked|panicked|FATAL|compile error' "$1/out.log" 2>/dev/null
    grep -E $'^(panic|error)\t' "$1/report" 2>/dev/null; } | head -3 | cut -c1-200 | tr '\n' ' '
}
step_present_calc() { present calc . makepad-app-calculator --no-default-features --features standalone; }
step_present_stage() { present stage apps/commercial makepad-stage; }
step_diff() { # the package programs build into the lease's warm target ($T), not a cold <tree>/target per gate
  # verify-tip passes VERIFY_BASE (the last verified tip): the diff programs test the JIT (libs/relax, tools/run,
  # platform) against rustc and the package programs' crates; no change there since the base: nothing to diff
  if [ "${DIFF_SCOPE:-all}" = affected ] && [ -n "${VERIFY_BASE:-}" ] && git cat-file -e "$VERIFY_BASE^{commit}" 2>/dev/null \
    && git diff --quiet "$VERIFY_BASE" HEAD -- libs/relax tools/run platform \
    && { [ -z "${VERIFY_CBASE:-}" ] || git -C apps/commercial diff --quiet "$VERIFY_CBASE" HEAD -- relax_tests; }; then
    echo "diff: skipped (no JIT, platform or diff-program change since ${VERIFY_BASE:0:10}; the nightly run diffs all)"; sum="$sum diff:skipped"; return 0
  fi
  DIFF_JOBS=${DIFF_JOBS:-8} DIFF_PKG_TARGET=${DIFF_PKG_TARGET:-$T} DIFF_CARGO_JOBS=${DIFF_CARGO_JOBS:-8} DIFF_TIERS="o1 base o3 counted" DIFF_CACHE=${X64_GATE_DIFF_CACHE:-$HOME/devhub/cache/diff_fast} tools/run/fixture/diff_fast.sh > "$log/diff" 2>&1
  df=$(tail -1 "$log/diff")
  # the program count is part of the verdict (files + package programs): a gate that ran fewer says why
  grep '^diff programs:' "$log/diff"; echo "$df"
  echo "$df" | grep -qE "^diff: ([0-9]+)/\1 equal" || { fail="$fail diff_fast"; grep -v '^diff \(tiers\|programs\):' "$log/diff" | tail -25 | cut -c1-240; }
  sum="$sum $df"
}
step_jit_suites() { # library test suites under the JIT, which pass under rustc: a JIT disagreement shows as a failing
  # test (csg_brep 2026-11-01: a pattern let without an initializer and an x64 imm32 truncation broke its fillets,
  # silently, with every other leg green). JIT_SUITES="<package> ..." overrides the list.
  local suite_status
  for p in $( { printf '%s\n' ${JIT_SUITES:-makepad-csg-brep}; sed -n 's/^suite //p' "$log/affected" 2>/dev/null; } | sort -u ); do
    if [ "$mode" = fast ] && proven "suite $p"; then
      echo "jit suite $p: proven"; unit_line proven "suite $p"; continue
    fi
    $B test "$p" > "$log/suite-$p" 2>&1
    suite_status=$?
    r=$(grep -E '^test result:' "$log/suite-$p" | tail -1)
    echo "jit suite $p: ${r:-no result}"
    case "$r" in
      "test result: ok."*)
        if [ "$mode" = fast ]; then
          if [ "$suite_status" = 0 ]; then unit_line ok "suite $p"
          else fail="$fail jit-suite:$p"; unit_line fail "suite $p"; fi
        fi;;
      *) fail="$fail jit-suite:$p"; test_failures "$log/suite-$p"
         [ "$mode" != fast ] || unit_line fail "suite $p";;
    esac
  done
}
step_x64_tests() { # SAFE1: every relax-backend / relax-codegen test on x64
  CARGO_INCREMENTAL=0 cargo test --release -p relax-backend -p relax-codegen --no-fail-fast -j8 > "$log/xtest" 2>&1
  bad=$(sed -n 's/^test \(.*\) \.\.\. FAILED$/\1/p' "$log/xtest" | sort -u)
  bad=$(printf '%s' "$bad" | tr '\n' ' ')
  [ -n "$bad" ] && fail="$fail x64-tests:[$bad]"
  grep -qE '^error(\[|:)' "$log/xtest" && fail="$fail x64-test-build"
}
step_fuzz() { # SAFE3: the tier fuzzer (codegen/tests/fuzz_tiers.rs, +checkra fatal). Fixed seeds BLOCK; a clock-seeded
  # random batch NEVER blocks (its disagreements print as FUZZ-NEW <seed> for triage: FUZZ_REDUCE=<seed> shrinks one)
  [ -f libs/relax/codegen/tests/fuzz_tiers.rs ] || return 0
  # the two batches run at once (each is one single-threaded test; the build is shared: the second waits on cargo's lock)
  first=$(( $(date +%s) % 1000000000 + 1000000 ))
  CARGO_INCREMENTAL=0 cargo test --release -p relax-codegen --test fuzz_tiers -j8 --no-run > "$log/fuzzb" 2>&1
  FUZZ_N=${FUZZ_RANDOM:-500} FUZZ_FIRST=$first CARGO_INCREMENTAL=0 cargo test --release -p relax-codegen --test fuzz_tiers -j8 random -- --nocapture > "$log/fuzzr" 2>&1 &
  fzr=$!
  ffirst=$(( 1 + (0x$(git rev-parse --short=7 HEAD) % 4) * ${FUZZ_FIXED:-500} ))
  FUZZ_N=${FUZZ_FIXED:-500} FUZZ_FIRST=$ffirst CARGO_INCREMENTAL=0 cargo test --release -p relax-codegen --test fuzz_tiers -j8 -- --nocapture > "$log/fuzz" 2>&1
  wait $fzr
  fz=$(grep -E '^fuzz: [0-9]+ programs' "$log/fuzz" | tail -1)
  if grep -qE '^test result: ok' "$log/fuzz"; then echo "fuzz fixed: $fz"; else
    echo "fuzz fixed FAILED: $fz"; grep -E '^fuzz: seed|panicked' "$log/fuzz" | cut -c1-240 | head -8; fail="$fail fuzz-fixed"; fi
  grep -E '^fuzz: seed' "$log/fuzzr" | sed 's/^fuzz: seed /FUZZ-NEW /' | cut -c1-240 | head -8
  echo "fuzz random (first=$first, never blocks): $(grep -E '^fuzz: [0-9]+ programs' "$log/fuzzr" | tail -1)"
  sum="$sum fuzz:${fz#fuzz: }"
}
step_zone() { # THE memory leg: settle_stress.sh (MEMSAFE: settles + settle rebuilds under ZAP + JOB_CHECK); zone_swap.sh on trees before rc-memb
  if [ -x tools/run/fixture/settle_stress.sh ]; then
    zs=$(PACKAGES=makepad-app-calculator WAIT=150 tools/run/fixture/settle_stress.sh 2>&1 | tail -1)
    echo "settle_stress: $zs"; echo "$zs" | grep -q FAIL && fail="$fail settle_stress"
  else
    zs=$(tools/run/fixture/zone_swap.sh 2>&1 | tail -1)
    echo "zone_swap: $zs"; echo "$zs" | grep -q FAIL && fail="$fail zone_swap"
  fi
}
said() { # said <ok prefix> <cut> <lines>: a fixture's verdict lines, cut to <cut> characters when its last line passed;
  # whole when it failed (its numbers are the why: a FAIL past 600 characters lost them, 11-05), and the fixture's
  # own logs ("logs in <dir>") kept beside the lease's (keep_logs.sh). Returns 1 when it failed.
  local d
  if [ "$(echo "$3" | tail -1 | cut -c1-${#1})" = "$1" ]; then echo "$3" | cut -c1-"$2"; return 0; fi
  echo "$3"
  d=$(echo "$3" | sed -n 's/.*logs in \([^ );]*\).*/\1/p' | tail -1)
  [ -n "$d" ] && bash "$KEEP_LOGS" "$d"
  return 1
}
step_warm_app_edit() { # JITCOLD2: a new item in an app file recompiles what names it, not the app (Sandbox: cold,
  # warm, then a new pub fn appended to one of its files; at most 480 first compiles, no JIT spinner).
  # MAX 480 (the script's default 400): the warm first-compile count grew with the app (observed
  # 400-405 across unrelated branches on 2026-10-30)
  [ -d apps/commercial/sandbox ] || { echo "warm_after_app_edit: skipped (no apps/commercial)"; return 0; }
  wa=$(WAIT=300 MAX=480 STEPS="item graph" tools/run/fixture/warm_after_app_edit.sh 2>&1 | tail -1)
  said "warm_after_app_edit: OK" 600 "$wa" || fail="$fail warm_after_app_edit"
  sum="$sum warm_app_edit:$(echo "$wa" | sed -n 's/.*\[item\][^]]*compiled \([0-9]*\) first.*/\1/p')"
}
step_warm_zero() { # JITLOOP-B: after one normal session the next Calculator launch shows no JIT spinner and no UI
  # block >= 100 ms (first compiles of never-run code are reported, not failed); Sandbox + Play stays a lane measurement
  wz=$(WAIT=200 tools/run/fixture/warm_zero.sh 2>&1 | tail -1)
  said "warm_zero: OK" 400 "$wz" || fail="$fail warm_zero"
  sum="$sum warm_zero:$(echo "$wz" | sed -n 's/.*warm: spinner \([0-9]*\), UI blocked max \([0-9]*\) ms.*/spinner\1.max\2ms/p')"
}
step_inspect_cold() { # JITDEADLOCK: inspect static/eval/locals in a loop during a cold Calculator start answer or bail,
  # and the app still presents with no long UI block (an inspect `static` during a cold start froze the app)
  ic=$(CLIENTS=1 ROUNDS=1 WAIT=150 tools/run/fixture/inspect_cold.sh 2>&1 | tail -3)
  said "inspect_cold: OK" 400 "$ic" || fail="$fail inspect_cold"
}
step_warm_hal() { # JITCOLD: a body-only HAL edit (a provided crate: the loader rebuilds) keeps the stored builds
  wh=$(WAIT=200 BUILD="cargo build --profile host -p makepad-run -p makepad-remote --bin makepad --bin makepad-launcher --bin makepad-remote -j8" tools/run/fixture/warm_after_hal.sh 2>&1 | tail -1)
  said "warm_after_hal: OK" 400 "$wh" || fail="$fail warm_after_hal"
}
step_relaunch() { # JITRELAUNCH: `session relaunch` 5x in place: the app stays alive and shows a new first frame
  # after every relaunch, with the window kept, state handed over, and no crash or fault line
  rl=$(WAIT=200 tools/run/fixture/relaunch.sh 2>&1 | tail -1)
  said "relaunch: OK" 400 "$rl" || fail="$fail relaunch"
}
step_boundary() { # JITPROV: the smoke apps and the WM reach provided crates only through their ops (`makepad check --boundary
  # all`: every body a run of the app compiles, from its main; a provided crate's static or rustc-laid field read from
  # JIT-compiled code is a violation, caught here instead of at a user's first start)
  bd=$(RUNS=0 SWEEP_OUT="$log/sweep" bash tools/run/fixture/provided_sweep.sh 2>&1)
  echo "$bd" | grep -E "^audit |^provided_sweep" | cut -c1-200
  echo "$bd" | grep -q "^provided_sweep: ok" || { fail="$fail boundary"; echo "$bd" | grep -A1 "^violation" | head -20 | cut -c1-300; }
}
# full-a + full-b + full-t = full; the jitlegs run A and B on pool boxes, each building, balanced on the measured step
# times (a pool box, 2026-10-30: A ~460 s, B ~370 s with the build; full in series ~930 s; T took ~160 s of A's and
# ~160 s of B's, 11-05)
STEPS_FULL="step_x64_tests step_fuzz step_checks step_boundary step_present_calc step_present_stage step_warm_zero step_warm_hal step_warm_app_edit step_inspect_cold step_relaunch step_diff step_jit_suites step_zone"
STEPS_A="step_x64_tests step_fuzz step_checks step_present_calc step_present_stage"
STEPS_B="step_boundary step_diff step_jit_suites step_zone"
# the verdicts that are times (a JIT spinner, first compiles in a frame, UI blocked ms, first frames): only a box of their
# own says them. Beside 8 other leases (32-44 of a 32-core pool box's cores granted) full-a's warm_after_app_edit failed with every
# session twice as slow, and passed 3 times on quiet boxes (11-05). The jitlegs `warm` leg runs them timed: a bench
# box, exclusive.
STEPS_T="step_warm_app_edit step_warm_zero step_warm_hal step_inspect_cold step_relaunch"
timed() { # timed <step fn>: runs it and prints "STEP <name> <s> s [start-end]" (the gate's wall-time breakdown)
  t=$(date +%s); t0=$(date +%T); "$1"; r=$?; echo "STEP ${1#step_} $(( $(date +%s) - t )) s [$t0-$(date +%T)]"; return $r
}
# ---- fast ----
step_jit() { # the JIT binary of this tree: the one this box built before from exactly its inputs (the hub names it:
  # MAKEPAD_LOADER, libs/devhub/src/lease/loaders.rs), else built here and kept (0 warnings)
  if [ -x "${MAKEPAD_LOADER:-}" ]; then B=$MAKEPAD_LOADER; echo "jit: kept ${MAKEPAD_LOADER##*-}"; return 0; fi
  out=$(bash tools/run/fixture/loader_build.sh 2>&1) || { fail="$fail build"; echo "$out"; return 1; }
  echo "$out" | grep -v '^LOADER-OK'
}
step_devhub() { # devhub's own binary when the bucket diff touches it (0 warnings)
  grep -qx 'devhub yes' "$log/affected" || return 0
  if proven "devhub makepad-devhub"; then echo "devhub: proven"; unit_line proven "devhub makepad-devhub"; return 0; fi
  CARGO_INCREMENTAL=0 cargo build --profile host -p makepad-devhub --bin makepad-devhub -j8 > "$log/devhub" 2>&1 || { fail="$fail devhub-build"; tail -20 "$log/devhub"; unit_line fail "devhub makepad-devhub"; return 1; }
  if grep -E "^(warning|error)" "$log/devhub" | grep -qv sandbox; then grep -E "^(warning|error)" "$log/devhub" | grep -v sandbox | head -20; fail="$fail devhub-warnings"; unit_line fail "devhub makepad-devhub"; return 1; fi
  # and its tests: the master restarts into this code, a break there stops the orchestration
  $B test makepad-devhub --lib > "$log/devhub-test" 2>&1 || { fail="$fail devhub-tests"; test_failures "$log/devhub-test"; unit_line fail "devhub makepad-devhub"; return 1; }
  grep -E "^test result" "$log/devhub-test" | head -2
  # the build a phone's JIT host embeds (no checkout: tools/run/android_host)
  $B test makepad-devhub --lib --no-default-features > "$log/devhub-test-bare" 2>&1 || { fail="$fail devhub-tests-no-checkout"; test_failures "$log/devhub-test-bare"; unit_line fail "devhub makepad-devhub"; return 1; }
  grep -E "^test result" "$log/devhub-test-bare" | head -1
  unit_line ok "devhub makepad-devhub"
}
step_affected() {
  python3 -I tools/run/fixture/affected.py "$QBASE" "$QCBASE" > "$log/affected" 2> "$log/affected.err" || { fail="$fail affected"; tail -5 "$log/affected.err"; return 1; }
  sed -n 's/^gone //p' "$log/affected" | while read -r unit; do unit_line gone "$unit"; done
  grep '^why' "$log/affected"; sum="$sum $(grep -c '^check' "$log/affected") checks $(grep -c '^present' "$log/affected") presents $(grep -c '^cargo' "$log/affected") cargo checks $(grep -c '^tests' "$log/affected") test checks"
  return 0
}
# reap_one: waits for one of at_once's runs still going (`running`: its unit by pid) to end, takes it off and says
# so (GATEEND <unit>). Their pids alone: bash keeps an ended job's status and a `wait -n` given its pid again
# returns at once, so the throttle, given every pid it had started, spun a core through each gate's whole run (the
# fast gate's checks, the tests shards' units: 39 sleeps in 20 min, 11-07). A wait that names no pid: none of them
# is a job of this shell, none runs.
reap_one() {
  local reaped="" p
  wait -n -p reaped "${!running[@]}" 2>/dev/null
  if [ -n "$reaped" ]; then set -- "$reaped"; else set -- "${!running[@]}"; fi
  for p; do echo "GATEEND ${running[$p]}"; unset "running[$p]"; done
}
# fast: the affected apps' checks and presents all at once on this one warm box, on its one JIT cache; each in its
# own subshell, so its output and failures are gathered here in affected.py's order
# Each run's start and end are said live (GATESTART <unit> <epoch s>, GATEEND <unit>), beside the runs' own output,
# which comes once they all ended: the master's check-in lists a unit running far over its last measure (bucket.rs
# slow_units). A test that hung busy (a stalled GStreamer pipeline polled at 4 % of a core) is no HUNG, and held
# its shard 73 min with nothing said (11-07).
# A line is taken once a run's slot is free: from the file, or from a next fn (into `f`, false at the end: the
# tests run's queue, next_told).
at_once() { # at_once <fn> <lines file> [at most N] [next fn]: runs `<fn> <fields of a line>` per line in parallel
  i=0; local max=${3:-0} next=${4:-next_line} running=() outs=() o f lines n_line=0
  mapfile -t lines < "$2"
  while :; do
    while [ "$max" -gt 0 ] && [ "${#running[@]}" -ge "$max" ]; do reap_one; done
    "$next" || break
    i=$((i + 1))
    # each run's files by its step and package (qcheck-4-makepad-widgets.out): numbered alone, the presents' runs
    # wrote over the checks' par-N files, and a failed check's output was gone from the kept logs (11-05)
    o="$log/${1%_one}-$i-${f[1]}"; outs+=("$o")
    # both files open here: present's own `for i` loop changed $i inside the subshell, so its failures
    # went to another run's .fail and the verdict passed with GATEUNIT fail lines
    ( fail=""; "$1" "${f[@]}" 3>&-; printf '%s' "$fail" >&3 ) > "$o.out" 2>&1 3> "$o.fail" &
    running[$!]=$(unit_of "${f[@]}")
    echo "GATESTART ${running[$!]} $(date +%s)"
  done
  # only its own: a bare wait also reaped the depcheck running beside it, whose `wait` then found no
  # child (11-03)
  while [ "${#running[@]}" -gt 0 ]; do reap_one; done
  for o in "${outs[@]}"; do
    cat "$o.out"; fail="$fail$(cat "$o.fail" 2>/dev/null)"
  done
}
next_line() { # at_once's next line of its file
  [ "$n_line" -lt "${#lines[@]}" ] || return 1
  read -r -a f <<< "${lines[n_line]}"; n_line=$((n_line + 1))
}
filter_units() {
  local unit f
  : > "$2"
  while read -r -a f; do
    unit=$(unit_of "${f[@]}")
    if proven "$unit"; then echo "q$1 ${f[1]}: proven"; unit_line proven "$unit"
    else printf '%s\n' "${f[*]}" >> "$2"; fi
  done < <(grep "^$1 " "$log/affected")
}
# a unit that fails says the command that shows it alone (GATEREPRO), for the lane to run
qcheck_one() {
  local unit=$(unit_of "$@")
  check_pkg "$2" "$2" "$3" "${@:4}"
  if [ -z "$fail" ]; then unit_line ok "$unit"; else echo "GATEREPRO $unit :: cd $3 && makepad check${4:+ ${*:4}} $2"; unit_line fail "$unit"; fi
}
qpresent_one() {
  local unit=$(unit_of "$@")
  pkg=$2; dir=$3; shift 3; present "$pkg" "$dir" "$pkg" "$@"
  if [ -z "$fail" ]; then unit_line ok "$unit"; else echo "GATEREPRO $unit :: cd $dir && makepad run $pkg${*:+ $*}"; unit_line fail "$unit"; fi
}
# the checks side by side on the lease's cores (MAKEPAD_RUN_JOBS), less the depcheck's while it runs beside them:
# at most one per CHECK_CORES of them, each on CHECK_CORES threads. Four a core, each on all the lease's threads, ran
# 20 checks on 160 threads: a 16-core box at load 52.8 under a gate charged 8 cores, its hub build and a lane's run
# waiting behind it (11-06). And at most one per CHECK_GB of the box's memory available at their start: 47 at once
# peaked at 27 GB on a 30 GB box beside its WM and AI hub, and the kernel's OOM kill took the box's user services
# down with them (11-03).
step_qcheck() {
  local cores=${MAKEPAD_RUN_JOBS:-8} avail max beside=""
  if [ -n "$depcheck" ] && kill -0 "$depcheck" 2>/dev/null; then
    cores=$(( cores - (cores + 1) / 2 )); beside=", the depcheck beside them"
  fi
  [ "$cores" -ge 1 ] || cores=1
  CHECK_JOBS=${CHECK_CORES:-2}; [ "$CHECK_JOBS" -le "$cores" ] || CHECK_JOBS=$cores
  max=$(( cores / CHECK_JOBS ))
  avail=$(awk '/^MemAvailable:/ { print int($2 / 1048576) }' /proc/meminfo 2>/dev/null)
  if [ -n "$avail" ]; then
    [ $(( avail / ${CHECK_GB:-1} )) -lt "$max" ] && max=$(( avail / ${CHECK_GB:-1} ))
  fi
  [ "$max" -ge 1 ] || max=1
  echo "qcheck: $(grep -c . "$log/checks") checks, at most $max at once on $CHECK_JOBS threads each$beside (${avail:-?} GB available)"
  at_once qcheck_one "$log/checks" "$max"
  CHECK_JOBS=""
}
# one cargo check of a lines file's packages (affected.py's `<word> <pkg> <dir>`), beside the JIT steps on half the
# lease's cores, niced. Its release profile has its own build directory in the node's one target: no wait on the JIT
# steps' cargo (host). A package fails when cargo could not compile it, or at `warnings` on a warning of its own
# (short-format lines under its directory); one it never reached (one it uses failed first) has no verdict. At
# `errors` a warning of its own is a `GATEUNIT warn` line, no failure.
# Each error or warning line is the package's whose directory holds its file most closely: the members package at
# apps/commercial took every Fab test's warning under its own name, and the command its name says (`cargo check
# --tests -p makepad-commercial-members`) compiles none of them (11-06). A unit that warns or fails says the command
# that shows it alone (GATEREPRO), for the lane to run.
# cargo_units <lines file> <LABEL> <failing step word> <errors|warnings> [cargo check args]
cargo_units() {
  local list=$1 label=$2 word=$3 level=$4 f out code unit own owned repro named=0 pkgs=(); shift 4
  while read -r -a f; do pkgs+=(-p "${f[1]}"); done < "$list"
  out=$(CARGO_TARGET_DIR=$T nice -n 10 cargo check --release --message-format short -j $(( (${MAKEPAD_RUN_JOBS:-8} + 1) / 2 )) "$@" "${pkgs[@]}" 2>&1); code=$?
  owned=$(echo "$out" | owner_of "$list")
  while read -r -a f; do
    unit=$(unit_of "${f[@]}")
    own=$(echo "$owned" | awk -F '\t' -v p="${f[1]}" '$1 == p { sub(/^[^\t]*\t/, ""); print }')
    repro="GATEREPRO $unit :: cargo check --release${*:+ $*} -p ${f[1]}"
    if echo "$out" | grep -qF "could not compile \`${f[1]}\`" || { [ "$level" = warnings ] && [ -n "$own" ]; }; then
      echo "$own" | head -40; echo "${f[1]}: cargo check failed"; echo "$repro"; unit_line fail "$unit"; fail="$fail ${f[1]}"; named=$((named + 1))
    else
      # at `errors`, its own warnings (each once: its lib and its test build both say it) before a warn line:
      # the bucket names who brought a new one, and the unit stays green
      if [ -n "$own" ]; then echo "$own" | awk '!seen[$0]++' | head -40; echo "$repro"; unit_line warn "$unit"; fi
      if [ "$code" = 0 ]; then unit_line ok "$unit"; fi
    fi
  done < "$list"
  if [ "$code" != 0 ] && [ "$named" = 0 ]; then
    echo "$out" | grep -E "^(error|warning)|: (error|warning)" | head -40; fail="$fail $word"
    # why, in one line that greps: one cargo check, so the crate that stopped it fails all those below
    echo "$label-FAIL: $(echo "$out" | grep -E '^error: could not compile' | sed -E 's/^error: //; s/ due to .*//' | sort -u | paste -sd, -) stopped the one cargo check of $(grep -c . "$list") crates; first: $(echo "$out" | grep -m1 -E ': error|^error')"
    # none to blame: none of them passed, and a later diff that reaches none of them is no pass
    while read -r -a f; do unit_line fail "$(unit_of "${f[@]}")"; done < "$list"
  fi
}
# cargo's short-format error and warning lines (stdin) as `<package>\t<line>`: the package of a lines file
# (`<word> <package> <dir>`) whose directory holds the line's file most closely (`.`: the root's, below every other)
owner_of() {
  awk -v list="$1" 'BEGIN { while ((getline l < list) > 0) { split(l, f, " "); dirs[f[3]] = f[2] } }
    /^[^ :]+:[0-9]+:[0-9]+: (error|warning)/ {
      file = substr($0, 1, index($0, ":") - 1); best = ""; most = -1
      for (d in dirs) { n = (d == ".") ? 0 : length(d); if ((d == "." || index(file, d "/") == 1) && n > most) { best = d; most = n } }
      if (most >= 0) print dirs[best] "\t" $0
    }'
}
# the other repo's crates that use a changed crate (affected.py `cargo` lines), their libs and bins (0 warnings)
step_depcheck() {
  [ -s "$log/cargo" ] || return 0
  cargo_units "$log/cargo" DEPCHECK cargo-check warnings
}
# the test targets the landings reach (affected.py `tests` lines: the changed packages and their users, both repos),
# after the depcheck in its slot, on what it checked: no other step compiles a #[cfg(test)] body, and two landings
# that merged cleanly broke makepad-fab's tests unseen, a new test calling build_preview with an argument the other
# landing took away (11-05). Errors fail it; a warning is a WARN to the landings that brought it (zero warnings is
# the rule, but a test target's unused helper is no break of the branch).
step_testcheck() {
  [ -s "$log/tests" ] || return 0
  cargo_units "$log/tests" TESTCHECK tests-check errors --tests
}
# the GPU guards (affected.py `guard` lines): render tests of user-visible effects that broke more than once with
# every test green, on this node's real GPU under the JIT, after every landing that reaches their package or the
# JIT. A filter that matches no test fails: a renamed guard is no pass. One unit `guard <pkg>` each: the bucket
# names the landing that changed its inputs, not every landing of the run.
step_guards() {
  local status r bad
  while read -r _ pkg dir filter; do
    (cd "$dir" && exec $B test "$pkg" --lib -- "$filter") > "$log/guard-$pkg" 2>&1
    status=$?
    r=$(grep -E '^test result:' "$log/guard-$pkg" | tail -1)
    echo "guard $pkg $filter: ${r:-no result}"
    bad=1
    case "$r" in
      "test result: ok. 0 passed"*) echo "guard $pkg: no test matches $filter";;
      "test result: ok."*) [ "$status" = 0 ] && bad=0;;
      *) test_failures "$log/guard-$pkg";;
    esac
    if [ "$bad" = 0 ]; then unit_line ok "guard $pkg"
    else fail="$fail guard-$pkg"; echo "GATEREPRO guard $pkg :: makepad test $pkg --lib -- $filter"; unit_line fail "guard $pkg"; fi
  done < "$log/guards"
}
# a cold present takes about PRESENT_CORES cores (its JIT compiling, its first pipelines): five side by side on
# a gate's 8 cores ran a pool box to load 31 and missed their first frames; so at most a present per
# PRESENT_CORES of the lease's cores runs at once, and they split the lease's cores (MAKEPAD_RUN_JOBS)
step_qpresent() {
  local cores=${MAKEPAD_RUN_JOBS:-8} at
  at=$(( cores / ${PRESENT_CORES:-4} )); [ "$at" -ge 1 ] || at=1
  PRESENT_JOBS=$(( cores / at ))
  at_once qpresent_one "$log/presents" "$at"
}
# A unit hangs when its processes (its session) do nothing for HUNG_QUIET s by the node's own idle look (devhub
# lease/exec.rs `busy`: under IDLE_PCT % of a core, past 0.2 s, and under 1 MB of I/O since the quiet began, or
# a process gone with what it had done): its stacks are kept, its processes stopped, and it fails HUNG while the
# other units go on. No time cap: a unit that works runs as long as it works. 9 min, a minute under its lease's
# own look (sched.rs QUIET_STALL_MS, 10 min): a hung test fails as itself and never makes its lease look hung
# and hold its box (a makepad-app-amp test sat in a GStreamer pipeline, its lease idle 10 min with hundreds of
# packages to go, holding its box's WM restart until it was cancelled, 11-07).
HUNG_QUIET=${HUNG_QUIET:-540} HUNG_LOOK=${HUNG_LOOK:-10} IDLE_PCT=5
# unit_activity <session>: its processes' CPU in µs (/proc stat utime, stime, cutime, cstime at 100 Hz), I/O
# bytes (io rchar + wchar, where readable: a JIT is not dumpable) and peak RSS in KB (status VmHWM: each one's
# own peak, between looks too) into ua_cpu, ua_io and ua_rss, without a fork
unit_activity() {
  local p s f k v
  ua_cpu=0; ua_io=0; ua_rss=0
  for p in /proc/[0-9]*; do
    read -r s 2>/dev/null < "$p/stat" || continue
    s=${s##*) }; read -r -a f <<< "$s"
    [ "${f[3]}" = "$1" ] || continue
    ua_cpu=$((ua_cpu + (f[11] + f[12] + f[13] + f[14]) * 10000))
    while read -r k v; do case $k in rchar:|wchar:) ua_io=$((ua_io + v));; esac; done 2>/dev/null < "$p/io"
    while read -r k v _; do case $k in VmHWM:) ua_rss=$((ua_rss + v)); break;; esac; done 2>/dev/null < "$p/status"
  done
}
# unit_watch <session> <out>: beside a run, until it is stopped: a run idle for HUNG_QUIET s gets its JIT stacks
# in <out>.hung, and its processes stopped. Its processes' summed peak RSS, the most at any look, in MB in
# <out>.peak (the tests run's GATEPEAK)
unit_watch() {
  local since=$SECONDS c0 i0 over p nap peak=0 out=$2
  trap '[ -z "$nap" ] || kill "$nap" 2>/dev/null; echo $((peak / 1024)) > "$out.peak"; exit 0' TERM
  unit_activity "$1"; c0=$ua_cpu; i0=$ua_io; peak=$ua_rss
  while :; do
    sleep "$HUNG_LOOK" & nap=$!; wait "$nap"; nap=""
    unit_activity "$1"; over=$(( (SECONDS - since) * 1000000 ))
    [ "$ua_rss" -le "$peak" ] || peak=$ua_rss
    if [ "$ua_cpu" -lt "$c0" ] || [ "$ua_io" -lt "$i0" ] || [ $((ua_cpu - c0)) -gt $(( over * IDLE_PCT / 100 > 200000 ? over * IDLE_PCT / 100 : 200000 )) ] || [ $((ua_io - i0)) -gt 1048576 ]; then
      since=$SECONDS; c0=$ua_cpu; i0=$ua_io; continue
    fi
    [ $((SECONDS - since)) -ge "$HUNG_QUIET" ] && break
  done
  echo $((peak / 1024)) > "$out.peak"
  { echo "== its processes did nothing (under $IDLE_PCT% of a core, no I/O) for $((SECONDS - since)) s; their JIT stacks"
    for p in /proc/[0-9]*; do
      read -r s 2>/dev/null < "$p/stat" || continue
      s=${s##*) }; read -r -a f <<< "$s"
      [ "${f[3]}" = "$1" ] || continue
      p=${p#/proc/}
      echo "-- pid $p: why-hung"; $B inspect "$p" why-hung 2>&1 | head -c 200000
      echo "-- pid $p: stacks"; $B inspect "$p" stacks thread=all depth=48 2>&1 | head -c 400000
    done; } > "$2.hung" 2>&1
  # its processes only: the session started for it
  for p in /proc/[0-9]*; do
    read -r s 2>/dev/null < "$p/stat" || continue
    s=${s##*) }; read -r -a f <<< "$s"
    [ "${f[3]}" = "$1" ] && kill -KILL "${p#/proc/}" 2>/dev/null
  done
}
# watched <out> <cmd...>: the command in a session of its own (a background job of this shell leads no group:
# setsid execs in place) with unit_watch beside it; its exit code, and <out>.hung when it hung
watched() {
  local out=$1 pid watcher code
  shift
  setsid "$@" > "$out" 2>&1 < /dev/null &
  pid=$!
  unit_watch "$pid" "$out" &
  watcher=$!
  wait "$pid"; code=$?
  kill "$watcher" 2>/dev/null; wait "$watcher" 2>/dev/null
  return "$code"
}
# hung_tests <run log> <--list log>: the tests the unit's last target started and did not finish: that target's
# tests (its --list section: the run's targets in the same order) less those it reported
hung_tests() {
  awk -v run="$1" 'BEGIN { while ((getline l < run) > 0) {
        if (l ~ /^running [0-9]+ tests?$/) { k++; delete done; open = 1 }
        else if (l ~ /^test result:/) open = 0
        else if (match(l, /^test .* \.\.\. /)) done[substr(l, 6, RLENGTH - 10)] = 1 } }
    open && /^[0-9]+ tests, [0-9]+ benchmarks$/ { s++; next }
    open && s == k - 1 && /: test$/ { name = substr($0, 1, length($0) - 6); if (!(name in done)) print name }' "$2"
}
# test_failures <log>: a test run's failures as the bucket log keeps them: every failing test named in full, each
# with its first panics whole (where, and the message's first line), then the rest that says why (errors, the
# runner's own lines, its failed results), that middle cut to 12 lines of 400 characters (`head -20 | cut -c1-400`
# dropped the third failing test's name and every long assertion, 11-07). A panic is a test's when its thread
# is named for the test, else (a JIT report, a thread of its own) when it came after the run's last test status
# line before the test's FAILED; one test's panics once per place, a resumed unwind's `Box<dyn Any>` only alone
test_failures() {
  awk '
  # a JSON string field of a report line, its first line, unescaped
  function jstr(s, key,    i, v) {
    i = index(s, "\"" key "\":\""); if (!i) return ""
    s = substr(s, i + length(key) + 4)
    if (!match(s, /^([^"\\]|\\.)*/)) return ""
    v = substr(s, 1, RLENGTH); gsub(/\\\\/, "\001", v)
    i = index(v, "\\n"); if (i) v = substr(v, 1, i - 1)
    gsub(/\\"/, "\"", v); gsub(/\\t/, "\t", v)
    # (an awk gsub writes a backslash its own way: one awk doubled it)
    while ((i = index(v, "\001")) > 0) v = substr(v, 1, i - 1) "\\" substr(v, i + 1)
    return v
  }
  function add(name, loc, msg,    k) {
    k = name SUBSEP loc; if (k in seen) return; seen[k] = 1
    if (msg == "Box<dyn Any>") { if (!(name in boxed)) boxed[name] = loc " -- " msg; return }
    np[name]++; pan[name, np[name]] = loc " -- " msg
  }
  function failed(name) { if (!(name in isfail)) { isfail[name] = 1; nf++; order[nf] = name } }
  function loose(loc, msg) { ns++; sl[ns] = loc; sm[ns] = msg; last_loc = loc; last_msg = msg; have_last = 1 }
  { sub(/\r$/, "") }
  want { want = 0; if (wname != "") add(wname, wloc, $0); else loose(wloc, $0); next }
  /^thread '\''.*'\''( \([0-9]+\))? panicked at .*:$/ {
    t = $0; sub(/^thread '\''/, "", t); sub(/'\''( \([0-9]+\))? panicked at .*$/, "", t)
    wloc = $0; sub(/^.* panicked at /, "", wloc); sub(/:$/, "", wloc)
    wname = (t == "main" || t == "<unnamed>") ? "" : t; want = 1; next
  }
  /^  report: \{"kind":"panic"/ { loose(jstr($0, "location"), jstr($0, "message")); next }
  /^test .* \.\.\. FAILED$/ {
    name = substr($0, 6, length($0) - 16); failed(name)
    for (i = 1; i <= ns; i++) add(name, sl[i], sm[i])
    if (!ns && !np[name] && !(name in boxed) && have_last) add(name, last_loc, last_msg)
    ns = 0; next
  }
  /^test .* \.\.\. (ok|ignored)/ { ns = 0; next }
  /^failures:$/ { list = 1; next }
  /^---- .* stdout ----$/ { list = 0; next }
  list && /^    [^ ]+$/ { failed(substr($0, 5)); next }
  /^test result:/ { list = 0 }
  /^error|^makepad test: |^makepad: |^test result: FAILED/ { nr++; if (nr <= 12) rest[nr] = substr($0, 1, 400) }
  END {
    for (i = 1; i <= nf; i++) {
      name = order[i]; print "FAILED " name
      for (j = 1; j <= np[name] && j <= 3; j++) print "  panicked at " pan[name, j]
      if (np[name] > 3) print "  (and " np[name] - 3 " more of its panics in the log)"
      if (!np[name] && (name in boxed)) print "  panicked at " boxed[name]
    }
    if (!nf) for (i = 1; i <= ns && i <= 3; i++) print "panicked at " sl[i] " -- " sm[i]
    for (i = 1; i <= nr && i <= 12; i++) print rest[i]
    if (nr > 12) print "(and " nr - 12 " more such lines in the log)"
  }' "$1"
}
# one package's test targets under the JIT, as `cargo test -p <pkg>` runs them (doc tests aside): its failing
# tests and panics, or HUNG and the tests it still ran, and the command that runs it alone (GATEREPRO)
testrun_one() {
  # (a split package's target: `test <pkg> <dir> --lib`, its unit `test <pkg> --lib`, its log its own)
  local unit code r hung run name=$2${4:+-${*:4}}
  unit=$(unit_of "$@"); name=${name// /-}
  run=(env ${TEST_JOBS:+MAKEPAD_RUN_JOBS=$TEST_JOBS} MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 $B test "$2" "${@:4}")
  local t0=$SECONDS
  watched "$log/testrun-$name" "${run[@]}"; code=$?
  # its seconds, which the next run deals the slowest units out by (GATE_TIMES); a hung unit's are no measure:
  # one stopped after 78 min took a shard of its own in the next run's deal, and every unit not measured yet with
  # it (as the slowest measured), 11-07
  [ "$code" != 0 ] && [ -s "$log/testrun-$name.hung" ] || echo "GATETIME $unit $((SECONDS - t0))"
  # its peak RSS, which the next run's shards start it by (GATE_PEAKS)
  [ -s "$log/testrun-$name.peak" ] && echo "GATEPEAK $unit $(< "$log/testrun-$name.peak")"
  r=$(grep -E '^test result:' "$log/testrun-$name" | awk '{ for (i = 1; i <= NF; i++) if ($i ~ /^(passed|failed);?$/) s[$i] += $(i - 1) } END { printf "%d passed, %d failed", s["passed;"], s["failed;"] }')
  # (a unit that ended by itself as its watch said HUNG: it ended)
  if [ "$code" != 0 ] && [ -s "$log/testrun-$name.hung" ]; then
    # inside a target: its tests, listed (watched too), less those it reported
    if awk '/^running [0-9]+ tests?$/ { o = 1 } /^test result:/ { o = 0 } END { exit !o }' "$log/testrun-$name"; then
      watched "$log/testrun-$name.list" "${run[@]}" -- --list
      hung=$(hung_tests "$log/testrun-$name" "$log/testrun-$name.list" | paste -sd' ' -)
    fi
    echo "$unit: HUNG ($(head -1 "$log/testrun-$name.hung" | sed 's/^== //; s/; their JIT stacks$//'): stopped; $r before it)"
    echo "HUNG: ${hung:-no test of a target: between its targets or before them}; its stacks: testrun-$name.hung in the run's kept logs"
    grep -E '"verdict"' "$log/testrun-$name.hung" | head -4 | cut -c1-400
    echo "GATEREPRO $unit :: makepad test $2${4:+ ${*:4}}${hung:+ -- $hung --exact}"
    unit_line fail "$unit"; fail="$fail test:$name"; return
  fi
  if [ "$code" = 0 ]; then echo "$unit: ok ($r, $(grep -c '^test result:' "$log/testrun-$name") targets)"; unit_line ok "$unit"; return; fi
  echo "$unit: FAILED (exit $code; $r)"
  test_failures "$log/testrun-$name"
  echo "GATEREPRO $unit :: makepad test $2${4:+ ${*:4}}"
  unit_line fail "$unit"; fail="$fail test:$name"
}
# shard_units <runs file>: the run's units, slowest first (GATE_TIMES: `<unit>=<s>` by `;`, each one's last
# measured seconds; one not measured yet as the median measured, at least 60 s: as the slowest, one hung unit's
# 4694 s made each new unit a shard's worth, 11-07). Every shard of a run makes the same list (the same tree,
# GATE_PROVEN, GATE_RETRY and GATE_TIMES): the queue they share (next_told). A package measured over GATE_SPLIT_S
# (its targets' sum once split) runs as one unit per test target (`test <pkg> --lib`, `--bin <name>`,
# `--test <name>`): makepad-fab's 25 targets in series took 1650 s, one unit longer than a shard's share (11-07).
shard_units() {
  python3 -I - "$1" "${GATE_TIMES:-}" "${GATE_PROVEN:-}" "${GATE_SPLIT_S:-600}" <<'PY'
import json, statistics, subprocess, sys
path, times, proven, split = sys.argv[1:5]
secs = {}
for t in times.split(";"):
    unit, _, s = t.rpartition("=")
    if unit:
        secs[unit] = float(s)
unknown = max(60.0, statistics.median(secs.values())) if secs else 60.0
proven = set(proven.split(";"))
def unit(w):
    return " ".join([w[0], w[1], *w[3:]])
def measure(pkg):
    parts = [s for u, s in secs.items() if u.startswith(f"test {pkg} ")]
    return sum(parts) if parts else secs.get(f"test {pkg}", 0.0)
metas = {}
def default_features(p):
    """The features `makepad test <pkg>` builds with (resolve.rs enabled_features of `default`)."""
    feats = p.get("features", {})
    hidden = {v[4:] for vs in feats.values() for v in vs if v.startswith("dep:")}
    implicit = {d.get("rename") or d["name"] for d in p.get("dependencies", []) if d.get("optional")} - hidden
    on, work = set(), ["default"]
    while work:
        r = work.pop()
        if r.startswith("dep:"):
            continue
        if "/" in r:
            dep = r.split("/", 1)[0]
            if dep in implicit:
                work.append(dep)
            continue
        if r in on:
            continue
        if r in feats:
            on.add(r)
            work.extend(feats[r])
        elif r in implicit:
            on.add(r)
    return on
def targets(pkg, d):
    """The package's test targets as `makepad test <pkg>` runs them (cargo test's: its lib, bins and tests, less
    those whose required features its default ones lack: named alone, such a target is an error)."""
    found = [p for m in metas.values() for p in m if p["name"] == pkg]
    if not found:
        meta = json.loads(subprocess.run(["cargo", "metadata", "--no-deps", "--offline", "--format-version", "1", "--manifest-path", f"{d}/Cargo.toml"], check=True, capture_output=True, text=True).stdout)
        metas[meta["workspace_root"]] = meta["packages"]
        found = [p for p in meta["packages"] if p["name"] == pkg]
    p = found[0]
    on = default_features(p)
    out = []
    for t in p["targets"]:
        if not t.get("test", True) or not set(t.get("required-features", [])) <= on:
            continue
        kinds = set(t["kind"])
        if kinds & {"lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"}:
            out.append(["--lib"])
        elif "bin" in kinds:
            out.append(["--bin", t["name"]])
        elif "test" in kinds:
            out.append(["--test", t["name"]])
    return sorted(out)
lines = []
for l in open(path).read().splitlines():
    w = l.split()
    if not w:
        continue
    if len(w) == 3 and measure(w[1]) > float(split):
        for sel in targets(w[1], w[2]):
            u = unit(w + sel)
            if u in proven:
                print(f"{u}: proven")
                print(f"GATEUNIT proven {u}")
            else:
                lines.append(" ".join(w + sel))
    else:
        lines.append(l)
def took(line):
    return secs.get(unit(line.split()), unknown)
lines.sort(key=lambda l: (-took(l), l))
open(path, "w").write("".join(l + "\n" for l in lines))
print(f"testrun: {len(lines)} units, {sum(took(l) for l in lines):.0f} s of their last measures, the slowest first")
PY
}
# next_told: at_once's next unit for a free slot, from the queue the run's shards share (GATE_QUEUE=1; every shard's
# $log/runs is the same, its digest said once: GATEQUEUE <lease> <units> <digest>). It asks the run's dealer (the
# master's tests run, bucket.rs Dealer: GATEWANT <lease> <n>, this lease's n-th ask) and waits for the answer in
# the lease's $DEVHUB_TOLD (`<n> <index> <digest>`: the queue's line <index>; `<n> -`: none left). A shard on a slow
# or busy box asks less often: a deal fixed at the start left the run's wall to the shard beside lane work, 1.20x
# its share (11-07).
next_told() {
  local said
  wants=$((wants + 1)); echo "GATEWANT ${DEVHUB_LEASE:-none} $wants"
  until said=$(grep -m1 "^$wants " "$DEVHUB_TOLD" 2>/dev/null); do sleep 0.2; done
  set -- $said
  [ "$2" != - ] || return 1
  if [ "$3" != "$queue_digest" ]; then
    echo "testrun: the dealer's queue $3 is not this shard's $queue_digest"; fail="$fail queue"; return 1
  fi
  read -r -a f <<< "$(sed -n "$(($2 + 1))p" "$log/runs")"
  got=$((got + 1))
  ram_room "$(unit_of "${f[@]}")"
}
# ram_room <unit>: waits (its units ending) until the units this shard runs and <unit> fit the RAM its lease got
# (DEVHUB_RAM_MB), each at its last peak (GATE_PEAKS; one not measured yet at an even share of it); alone it
# starts anyway. Three units at once peaked at 27-36 GB, so a shard asked 42.7 GB, which only an empty 60 GB box
# beside a resident lane had (11-07).
ram_room() {
  [ -n "${DEVHUB_RAM_MB:-}" ] || return 0
  local u sum
  while [ "${#running[@]}" -gt 0 ]; do
    sum=${PEAK_MB[$1]:-$((DEVHUB_RAM_MB / at))}
    for u in "${running[@]}"; do sum=$((sum + ${PEAK_MB[$u]:-$((DEVHUB_RAM_MB / at))})); done
    [ "$sum" -gt "$DEVHUB_RAM_MB" ] || return 0
    reap_one
  done
}
# the runs side by side on the lease's cores: at most one per TEST_CORES of them, each with that many JIT jobs
step_testrun() {
  local cores=${MAKEPAD_RUN_JOBS:-8} at
  at=$(( cores / ${TEST_CORES:-4} )); [ "$at" -ge 1 ] || at=1
  TEST_JOBS=$(( cores / at ))
  if [ "${GATE_QUEUE:-}" = 1 ]; then
    echo "testrun: units of the run's queue, at most $at at once"
    at_once testrun_one "$log/runs" "$at" next_told
  else
    echo "testrun: $(grep -c . "$log/runs") units, at most $at at once"
    at_once testrun_one "$log/runs" "$at"; got=$(grep -c . "$log/runs")
  fi
}
if [ "$mode" = testrun ]; then
  QBASE=${2:?testrun <base sha> <commercial base sha|->}; QCBASE=${3:--}
  python3 -I "$here/affected.py" --tests "$QBASE" "$QCBASE" > "$log/affected" 2> "$log/affected.err" || { tail -5 "$log/affected.err"; echo "NODEGATE-FAIL: affected"; exit 1; }
  : > "$log/runs"
  while read -r -a f; do
    if proven "test ${f[1]}"; then echo "test ${f[1]}: proven"; unit_line proven "test ${f[1]}"
    else printf 'test %s %s\n' "${f[1]}" "${f[2]}" >> "$log/runs"; fi
  done < <(grep '^tests ' "$log/affected")
  shard_units "$log/runs" || { echo "NODEGATE-FAIL: shard"; exit 1; }
  declare -A PEAK_MB=()
  IFS=';' read -r -a peaks <<< "${GATE_PEAKS:-}"
  for pk in "${peaks[@]}"; do [ -z "$pk" ] || PEAK_MB[${pk%=*}]=${pk##*=}; done
  got=0; wants=0
  if [ "${GATE_QUEUE:-}" = 1 ]; then
    [ -n "${DEVHUB_TOLD:-}" ] || { echo "NODEGATE-FAIL: queue (no \$DEVHUB_TOLD: no devhub lease of a hub that tells one)"; exit 1; }
    queue_digest=$(git hash-object "$log/runs" | cut -c1-12)
    echo "GATEQUEUE ${DEVHUB_LEASE:-none} $(grep -c . "$log/runs") $queue_digest"
  fi
  if [ -s "$log/runs" ] && timed step_jit; then timed step_testrun; fi
  who="[lease ${DEVHUB_LEASE:-none} lane ${MAKEPAD_TEST_LANE:-none} at $(git rev-parse --short=10 HEAD 2>/dev/null || echo unknown) in $PWD]"
  if [ -n "$fail" ]; then echo "NODEGATE-FAIL:$fail $who"; exit 1; fi
  echo "NODEGATE-OK testrun $got packages $who"; exit 0
fi
if [ "$mode" = tests ]; then
  QBASE=${2:?tests <base sha> <commercial base sha|->}; QCBASE=${3:--}
  python3 -I "$here/affected.py" --tests "$QBASE" "$QCBASE" > "$log/affected" 2> "$log/affected.err" || { tail -5 "$log/affected.err"; echo "NODEGATE-FAIL: affected"; exit 1; }
  filter_units tests "$log/tests"
  timed step_testcheck
  if [ -n "$fail" ]; then echo "NODEGATE-FAIL:$fail"; exit 1; fi
  echo "NODEGATE-OK tests $(grep -c . "$log/tests") test checks"; exit 0
fi
if [ "$mode" = checks ]; then
  QBASE=${2:?checks <base sha> <commercial base sha|->}; QCBASE=${3:--}
  python3 -I "$here/affected.py" "$QBASE" "$QCBASE" > "$log/affected" 2> "$log/affected.err" || { tail -5 "$log/affected.err"; echo "NODEGATE-FAIL: affected"; exit 1; }
  grep '^why' "$log/affected"
  filter_units check "$log/checks"
  if [ -x "${MAKEPAD_LOADER:-}" ]; then B=$MAKEPAD_LOADER
  else cargo build --profile host -q -p makepad-run --bin makepad || { echo "NODEGATE-FAIL: build"; exit 1; }; B=$T/host/makepad; fi
  timed step_qcheck
  if [ -n "$fail" ]; then echo "NODEGATE-FAIL:$fail"; exit 1; fi
  echo "NODEGATE-OK checks $(grep -c . "$log/checks") checks"; exit 0
fi
if [ "$mode" = hot ]; then
  QBASE=${2:?hot <makepad base sha> <commercial base sha|->}; QCBASE=${3:--}
  export PRESENT_WARM=1 PRESENT_WAIT=240 SETTLE=3  # the wait: see fast below
  if HOT=1 timed step_affected; then
    grep '^present ' "$log/affected" > "$log/presents"
    grep '^guard ' "$log/affected" > "$log/guards"
    if timed step_jit; then
      timed step_qpresent
      [ ! -s "$log/guards" ] || timed step_guards
    fi
  fi
  who="[lease ${DEVHUB_LEASE:-none} lane ${MAKEPAD_TEST_LANE:-none} at $(git rev-parse --short=10 HEAD 2>/dev/null || echo unknown) in $PWD]"
  if [ -n "$fail" ]; then echo "NODEGATE-FAIL:$fail $who"; exit 1; fi
  echo "NODEGATE-OK hot$sum $who"; exit 0
fi
if [ "$mode" = fast ]; then
  QBASE=${2:?fast <makepad base sha> <commercial base sha|->}; QCBASE=${3:--}
  # warm presents take 3-20 s alone, but a JIT binary new to the box, built here or prebuilt, starts on an empty JIT
  # cache: its presents are cold, and six cold boots at once on a loaded box took over 60 s (Stage, Sandbox and Fab
  # on a pool box): a hang still fails, later
  export PRESENT_WARM=1 PRESENT_WAIT=${PRESENT_WAIT:-240}
  timed step_private
  if timed step_affected; then
    filter_units check "$log/checks"
    filter_units present "$log/presents"
    filter_units cargo "$log/cargo"
    filter_units tests "$log/tests"
    # the depcheck runs beside the presents (screen- and frame-bound: few cores busy), not beside the JIT build
    # or the checks, which it slowed by 11-27 s on a pool box; with no presents, beside the checks. Its output and
    # failures join after them.
    depcheck=""
    start_depcheck() { rm -f "$log/depcheck.fail"; ( fail=""; timed step_depcheck; timed step_testcheck; printf '%s' "$fail" > "$log/depcheck.fail" ) > "$log/depcheck.out" 2>&1 & depcheck=$!; }
    # a JIT or kernel change runs the library suites under the JIT too (the full gate always does)
    suites=0; git diff --quiet "$QBASE" HEAD -- libs/relax libs/csg 2>/dev/null || suites=1
    ! grep -q '^suite ' "$log/affected" || suites=1
    need_jit=0
    [ ! -s "$log/checks" ] && [ ! -s "$log/presents" ] && [ -z "${GATE_RETRY:-}" ] || need_jit=1
    if grep -qx 'devhub yes' "$log/affected" && ! proven "devhub makepad-devhub"; then need_jit=1; fi
    if [ "$suites" = 1 ]; then
      for p in ${JIT_SUITES:-makepad-csg-brep}; do proven "suite $p" || need_jit=1; done
    fi
    if [ "$need_jit" = 0 ]; then echo "jit: skipped (every unit proven)"; fi
    if [ "$need_jit" = 0 ] || timed step_jit; then
      timed step_devhub
      [ -s "$log/presents" ] || start_depcheck
      timed step_qcheck
      [ -n "$depcheck" ] || start_depcheck
      timed step_qpresent
      [ "$suites" = 0 ] || timed step_jit_suites
    fi
    # (a JIT that did not build: alone, its verdict all the same)
    [ -n "$depcheck" ] || start_depcheck
    # its verdict is the file it writes last: a depcheck that ended without one (killed, or not waited
    # for) fails the gate, never passes unseen
    wait "$depcheck"; code=$?; cat "$log/depcheck.out"
    if [ "$code" = 0 ] && [ -f "$log/depcheck.fail" ]; then fail="$fail$(cat "$log/depcheck.fail")"
    else echo "depcheck: ended without a verdict (exit $code)"; fail="$fail depcheck"; fi
  fi
  who="[lease ${DEVHUB_LEASE:-none} lane ${MAKEPAD_TEST_LANE:-none} at $(git rev-parse --short=10 HEAD 2>/dev/null || echo unknown) in $PWD]"
  if [ -n "$fail" ]; then echo "NODEGATE-FAIL:$fail $who"; exit 1; fi
  echo "NODEGATE-OK fast$sum $who"; exit 0
fi
timed step_private
if timed step_build; then
  case $mode in
    full) for s in $STEPS_FULL; do timed $s; done;;
    full-a) for s in $STEPS_A; do timed $s; done;;
    full-b) for s in $STEPS_B; do timed $s; done;;
    full-t) for s in $STEPS_T; do timed $s; done;;
    stage) timed step_check_stage; timed step_present_stage;;
    *) fail="$fail bad-mode";;
  esac
fi
# whose verdict this is: the lease, lane and commit (a lane reading it from a shared file checks them)
who="[lease ${DEVHUB_LEASE:-none} lane ${MAKEPAD_TEST_LANE:-none} at $(git rev-parse --short=10 HEAD 2>/dev/null || echo unknown)]"
if [ -n "$fail" ]; then echo "NODEGATE-FAIL:$fail $who"; exit 1; fi
echo "NODEGATE-OK$sum $who"
