#!/bin/bash
# land_gate.sh <fast <base> <cbase>|hot <base> <cbase>|full|full-a|full-b|stage>: checks under a devhub lease.
# The fast and hot buckets check a landed work/main tip against their previous tip. They never block landing.
#   full  : build (0 warnings), rustc-check, relax-compiler libtest build, x64 relax tests, tier fuzzer (fixed
#           seeds block, random batch never), makepad check of the 7 gate packages + Stage, headless Calc+Stage
#           present, boundary (provided_sweep.sh audits of the six smoke apps), warm_zero (Calculator: the warm launch shows no JIT spinner), warm_after_app_edit (Sandbox: a
#           new item in an app file stays warm), warm_after_hal (a HAL body edit keeps the stored builds), inspect_cold
#           (inspect during a cold start never stalls the app), relaunch (in-place relaunches keep the app alive,
#           show new first frames with the window kept and state handed over, and have no faults), diff_fast, zone leg
#   full-a/full-b: the two halves of full (STEPS_A: cargo legs, checks, presents, warm_app_edit; STEPS_B: the other
#           JIT runtime legs), for two nodes at once; together exactly the steps of full
#   fast  : the JIT binary of the landed tree (kept or built here, 0 warnings), devhub's binary when changed,
#           checks and warm presents of the affected set across the bucket's landings, plus affected JIT suites.
#           GATE_PROVEN skips units whose inputs already passed.
#   hot   : boot the directly touched apps and always Calculator (one small drawing app: a broken draw path
#           fails every app) on a Linux node's warm JIT, then settle for 3 s.
#   stage : build, makepad check makepad-stage, headless Stage present (commercial-only branches)
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
# the node's one JIT through cargo (rebuilt when its sources changed, a ~0.2 s no-op otherwise)
B="cargo run --profile host -q --manifest-path $PWD/Cargo.toml -p makepad-run --bin makepad --"
log=$(mktemp -d "${TMPDIR:-/tmp}/landgate.XXXXXX") || exit 2
trap 'rm -rf -- "$log"' EXIT  # tmpfs /tmp on the nodes: orphaned gate dirs filled it and crashed a hosted app
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
  full=$(cd "$3" && $B check "${@:4}" "$2" 2>&1)
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
  lt=$(bash tools/run/fixture/lazy_typeck.sh 2>&1); echo "$lt"; case "$lt" in *"lazy_typeck: ok"*) ;; *) fail="$fail lazy-typeck";; esac
  ag=$(bash tools/run/fixture/agree.sh 2>&1); echo "$ag"; case "$ag" in *"agree: ok"*) ;; *) fail="$fail agree";; esac
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
  for p in ${JIT_SUITES:-makepad-csg-brep}; do
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
      *) fail="$fail jit-suite:$p"; grep -E ' FAILED$|panicked|error' "$log/suite-$p" | head -8 | cut -c1-240
         [ "$mode" != fast ] || unit_line fail "suite $p";;
    esac
  done
}
step_x64_tests() { # SAFE1: every relax-backend / relax-codegen test on x64 (X64_GATE_KNOWN="test::name ..." = known failures)
  CARGO_INCREMENTAL=0 cargo test --release -p relax-backend -p relax-codegen --no-fail-fast -j8 > "$log/xtest" 2>&1
  bad=$(sed -n 's/^test \(.*\) \.\.\. FAILED$/\1/p' "$log/xtest" | sort -u)
  for k in ${X64_GATE_KNOWN:-}; do bad=$(printf '%s\n' "$bad" | grep -vx "$k"); done
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
step_warm_app_edit() { # JITCOLD2: a new item in an app file recompiles what names it, not the app (Sandbox: cold,
  # warm, then a new pub fn appended to one of its files; at most 480 first compiles, no JIT spinner).
  # MAX 480 (the script's default 400): the warm first-compile count grew with the app (observed
  # 400-405 across unrelated branches on 2026-10-30)
  [ -d apps/commercial/sandbox ] || { echo "warm_after_app_edit: skipped (no apps/commercial)"; return 0; }
  wa=$(WAIT=300 MAX=480 STEPS="item graph" tools/run/fixture/warm_after_app_edit.sh 2>&1 | tail -1 | cut -c1-600)
  echo "$wa"; echo "$wa" | grep -q "^warm_after_app_edit: OK" || fail="$fail warm_after_app_edit"
  sum="$sum warm_app_edit:$(echo "$wa" | sed -n 's/.*\[item\][^]]*compiled \([0-9]*\) first.*/\1/p')"
}
step_warm_zero() { # JITLOOP-B: after one normal session the next Calculator launch shows no JIT spinner and no UI
  # block >= 100 ms (first compiles of never-run code are reported, not failed); Sandbox + Play stays a lane measurement
  wz=$(WAIT=200 tools/run/fixture/warm_zero.sh 2>&1 | tail -1 | cut -c1-400)
  echo "$wz"; echo "$wz" | grep -q "^warm_zero: OK" || fail="$fail warm_zero"
  sum="$sum warm_zero:$(echo "$wz" | sed -n 's/.*warm: spinner \([0-9]*\), UI blocked max \([0-9]*\) ms.*/spinner\1.max\2ms/p')"
}
step_inspect_cold() { # JITDEADLOCK: inspect static/eval/locals in a loop during a cold Calculator start answer or bail,
  # and the app still presents with no long UI block (an inspect `static` during a cold start froze the app)
  ic=$(CLIENTS=1 ROUNDS=1 WAIT=150 tools/run/fixture/inspect_cold.sh 2>&1 | tail -3 | cut -c1-400)
  echo "$ic"; echo "$ic" | tail -1 | grep -q "^inspect_cold: OK" || fail="$fail inspect_cold"
}
step_warm_hal() { # JITCOLD: a body-only HAL edit (a provided crate: the loader rebuilds) keeps the stored builds
  wh=$(WAIT=200 BUILD="cargo build --profile host -p makepad-run -p makepad-remote --bin makepad --bin makepad-launcher --bin makepad-remote -j8" tools/run/fixture/warm_after_hal.sh 2>&1 | tail -1 | cut -c1-400)
  echo "$wh"; echo "$wh" | grep -q "^warm_after_hal: OK" || fail="$fail warm_after_hal"
}
step_relaunch() { # JITRELAUNCH: `session relaunch` 5x in place: the app stays alive and shows a new first frame
  # after every relaunch, with the window kept, state handed over, and no crash or fault line
  rl=$(WAIT=200 tools/run/fixture/relaunch.sh 2>&1 | tail -1 | cut -c1-400)
  echo "$rl"; echo "$rl" | grep -q "^relaunch: OK" || fail="$fail relaunch"
}
step_boundary() { # JITPROV: the smoke apps and the WM reach provided crates only through their ops (`makepad check --boundary
  # all`: every body a run of the app compiles, from its main; a provided crate's static or rustc-laid field read from
  # JIT-compiled code is a violation, caught here instead of at a user's first start)
  bd=$(RUNS=0 SWEEP_OUT="$log/sweep" bash tools/run/fixture/provided_sweep.sh 2>&1)
  echo "$bd" | grep -E "^audit |^provided_sweep" | cut -c1-200
  echo "$bd" | grep -q "^provided_sweep: ok" || { fail="$fail boundary"; echo "$bd" | grep -A1 "^violation" | head -20 | cut -c1-300; }
}
# full-a + full-b = full; land-batch.sh runs the two halves on two pool nodes at once (each builds), balanced on the
# measured step times (a pool box, 2026-10-30: A ~460 s, B ~370 s with the build; full in series ~930 s)
STEPS_FULL="step_x64_tests step_fuzz step_checks step_boundary step_present_calc step_present_stage step_warm_zero step_warm_hal step_warm_app_edit step_inspect_cold step_relaunch step_diff step_jit_suites step_zone"
STEPS_A="step_x64_tests step_fuzz step_checks step_present_calc step_present_stage step_warm_app_edit"
STEPS_B="step_boundary step_warm_zero step_warm_hal step_inspect_cold step_relaunch step_diff step_jit_suites step_zone"
timed() { # timed <step fn>: runs it and prints "STEP <name> <s> s [start-end]" (the gate's wall-time breakdown)
  t=$(date +%s); t0=$(date +%T); "$1"; r=$?; echo "STEP ${1#step_} $(( $(date +%s) - t )) s [$t0-$(date +%T)]"; return $r
}
# ---- fast ----
step_jit() { # the JIT binary of this tree: the one this box built before from exactly its inputs (the hub names it:
  # MAKEPAD_LOADER, libs/devhub/src/lease/loaders.rs), else built here and kept (0 warnings)
  if [ -x "${MAKEPAD_LOADER:-}" ]; then B=$MAKEPAD_LOADER; echo "jit: kept ${MAKEPAD_LOADER##*-}"; return 0; fi
  out=$(bash tools/run/fixture/loader_build.sh 2>&1) || { fail="$fail build"; echo "$out"; return 1; }
  echo "$out" | grep -v '^LOADER-OK'
  # a new JIT binary starts on an empty JIT cache: its presents are cold, and six cold boots at once on a
  # loaded box took over 60 s (Stage, Sandbox and Fab on a pool box, 11:27): a hang still fails, later
  PRESENT_WAIT=240
}
step_devhub() { # devhub's own binary when the bucket diff touches it (0 warnings)
  grep -qx 'devhub yes' "$log/affected" || return 0
  if proven "devhub makepad-devhub"; then echo "devhub: proven"; unit_line proven "devhub makepad-devhub"; return 0; fi
  CARGO_INCREMENTAL=0 cargo build --profile host -p makepad-devhub --bin makepad-devhub -j8 > "$log/devhub" 2>&1 || { fail="$fail devhub-build"; tail -20 "$log/devhub"; unit_line fail "devhub makepad-devhub"; return 1; }
  if grep -E "^(warning|error)" "$log/devhub" | grep -qv sandbox; then grep -E "^(warning|error)" "$log/devhub" | grep -v sandbox | head -20; fail="$fail devhub-warnings"; unit_line fail "devhub makepad-devhub"; return 1; fi
  # and its tests: the master restarts into this code, a break there stops the orchestration
  $B test makepad-devhub --lib > "$log/devhub-test" 2>&1 || { fail="$fail devhub-tests"; grep -E "FAILED|panicked|^error|test result" "$log/devhub-test" | head -20; unit_line fail "devhub makepad-devhub"; return 1; }
  grep -E "^test result" "$log/devhub-test" | head -2
  unit_line ok "devhub makepad-devhub"
}
step_affected() {
  python3 -I tools/run/fixture/affected.py "$QBASE" "$QCBASE" > "$log/affected" 2> "$log/affected.err" || { fail="$fail affected"; tail -5 "$log/affected.err"; return 1; }
  grep '^why' "$log/affected"; sum="$sum $(grep -c '^check' "$log/affected") checks $(grep -c '^present' "$log/affected") presents"
  return 0
}
# fast: the affected apps' checks and presents all at once on this one warm box, on its one JIT cache; each in its
# own subshell, so its output and failures are gathered here in affected.py's order
at_once() { # at_once <fn> <lines file> [at most N]: runs `<fn> <fields of a line>` per line in parallel
  i=0; local max=${3:-0} pids=() p live
  while read -r -a f; do
    i=$((i + 1))
    while [ "$max" -gt 0 ]; do
      live=0; for p in "${pids[@]}"; do kill -0 "$p" 2>/dev/null && live=$((live + 1)); done
      [ "$live" -lt "$max" ] && break
      wait -n 2>/dev/null || sleep 0.2
    done
    # both files open here: present's own `for i` loop changed $i inside the subshell, so its failures
    # went to another par-N.fail and the verdict passed with GATEUNIT fail lines
    ( fail=""; "$1" "${f[@]}" 3>&-; printf '%s' "$fail" >&3 ) > "$log/par-$i.out" 2>&1 3> "$log/par-$i.fail" &
    pids+=($!)
  done < "$2"
  wait
  for n in $(seq 1 $i); do
    cat "$log/par-$n.out"; fail="$fail$(cat "$log/par-$n.fail" 2>/dev/null)"
  done
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
qcheck_one() {
  local unit=$(unit_of "$@")
  check_pkg "$2" "$2" "$3" "${@:4}"
  if [ -z "$fail" ]; then unit_line ok "$unit"; else unit_line fail "$unit"; fi
}
qpresent_one() {
  local unit=$(unit_of "$@")
  pkg=$2; dir=$3; shift 3; present "$pkg" "$dir" "$pkg" "$@"
  if [ -z "$fail" ]; then unit_line ok "$unit"; else unit_line fail "$unit"; fi
}
step_qcheck() { at_once qcheck_one "$log/checks"; }
# a cold present takes about PRESENT_CORES cores (its JIT compiling, its first pipelines): five side by side on
# a gate's 8 cores ran a pool box to load 31 and missed their first frames; so at most a present per
# PRESENT_CORES of the lease's cores runs at once, and they split the lease's cores (MAKEPAD_RUN_JOBS)
step_qpresent() {
  local cores=${MAKEPAD_RUN_JOBS:-8} at
  at=$(( cores / ${PRESENT_CORES:-4} )); [ "$at" -ge 1 ] || at=1
  PRESENT_JOBS=$(( cores / at ))
  at_once qpresent_one "$log/presents" "$at"
}
if [ "$mode" = hot ]; then
  QBASE=${2:?hot <makepad base sha> <commercial base sha|->}; QCBASE=${3:--}
  export PRESENT_WARM=1 PRESENT_WAIT=60 SETTLE=3
  if HOT=1 timed step_affected; then
    grep '^present ' "$log/affected" > "$log/presents"
    if timed step_jit; then timed step_qpresent; fi
  fi
  who="[lease ${DEVHUB_LEASE:-none} lane ${MAKEPAD_TEST_LANE:-none} at $(git rev-parse --short=10 HEAD 2>/dev/null || echo unknown) in $PWD]"
  if [ -n "$fail" ]; then echo "NODEGATE-FAIL:$fail $who"; exit 1; fi
  echo "NODEGATE-OK hot$sum $who"; exit 0
fi
if [ "$mode" = fast ]; then
  QBASE=${2:?fast <makepad base sha> <commercial base sha|->}; QCBASE=${3:--}
  export PRESENT_WARM=1 PRESENT_WAIT=${PRESENT_WAIT:-60}  # warm presents take 3-20 s alone, more side by side: a hang fails in 60 s
  timed step_private
  if timed step_affected; then
    filter_units check "$log/checks"
    filter_units present "$log/presents"
    # a JIT or kernel change runs the library suites under the JIT too (the full gate always does)
    suites=0; git diff --quiet "$QBASE" HEAD -- libs/relax libs/csg 2>/dev/null || suites=1
    need_jit=0
    [ ! -s "$log/checks" ] && [ ! -s "$log/presents" ] || need_jit=1
    if grep -qx 'devhub yes' "$log/affected" && ! proven "devhub makepad-devhub"; then need_jit=1; fi
    if [ "$suites" = 1 ]; then
      for p in ${JIT_SUITES:-makepad-csg-brep}; do proven "suite $p" || need_jit=1; done
    fi
    if [ "$need_jit" = 0 ]; then echo "jit: skipped (every unit proven)"; fi
    if [ "$need_jit" = 0 ] || timed step_jit; then
      timed step_devhub; timed step_qcheck; timed step_qpresent
      [ "$suites" = 0 ] || timed step_jit_suites
    fi
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
    stage) timed step_check_stage; timed step_present_stage;;
    *) fail="$fail bad-mode";;
  esac
fi
# whose verdict this is: the lease, lane and commit (a lane reading it from a shared file checks them)
who="[lease ${DEVHUB_LEASE:-none} lane ${MAKEPAD_TEST_LANE:-none} at $(git rev-parse --short=10 HEAD 2>/dev/null || echo unknown)]"
if [ -n "$fail" ]; then echo "NODEGATE-FAIL:$fail $who"; exit 1; fi
echo "NODEGATE-OK$sum $who"
