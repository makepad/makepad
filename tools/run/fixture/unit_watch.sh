#!/bin/bash
# unit_watch.sh <session> <out>: land_gate.sh's watch beside one unit's run (its `watched`), until it is stopped
# (TERM). The unit hangs, and the watch keeps its JIT stacks in <out>.hung and stops its processes, when:
# - a process of its session parks (a panic, a compile error, a stale access or a failed update waiting at its
#   call: their `makepad: ... parked` line in the unit's output <out> or in a JIT's session log under $TMPDIR).
#   A unit runs unattended, so nobody resumes it; it fails HUNG at the next look with the park's text. An app a
#   test started parked at a compile error and held its unit 64 min, until its lease ended (11-07). A panic
#   that parks for less than a look (an unwatched one's brief wait) goes on by itself and is no hang.
# - or its processes do nothing for HUNG_QUIET s by the node's own idle look (devhub lease/exec.rs `busy`: under
#   IDLE_PCT % of a core, past 0.2 s, and under 1 MB of storage I/O since the quiet began, or a process gone with
#   what it had done). Storage I/O is `io`'s read_bytes and write_bytes, as the lease's scope's io.stat counts
#   it. A socket's or a pipe's traffic is none: a test polling its parked app's remote every 50 ms moved
#   10-20 KB/s between its own processes at under 1% of a core, read as busy, and held its unit 64 min (11-07).
# - or, busy or not, it runs past its bound: its last measure MEASURED_S (the tests run's GATE_TIMES) twice over,
#   then HUNG_QUIET more. A CI leg gets its full capture past twice its measure, and a
#   unit measured at 15 s spun a driver thread at 96% of a core in libnvidia-eglcore, its main waiting in the
#   driver, busy by every look, for 732 min (11-08). Twice the measure alone fails about ten healthy units a run
#   (the last 25 tests runs: 307 of 8309 units past twice their last measure); with the quiet a hung unit is given
#   on top, one, a real hang. A unit not measured yet has no bound.
# Otherwise there is no time cap: a unit that works runs as long as it works. HUNG_QUIET is 9 min, a minute under
# its lease's own look (sched.rs QUIET_STALL_MS, 10 min), so a hung test fails as itself and never makes its lease
# look hung and hold its box (a makepad-app-amp test sat in a GStreamer pipeline, its lease idle 10 min with
# hundreds of packages to go, holding its box's WM restart until it was cancelled, 11-07).
# Before it stops them, the watch keeps their state in <out>.hung, the cause first: each JIT's `capture` (the
# first-frame timeout's: why-hung, every stack, waits, the JIT's queue, its HAL shell, pace and pump (the WM wire),
# its flight recording, kept beside as <out>.<pid>.mkpf), every thread's state, CPU over a second and kernel wait
# channel (a driver's own threads too, where a JIT's walk stops), the box's WMs' uptimes (a display reset under
# the unit) and the GPU's (nvidia-smi: said if it has not answered, never waited on past the capture).
# Its processes' summed peak RSS, the most at any look, in MB in <out>.peak (the tests run's GATEPEAK).
# B: the JIT command for the captures (`makepad`); HUNG_QUIET, HUNG_LOOK (s between looks), IDLE_PCT,
# MEASURED_S (none: no bound).
set -u
session=$1 out=$2
HUNG_QUIET=${HUNG_QUIET:-540} HUNG_LOOK=${HUNG_LOOK:-10} IDLE_PCT=${IDLE_PCT:-5} B=${B:-makepad} MEASURED_S=${MEASURED_S:-}
bound=""
[ -z "$MEASURED_S" ] || bound=$((2 * MEASURED_S + HUNG_QUIET))
# unit_activity: the session's processes' CPU in µs (/proc stat utime, stime, cutime, cstime at 100 Hz), storage
# I/O bytes (io read_bytes + write_bytes) and peak RSS in KB (status VmHWM: each one's
# own peak, between looks too) into ua_cpu, ua_io and ua_rss, and their pids into ua_pids, without a fork
unit_activity() {
  local p s f k v
  ua_cpu=0; ua_io=0; ua_rss=0; ua_pids=()
  for p in /proc/[0-9]*; do
    read -r s 2>/dev/null < "$p/stat" || continue
    s=${s##*) }; read -r -a f <<< "$s"
    [ "${f[3]}" = "$session" ] || continue
    ua_pids+=("${p#/proc/}")
    ua_cpu=$((ua_cpu + (f[11] + f[12] + f[13] + f[14]) * 10000))
    while read -r k v; do case $k in read_bytes:|write_bytes:) ua_io=$((ua_io + v));; esac; done 2>/dev/null < "$p/io"
    while read -r k v _; do case $k in VmHWM:) ua_rss=$((ua_rss + v)); break;; esac; done 2>/dev/null < "$p/status"
  done
}
# parked_line: the first park of the unit that waits past a look, from <out> and its processes' session logs
# (tui.rs: $TMPDIR/makepad-run-session-<pid>-<ns>.log), with the compile error it parked at; none: empty
parked_line() {
  local f files=("$out") line s p
  for p in "${ua_pids[@]}"; do
    for f in "${TMPDIR:-/tmp}/makepad-run-session-$p-"*.log; do [ -e "$f" ] && files+=("$f"); done
  done
  for f in "${files[@]}"; do
    while IFS= read -r line; do
      case $line in
        "makepad: panic parked on thread "*" s -- attach: "*)
          s=${line% s -- attach: *}; s=${s##* for }
          [[ $s =~ ^[0-9]+$ ]] && [ "$s" -le "$HUNG_LOOK" ] && continue ;;
        *"compile error parked"*)
          p=$(grep -m1 -E 'compile error in ' "$f" 2>/dev/null | cut -c1-300)
          [ -z "$p" ] || line="$p; $line" ;;
      esac
      echo "${line:0:600}"; return
    done < <(grep -E '^makepad: (panic parked on thread |compile error parked on thread |stale access parked as P[0-9]|.* -- parked as P[0-9]+ \(the app holds still)' "$f" 2>/dev/null)
  done
}
stop() { echo $((peak / 1024)) > "$out.peak"; }
nap=""; peak=0
trap '[ -z "$nap" ] || kill "$nap" 2>/dev/null; stop; exit 0' TERM
since=$SECONDS
unit_activity; c0=$ua_cpu; i0=$ua_io; peak=$ua_rss
why=""
while :; do
  sleep "$HUNG_LOOK" & nap=$!; wait "$nap"; nap=""
  unit_activity; over=$(( (SECONDS - since) * 1000000 ))
  [ "$ua_rss" -le "$peak" ] || peak=$ua_rss
  parked=$(parked_line)
  if [ -n "$parked" ]; then why="a process of it parked, which nobody resumes in a gate: $parked"; break; fi
  if [ -n "$bound" ] && [ "$SECONDS" -ge "$bound" ]; then
    why="it ran $SECONDS s, past its bound of $bound s (twice its measured $MEASURED_S s, then $HUNG_QUIET s)"; break
  fi
  if [ "$ua_cpu" -lt "$c0" ] || [ "$ua_io" -lt "$i0" ] || [ $((ua_cpu - c0)) -gt $(( over * IDLE_PCT / 100 > 200000 ? over * IDLE_PCT / 100 : 200000 )) ] || [ $((ua_io - i0)) -gt 1048576 ]; then
    since=$SECONDS; c0=$ua_cpu; i0=$ua_io; continue
  fi
  if [ $((SECONDS - since)) -ge "$HUNG_QUIET" ]; then why="its processes did nothing (under $IDLE_PCT% of a core, no I/O) for $((SECONDS - since)) s"; break; fi
