#!/bin/bash
# box.sh <snapdir> [plan-file] : the whole scoreboard on this bench box, unattended. Builds (snap.sh, untimed slot),
# then one timing ticket per workload (tq.sh: exclusive box) running sbstage.py, each appending to
# <snapdir>/results.jsonl. Progress: <snapdir>/box.status (one line per stage), log <snapdir>/box.log.
# Plan file: one workload per line ("rw:inflate_real", "game:race", ...; '#' comments; optional per-line
# sbstage args after the workload, e.g. "start:stage --settle 90").
set -u
H=$(cd "$(dirname "$0")" && pwd)
D=$(cd "$1" && pwd); P=${2:-$H/plan-x64.txt}
# the board's own result file (tests and ad-hoc stages write results.jsonl)
export SB_RESULTS=$D/board.jsonl
TQ=${SB_TQ:-$HOME/rc/tq.sh}; SLOT=${SB_SLOT:-$HOME/rc/slot.sh}
st() { echo "$(date +%F' '%T) $*" >> "$D/box.status"; }
st "start plan $P"
if [ "${SB_NOBUILD:-0}" != 1 ] && ! grep -q SNAP-OK "$D/snap.out" 2>/dev/null; then
  st "build"
  $SLOT build 1 bash "$H/snap.sh" "$D" > "$D/snap.out" 2>&1
  grep -q SNAP-OK "$D/snap.out" || st "BUILD PARTIAL: $(tail -1 "$D/snap.out")"
fi
grep -v '^#' "$P" | while read -r wl args; do
  [ -z "$wl" ] && continue
  if grep -F "\"wl\": \"$wl\"" "$SB_RESULTS" 2>/dev/null | grep -q '"ok": true' && [ "${SB_RERUN:-0}" != 1 ]; then st "skip $wl (has results)"; continue; fi
  st "stage $wl"
  # shellcheck disable=SC2086
  $TQ python3 "$H/sbstage.py" "$D" "$wl" ${SB_ARGS:-} $args >> "$D/box.log" 2>&1 < /dev/null
  st "done $wl rc=$?"
done
st "ALL-DONE"
