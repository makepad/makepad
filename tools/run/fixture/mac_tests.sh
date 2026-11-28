#!/bin/bash
# mac_tests.sh <makepad base sha> <commercial base sha|->: the mac bucket's gate (bucket.rs `mac`), on a Mac's lease
# in its checkout of the tip. It runs the test targets only a Mac can fail (a package's
# `[package.metadata.makepad.macos-tests]`, affected.py `macos_tests`) that the landings since <base> reach
# (affected.py --runs with GATE_MACOS=1: a JIT or workspace-wide change reaches them all), each under this box's JIT
# ($MAKEPAD_LOADER) as `devhub test` runs it, one at a time. The Linux buckets build them as empty targets, so
# nothing after a landing ran them (a menu-bar item's restart guard, Metal parity, 11-17).
# Then the boundary audits of the apps the landings reach (affected.py `boundary` lines; land_gate.sh
# qboundary_one's lines on the Linux fast bucket): on a Mac the provided crates build their Apple-only code, which no
# Linux audit lowers (a Mac-only audit had 26 violations, a native click among them that never answered, 11-24).
# Its lines are land_gate.sh's (GATESTART, GATEEND, GATETIME, a unit's FAILED/panicked/error lines before its
# GATEUNIT ok|fail, NODEGATE-OK|FAIL), for the bucket to read the same way. Written for the Mac's own bash 3.2
# (no setsid, no `wait -n`, no /proc), which land_gate.sh needs.
# A unit hangs when its JIT parks (a panic or compile error waiting at its call: nobody resumes an unattended run),
# or when its tests give no new result for HUNG_QUIET s while its processes stay under 5% of a core. Its state goes
# into its log (`sample`), it is stopped by its pid and its children's, and it fails HUNG. A unit at work, or one
# still building, is never stopped.
# A red unit runs once more on this box at this tip (the same lease) before anyone is named (main, 11-24): green
# there, it is a flake (GATEFLAKE <unit> :: <its first run's failing tests>), recorded in the hub's flake list
# (bucket.rs `record_flake`), and its unit is ok; red again, it fails as before. Both runs' outputs go into the
# lease's keep dir (GATEOUT), which the hub keeps before the lease ends. A one-off red of an unrelated Fab test
# named the last landing of its batch (11-23).
set -u
QBASE=${1:?mac_tests.sh <makepad base sha> <commercial base sha|->}; QCBASE=${2:--}
here=$(cd "$(dirname "$0")" && pwd)
log=$(mktemp -d "${TMPDIR:-/tmp}/macgate.XXXXXX") || exit 2
run_dir=${log##*/}
HUNG_QUIET=${HUNG_QUIET:-600}
TP="--profile release-checks"
fail=""
who="[lease ${DEVHUB_LEASE:-none} lane ${MAKEPAD_TEST_LANE:-none} at $(git rev-parse --short=10 HEAD 2>/dev/null || echo unknown) in $PWD]"
# its start, as land_gate.sh says its own (`GATE <mode>: `): a gate that said it and exited 0 without its verdict line
# said nothing of the landings, and the bucket runs it once more (bucket.rs `unsaid`)
echo "GATE mac: the test targets only a Mac can fail that the landings reach, one at a time under this box's JIT"
# The JIT of this tree: the one this box built before from exactly its inputs (the hub names it: $MAKEPAD_LOADER), else
# built here and kept by content as land_gate.sh's step_jit does (loader_build.sh): a Mac whose prebuild had not built the
# tip named none, and its run stopped before any unit (11-17).
B=${MAKEPAD_LOADER:-}
if [ -z "$B" ] || [ ! -x "$B" ]; then
  echo "mac gate: this box holds no JIT of this tree (\$MAKEPAD_LOADER '$B'): it builds one"
  out=$(bash "$here/loader_build.sh" 2>&1) || { echo "$out"; echo "GATESTEP fail jit"; echo "NODEGATE-FAIL: jit $who"; exit 1; }
  echo "$out" | grep -v '^LOADER-OK'
  B=${CARGO_TARGET_DIR:-$PWD/target}/host/makepad
  [ -x "$B" ] || { echo "mac gate: no JIT at $B after its build"; echo "GATESTEP fail jit"; echo "NODEGATE-FAIL: jit $who"; exit 1; }
fi
echo "mac gate: the JIT $B"
GATE_MACOS=1 python3 -I "$here/affected.py" --runs "$QBASE" "$QCBASE" > "$log/affected" 2> "$log/affected.err" || {
  tail -5 "$log/affected.err"; echo "GATESTEP fail affected"; echo "NODEGATE-FAIL: affected $who"; exit 1
}
echo "mac gate: $(grep -c '^tests ' "$log/affected") test unit(s) and $(grep -c '^boundary ' "$log/affected") boundary audit(s) the landings reach"
# tree <pid>: it and its descendants, by parent pid (never by name)
tree() { local c; echo "$1"; for c in $(pgrep -P "$1" 2>/dev/null); do tree "$c"; done; }
# cpu <pid>: the % of a core it and its descendants use now
cpu() { local p; for p in $(tree "$1"); do ps -o %cpu= -p "$p" 2>/dev/null; done | awk '{ s += $1 } END { printf "%d", s }'; }
# results <out>: how many test results its tests gave so far
results() { grep -cE '^test .* \.\.\. |^test result:' "$1" 2>/dev/null; }
# running <out>: a target's tests run (its `running N tests` past its last `test result:`)
running() { awk '/^running [0-9]+ tests?$/ { d++ } /^test result:/ { if (d) d-- } END { exit !d }' "$1"; }
stop() { local p; for p in $(tree "$1"); do kill -KILL "$p" 2>/dev/null; done; }
stopped_by_told() {
  [ -n "${DEVHUB_TOLD:-}" ] && [ -r "$DEVHUB_TOLD" ] || return 1
  awk -v p="stop $1 :: " 'index($0, p) == 1 {
    r = substr($0, length(p) + 1); i = index(r, " :: ")
    if (i) { who = substr(r, 1, i - 1); msg = substr(r, i + 4) }
    else { who = r; sub(/ ::$/, "", who); msg = "" }
    if (who == "") who = "main"
    print "stopped by " who (msg != "" ? ": " msg : ""); f = 1; exit
  } END { exit !f }' "$DEVHUB_TOLD"
}
# run_unit <out>: the unit's tests into <out> under the hang watch; sets code (124: hung) and hung
run_unit() {
  out=$1
  MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 "$B" test "$pkg" $TP $rest > "$out" 2>&1 < /dev/null &
  pid=$!
  stopped_by_hand="" hung="" seen=-1 quiet_from=$SECONDS
  while kill -0 "$pid" 2>/dev/null; do
    sleep 5
    kill -0 "$pid" 2>/dev/null || break
    if stop_why=$(stopped_by_told "$unit"); then
      hung="$stop_why, $((SECONDS - t0)) s into the unit"; stopped_by_hand=1; break
    fi
    park=$(grep -m1 -E '^relax: (panic parked on thread |compile error parked on thread |stale access parked as P[0-9]|.* -- parked as P[0-9]+ \(the app holds still)' "$out")
    if [ -n "$park" ]; then hung="its JIT parked: $park"; break; fi
    n=$(results "$out")
    if [ "$n" != "$seen" ] || ! running "$out" || [ "$(cpu "$pid")" -ge 5 ]; then seen=$n quiet_from=$SECONDS; continue; fi
    if [ $((SECONDS - quiet_from)) -ge "$HUNG_QUIET" ]; then hung="no new test result for $HUNG_QUIET s, its processes idle"; break; fi
  done
  if [ -n "$hung" ]; then
    { echo "== its state as it hung"; sample "$pid" 2 2>&1 | head -200; } >> "$out"
    stop "$pid"
    wait "$pid" 2>/dev/null
    code=124
  else
    wait "$pid"; code=$?
  fi
}
# counts <out>: its tests' passed and failed
counts() { awk '/^test result:/ { for (i = 1; i <= NF; i++) if ($i ~ /^(passed|failed);?$/) s[$i] += $(i - 1) } END { printf "%d passed, %d failed", s["passed;"], s["failed;"] }' "$1"; }
# failing <out>: its failing tests' and errors' lines
failing() { grep -E '^(FAILED |panicked at |error(\[E[0-9]+\])?: )' "$1" | head -60; }
# keep <out>..: into the lease's keep dir, which the hub keeps before the lease ends; prints their names there
keep() {
  local f
  [ -n "${DEVHUB_KEEP:-}" ] && mkdir -p "$DEVHUB_KEEP/$run_dir" || return 0
  for f; do
    cp -p "$f" "$DEVHUB_KEEP/$run_dir/mac-${f##*/}" && printf ' mac-%s' "${f##*/}"
  done
}
keep_manifest() {
  python3 -I "$here/test_failures.py" --manifest "$DEVHUB_KEEP/$run_dir/gate-manifest.jsonl" \
    --run-pointer "$DEVHUB_KEEP/gate-run.${DEVHUB_LEASE:-local}" "$run_dir" --kind unit --unit "$unit" "$@" 4>&-
}
# why a boundary audit exits non-zero, counted and named in one line, so a red always says why: an
# audited crate's violations (crate, body) and its failed option entries (crate, `Option<T>`), the first
# three of each (another provided crate's `violation other` rows are listed, never failed); with no such
# row, how it ended (a signal, its first error line, else its last line). Its twins: land_gate.sh, provided_sweep.sh.
boundary_why() { # boundary_why <its exit> < its output
  awk -F'\t' -v rc="$1" '
    $1 == "violation" && $2 == "FAIL" { if (++v <= 3) vn = vn (v > 1 ? "; " : "") $3 " " substr($4, 1, 100) }
    $1 == "option-entry" && $2 == "FAIL" { if (++o <= 3) on = on (o > 1 ? "; " : "") $3 " " $4 }
    (/^(relax|error)(:|\[)/ || /panicked at/) && err == "" { err = substr($0, 1, 200) }
    /[^ ]/ { last = substr($0, 1, 200) }
    END {
      if (v) s = v " violation(s) (" vn (v > 3 ? "; ..." : "") ")"
      if (o) s = s (s ? ", " : "") o " failed option entr" (o == 1 ? "y" : "ies") " (" on (o > 3 ? "; ..." : "") ")"
      if (s == "") s = "no failing row; " (rc > 128 ? "killed by signal " rc - 128 : "it ended") ": " (err != "" ? err : last)
      print s
    }'
}
# audit <app> <dir>: its boundary audit as qboundary_one runs it (`check --boundary all` lowers every body a run of the
# app compiles and fails on each provided crate's static or rustc-laid field JIT code reaches); one run, no rerun: an
# audit reads code, not timing
audit() {
  local unit="boundary $1" out="$log/unit-$units.log" code kept
  echo "GATESTART $unit $(date +%s)"
  t0=$SECONDS
  (cd "$2" && MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 "$B" check --boundary all "$1") > "$out" 2>&1 < /dev/null &
  local pid=$! stop_why
  while kill -0 "$pid" 2>/dev/null; do
    sleep 2
    kill -0 "$pid" 2>/dev/null || break
    if stop_why=$(stopped_by_told "$unit"); then
      sample "$pid" 2 >> "$out" 2>&1
      stop "$pid"; wait "$pid" 2>/dev/null
      echo "GATEEND $unit"
      echo "HUNG: $stop_why, $((SECONDS - t0)) s into the unit; its stacks: mac-${out##*/} in the run's kept logs"
      kept=$(keep "$out"); [ -z "$kept" ] || { keep_manifest $kept; echo "GATEOUT $unit ::$kept"; }
      echo "GATEUNIT fail $unit"; fail="$fail $unit"; return
    fi
  done
  wait "$pid"; code=$?
  echo "GATEEND $unit"
  echo "GATETIME $unit $((SECONDS - t0))"
  # (what failed, counted and named: land_gate.sh qboundary_one)
  local rows='^(violation|option-entry)[[:space:]]+FAIL' why=""
  [ "$code" = 0 ] || why=": $(boundary_why "$code" < "$out")"
  echo "boundary $1: exit $code$why, $(grep -m1 '^# lowered' "$out" | cut -c3-)"
  if [ "$code" = 0 ]; then echo "GATEUNIT ok $unit"; return; fi
  # an audit that stopped before its rows (a crash, a JIT's exit): its output's end, where a run says why
  if grep -qE "$rows" "$out"; then grep -E -A1 "$rows" "$out" | cut -c1-600 | head -40
  else grep -vE '^ *$' "$out" | tail -40 | cut -c1-300; fi
  kept=$(keep "$out"); [ -z "$kept" ] || { keep_manifest $kept; echo "GATEOUT $unit ::$kept"; }
  echo "GATEREPRO $unit :: cd $2 && makepad check --boundary all $1"
  echo "GATEUNIT fail $unit"
  fail="$fail $unit"
}
units=0
while read -r word pkg dir rest; do
  [ "$word" = boundary ] && { units=$((units + 1)); audit "$pkg" "$dir"; continue; }
  [ "$word" = tests ] || continue
  units=$((units + 1))
  unit="test $pkg${rest:+ $rest}"
  echo "GATESTART $unit $(date +%s)"
  t0=$SECONDS
  run_unit "$log/unit-$units.log"
  red=""
  if [ "$code" != 0 ] && [ -z "$stopped_by_hand" ]; then
    red=$out red_code=$code red_hung=$hung
    echo "$unit: red (exit $code${hung:+, hung: $hung}; $(counts "$out")): it runs once more on this box at this tip before anyone is named"
    failing "$out" | sed 's/^/  its first run: /'
    t0=$SECONDS
    run_unit "$log/unit-$units-rerun.log"
  fi
  echo "GATEEND $unit"
  if [ -n "$stopped_by_hand" ]; then
    echo "HUNG: $hung; its stacks: mac-${out##*/} in the run's kept logs"
    kept=$(keep ${red:+"$red"} "$out"); [ -z "$kept" ] || { keep_manifest $kept; echo "GATEOUT $unit ::$kept"; }
    echo "GATEUNIT fail $unit"; fail="$fail $unit"; continue
  fi
  grep -qE '^running [0-9]+ tests?$' "$out" && [ -z "$hung" ] && echo "GATETIME $unit $((SECONDS - t0))"
  r=$(counts "$out")
  if [ "$code" = 0 ]; then
    if [ -n "$red" ]; then
      echo "$unit: a flake: red (exit $red_code${red_hung:+, hung}), then green on its rerun ($r)"
      tail -25 "$red" | cut -c1-300 | sed 's/^/  its first run: /'
      kept=$(keep "$red" "$out"); [ -z "$kept" ] || { keep_manifest $kept; echo "GATEOUT $unit ::$kept"; }
      echo "GATEFLAKE $unit :: $(grep -E '^FAILED [^ ]+$' "$red" | sed 's/^FAILED //' | sort -u | tr '\n' ' ')"
    fi
    echo "$unit: ok ($r, $((SECONDS - t0)) s)"
    echo "GATEUNIT ok $unit"
  else
    [ -z "$hung" ] || echo "HUNG: $unit: $hung; stopped; $r before it"
    failing "$out"
    echo "$unit: FAILED (exit $code, $r)${red:+, red on its rerun too}; its output's end:"
    tail -25 "$out" | cut -c1-300
    kept=$(keep ${red:+"$red"} "$out"); [ -z "$kept" ] || { keep_manifest $kept; echo "GATEOUT $unit ::$kept"; }
    echo "GATEUNIT fail $unit"
    fail="$fail $unit"
  fi
done < "$log/affected"
if [ -n "$fail" ]; then echo "NODEGATE-FAIL:$fail $who"; exit 1; fi
echo "NODEGATE-OK mac $units unit(s) $who"