done
stop
# threads <pid>...: each thread of them: its state, its CPU in % of a core over a second, its kernel wait channel
threads() {
  local p t s f n=() c0=() i=0
  for p in "$@"; do for t in /proc/"$p"/task/[0-9]*; do
    read -r s 2>/dev/null < "$t/stat" || continue
    s=${s##*) }; read -r -a f <<< "$s"; n+=("$t"); c0+=($((f[11] + f[12])))
  done; done
  sleep 1
  for t in "${n[@]}"; do
    read -r s 2>/dev/null < "$t/stat" || { i=$((i + 1)); continue; }
    local comm=${s#*(}; comm=${comm%)*}; s=${s##*) }; read -r -a f <<< "$s"
    echo "  ${t#/proc/} $comm ${f[0]} $((f[11] + f[12] - c0[i]))% wchan $(cat "$t/wchan" 2>/dev/null)"
    i=$((i + 1))
  done
}
unit_activity
# the GPU's read first, beside the rest
gpu="$out.gpu"
if command -v nvidia-smi > /dev/null 2>&1; then { nvidia-smi > "$gpu" 2>&1; echo "== answered" >> "$gpu"; } & fi
# its threads as they hung, before the captures make its JITs busy
th=$(threads "${ua_pids[@]}")
caps="" cause=""
for p in "${ua_pids[@]}"; do
  a=$($B inspect "$p" capture "why=the tests gate: $why" as=json 2>&1 | head -c 20000)
  path=$(sed -n 's/.*"path":"\([^"]*\)".*/\1/p' <<< "$a" | head -1)
  c=$(sed -n 's/.*"cause":"\(\([^"\\]\|\\.\)*\)".*/\1/p' <<< "$a" | head -1)
  [ -n "$cause" ] || cause=$c
  caps+="-- pid $p: capture${path:+ $path}"$'\n'
  if [ -n "$path" ] && [ -f "$path" ]; then
    caps+=$(head -c 600000 "$path")$'\n'
    [ ! -f "${path%.txt}.mkpf" ] || cp "${path%.txt}.mkpf" "$out.$p.mkpf"
  else
    caps+="$a"$'\n'
  fi
done
# the busiest thread: a spin the JIT's walk cannot follow (into a driver without unwind tables) still shows here
top=$(awk '{ for (i = 2; i <= NF; i++) if ($i ~ /^[0-9]+%$/ && $i + 0 > best) { best = $i + 0; line = $0 } } END { if (best > 0) print line }' <<< "$th" | sed 's/^ *//')
{ echo "== $why${cause:+; the JIT says: $cause}${top:+; the busiest thread: $top}; their state"
  echo "-- threads (pid/task/tid, name, state, % of a core over 1 s, kernel wait channel)"; echo "$th"
  echo "-- the box's WMs"
  $B inspect list 2>/dev/null | sed -n 's/.*"pid":\([0-9]*\),"app":"\([^"]*wm[^"]*\)".*/\1 \2/p' | while read -r p a; do
    echo "  $a pid $p up $(ps -o etimes= -p "$p" 2>/dev/null | tr -d ' ') s"
  done
  echo "-- the GPU"
  if [ ! -e "$gpu" ]; then echo "  (no nvidia-smi on this box)"
  elif grep -q '^== answered$' "$gpu"; then grep -v '^== answered$' "$gpu"
  else echo "  (nvidia-smi has not answered by the end of the capture: the driver may be stuck)"; cat "$gpu"; fi
  printf '%s' "$caps"; } > "$out.hung" 2>&1
# its processes only: the session started for it
for p in /proc/[0-9]*; do
  read -r s 2>/dev/null < "$p/stat" || continue
  s=${s##*) }; read -r -a f <<< "$s"
  [ "${f[3]}" = "$session" ] && kill -KILL "${p#/proc/}" 2>/dev/null
done
exit 0
