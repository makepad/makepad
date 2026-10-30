#!/bin/bash
# land_gate.sh <quick <base> <cbase>|full|full-a|full-b|stage>: the landing gate, run from the root of the tree under test on an x86_64 node,
# under a devhub gate lease (land.sh: `makepad devhub lease gate --rev <branch>,<commercial> -- bash
# tools/run/fixture/land_gate.sh full`). The tree is work + the branch being landed + the nested apps/commercial.
#   full  : build (0 warnings), rustc-check, relax-compiler libtest build, x64 relax tests, tier fuzzer (fixed
#           seeds block, random batch never), makepad check of the 7 gate packages + Stage, headless Calc+Stage
#           present, warm_zero (Calculator: the warm launch shows no JIT spinner), warm_after_app_edit (Sandbox: a
#           new item in an app file stays warm), warm_after_hal (a HAL body edit keeps the stored builds), inspect_cold
#           (inspect during a cold start never stalls the app), relaunch (session relaunch never leaves the app
#           dead), diff_fast, zone leg
#   full-a/full-b: the two halves of full (STEPS_A: cargo legs, checks, presents, warm_app_edit; STEPS_B: the other
#           JIT runtime legs), for two nodes at once; together exactly the steps of full
#   quick : the quick land (land-quick.sh): in the node's ONE persistent warm gate tree per lane ($HOME/devhub/gate/<lane>,
#           the new commits checked out, its own target: the JIT binary rebuilds only when its own crates changed, 0
#           warnings), `makepad check` of the affected set (affected.py: changed packages and their dependents within
#           the gate allowlist; Calculator always checked) and a headless present of the affected apps on the node's shared
#           warm JIT cache. The full legs run after landing (verify-tip.sh).
#   stage : build, makepad check makepad-stage, headless Stage present (commercial-only branches)
# Prints "NODEGATE-OK <summary>" or "NODEGATE-FAIL: <steps>"; exit 0 / 1. Every step is a function named step_*;
# the zone leg is the ONE function step_zone (settle_stress.sh).
mode=${1:-full}
T=${CARGO_TARGET_DIR:-$PWD/target}
B=$T/release/makepad
log=$(mktemp -d "${TMPDIR:-/tmp}/landgate.XXXXXX") || exit 2
trap 'rm -rf -- "$log"' EXIT  # tmpfs /tmp on the nodes: orphaned gate dirs filled it and crashed a hosted app
fail=""; sum=""
export MAKEPAD_RUN_STD=$PWD/libs/relax/compiler/std

