#!/bin/bash
# usage: run.sh <makepad binary>: a program under `makepad run` whose system heap is corrupt aborts in
# glibc's malloc, with that malloc's arena locked. The process ends by SIGABRT (shell status 134)
# within 10 s of the program's start, and its output holds the JIT's abort report: the aborting
# thread never reaches the system heap again (11-09: three test threads of a JIT waited on that
# arena's lock in their abort handlers for 10 minutes, and no report came), and the report waits
# for no lock. Two cases:
#   plain  glibc's line and the report (what is still in the session log's pipe when the process
#          dies reaches the terminal);
#   held   MAKEPAD_RUN_ABORT_HOLD=locks: a thread takes every lock the report could wait for and
#          keeps it, as one stuck in the corrupt heap would; the report still comes, naming each
#          part it left out;
#   stuck  MAKEPAD_RUN_ABORT_HOLD=stuck: the report's lookups never return, as on something the
#          aborting thread holds; the process still ends, naming where the report stopped, with
#          the frames by address.
# glibc only: elsewhere it says so. Prints "abort_heap: ok" or "abort_heap: FAILED".
H=$(cd "$(dirname "$0")" && pwd)
if [ "$(uname -s)" != Linux ]; then echo "abort_heap: ok (glibc's malloc checks are Linux's)"; exit 0; fi
export MAKEPAD_RUN_CACHE=${MAKEPAD_RUN_CACHE:-off}
# (no core file: writing one is most of the time a JIT's abort takes)
ulimit -c 0
mkdir -p "$H/target"
ok=1
run_case() { # <name> [env assignment]
  local name="$1" LOG="$H/target/run-$1.log"; shift
  (cd "$H/app" && exec env "$@" MAKEPAD_RUN_STD=$H/../../std "$BIN" run --quiet abort-heap-app > "$LOG" 2>&1) &
  local pid=$!
  for i in $(seq 1 1200); do grep -q started "$LOG" 2>/dev/null && break; kill -0 $pid 2>/dev/null || break; sleep 0.1; done
  local t0=$(date +%s%N)
  for i in $(seq 1 100); do kill -0 $pid 2>/dev/null || break; sleep 0.1; done
  if kill -0 $pid 2>/dev/null; then
    echo "$name: still running 10 s after the program started"; kill -KILL $pid; ok=0
  fi
  wait $pid 2>/dev/null; local st=$?
  echo "$name: status $st after $(( ($(date +%s%N) - t0) / 1000000 )) ms (want 134)"
  [ "$st" = 134 ] || ok=0
  grep -q -a 'started' "$LOG" || { echo "$name: the program never started"; ok=0; }
  grep -q -a 'SIGABRT on thread' "$LOG" || { echo "$name: no abort line"; ok=0; }
}
has_report() { grep -q -a '^  report: {' "$H/target/run-$1.log" || { echo "$1: no abort report"; ok=0; }; }
BIN="$1"
run_case plain MAKEPAD_RUN_ABORT_HOLD=
has_report plain
grep -q -a -E 'malloc\(\): corrupted top size|malloc\(\): invalid' "$H/target/run-plain.log" || { echo "plain: no glibc line: lost in the pipe, or the fixture corrupted nothing malloc reads"; ok=0; }
run_case held MAKEPAD_RUN_ABORT_HOLD=locks
has_report held
for want in 'frames: FRAMES lock held' 'backtrace: code table lock held' 'frames: code table lock held' 'poisoned: POISONED lock held' 'makepad: tee: TEE lock held'; do
  grep -q -a -F "$want" "$H/target/run-held.log" || { echo "held: no \"$want\""; ok=0; }
done
run_case stuck MAKEPAD_RUN_ABORT_HOLD=stuck
grep -q -a -E "the abort's report is stuck in its lookups on the compiler's owner.*frames by address: 0x" "$H/target/run-stuck.log" || { echo "stuck: no line naming where the report stopped, with frames"; ok=0; }
[ $ok = 1 ] && echo "abort_heap: ok" || { echo "abort_heap: FAILED"; exit 1; }
