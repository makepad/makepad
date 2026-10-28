#!/bin/bash
# Regression test (JITDEADLOCK): makepad inspect `static`, `eval` and `locals` sent while the app is still
# compiling lazily (a cold start) answer or bail, and never stall the app. The bug: `inspect static` during a
# cold Sandbox start froze it. The owner, loading the eval module, waited in its front write for a front read
# that the O2 thread held around a type check ahead (prefront_twin); that check, preempted by the waiting
# writer, waited for the writer to finish -- and the program's main thread queued behind the owner.
# Per round: Calculator from a fresh run home (cold: nothing stored), hidden and muted; as soon as inspect
# answers, CLIENTS clients (default 3) loop static / eval / locals requests (PACE s apart, default 0) until
# HOLD s (default 10) after the present. Checks per round:
#   - every inspect request ends within REQ_MAX s (an answer or an error line);
#   - the app presents within WAIT s;
#   - after the present no UI-thread JIT wait reaches UI_MAX ms (`jit`'s session line: UI blocked max);
#   - no request stays unanswered for STALL s (then the app's owner, waits, threads and stacks are written to
#     <dir>/r<round>/stall; STALL_GDB=1 adds gdb's native stacks where a debugger is allowed).
# CLIENTS=0 is the control (no requests). VERBOSE=1 prints the answers by kind and the stall capture;
# PROFILE=1 the CPU's top functions 5 s into the start.
# Run from the workspace root after `cargo build --release -p makepad-run`. ROUNDS (default 3), WAIT (300),
# REQ_MAX (40), UI_MAX (2000), STALL (12), TARGET (target/release), KEEP=1 keeps the logs.
# Prints one line per round and "inspect_cold: OK ..." or "inspect_cold: FAIL ..."; exit 1 on a failure.
T=${TARGET:-target/release}
B=$(cd "$(dirname "$T/makepad")" && pwd)/makepad
CLIENTS=${CLIENTS:-3}; PACE=${PACE:-0}; HOLD=${HOLD:-10}; ROUNDS=${ROUNDS:-3}; WAIT=${WAIT:-300}; REQ_MAX=${REQ_MAX:-40}; UI_MAX=${UI_MAX:-2000}; STALL=${STALL:-12}
dir=$(mktemp -d "${TMPDIR:-/tmp}/inspect_cold.XXXXXX") || exit 1
fails=0
now_ms() { date +%s%3N; }
# ask <args...>: one inspect request (prints its answer)
ask() { TMPDIR="$dir/t" "$B" inspect $pid "$@" 2>&1; }
# client <k>: inspect requests in a loop until $d/stop exists; one line per request: <cmd> <ms> <first answer line>
client() {
  k=$1; i=0
  while [ ! -e "$d/stop" ] && kill -0 $pid 2>/dev/null; do
    i=$((i + 1))
    case $(( (i + k) % 3 )) in
      0) c="static makepad_platform::frame_trace::PRESENTED" ;;
      1) c="eval in=makepad_app_calculator 40+2" ;;
      2) c="locals thread=main" ;;
    esac
    t0=$(now_ms)
    r=$(ask $c | head -1 | cut -c1-200)
    t1=$(now_ms)
    echo "${c%% *} $((t1 - t0)) $r" >> "$d/req.$k"
    [ "$PACE" != 0 ] && sleep $PACE
  done
}
# watchdog: the app's state the first time a request runs past STALL s (the holder of a stall)
watchdog() {
  while [ ! -e "$d/stop" ] && kill -0 $pid 2>/dev/null; do
    # a request is stalled when no client wrote a line for STALL s while the app lives
    newest=0
    for k in $(seq 1 $CLIENTS); do
      [ -e "$d/req.$k" ] && m=$(stat -c %Y "$d/req.$k") && [ "$m" -gt $newest ] && newest=$m
    done
    [ $newest = 0 ] && newest=$start_s
    if [ $(( $(date +%s) - newest )) -ge $STALL ] && [ ! -e "$d/stall" ]; then
      { echo "== owner"; ask owner; echo "== waits"; ask waits; echo "== threads"; ask threads; echo "== stacks"; ask stacks thread=all depth=48
        # STALL_GDB=1 (Linux nodes where a debugger is allowed): every thread's native stack
        [ "${STALL_GDB:-0}" = 1 ] && { echo "== gdb"; gdb -p $pid -batch -ex "set pagination off" -ex "thread apply all bt 60" 2>&1; }; } > "$d/stall" 2>&1
    fi
    sleep 1
  done
}
for round in $(seq 1 $ROUNDS); do
  d=$dir/r$round; mkdir -p "$d/home" "$d/t"
  MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 SANDBOX_MUTE=1 MUSIC_MUTE=1 TMPDIR="$d/t" \
    MAKEPAD_HOME="$d/home" MAKEPAD_RUN_HOME="$d/run" MAKEPAD_RUN_CACHE_DIR="$d/run/cache" MAKEPAD_RUN_TARGET_DIR="$d/run/target" \
    "$B" run --report-to "$d/report" makepad-app-calculator --no-default-features --features standalone > "$d/log" 2>&1 &
  pid=$!
  start_s=$(date +%s); t_start=$(now_ms)
  # inspect answers from early in the start (the JIT's inspect thread), long before the present
  n=0
  until ask ping > /dev/null; do
    n=$((n + 1)); [ $n -ge $((WAIT * 10)) ] && break
    kill -0 $pid 2>/dev/null || break
    sleep 0.1
  done
  t_ping=$(now_ms)
  cpids=""
  for k in $(seq 1 $CLIENTS); do client $k & cpids="$cpids $!"; done
  [ $CLIENTS -gt 0 ] && { watchdog & cpids="$cpids $!"; }

  # PROFILE=1: where the CPU goes 5 s into the start (the top functions over 4 s)
  [ "${PROFILE:-0}" = 1 ] && { sleep 5; ask top by=time s=4 n=40 > "$d/prof" 2>&1; }
  # until the present, then HOLD s more of requests against the presenting app (or WAIT)
  t_present=""
  while kill -0 $pid 2>/dev/null && [ $(( $(date +%s) - start_s )) -lt $WAIT ]; do
    if [ -z "$t_present" ] && grep -qx present "$d/report" 2>/dev/null; then t_present=$(now_ms); fi
    [ -n "$t_present" ] && [ $(( $(now_ms) - t_present )) -ge $((HOLD * 1000)) ] && break
    sleep 0.2
  done
  session=$(ask jit ms=100 | grep '^session:')
  # no present: what the app is doing (who waits for what, the owner's service)
  [ -z "$t_present" ] && [ ! -e "$d/stall" ] && kill -0 $pid 2>/dev/null && { echo "== owner"; ask owner; echo "== waits"; ask waits; echo "== live"; ask live; } > "$d/stall" 2>&1
  touch "$d/stop"
  # a client stuck in a request (the bug) does not see the stop: its recorded pid is ended
  n=0
  while [ $n -lt 100 ] && for c in $cpids; do kill -0 $c 2>/dev/null && break; done; do n=$((n + 1)); sleep 0.1; done
  for c in $cpids; do kill -0 $c 2>/dev/null && { kill $c 2>/dev/null; echo "client $c still in a request at the end" >> "$d/stuck"; }; done
  kill -TERM $pid 2>/dev/null
  n=0
  while [ $n -lt 100 ] && kill -0 $pid 2>/dev/null; do n=$((n + 1)); sleep 0.1; done
  kill -0 $pid 2>/dev/null && { kill -KILL $pid 2>/dev/null; echo "the app did not end on SIGTERM" >> "$d/stuck"; }
  wait $pid 2>/dev/null
  cat "$d"/req.* > "$d/reqs" 2>/dev/null
  nreq=$(wc -l < "$d/reqs" 2>/dev/null || echo 0)
  slowest=$(sort -k2 -n "$d/reqs" 2>/dev/null | tail -1)
  slow_ms=$(echo "$slowest" | awk '{print $2}')
  answered=$(grep -vc ' error' "$d/reqs" 2>/dev/null)
  ui=$(echo "$session" | sed -n 's/.*UI blocked max \([0-9]*\) ms.*/\1/p')
  why=""
  [ -z "$t_present" ] && why="$why no-present"
  [ -n "$slow_ms" ] && [ "$slow_ms" -gt $((REQ_MAX * 1000)) ] && why="$why request-${slow_ms}ms"
  [ -z "$ui" ] && why="$why no-session-line"
  [ -n "$ui" ] && [ "$ui" -ge "$UI_MAX" ] && why="$why ui-block-${ui}ms"
  [ -e "$d/stall" ] && why="$why stall(see $d/stall)"
  [ -e "$d/stuck" ] && why="$why $(tr '\n' ';' < "$d/stuck")"
  line="round $round: present $([ -n "$t_present" ] && echo $((t_present - t_start)) || echo never) ms, inspect from $((t_ping - t_start)) ms, $nreq requests ($answered answered), slowest ${slowest:-none}, UI blocked max ${ui:-?} ms"
  if [ -z "$why" ]; then echo "$line"; else echo "$line: FAIL$why"; fails=$((fails + 1)); fi
  # what the requests answered (command and the answer's start, most frequent first)
  [ -n "$VERBOSE" ] && awk '{ $2 = ""; print substr($0, 1, 110) }' "$d/reqs" | sort | uniq -c | sort -rn | head -12 | sed 's/^/  | /'
  [ -n "$VERBOSE" ] && [ -e "$d/stall" ] && cat "$d/stall"
  [ -n "$VERBOSE" ] && [ -e "$d/prof" ] && sed 's/^/  % /' "$d/prof" | sed 's/^/  ! /'
done
if [ $fails = 0 ]; then
  echo "inspect_cold: OK $ROUNDS rounds"
  [ "${KEEP:-0}" != 1 ] && rm -rf -- "$dir"
  exit 0
fi
echo "inspect_cold: FAIL $fails/$ROUNDS rounds; logs in $dir"
exit 1