step_build() {
  CARGO_INCREMENTAL=0 cargo build --release -p makepad-run -p makepad-remote --bin makepad --bin makepad-remote -j8 > "$log/build" 2>&1 || { fail="$fail build"; tail -20 "$log/build"; return 1; }
  if grep -E "^(warning|error)" "$log/build" | grep -qv sandbox; then grep -E "^(warning|error)" "$log/build" | grep -v sandbox | head -20; fail="$fail build-warnings"; return 1; fi
}
step_rustc_check() {
  CARGO_INCREMENTAL=0 cargo check --release -p makepad-widgets -j8 > "$log/rustc" 2>&1 || { grep -E "^error" -A6 "$log/rustc" | head -20; fail="$fail rustc-check"; }
}
step_libtest_build() {
  CARGO_INCREMENTAL=0 cargo test --release -p relax-compiler --lib --no-run -j8 > "$log/libtest" 2>&1 || { grep -E "^error" -A6 "$log/libtest" | head -20; fail="$fail relax-compiler-libtest-build"; }
}
check_pkg() { # check_pkg <label> <pkg> <dir>
  full=$(cd "$3" && $B check "$2" 2>&1); echo "$full" | grep -B2 -A12 -E "^error|: error" | head -60
  line=$(echo "$full" | grep -E " errors$|load error" | sed "s/^.*all, /$1: /")
  echo "$line"; echo "$line" | grep -qE "^$1: 0 errors$" || fail="$fail $1"
}
step_checks() {
  for p in makepad-example-counter makepad-app-calculator makepad-platform makepad-draw makepad-widgets makepad-widgets-core makepad-app-wm-all; do check_pkg $p $p .; done
  step_check_stage
}
step_check_stage() { check_pkg stage makepad-stage apps/commercial; }
present() { # present <name> <dir> <pkg args...>: headless cold run from a fresh home; 1 when the app reports `present`
  name=$1; dir=$2; shift 2
  s=$log/hrun-$name; mkdir -p "$s"
  rh=${PRESENT_HOME:-$s/home}  # quick: the node's shared warm JIT home (the cache is keyed by the loader build id)
  (cd "$dir" && exec env MAKEPAD_RUN_HOME=$rh MAKEPAD_RUN_CACHE_DIR=$rh/cache MAKEPAD_RUN_TARGET_DIR=$rh/target \
    MAKEPAD_HIDE_WINDOWS=1 SANDBOX_MUTE=1 MAKEPAD_TRACE=startup $B run --report-to $s/report "$@" -- --remote > $s/out.log 2>&1) &
  pid=$!
  for i in $(seq 1 240); do
    grep -q present $s/report 2>/dev/null && break
    kill -0 $pid 2>/dev/null || break
    sleep 0.5
  done
  sleep 2
  kill -0 $pid 2>/dev/null && kill $pid
  wait $pid 2>/dev/null
  n=$(grep -cx present $s/report 2>/dev/null)
  echo "$name present: $n"
  [ "$n" = 1 ] || { fail="$fail $name-present"; tail -20 "$s/out.log"; }
}
step_present_calc() { present calc . makepad-app-calculator --no-default-features --features standalone; }
step_present_stage() { present stage apps/commercial makepad-stage; }
step_diff() { # the package programs build into the lease's warm target ($T), not a cold <tree>/target per gate
  TARGET=$T/release DIFF_JOBS=${DIFF_JOBS:-8} DIFF_PKG_TARGET=${DIFF_PKG_TARGET:-$T} DIFF_CARGO_JOBS=${DIFF_CARGO_JOBS:-8} DIFF_TIERS="o1 base o3 counted" DIFF_CACHE=${X64_GATE_DIFF_CACHE:-$HOME/devhub/cache/diff_fast} tools/run/fixture/diff_fast.sh > "$log/diff" 2>&1
  df=$(tail -1 "$log/diff")
  # the program count is part of the verdict (files + package programs): a gate that ran fewer says why
  grep '^diff programs:' "$log/diff"; echo "$df"
  echo "$df" | grep -qE "^diff: ([0-9]+)/\1 equal" || { fail="$fail diff_fast"; grep -v '^diff \(tiers\|programs\):' "$log/diff" | tail -25 | cut -c1-240; }
  sum="$sum $df"
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
  FUZZ_N=${FUZZ_RANDOM:-2000} FUZZ_FIRST=$first CARGO_INCREMENTAL=0 cargo test --release -p relax-codegen --test fuzz_tiers -j8 random -- --nocapture > "$log/fuzzr" 2>&1 &
  fzr=$!
  FUZZ_N=${FUZZ_FIXED:-2000} FUZZ_FIRST=1 CARGO_INCREMENTAL=0 cargo test --release -p relax-codegen --test fuzz_tiers -j8 -- --nocapture > "$log/fuzz" 2>&1
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
    zs=$(TARGET=$T/release PACKAGES=makepad-app-calculator WAIT=150 tools/run/fixture/settle_stress.sh 2>&1 | tail -1)
    echo "settle_stress: $zs"; echo "$zs" | grep -q FAIL && fail="$fail settle_stress"
  else
    zs=$(TARGET=$T/release tools/run/fixture/zone_swap.sh 2>&1 | tail -1)
    echo "zone_swap: $zs"; echo "$zs" | grep -q FAIL && fail="$fail zone_swap"
  fi
}
step_warm_app_edit() { # JITCOLD2: a new item in an app file recompiles what names it, not the app (Sandbox: cold,
  # warm, then a new pub fn appended to one of its files; at most 480 first compiles, no JIT spinner).
  # MAX 480 (the script's default 400): the warm first-compile count grew with the app (observed
  # 400-405 across unrelated branches on 2026-10-30)
  [ -d apps/commercial/sandbox ] || { echo "warm_after_app_edit: skipped (no apps/commercial)"; return 0; }
  wa=$(TARGET=$T/release WAIT=300 MAX=480 STEPS="item graph" tools/run/fixture/warm_after_app_edit.sh 2>&1 | tail -1 | cut -c1-600)
  echo "$wa"; echo "$wa" | grep -q "^warm_after_app_edit: OK" || fail="$fail warm_after_app_edit"
  sum="$sum warm_app_edit:$(echo "$wa" | sed -n 's/.*\[item\][^]]*compiled \([0-9]*\) first.*/\1/p')"
}
step_warm_zero() { # JITLOOP-B: after one normal session the next Calculator launch shows no JIT spinner and no UI
  # block >= 100 ms (first compiles of never-run code are reported, not failed); Sandbox + Play stays a lane measurement
  wz=$(TARGET=$T/release WAIT=200 tools/run/fixture/warm_zero.sh 2>&1 | tail -1 | cut -c1-400)
  echo "$wz"; echo "$wz" | grep -q "^warm_zero: OK" || fail="$fail warm_zero"
  sum="$sum warm_zero:$(echo "$wz" | sed -n 's/.*warm: spinner \([0-9]*\), UI blocked max \([0-9]*\) ms.*/spinner\1.max\2ms/p')"
}
step_inspect_cold() { # JITDEADLOCK: inspect static/eval/locals in a loop during a cold Calculator start answer or bail,
  # and the app still presents with no long UI block (an inspect `static` during a cold start froze the app)
  ic=$(TARGET=$T/release CLIENTS=1 ROUNDS=2 WAIT=150 tools/run/fixture/inspect_cold.sh 2>&1 | tail -3 | cut -c1-400)
  echo "$ic"; echo "$ic" | tail -1 | grep -q "^inspect_cold: OK" || fail="$fail inspect_cold"
}
step_warm_hal() { # JITCOLD: a body-only HAL edit (a provided crate: the loader rebuilds) keeps the stored builds
  wh=$(TARGET=$T/release WAIT=200 BUILD="cargo build --release -p makepad-run -p makepad-remote --bin makepad --bin makepad-remote -j8" tools/run/fixture/warm_after_hal.sh 2>&1 | tail -1 | cut -c1-400)
  echo "$wh"; echo "$wh" | grep -q "^warm_after_hal: OK" || fail="$fail warm_after_hal"
}
step_relaunch() { # JITRELAUNCH: `session relaunch` 5x in place, 5x with stops that time out (late handover), and the
  # process-restart fallback: the app is alive and presents after every relaunch (RU0001 "still owns" left it dead)
  rl=$(TARGET=$T/release WAIT=200 tools/run/fixture/relaunch.sh 2>&1 | tail -1 | cut -c1-400)
  echo "$rl"; echo "$rl" | grep -q "^relaunch: OK" || fail="$fail relaunch"
}
# full-a + full-b = full; land-batch.sh runs the two halves on two pool nodes at once (each builds), balanced on the
# measured step times (107, 2026-10-30: A ~460 s, B ~370 s with the build; full in series ~930 s)
STEPS_FULL="step_x64_tests step_fuzz step_checks step_present_calc step_present_stage step_warm_zero step_warm_hal step_warm_app_edit step_inspect_cold step_relaunch step_diff step_zone"
STEPS_A="step_x64_tests step_fuzz step_checks step_present_calc step_present_stage step_warm_app_edit"
STEPS_B="step_warm_zero step_warm_hal step_inspect_cold step_relaunch step_diff step_zone"
timed() { # timed <step fn>: runs it and prints "STEP <name> <s> s [start-end]" (the gate's wall-time breakdown)
  t=$(date +%s); t0=$(date +%T); "$1"; r=$?; echo "STEP ${1#step_} $(( $(date +%s) - t )) s [$t0-$(date +%T)]"; return $r
}
# ---- quick: the node's persistent warm tree ----
step_tree() { # check out the gated commits in $G (only changed files are rewritten: cargo's mtimes stay warm)
  local sha csha=- store cstore
  sha=$(git rev-parse HEAD); store=$(git rev-parse --path-format=absolute --git-common-dir)
  if [ -e apps/commercial/.git ]; then csha=$(git -C apps/commercial rev-parse HEAD); cstore=$(git -C apps/commercial rev-parse --path-format=absolute --git-common-dir); fi
  if [ -e "$G/.git" ]; then
    git -C "$G" checkout -q -f --detach "$sha" && git -C "$G" clean -q -fd || { fail="$fail tree"; return 1; }
  else
    git --git-dir="$store" worktree add -q --detach "$G" "$sha" || { fail="$fail tree"; return 1; }
  fi
  if [ "$csha" != - ]; then
    if [ -e "$G/apps/commercial/.git" ]; then
      git -C "$G/apps/commercial" checkout -q -f --detach "$csha" && git -C "$G/apps/commercial" clean -q -fd || { fail="$fail tree"; return 1; }
    else
      git --git-dir="$cstore" worktree add -q --detach "$G/apps/commercial" "$csha" || { fail="$fail tree"; return 1; }
    fi
  fi
}
step_jit() { # the JIT binary of this tree: cargo's no-op unless the JIT's own crates changed (0 warnings)
  CARGO_INCREMENTAL=0 cargo build --release -p makepad-run --bin makepad -j8 > "$log/build" 2>&1 || { fail="$fail build"; tail -20 "$log/build"; return 1; }
  if grep -E "^(warning|error)" "$log/build" | grep -qv sandbox; then grep -E "^(warning|error)" "$log/build" | grep -v sandbox | head -20; fail="$fail build-warnings"; return 1; fi
  grep -E '^ +Compiling' "$log/build" | wc -l | sed 's/^ */jit: /; s/$/ crates rebuilt/'
}
step_affected() {
  python3 -I tools/run/fixture/affected.py "$QBASE" "$QCBASE" > "$log/affected" 2> "$log/affected.err" || { fail="$fail affected"; tail -5 "$log/affected.err"; return 1; }
  grep '^why' "$log/affected"; sum="$sum $(grep -c '^check' "$log/affected") checks $(grep -c '^present' "$log/affected") presents"
}
step_qcheck() { while read -r _ pkg dir; do check_pkg "$pkg" "$pkg" "$dir"; done < <(grep '^check ' "$log/affected"); }
step_qpresent() { while read -r _ pkg dir args; do present "$pkg" "$dir" "$pkg" $args; done < <(grep '^present ' "$log/affected"); }
# every mode runs in the node's ONE persistent warm gate tree (quick land and verify-tip share it and its target and
# JIT binary: gate slots are one per node, the flock orders the rest); the lease's own worktree only names the commits
G=${GATE_TREE:-$HOME/devhub/gate/landgate}  # (named for the first quick lands; verify-tip uses it too)
mkdir -p "$(dirname "$G")" || exit 2
exec 9> "$G.lock"; flock 9
if ! timed step_tree; then echo "NODEGATE-FAIL:$fail [lease ${DEVHUB_LEASE:-none} lane ${MAKEPAD_TEST_LANE:-none}]"; exit 1; fi
cd "$G" || exit 2
export CARGO_TARGET_DIR=$G/target MAKEPAD_RUN_STD=$G/libs/relax/compiler/std
T=$CARGO_TARGET_DIR; B=$T/release/makepad
if [ "$mode" = quick ]; then
  QBASE=${2:?quick <makepad base sha> <commercial base sha|->}; QCBASE=${3:--}
  export PRESENT_HOME=$HOME/devhub/gate/runhome
  timed step_jit && timed step_affected && { timed step_qcheck; timed step_qpresent; }
  who="[lease ${DEVHUB_LEASE:-none} lane ${MAKEPAD_TEST_LANE:-none} at $(git rev-parse --short=10 HEAD 2>/dev/null || echo unknown) in $G]"
  if [ -n "$fail" ]; then echo "NODEGATE-FAIL:$fail $who"; exit 1; fi
  echo "NODEGATE-OK quick$sum $who"; exit 0
fi
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
