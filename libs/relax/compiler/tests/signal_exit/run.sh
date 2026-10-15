#!/bin/bash
# usage: run.sh <makepad binary>: a program under `makepad run` ends on SIGTERM and on SIGINT as the same program
# built by rustc does (default dispositions: killed by the signal, shell status 143 / 130), within 3 s of the
# signal, both while main runs JIT code and while another thread sleeps; the run's exit summary does not hang it.
# The signal goes to Relax's own PID (not a wrapper). Prints "signal_exit: ok" or "signal_exit: FAILED".
H=$(cd "$(dirname "$0")" && pwd)
export MAKEPAD_RUN_CACHE=${MAKEPAD_RUN_CACHE:-off}
ok=1
for sig in TERM INT; do
  LOG=$H/target/run-$sig.log
  mkdir -p "$H/target"
  # (a shell starts `&` jobs with SIGINT ignored; set -m gives the job its own process group with default dispositions)
  set -m
  (cd "$H/app" && MAKEPAD_RUN_STD=$H/../../std exec "$1" run --quiet signal-exit-app > "$LOG" 2>&1) &
  pid=$!
  set +m
  for i in $(seq 1 600); do grep -q started "$LOG" 2>/dev/null && break; sleep 0.1; done
  sleep 0.5
  t0=$(date +%s%N)
  kill -$sig $pid
  for i in $(seq 1 50); do kill -0 $pid 2>/dev/null || break; sleep 0.1; done
  if kill -0 $pid 2>/dev/null; then
    echo "SIG$sig: still running 5 s after the signal"; kill -KILL $pid; ok=0
  fi
  wait $pid; st=$?
  ms=$(( ($(date +%s%N) - t0) / 1000000 ))
  want=$([ $sig = TERM ] && echo 143 || echo 130)
  echo "SIG$sig: status $st after ${ms} ms (want $want, < 3000 ms)"
  { [ "$st" = "$want" ] && [ "$ms" -lt 3000 ]; } || ok=0
done
[ $ok = 1 ] && echo "signal_exit: ok" || { echo "signal_exit: FAILED"; exit 1; }
