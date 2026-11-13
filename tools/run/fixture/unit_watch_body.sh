# unit_watch_body.sh, sourced by unit_watch.sh <session> <out>: land_gate.sh's watch beside one unit's run (its
# `watched`), until it is stopped
# (TERM). The unit hangs, and the watch keeps its JIT stacks in <out>.hung and stops its processes, when:
# - a process of its session parks (a panic, a compile error, a stale access or a failed update waiting at its
#   call: their `makepad: ... parked` line in the unit's output <out> or in a JIT's session log under $TMPDIR).
#   A unit runs unattended, so nobody resumes it; it fails HUNG at the next look with the park's text. An app a
#   test started parked at a compile error and held its unit 64 min, until its lease ended (11-07). A panic
#   that parks for less than a look (an unwatched one's brief wait) goes on by itself and is no hang.
# - or, while a target's tests run (its `running N tests` to its own `test result:`), it makes no progress
#   for HUNG_QUIET s: no new test result, and its processes do nothing by the node's own idle look (devhub
#   lease/exec.rs `busy`: under IDLE_PCT % of a core, past 0.2 s, and under 1 MB of storage I/O since the quiet
#   began, or a process gone with what it had done). Storage I/O is `io`'s read_bytes and write_bytes, as the
#   lease's scope's io.stat counts it. A socket's or a pipe's traffic is none: a test polling its parked app's
#   remote every 50 ms moved 10-20 KB/s between its own processes at under 1% of a core, read as busy, and held
#   its unit 64 min (11-07). A build (no target's tests running: its compile, a cold JIT's, or its wait for the
#   box's JIT store) is never a hang, and nor is a unit at work, however long it takes: a bound from its warm
#   measure (twice it, then the quiet) failed a cold JIT's `makepad-fab --bin` HUNG at 672 s against a warm 66 s,
#   its first frame at 471 s and a test thread at a core in JIT code (11-09), and would fail every landing that
#   changes the JIT.
# - or, past twice its measure (MEASURE_S: its last run's seconds, 0: none yet), its tests run and make no progress,
#   by the same look, for its measure (at least two looks, at most HUNG_QUIET): the user's rule for a CI leg past
#   twice its measured time (11-02) is its JIT's capture and a failure with the cause, and a unit is held to its
#   own measure as a whole lease is to its class's (sched.rs say_stalled). Held to the 9 min alone, `test
#   relax-runner` (measured 27 s) sat 9 min, its harness parked reading a child that waited on nothing, and its
#   shard and box with it (L939783, 11-13). A build and a unit at work are still never stopped.
# - or its CPU is no work of its own: no new test result, no I/O and no process gone for HUNG_QUIET s while its
#   tests run, and every process of it busy over a second answers the JIT's why-hung with main idle (or waiting
#   on the GPU) and every busy thread of it in a shared library alone, no frame in the JIT's image or the
#   program's (driver_spin): a thread spinning in a GPU driver's fence wait. Counted as work, such a thread held
#   a unit 732 min (11-08), and a full run's `test makepad-ai-vulkan` (measured 37 s) 35 min, its main idle and
#   one thread at 94% in libnvidia-eglcore, until it was cancelled by hand (11-12). Asked once a minute past the
#   quiet; a process the JIT does not answer for, main at work, or a busy thread with a frame of its own code is
#   work.
# A unit that spins in its own code runs on here: by its stacks a spin there is no different from work (six
# threads in `while !go` and a test thread spinning in a budget callback would have held one for ever, 11-09).
# Such a unit is said, never stopped: once its tests run BUSY_S s with no new test result (its busy measure,
# land_gate.sh's: far past its last one; 0: never), the watch keeps one sample of its state in <out>.busy (as
# <out>.hung's, with its side files under <out>.busy.*), into the lease's keep dir ($DEVHUB_KEEP/<out's dir's
# name>/) too, and says `GATEBUSY <unit> :: <epoch s of its last result> <sample's path in the keep dir> <what the
# sample says>` live on fd 4 (land_gate.sh's at_once: the shard's log, beside its GATESTART lines), which the
# master's check-in lists (bucket.rs slow_units) until the unit ends. A new result after it says `GATEBUSY <unit>
# :: 0`, and a later stall is sampled and said again. Main or the unit's owner decides what to do with it.
# Otherwise there is no time cap: a unit that works runs as long as it works. HUNG_QUIET is 9 min, a minute under
# its lease's own look (sched.rs QUIET_STALL_MS, 10 min), so a hung test fails as itself and never makes its lease
# look hung and hold its box (a makepad-app-amp test sat in a GStreamer pipeline, its lease idle 10 min with
# hundreds of packages to go, holding its box's WM restart until it was cancelled, 11-07).
# Before it stops them, the watch keeps their state in <out>.hung, the cause first: each JIT's `capture` (the
# first-frame timeout's: why-hung, every stack, waits, the JIT's queue, its HAL shell, pace and pump (the WM wire),
# its flight recording, kept beside as <out>.<pid>.mkpf), every thread's state, CPU over a second and kernel wait
# channel (a driver's own threads too, where a JIT's walk stops), the box's WMs' uptimes (a display reset under
# the unit) and the GPU's (nvidia-smi: said if it has not answered, never waited on past the capture).
# The most it held at once, in MB in <out>.peak (the tests run's GATEPEAK), and its five largest processes then in
# <out>.peak.by (peak_look).
# B: the JIT command for the captures (`makepad`); HUNG_QUIET, HUNG_LOOK (s between looks), IDLE_PCT; BUSY_S,
# BUSY_UNIT (its GATEBUSY name); MEASURE_S. What sources this gives it its clock and /proc: `now`, `proc`, `nap`,
# wait_s <s>, read_gpu <file> and wall (the epoch s).
set -u
session=$1 out=$2
HUNG_QUIET=${HUNG_QUIET:-540} HUNG_LOOK=${HUNG_LOOK:-10} IDLE_PCT=${IDLE_PCT:-5} B=${B:-makepad}
BUSY_S=${BUSY_S:-0} BUSY_UNIT=${BUSY_UNIT:-} MEASURE_S=${MEASURE_S:-0}
# quiet_now: the quiet that fails it now, into quiet, and what it says of it into past: HUNG_QUIET, or past twice its
# measure, that measure (at least two looks, at most HUNG_QUIET)
quiet_now() {
  quiet=$HUNG_QUIET past=""
  [ "$MEASURE_S" -gt 0 ] && [ "$now" -ge $((2 * MEASURE_S)) ] || return 0
  local q=$MEASURE_S
  [ "$q" -ge $((2 * HUNG_LOOK)) ] || q=$((2 * HUNG_LOOK))
  [ "$q" -lt "$HUNG_QUIET" ] || return 0
  quiet=$q past=", past twice its measured $MEASURE_S s"
}
# unit_activity: the session's processes' CPU in µs (/proc stat utime, stime, cutime, cstime at 100 Hz), storage
# I/O bytes (io read_bytes + write_bytes) and resident memory in KB: their RSS now, summed (status VmRSS), and the
# largest one's own peak (VmHWM: between looks too; its pid in ua_hwm_pid) into ua_cpu, ua_io, ua_rss and ua_hwm,
# their pids into ua_pids and each one's RSS into ua_prss, without a fork
unit_activity() {
  local p s f k v h r
  ua_cpu=0; ua_io=0; ua_rss=0; ua_hwm=0; ua_hwm_pid=""; ua_pids=(); ua_prss=()
  for p in "$proc"/[0-9]*; do
    read -r s 2>/dev/null < "$p/stat" || continue
    s=${s##*) }; read -r -a f <<< "$s"
    [ "${f[3]}" = "$session" ] || continue
    ua_pids+=("${p##*/}")
    ua_cpu=$((ua_cpu + (f[11] + f[12] + f[13] + f[14]) * 10000))
    while read -r k v; do case $k in read_bytes:|write_bytes:) ua_io=$((ua_io + v));; esac; done 2>/dev/null < "$p/io"
    h=0 r=0
    while read -r k v _; do case $k in VmHWM:) h=$v;; VmRSS:) r=$v; break;; esac; done 2>/dev/null < "$p/status"
    ua_rss=$((ua_rss + r)); ua_prss+=("$r")
    [ "$h" -le "$ua_hwm" ] || { ua_hwm=$h; ua_hwm_pid=${p##*/}; }
  done
}
# gb <KB>: in GB to a tenth, into gbv; cmd_of <pid>: its command line cut to 80 characters (its name without one),
# into cmd; neither forks
gb() { local t=$(( ($1 * 10 + 524288) / 1048576 )); gbv="$((t / 10)).$((t % 10)) GB"; }
cmd_of() {
  local -a a=()
  mapfile -d '' -t a 2>/dev/null < "$proc/$1/cmdline"
  cmd="${a[*]}"
  [ -n "$cmd" ] || read -r cmd 2>/dev/null < "$proc/$1/comm" || cmd="pid $1"
  cmd=${cmd:0:80}
}
# peak_look: the most the unit held at once so far, in KB in peak: its processes' RSS summed at this look, or one
# process's own peak (VmHWM, between looks too) when that is more. Never their VmHWMs summed: peaks of different
# moments, and of processes long gone by then, read a force_mode unit's 12 GB as 30 GB, which every shard it
# was dealt to was fitted by (11-10). At a new peak, its five largest processes then, largest first, into
# <out>.peak.by: a build the unit set off is named
peak_look() {
  local i j k n=${#ua_pids[@]} by="" idx=()
  if [ "$ua_hwm" -gt "$ua_rss" ]; then
    [ "$ua_hwm" -gt "$peak" ] || return 0
    peak=$ua_hwm; gb "$ua_hwm"; cmd_of "$ua_hwm_pid"
    by="$gbv $cmd (its own peak, between looks)"
  else
    [ "$ua_rss" -gt "$peak" ] || return 0
    peak=$ua_rss
    for ((i = 0; i < n; i++)); do idx+=("$i"); done
    for ((i = 0; i < n && i < 5; i++)); do
      for ((j = i + 1; j < n; j++)); do
        [ "${ua_prss[idx[j]]}" -le "${ua_prss[idx[i]]}" ] || { k=${idx[i]}; idx[i]=${idx[j]}; idx[j]=$k; }
      done
      gb "${ua_prss[idx[i]]}"; cmd_of "${ua_pids[idx[i]]}"
      by+="${by:+; }$gbv $cmd"
    done
  fi
  printf '%s\n' "$by" > "$out.peak.by"
}
# parked_line: the first park of the unit that waits past a look, from <out> and its processes' session logs
# (tui.rs: $TMPDIR/relax-session-<pid>-<ns>.log), with the compile error it parked at; none: empty
parked_line() {
  local f files=("$out") line s p
  for p in "${ua_pids[@]}"; do
    for f in "${TMPDIR:-/tmp}/relax-session-$p-"*.log; do [ -e "$f" ] && files+=("$f"); done
  done
  for f in "${files[@]}"; do
    while IFS= read -r line; do
      case $line in
        "relax: panic parked on thread "*" s -- attach: "*)
          s=${line% s -- attach: *}; s=${s##* for }
          [[ $s =~ ^[0-9]+$ ]] && [ "$s" -le "$HUNG_LOOK" ] && continue ;;
        *"compile error parked"*)
          p=$(grep -m1 -E 'compile error in ' "$f" 2>/dev/null | cut -c1-300)
          [ -z "$p" ] || line="$p; $line" ;;
      esac
      echo "${line:0:600}"; return
    done < <(grep -E '^relax: (panic parked on thread |compile error parked on thread |stale access parked as P[0-9]|.* -- parked as P[0-9]+ \(the app holds still)' "$f" 2>/dev/null)
  done
}
# out_state: from the unit's output, the test results it reported (out_results) and whether a target's tests run
# now (out_in 1: a `running N tests` has no `test result:` of its own after it). A test that runs one of its
# target's tests again in a child process whose output is the unit's (the harness's self-exec) adds the child's own
# `running 1 test` to `test result:` inside its target's: nested, not its target's end. Read as its end, it made
# the rest of relax-compiler's tests a build here, and one idle in a test that waited on such a child held its
# shard 28 min to 1h38m, never HUNG, and its run ended with no verdict, three times (11-11, 11-12)
out_state() {
  read -r out_results out_in < <(awk '/^running [0-9]+ tests?$/ { d++ } /^test result:/ { if (d) d-- } /^test .* \.\.\. / { n++ } END { print n + 0, (d > 0) }' "$out" 2>/dev/null)
}
# threads <pid>...: each thread of them: its state, its CPU in % of a core over a second, its kernel wait channel
threads() {
  local p t s f n=() c0=() i=0
  for p in "$@"; do for t in "$proc/$p"/task/[0-9]*; do
    read -r s 2>/dev/null < "$t/stat" || continue
    s=${s##*) }; read -r -a f <<< "$s"; n+=("${t#"$proc"/}"); c0+=($((f[11] + f[12])))
  done; done
  wait_s 1
  for t in "${n[@]}"; do
    read -r s 2>/dev/null < "$proc/$t/stat" || { i=$((i + 1)); continue; }
    local comm=${s#*(}; comm=${comm%)*}; s=${s##*) }; read -r -a f <<< "$s"
    echo "  $t $comm ${f[0]} $((f[11] + f[12] - c0[i]))% wchan $(cat "$proc/$t/wchan" 2>/dev/null)"
    i=$((i + 1))
  done
}
# keep_state <file> <why> <side>: its processes' state into <file>, the cause first (the header's list), its side
# files (the GPU's answer, the JITs' flight recordings) as <side>.gpu and <side>.<pid>.mkpf; what the JIT says
# into cause and the busiest thread into top
keep_state() {
  local file=$1 why=$2 side=$3 gpu=$3.gpu th caps="" p a path c
  cause="" top=""
  unit_activity
  # the GPU's read first, beside the rest
  read_gpu "$gpu"
  # its threads as they are, before the captures make its JITs busy
  th=$(threads "${ua_pids[@]}")
  for p in "${ua_pids[@]}"; do
    a=$($B inspect "$p" capture "why=the tests gate: $why" as=json 2>&1 | head -c 20000)
    path=$(sed -n 's/.*"path":"\([^"]*\)".*/\1/p' <<< "$a" | head -1)
    c=$(sed -n 's/.*"cause":"\(\([^"\\]\|\\.\)*\)".*/\1/p' <<< "$a" | head -1)
    [ -n "$cause" ] || cause=$c
    caps+="-- pid $p: capture${path:+ $path}"$'\n'
    if [ -n "$path" ] && [ -f "$path" ]; then
      caps+=$(head -c 600000 "$path")$'\n'
      [ ! -f "${path%.txt}.mkpf" ] || cp "${path%.txt}.mkpf" "$side.$p.mkpf"
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
    printf '%s' "$caps"; } > "$file" 2>&1
}
# driver_spin: the unit's threads busy over a second (threads), when each of their processes answers the JIT's
# why-hung with main idle or waiting on the GPU and each thread of it busy in that probe (its own line's cpu_pct,
# or other_threads' `<name> T<n> <state> <pct>%: <top>`) runs in shared libraries alone: what spins, as `pid 7:
# main idle, T28 94% in libnvidia-eglcore.so.610.57.04`; else empty (none busy, no answer, main at work, or a
# frame of its own)
driver_spin() {
  local th p a d said="" pids=()
  th=$(threads "${ua_pids[@]}")
  while read -r p; do pids+=("$p"); done < <(awk -v min="$IDLE_PCT" '{ for (i = 2; i < NF; i++) if ($i ~ /^[0-9]+%$/ && $(i + 1) == "wchan" && $i + 0 >= min) { split($1, t, "/"); print t[1]; break } }' <<< "$th" | sort -u)
  [ "${#pids[@]}" -gt 0 ] || return 0
  for p in "${pids[@]}"; do
    a=$($B inspect "$p" why-hung as=json 2>/dev/null | head -c 200000)
    d=$(awk -v min="$IDLE_PCT" '
      function lib(m) { return m ~ /\.(so|dylib|dll)(\.[0-9]+)*$/ }
      /^\{"verdict":/ { match($0, /"verdict":"[^"]*"/); v = substr($0, RSTART + 11, RLENGTH - 12) }
      /^\{"thread":/ {
        c = 0; if (match($0, /"cpu_pct":[0-9.]+/)) c = substr($0, RSTART + 10, RLENGTH - 10) + 0
        if (c < min) next
        id = "?"; if (match($0, /"id":"[^"]*"/)) id = substr($0, RSTART + 6, RLENGTH - 7)
        r = $0; n = 0; in1 = ""
        while (match(r, /"in":"[^"]*"/)) { m = substr(r, RSTART + 6, RLENGTH - 7); r = substr(r, RSTART + RLENGTH); n++; if (!lib(m)) own = 1; else if (in1 == "") in1 = m }
        if (!n) own = 1; else spin = spin (spin == "" ? "" : ", ") id " " c "% in " in1
      }
      /^\{"other_threads":/ {
        r = $0
        while (match(r, / T[0-9]+ [a-z ]+ [0-9]+%: [^"]*/)) {
          e = substr(r, RSTART, RLENGTH); r = substr(r, RSTART + RLENGTH)
          split(e, h, "%: "); c = h[1]; sub(/.* /, "", c)
          if (c + 0 < min) continue
          m = h[2]; sub(/\+0x[0-9a-f]+$/, "", m)
          if (!lib(m)) own = 1; else { split(e, w, " "); spin = spin (spin == "" ? "" : ", ") w[1] " " c "% in " m }
        }
      }
      END { if (!own && spin != "" && (v == "idle" || v == "gpu wait")) print "main " v ", " spin }' <<< "$a")
    [ -n "$d" ] || return 0
    said+="${said:+; }pid $p: $d"
  done
  echo "$said"
}
# say <line>: live, on fd 4 (land_gate.sh's at_once: the shard's log); none there, nowhere
say() { { printf '%s\n' "$1" >&4; } 2> /dev/null || :; }
# busy_said: its stall was said (GATEBUSY); moved: when its tests last moved (a build, a new result); r1: its results then
busy_said="" moved=0 r1=0
stop() { echo $((peak / 1024)) > "$out.peak"; }
peak=0
trap '[ -z "$nap" ] || kill "$nap" 2>/dev/null; stop; exit 0' TERM
since=$now rsince=$now spin_at=0
unit_activity; c0=$ua_cpu; i0=$ua_io; peak_look
out_state; r0=$out_results
why=""
while :; do
  wait_s "$HUNG_LOOK"
  unit_activity; over=$(( (now - since) * 1000000 ))
  peak_look
  parked=$(parked_line)
  if [ -n "$parked" ]; then why="a process of it parked, which nobody resumes in a gate: $parked"; break; fi
  out_state
  # its tests moved (a build, a new result), or they ran BUSY_S s since with no new result: a stall, said once
  if [ "$out_in" != 1 ] || [ "$out_results" -gt "$r1" ]; then
    [ -z "$busy_said" ] || say "GATEBUSY $BUSY_UNIT :: 0"
    busy_said="" moved=$now r1=$out_results
  elif [ "$BUSY_S" -gt 0 ] && [ -z "$busy_said" ] && [ $((now - moved)) -ge "$BUSY_S" ]; then
    keep_state "$out.busy" "busy: no new test result for $((now - moved)) s while its tests ran, $now s into the unit; nothing stops it" "$out.busy"
    kept=$(basename "$(dirname "$out")")/$(basename "$out").busy
    [ -z "${DEVHUB_KEEP:-}" ] || { mkdir -p "$DEVHUB_KEEP/${kept%/*}" && cp -p "$out.busy" "$DEVHUB_KEEP/$kept"; }
    s="${cause:+the JIT says: $cause; }${top:+the busiest thread: $top}"
    say "GATEBUSY $BUSY_UNIT :: $(( $(wall) - (now - moved) )) $kept ${s:0:300}"
    busy_said=1
  fi
  # progress: a build (no target's tests run), a new test result, a process gone with what it had done or storage
  # I/O; its CPU past IDLE_PCT of a core too, unless that CPU spins outside its code past the quiet (driver_spin)
  if [ "$out_in" != 1 ] || [ "$out_results" -gt "$r0" ] || [ "$ua_cpu" -lt "$c0" ] || [ "$ua_io" -lt "$i0" ] || [ $((ua_io - i0)) -gt 1048576 ]; then
    since=$now rsince=$now; c0=$ua_cpu; i0=$ua_io; r0=$out_results; continue
  fi
  quiet_now
  if [ $((ua_cpu - c0)) -gt $(( over * IDLE_PCT / 100 > 200000 ? over * IDLE_PCT / 100 : 200000 )) ]; then
    since=$now; c0=$ua_cpu; i0=$ua_io
    if [ $((now - rsince)) -ge "$quiet" ] && [ "$now" -ge "$spin_at" ]; then
      spin_at=$((now + 60)); spin=$(driver_spin)
      if [ -n "$spin" ]; then why="its processes did no work of their own (no test result, no I/O; their CPU a spin outside their code: $spin) for $((now - rsince)) s while its tests ran, $now s into the unit$past"; break; fi
    fi
    continue
  fi
  if [ $((now - since)) -ge "$quiet" ]; then why="its processes did nothing (no test result, under $IDLE_PCT% of a core, no I/O) for $((now - since)) s while its tests ran, $now s into the unit$past"; break; fi
done
stop
keep_state "$out.hung" "$why" "$out"
# its processes only: the session started for it
for p in "$proc"/[0-9]*; do
  read -r s 2>/dev/null < "$p/stat" || continue
  s=${s##*) }; read -r -a f <<< "$s"
  [ "${f[3]}" = "$session" ] && kill -KILL "${p##*/}" 2>/dev/null
done
exit 0
