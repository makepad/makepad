#!/bin/bash
# unit_watch.sh <session> <out>: the watch beside one unit's run (unit_watch_body.sh says what it does), on this
# box's clock and /proc.
# The watch's time in s (now) and the /proc it reads (proc); wait_s <s> moves them on.
t0=$SECONDS now=0 proc=/proc nap=""
# wall: the epoch s
wall() { date +%s; }
wait_s() {
  sleep "$1" & nap=$!; wait "$nap"; nap=""; now=$((SECONDS - t0))
}
# read_gpu <file>: the GPU's state into it, beside the rest
read_gpu() {
  command -v nvidia-smi > /dev/null 2>&1 || return 0
  { nvidia-smi > "$1" 2>&1; echo "== answered" >> "$1"; } &
}
. "$(dirname "${BASH_SOURCE[0]}")/unit_watch_body.sh"
