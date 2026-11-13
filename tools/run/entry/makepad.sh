#!/bin/sh
# Makepad for macOS and Linux: gets Rust, compiles Makepad's loader (makepad run) from its source on
# this computer and starts the runner. A terminal shows its steps on one screen, redrawn in place
# (the Builder's look; makepad.bat draws the same on Windows); other output gets plain progress
# lines. Cargo's full output stays in the build log, curl's errors in the download log.
# Read it before you run it; it is this one file. Rust stays in its own folder; no system install.
#
#   curl -fsSL https://makepad.nl/runner/unix | sh
#
# 1. Use this folder's Rust if ready, else a suitable installed stable Rust (1.98.0 or newer).
#    Otherwise get Rust 1.98.0 (rustc, cargo, rust-std) from static.rust-lang.org, fetched as
#    25 MB pieces over parallel connections, checked against the sizes and SHA-256 sums below.
# 2. At the same time, the loader's source: a ZIP the service's latest licence-free `runner`
#    release names (size and SHA-256 checked before unpacking), cut from the same Makepad commit as
#    the runner, so the two always fit; and, with no arguments, the packs of the runner's git
#    repositories, which makepad run takes instead of downloading them itself.
# 3. The loader and its tiny app launcher are compiled for this computer's processor (`cargo build --profile host --offline`:
#    the ZIP holds every crate) the moment Rust and the source are in place, and started with no
#    arguments: it runs the runner, which downloads the rest. With arguments, the loader runs with
#    them instead. The loader ZIP's SHA-256 keys .version and, after a successful build,
#    .loader-sha256; .built also records Rust and the target triple. An unchanged ZIP reuses the
#    source; matching SHA and Rust stamps reuse the binary. A changed ZIP replaces only the files
#    that changed, so only their crates compile again. Under cache/loader, current selects src or
#    a commit folder; pending rolls back to previous if the new runner did not confirm its boot.
#
# In its folder it creates rust/ (the toolchain), cache/ (downloads, the loader, its caches and
# logs), apps/ (what `makepad` and `makepad app <id>` run: the runner's and each app's launch
# record), runner/ (the runner's own state: licence, catalog, launch logs) and target/ (what makepad
# run builds for the apps: build scripts, unit graphs, code cache). The folder is this
# file's own when it holds nothing else (or is an installation already), else ~/makepad-runner
# (piped into sh, or saved among other downloads); $MAKEPAD_LOADER_ROOT overrides. Run it again to
# update and start, reusing downloads and compiled crates. The system needs a C linker (Apple's command line tools; on
# Linux cc) and, on Linux, OpenSSL 3's development files and the desktop/audio libraries;
# it names the packages when missing.
set -u
umask 022

rust_version=1.98.0
rust_date=2026-08-20
dist=https://static.rust-lang.org/dist/$rust_date
piece=25000000
service=${MAKEPAD_LOADER_SERVICE:-https://makepad.nl/api/loader}
service=${service%/}

# --- Progress and jobs --------------------------------------------------------------------
logs=
jobs=
job_names='fetch build unpack-rustc unpack-rust-std unpack-cargo'
prefetch_pids=

# say TEXT: a line of plain output (a terminal shows the screen instead)
say() { [ -n "${tty_out:-}" ] || printf '  %s\n' "$1"; }
now() { date +%s; }
# stamp NAME appends "NAME HH:MM:SS.cc" to the timing log
stamp() {
    [ -n "$logs" ] && [ -d "$logs" ] || return 0
    t=$(date +%H:%M:%S.%N)
    case "$t" in *N) t=${t%.N} ;; *) t=$(printf '%s' "$t" | cut -c1-11) ;; esac
    printf '%s %s\n' "$1" "$t" >> "$logs/bootstrap.log"
}
# Stop the recorded tree from the parent down before enumerating children: a stopped parent
# cannot fork another child between the snapshot and termination. ps is available on both hosts.
stop_tree() (
    stop_pid=$1
    case "$stop_pid" in ''|0|1|*[!0-9]*) printf '  Invalid job PID: %s\n' "$stop_pid" >&2; exit 1 ;; esac
    if ! kill -STOP "$stop_pid" 2>/dev/null; then
        state=$(ps -p "$stop_pid" -o stat=)
        case "$state" in ''|*Z*) exit 0 ;; esac
        printf '  Could not stop job process %s\n' "$stop_pid" >&2
        exit 1
    fi
    failed=0
    if snapshot=$(ps -axo pid=,ppid=); then
        children=$(printf '%s\n' "$snapshot" | awk -v parent="$stop_pid" '$2 == parent { print $1 }')
        for child_pid in $children; do stop_tree "$child_pid" || failed=1; done
    else
        printf '  Could not inspect children of job process %s\n' "$stop_pid" >&2
        failed=1
    fi
    if ! kill -KILL "$stop_pid" 2>/dev/null; then
        state=$(ps -p "$stop_pid" -o stat=)
        case "$state" in ''|*Z*) ;; *) printf '  Could not kill job process %s\n' "$stop_pid" >&2; failed=1 ;; esac
    fi
    deadline=$(($(now) + 5))
    while :; do
        state=$(ps -p "$stop_pid" -o stat=)
        case "$state" in ''|*Z*) break ;; esac
        if [ "$(now)" -ge "$deadline" ]; then
            printf '  Could not confirm job process %s stopped\n' "$stop_pid" >&2
            failed=1
            break
        fi
        sleep 0.1 2>/dev/null || sleep 1
    done
    exit "$failed"
)
# The background jobs this script started, stopped and reaped on failure or cancellation.
stop_jobs() {
    [ -n "$jobs" ] && [ -d "$jobs" ] || return 0
    stop_failed=0
    for job in $job_names; do
        if [ -f "$jobs/$job.pid" ] && [ ! -f "$jobs/$job.done" ]; then
            stop_tree "$(cat "$jobs/$job.pid")" || stop_failed=1
            if [ "$stop_failed" = 0 ] && [ -f "$jobs/$job.waiter.pid" ]; then
                wait "$(cat "$jobs/$job.waiter.pid")" 2>/dev/null || :
            fi
        fi
    done
    # A download that ended was reaped and its PID can be anyone's by now: only a child of this
    # script is stopped.
    for prefetch_pid in $prefetch_pids; do
        [ "$(ps -o ppid= -p "$prefetch_pid" | tr -d ' ')" = "$$" ] || continue
        stop_tree "$prefetch_pid" || stop_failed=1
        wait "$prefetch_pid" 2>/dev/null || :
    done
    return "$stop_failed"
}
# A run that was killed leaves its jobs' PID files, and those PIDs can be anyone's by now. They go
# before anything can stop a job; if they cannot, nothing here acts on them.
forget_old_jobs() {
    for job in $job_names; do
        if ! rm -f "$jobs/$job.pid" "$jobs/$job.waiter.pid"; then
            old_jobs=$jobs
            jobs=
            fail "Cannot clear the old job records in $old_jobs"
        fi
    done
}
# fail MESSAGE [LOG]: the error (on the screen below its rule, the step crossed out) and the end of
# LOG with its path on stderr, exit 1
fail() {
    stamp failed
    stop_jobs
    if [ -n "${framed:-}" ]; then screen_fail "$1" "${2:-}"; else printf '  Error: %s\n' "$1" >&2; fi
    if [ $# -gt 1 ] && [ -f "$2" ]; then
        if grep -q '^error' "$2"; then
            grep -n -B1 -A8 '^error' "$2" | tail -20 >&2
        else
            tail -20 "$2" >&2
        fi
        printf '  Full log: %s\n' "$2" >&2
    fi
    exit 1
}
trap 'stamp stopped; stop_jobs; exit 130' INT TERM
# job NAME COMMAND...: runs COMMAND in the background; its PID in jobs/NAME.pid, its exit code
# in jobs/NAME.done when it ends
job() {
    name=$1
    shift
    rm -f "$jobs/$name.pid" "$jobs/$name.done"
    (
        "$@" &
        echo $! > "$jobs/$name.pid"
        wait $!
        echo $? > "$jobs/$name.code"
        mv -f "$jobs/$name.code" "$jobs/$name.done"
    ) &
    echo $! > "$jobs/$name.waiter.pid"
    while [ ! -f "$jobs/$name.pid" ]; do sleep 0.1 2>/dev/null || sleep 1; done
}
sha256() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }
size_of() { wc -c < "$1" | tr -d ' '; }
# a path inside a double-quoted curl config value
cfgq() { printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'; }
# One-line pointers and stamps are published only after the whole line is written.
atomic_line() { printf '%s\n' "$2" > "$1.part" && mv -f "$1.part" "$1"; }
# rust_llvm_link DIR: the rustc component ships rust-objcopy (rustc strips macOS release builds with
# it) in lib/rustlib/<host>/bin, where it looks for LLVM in ../lib, but puts LLVM itself in lib/;
# only the llvm-tools component (a second copy of LLVM) fills that ../lib. A link there instead.
rust_llvm_link() {
    for rl_lib in "$1"/lib/libLLVM*; do
        [ -f "$rl_lib" ] || continue
        rl_at=$1/lib/rustlib/$triple/lib/${rl_lib##*/}
        [ -e "$rl_at" ] || ln -sf "../../../${rl_lib##*/}" "$rl_at" || return 1
    done
}
loader_folder_ready() {
    case "$1" in
        src) ;;
        *[!0123456789abcdef]*) return 1 ;;
        *) [ "${#1}" -eq 40 ] || return 1 ;;
    esac
    [ -f "$loader_root/$1/target/host/makepad" ]
}

# --- The screen -----------------------------------------------------------------------------
# On a terminal: one screen in the Builder's look, redrawn in place five times a second. A header
# (the time so far on its right), a row for each step (Rust, the loader's source, its build, the
# Runner) with a bar, sizes and speed or crates, a rule, a line saying what goes on now, the keys;
# on a terminal wide enough the Makepad mark beside it, in a grey for the terminal's background
# (OSC 11, or COLORFGBG). Lines longer than the terminal are cut, not wrapped, and a resized
# terminal is drawn again. Other output (a file, a pipe) gets plain lines: one when a step changes
# state, and every ten seconds while it works. makepad.bat draws the same screen on Windows.
# While the screen is up, stderr goes to cache/logs/stderr.log and is shown below the screen when it
# ends; a script the shell stops on an error of its own (fail and ctrl+c end the screen themselves)
# gets a failure screen with the shell's message below it.
e=$(printf '\033')
tty_out= stty_saved=
if [ -t 1 ] && [ "${TERM:-dumb}" != dumb ] && stty_saved=$(stty -g </dev/tty 2>/dev/null) && [ -n "$stty_saved" ]; then
    tty_out=1
fi
c_dim= c_b= c_ok= c_acc= c_bacc= c_warn= c_red= c_r0=
if [ -n "$tty_out" ]; then
    c_dim="${e}[2m" c_b="${e}[1m" c_ok="${e}[32m" c_acc="${e}[38;2;255;92;57m" c_bacc="${e}[1;38;2;255;92;57m"
    c_warn="${e}[33m" c_red="${e}[31m" c_r0="${e}[0m"
fi
rule='────────────────────────────────────────────────────────────────────────────'
# The Makepad mark, small (30 x 9), as the Builder drew it
logo='         ⣼⣆        ⣼⣆
        ⣼⣿⣿⣧      ⣼⣿⣿⣧
      ⢀⣾⣿⣿⣿⣿⣧⡀  ⢀⣾⣿⣿⣿⣿⣧
     ⢀⣾⣿⣿⣿⣿⣿⣿⣷⡀⢀⣾⣿⣿⣿⣿⣿⣿⣷⡀
    ⢀⣼⣿⣿⣿⣿⣿⣿⣿⣿⡇⢼⣿⣿⣿⣿⣿⣿⣿⣿⣇⡀
   ⢠⣿⣯⣻⣿⣿⣿⣿⣿⣿⡟ ⠈⢻⣿⣿⣿⣿⣿⣿⣟⣾⣷⡀
  ⢠⣿⣿⣿⣷⣻⣿⣿⣿⣿⡟    ⢻⣿⣿⣿⣿⢟⣾⣿⣿⣿⡄
 ⣰⣿⣿⣿⣿⣿⣷⡿⣿⣿⠏      ⠻⣿⣿⢯⣿⣿⣿⣿⣿⣿⡄
⣰⣿⣿⣿⣿⣿⣿⣿⣿⡽⠏        ⠹⣯⣿⣿⣿⣿⣿⣿⣿⣿⣆'
logo_out= logo_y=2
while IFS= read -r logo_line; do logo_out="$logo_out${e}[${logo_y};86H$logo_line"; logo_y=$((logo_y + 1)); done <<LOGO
$logo
LOGO
logo_dark="${e}[38;2;44;47;52m" logo_light="${e}[38;2;226;227;229m" logo_color=$logo_dark
row_1= row_2= row_3= row_4= info_1= info_2= info_3= info_4=
# plain output: a row's last state and when it is said again
last_1=next last_2=next last_3=next last_4=next next_1=9999999999 next_2=9999999999 next_3=9999999999 next_4=9999999999
name_1="Rust $rust_version" cur_row= foot= hint='ctrl+c stops · run makepad.sh again to continue'
note= note2= note3= where= screen_err= framed= bg_asked= clock= clock_stop= t0= spin_frame=0 winched=1 term_rows=24 term_cols=80

# out TEXT: TEXT to the terminal through printf(1), never the shell's own printf. A resize (SIGWINCH,
# trapped here) or a job ending (SIGCHLD) interrupts a write the terminal is slow to take, and bash
# 3.2 (macOS's /bin/sh) then keeps the unwritten rest of the frame in its buffer and writes it out
# with whatever it writes next, even into a value read back with $(...): the time read for the
# build's clock came back as the time and half a frame, and the arithmetic on it stopped the
# script. printf(1) has the signals' defaults (ignored, nothing interrupted) and keeps nothing.
out() { env printf '%s' "$1"; }
pad() { padded=$1; while [ "${#padded}" -lt "$2" ]; do padded="$padded "; done; }
short() { case "$1" in "$HOME"/*) printf '~/%s' "${1#"$HOME"/}" ;; *) printf '%s' "$1" ;; esac; }
# mmss SECONDS -> mmss: m:ss, or h:mm:ss from an hour on
mmss() {
    mm_s=$(($1 % 60)); [ "$mm_s" -ge 10 ] || mm_s=0$mm_s
    if [ "$1" -ge 3600 ]; then
        mm_m=$(($1 / 60 % 60)); [ "$mm_m" -ge 10 ] || mm_m=0$mm_m
        mmss=$(($1 / 3600)):$mm_m:$mm_s
    else
        mmss=$(($1 / 60)):$mm_s
    fi
}
clock() { [ -n "$clock_stop" ] || { mmss $(($(now) - ${t0:-$(now)})); clock=$mmss; }; }
# spin -> spin_c: the spinner's frame, one step a tick
spin() {
    case $((spin_frame % 10)) in 0) spin_c=⠋ ;; 1) spin_c=⠙ ;; 2) spin_c=⠹ ;; 3) spin_c=⠸ ;; 4) spin_c=⠼ ;; 5) spin_c=⠴ ;; 6) spin_c=⠦ ;; 7) spin_c=⠧ ;; 8) spin_c=⠇ ;; *) spin_c=⠏ ;; esac
}
# bar FILLED -> bar_s: FILLED of 12 in Makepad's coral, the rest dim
bar() {
    bar_on= bar_off= bar_i=0
    while [ "$bar_i" -lt 12 ]; do
        if [ "$bar_i" -lt "$1" ]; then bar_on="${bar_on}━"; else bar_off="${bar_off}─"; fi
        bar_i=$((bar_i + 1))
    done
    bar_s="${c_acc}${bar_on}${c_dim}${bar_off}${c_r0} "
}
# tick: a fifth of a second on a terminal (the spinner moves), a second otherwise
tick() {
    if [ -n "$tty_out" ]; then sleep 0.2 2>/dev/null || sleep 1; else sleep 1; fi
    spin_frame=$((spin_frame + 1))
}
# row N KIND STATE INFO [DONE OF]: step row N (1 Rust, 2 the loader's source, 3 its build, 4 the
# Runner) as KIND (wait, doing, spin, ok, fail), its STATE (one column), a bar of DONE out of OF,
# and INFO (colours included); `draw` shows it. Plain output gets it as a line when the state
# changes, and every ten seconds while it stays.
row() {
    case "$2" in
        wait) r_g="${c_dim}○" ;;
        doing) r_g="${c_acc}●" ;;
        spin) spin; r_g="${c_acc}${spin_c}" ;;
        ok) r_g="${c_ok}✓" ;;
        *) r_g="${c_red}✗" ;;
    esac
    pad "$3" 12
    r_s=$padded
    [ "$2" != wait ] || r_s="${c_dim}${padded}"
    r_bar=
    if [ -n "$tty_out" ] && [ $# -ge 6 ] && [ "$6" -gt 0 ]; then
        r_fill=$(($5 * 12 / $6))
        [ "$r_fill" -le 12 ] || r_fill=12
        bar "$r_fill"
        r_bar=$bar_s
    fi
    r_line="$r_g${c_r0} $r_s${c_r0}$r_bar$4"
    case "$1" in
        1) row_1=$r_line info_1=$4 r_last=$last_1 r_next=$next_1 r_name=$name_1 ;;
        2) row_2=$r_line info_2=$4 r_last=$last_2 r_next=$next_2 r_name='Loader source' ;;
        3) row_3=$r_line info_3=$4 r_last=$last_3 r_next=$next_3 r_name='Loader build' ;;
        *) row_4=$r_line info_4=$4 r_last=$last_4 r_next=$next_4 r_name=Runner ;;
    esac
    [ -z "$tty_out" ] || return 0
    r_now=$(now)
    if [ "$r_last" = "$3" ] && [ "$r_now" -lt "$r_next" ]; then return 0; fi
    r_next=$((r_now + 10))
    case "$1" in 1) last_1=$3 next_1=$r_next ;; 2) last_2=$3 next_2=$r_next ;; 3) last_3=$3 next_3=$r_next ;; *) last_4=$3 next_4=$r_next ;; esac
    if [ -n "$4" ]; then printf '  %s: %s · %s\n' "$r_name" "$3" "$4"; else printf '  %s: %s\n' "$r_name" "$3"; fi
}
# draw: the whole screen as one write (nothing flickers), each line cleared after its text, the
# cursor parked below it; after a resize the terminal is cleared first. Every line fits 80 columns.
draw() {
    [ -n "$framed" ] || return 0
    d_out=
    if [ "$winched" = 1 ]; then
        winched=0
        set -- $(stty size </dev/tty 2>/dev/null)
        term_rows=${1:-24} term_cols=${2:-80}
        d_out="${e}[H${e}[2J"
        # the mark's grey for this terminal, asked the first time it is wide enough to show it
        if [ "$term_cols" -ge 116 ] && [ -z "$bg_asked" ]; then bg_asked=1; light_background; fi
    fi
    clock
    d_el="elapsed $clock"
    pad "$name_1" 15
    d_out="$d_out${e}[2;3H${c_bacc}Makepad${c_r0}${c_b} Runner${c_r0}${e}[K${e}[2;$((80 - ${#d_el}))H${c_dim}${d_el}${c_r0}"
    d_out="$d_out${e}[3;3H${c_dim}${where}${c_r0}${e}[K${e}[4;3H${c_dim}${rule}${c_r0}${e}[K${e}[5;1H${e}[K"
    d_out="$d_out${e}[6;3H$padded$row_1${c_r0}${e}[K${e}[7;3HLoader source  $row_2${c_r0}${e}[K"
    d_out="$d_out${e}[8;3HLoader build   $row_3${c_r0}${e}[K${e}[9;3HRunner         $row_4${c_r0}${e}[K"
    d_out="$d_out${e}[10;1H${e}[K${e}[11;3H${c_dim}${rule}${c_r0}${e}[K${e}[12;3H${foot}${c_r0}${e}[K${e}[13;3H${c_dim}${hint}${c_r0}${e}[K"
    d_out="$d_out${e}[14;3H${c_warn}${note}${c_r0}${e}[K${e}[15;3H${c_dim}${note2}${c_r0}${e}[K${e}[16;3H${c_warn}${note3}${c_r0}${e}[K"
    if [ "$term_cols" -ge 116 ] && [ "$term_rows" -ge 18 ]; then d_out="$d_out$logo_color$logo_out$c_r0"; fi
    out "$d_out${e}[17;1H"
}
# The terminal's background (OSC 11, or COLORFGBG): a light one gets a light grey mark. Asked once,
# when the mark first shows; dark when nobody answers within 0.3 s.
light_background() {
    case "${COLORFGBG:-}" in *';7' | *';15') logo_color=$logo_light; return 0 ;; *';'*) return 0 ;; esac
    lb_reply=$(stty -echo -icanon min 0 time 3 </dev/tty 2>/dev/null && printf '%s]11;?\007' "$e" >/dev/tty && dd bs=64 count=1 </dev/tty 2>/dev/null)
    stty -echo icanon </dev/tty 2>/dev/null || :
    lb_rgb=$(printf '%s' "$lb_reply" | sed -n 's/.*rgb:\([0-9a-fA-F][0-9a-fA-F]\)[0-9a-fA-F]*\/\([0-9a-fA-F][0-9a-fA-F]\)[0-9a-fA-F]*\/\([0-9a-fA-F][0-9a-fA-F]\).*/\1 \2 \3/p')
    [ -n "$lb_rgb" ] || return 0
    set -- $lb_rgb
    # relative luminance on a 0-255 scale: light above the middle
    [ $(( (2126 * 0x$1 + 7152 * 0x$2 + 722 * 0x$3) / 10000 )) -le 127 ] || logo_color=$logo_light
}
# screen_start: the screen (keys typed meanwhile not echoed), or the folder on plain output
screen_start() {
    where="Folder: $(short "$root")"
    t0=$(now)
    foot='Looking at what is here already.'
    for r_n in 1 2 3 4; do row $r_n wait next ''; done
    if [ -z "$tty_out" ]; then
        printf '  %s\n' "$where"
        [ -z "$note" ] || printf '  %s\n' "$note"
        [ -z "$note2" ] || printf '  %s\n' "$note2"
        return 0
    fi
    stty -echo </dev/tty 2>/dev/null || :
    if mkdir -p "$logs_dir" 2>/dev/null && : 2>/dev/null > "$logs_dir/stderr.log"; then
        screen_err=$logs_dir/stderr.log
        exec 3>&2 2>>"$screen_err"
    fi
    out "${e}]0;Makepad${e}\\${e}[?25l${e}[?7l"
    framed=1
    trap 'winched=1' WINCH
    draw
}
# screen_end: the terminal as it was (keys typed meanwhile, or an answer that came late, read
# away), the screen left standing and the cursor below it, and below that what went to stderr while
# it was up (control characters taken out, so none of it moves the cursor or recolours the terminal)
screen_end() {
    [ -n "$framed" ] || return 0
    framed=
    trap - WINCH
    stty -icanon min 0 time 0 </dev/tty 2>/dev/null && dd bs=4096 count=1 </dev/tty >/dev/null 2>&1
    stty "$stty_saved" </dev/tty 2>/dev/null || :
    out "${e}[?7h${e}[?25h${e}[17;1H"
    if [ -n "$screen_err" ]; then
        exec 2>&3 3>&-
        [ ! -s "$screen_err" ] || tr -d '\000-\010\013-\037\177' < "$screen_err" | sed 's/^/  /' >&2
        screen_err=
    fi
}
# notice TEXT: a line in yellow below the screen's keys (plain output: a line of its own)
notice() {
    note3="${note3:+$note3 }$1"
    say "$1"
}
# ready MESSAGE: every step done, the clock stopped
ready() {
    clock
    clock_stop=1
    foot="${c_ok}✓${c_r0} Ready in $clock. $1"
    hint=
    draw
    [ -n "$tty_out" ] || printf '  Ready in %s. %s\n' "$clock" "$1"
}
# screen_fail MESSAGE [LOG]: the current step crossed out, the message's first line below the rule
# (and, with a LOG, that its end follows); all of it below the screen when longer than that line
screen_fail() {
    case "$cur_row" in
        1) row 1 fail stopped "$info_1" ;;
        2) row 2 fail stopped "$info_2" ;;
        3) row 3 fail failed "$info_3" ;;
        4) row 4 fail stopped "$info_4" ;;
    esac
    sf_first=${1%%"
"*}
    foot="${c_red}✗${c_r0} $sf_first"
    hint=
    [ -z "${2:-}" ] || hint='The end of its log is below.'
    draw
    screen_end
    if [ "$sf_first" != "$1" ] || [ "${#sf_first}" -gt 76 ]; then printf '  %s\n' "$1" >&2; fi
}
# screen_stopped: ctrl+c, which may have stopped a frame halfway: the screen drawn again in full
screen_stopped() {
    [ -n "$framed" ] || return 0
    winched=1
    foot="${c_warn}✗${c_r0} Stopped. Run makepad.sh again to continue: what is here stays."
    hint=
    draw
    screen_end
}
# on_exit STATUS (the EXIT trap): a screen still up after a nonzero exit means the shell stopped the
# script on an error of its own (an expansion, an unset variable): its jobs stopped, its step crossed
# out, the shell's message below the screen
on_exit() {
    if [ -n "$framed" ] && [ "$1" != 0 ]; then
        stamp shell-error
        stop_jobs
        clock_stop=1
        screen_fail "makepad.sh stopped on an error of its own; the shell's message is below."
    fi
    screen_end
}
trap 'stamp stopped; stop_jobs; screen_stopped; exit 130' INT TERM
trap 'on_exit $?' EXIT
# amount BYTES TOTAL SPEED -> amount: " 43%   98 of 218 MB    12.3 MB/s" (KB below 10 MB; no
# speed until there is one)
amount() {
    a_pct=0
    [ "$2" -le 0 ] || a_pct=$(($1 * 100 / $2))
    [ "$a_pct" -le 100 ] || a_pct=100
    if [ "$2" -ge 10485760 ]; then a_u=MB a_d=1048576 a_w=3; else a_u=KB a_d=1024 a_w=5; fi
    a_p=$a_pct; while [ "${#a_p}" -lt 3 ]; do a_p=" $a_p"; done
    a_h=$(($1 / a_d)); while [ "${#a_h}" -lt "$a_w" ]; do a_h=" $a_h"; done
    amount="$a_p%  $a_h of $(($2 / a_d)) $a_u"
    a_t10=$(($3 * 10 / 1048576))
    if [ "$a_t10" -gt 0 ]; then
        a_s="$((a_t10 / 10)).$((a_t10 % 10)) MB/s"
        while [ "${#a_s}" -lt 10 ]; do a_s=" $a_s"; done
        amount="$amount  $a_s"
    fi
}

# rust_check --probe MIN: read-only. Prints nothing; sets rust_sysroot, or
# rust_reason. Only the toolchain's own binaries run, so no rustup proxy can
# download or switch anything.
rust_check() {
    rust_reason= rust_sysroot=
    rc_toolchain=$(cd / && unset RUSTUP_TOOLCHAIN && RUSTUP_AUTO_INSTALL=0 rustc --print sysroot 2>/dev/null) || { rust_reason='No installed Rust was found on PATH.'; return 1; }
    [ -n "$rc_toolchain" ] || { rust_reason='No installed Rust was found on PATH.'; return 1; }
    rc_min=$2
    case "$rc_toolchain" in /*) ;; *) rust_reason='The recorded Rust location is not an absolute path.'; return 1 ;; esac
    [ -d "$rc_toolchain" ] || { rust_reason="Rust is no longer installed at $rc_toolchain."; return 1; }
    rc_toolchain=$(cd -P -- "$rc_toolchain" && pwd)
    [ -x "$rc_toolchain/bin/rustc" ] || { rust_reason="No rustc in $rc_toolchain/bin."; return 1; }
    rc_meta=$("$rc_toolchain/bin/rustc" --version --verbose 2>/dev/null) || { rust_reason="rustc in $rc_toolchain/bin does not run."; return 1; }
    rc_version=$(printf '%s\n' "$rc_meta" | awk '$1 == "release:" { print $2 }')
    rc_host=$(printf '%s\n' "$rc_meta" | awk '$1 == "host:" { print $2 }')
    [ "$rc_host" = "$triple" ] || { rust_reason="Installed Rust is built for ${rc_host:-an unknown host}; Makepad needs $triple."; return 1; }
    rc_status=0
    version_at_least "$rc_version" "$rc_min" || rc_status=$?
    case "$rc_status" in
        0) ;;
        1) rust_reason="Installed Rust $rc_version is not a stable release; Makepad needs $rc_min or newer."; return 1 ;;
        *) rust_reason="Installed Rust $rc_version is older than $rc_min."; return 1 ;;
    esac
    [ -x "$rc_toolchain/bin/cargo" ] || { rust_reason='Installed Rust has no cargo beside rustc.'; return 1; }
    rc_cargo=$("$rc_toolchain/bin/cargo" --version 2>/dev/null | awk '$1 == "cargo" { print $2 }') || :
    version_at_least "${rc_cargo:-none}" "$rc_min" || { rust_reason="Installed cargo ${rc_cargo:-is unusable and} is older than $rc_min."; return 1; }
    [ -d "$rc_toolchain/lib/rustlib/$triple/lib" ] || { rust_reason="Installed Rust has no standard library for $triple."; return 1; }
    if [ "$(uname -s)" = Darwin ]; then
        # Release builds strip through rustc's bundled rust-objcopy; report a
        # copy that cannot load its LLVM library, never repair it.
        "$rc_toolchain/lib/rustlib/$triple/bin/rust-objcopy" --version >/dev/null 2>&1 || { rust_reason="Installed Rust's rust-objcopy does not run; release builds could not be stripped."; return 1; }
    fi
    rust_sysroot=$rc_toolchain
}
# version_at_least ACTUAL MINIMUM: 0 when newer or equal, 1 when not a
# stable x.y.z, 2 when older. Components compare as numbers.
version_at_least() {
    awk -v actual="$1" -v minimum="$2" 'BEGIN {
        if (actual !~ /^[0-9]+[.][0-9]+[.][0-9]+$/) exit 1
        split(actual, a, "."); split(minimum, b, ".")
        for (i = 1; i <= 3; i++) { if (a[i] + 0 > b[i] + 0) exit 0; if (a[i] + 0 < b[i] + 0) exit 2 }
        exit 0
    }'
}

# --- Where, and what this computer has ------------------------------------------------------
case "$0" in
    sh | -sh | */sh | bash | -bash | */bash | dash | */dash | zsh | */zsh) script_dir= ;;
    *) script_dir=; [ -f "$0" ] && script_dir=$(cd -P -- "$(dirname -- "$0")" && pwd -P) ;;
esac
# A release service of its own (a test's) installs only into a folder named for it: a run that sets
# MAKEPAD_LOADER_SERVICE without MAKEPAD_LOADER_ROOT stops before it makes or changes anything.
if [ -n "${MAKEPAD_LOADER_SERVICE:-}" ] && [ -z "${MAKEPAD_LOADER_ROOT:-}" ]; then
    fail "MAKEPAD_LOADER_SERVICE is set without MAKEPAD_LOADER_ROOT: give the run a folder of its own."
fi
# The folder: this file's own when it is an installation already, or holds nothing else (a folder
# made for it); else ~/makepad-runner (piped into sh, or a download among other files), where the
# file copies itself.
if [ -n "$script_dir" ] && [ -z "${MAKEPAD_LOADER_ROOT:-}" ] && [ ! -d "$script_dir/cache" ] \
    && [ -n "$(ls -A "$script_dir" | grep -v -x -e "$(basename -- "$0")" -e .DS_Store)" ]; then
    script_dir_other=1
fi
if [ -n "$script_dir" ] && [ -z "${script_dir_other:-}" ]; then default_root=$script_dir; else default_root=$HOME/makepad-runner; fi
root=${MAKEPAD_LOADER_ROOT:-$default_root}
mkdir -p "$root" 2>/dev/null && root=$(cd -P -- "$root" && pwd -P) || fail "Cannot create the folder $root"
rust_own=$root/rust
rust=$rust_own
cache=$root/cache
loader_root=$cache/loader
downloads=$cache/downloads
logs_dir=$cache/logs
jobs=$cache/jobs
forget_old_jobs
if [ -n "${script_dir_other:-}" ]; then
    note='This folder holds other files.'
    note2='Makepad goes into the folder above: start makepad.sh there next time.'
fi
svchost=${service#*://}
svchost=${svchost%%/*}
screen_start

case "$(uname -s)/$(uname -m)" in
    Darwin/arm64 | Darwin/aarch64) triple=aarch64-apple-darwin ;;
    Linux/x86_64) triple=x86_64-unknown-linux-gnu ;;
    # the loader's JIT builds for the CPU it runs on, and has no Intel Mac or ARM64 Linux back end glue
    *) fail "Makepad supports macOS on Apple silicon, and glibc Linux on x86_64." ;;
esac
# "<part> <bytes> <sha256>" for rustc, rust-std and cargo of this platform
case "$triple" in
    aarch64-apple-darwin) parts='rustc 83206125 7b39cbbb11995584741b5821511b8706ab8ecab3bc7e12b11fb1ab50cab9d079
rust-std 46977544 49dc82ff5b6fb033baa6e85635ed9cf07a27e5297a22314808ab4cf65a81335a
cargo 12993393 ae556d0783f4ab2a6fccd4674158df08cf36526b73de03b7fde0e10b2bfc3d2e' ;;
    x86_64-unknown-linux-gnu) parts='rustc 134061173 18ed6559de1b8ea6b77474ea86992b9a507d3a3d134d9ee017d30cf3f406e3ee
rust-std 48777886 8aa6405356392ce50160d1b286e86091c5e14adae3061115699c84ed4394d546
cargo 15858529 18bf1598891b30dd5eb52a337d08a92b4456255ddbe4c1ab996ffb578077031c' ;;
esac
part_names='rustc rust-std cargo'
part_field() { printf '%s\n' "$parts" | awk -v p="$1" -v f="$2" '$1 == p { print $f }'; }

# https only, except a release service on this machine (tests, a local service)
case "$service" in
    https://*) proto='=https' ;;
    http://127.0.0.1:* | http://localhost:*) proto='=http,https' ;;
    # MAKEPAD_ALLOW_HTTP=1 (tests only): a plain-http release service on another machine
    http://*) [ "${MAKEPAD_ALLOW_HTTP:-}" = 1 ] && proto='=http,https' || fail "The release service must be an https URL - $service" ;;
    *) fail "The release service must be an https URL - $service" ;;
esac
command -v curl >/dev/null 2>&1 || fail "Makepad needs curl - install it, then run this again"
command -v tar >/dev/null 2>&1 || fail "Makepad needs tar - install it, then run this again"

# The system's linker and Linux build/runtime libraries: named, never installed here.
if [ "$(uname -s)" = Darwin ]; then
    if ! xcode-select -p >/dev/null 2>&1 || ! /usr/bin/xcrun --find ld >/dev/null 2>&1; then
        fail "Makepad needs Apple's command line tools - install them, then run this again"
    fi
else
    missing=
    command -v cc >/dev/null 2>&1 || missing="a C linker (cc)"
    for lib in libxkbcommon.so.0 libasound.so.2 libpulse.so.0 libX11.so.6 libXcursor.so.1 libdrm.so.2 libwayland-client.so.0 libssl.so.3 libcrypto.so.3 libssl.so; do
        found=
        for path in /usr/lib/"$lib" /usr/lib64/"$lib" /usr/lib/*-linux-gnu/"$lib"; do
            if [ -e "$path" ]; then found=1; break; fi
        done
        [ -n "$found" ] || missing="${missing:+$missing, }$lib"
    done
    if [ -n "$missing" ]; then
        fail "Makepad needs $missing. Install the build and runtime packages, then run this again:
    apt: sudo apt-get install build-essential libssl-dev libxkbcommon0 libasound2-dev libpulse0 libx11-6 libxcursor1 libdrm2 libwayland-client0
    dnf: sudo dnf install gcc openssl-devel libxkbcommon alsa-lib pulseaudio-libs libX11 libXcursor libdrm wayland-libs
    pacman: sudo pacman -S --needed base-devel openssl libxkbcommon alsa-lib libpulse libx11 libxcursor libdrm wayland
    zypper: sudo zypper install gcc libopenssl-devel libxkbcommon0 libasound2 libpulse0 libX11-6 libXcursor1 libdrm2 libwayland-client0"
    fi
fi
mkdir -p "$downloads" "$logs_dir" "$jobs" "$loader_root" || fail "Cannot create $cache"
logs=$logs_dir
stamp start
export RELAX_HOME="$root"

# Restore an unconfirmed switch before any decision about the loader already here.
if [ -e "$loader_root/pending" ]; then
    previous=$(cat "$loader_root/previous" 2>/dev/null)
    loader_folder_ready "$previous" || previous=src
    if [ "$previous" = src ]; then
        rm -f "$loader_root/current" || fail "Cannot remove $loader_root/current"
    else
        atomic_line "$loader_root/current" "$previous" || fail "Cannot write $loader_root/current"
    fi
    atomic_line "$loader_root/rolled-back" 'the new runner did not confirm its first start' \
        || fail "Cannot write $loader_root/rolled-back"
    rm -f "$loader_root/pending" || fail "Cannot remove $loader_root/pending"
    notice "Went back to $(printf '%s' "$previous" | cut -c1-10): the new runner did not confirm its first start."
fi
active=$(cat "$loader_root/current" 2>/dev/null)
loader_folder_ready "$active" || active=src
src=$loader_root/$active
makepad=$src/target/host/makepad

# --- The runner release, from the start ---------------------------------------------------------
# The release service's latest licence-free `runner` release names the runner's git repositories
# on the service's read-only git face and the loader's source cut from the same Makepad commit.
# With no arguments the folder moves to it: each commit's pack goes into
# $RELAX_HOME/cache/incoming/<commit>.pack (renamed into place when complete), where makepad run
# takes it instead of downloading, and the loader is compiled for it. With arguments a loader
# compiled before stays as it is (the runner and the apps it recorded were made for it).
runner_release=
repos=
release_json=
# With a loader and a runner here already, the release service gets a few seconds: offline or
# unreachable, the folder's stores and loader start at once (makepad run runs the stored commits).
meta="--retry 2 -m 60"
if [ -f "$src/.built" ] && [ -f "$cache/runner/release" ]; then meta="--connect-timeout 3 -m 6"; fi
mkdir -p "$cache/runner" "$cache/incoming"
if [ $# -eq 0 ] || [ ! -f "$src/.built" ]; then
    cur_row=2
    row 2 doing asking "${c_dim}$svchost for the latest Runner release${c_r0}"
    foot='Asking the release service for the latest Runner.'
    draw
    release_json=$(curl -fsS --proto "$proto" $meta "$service/public/app/runner/latest" 2>>"$logs/download.log") || release_json=
fi
runner_release=$(printf '%s' "$release_json" | sed -n 's/.*"release":"\([A-Za-z0-9_-]*\)".*/\1/p')
prefetch_count=0
if [ $# -eq 0 ] && [ -n "$runner_release" ]; then
    # one "<name> <path> <commit>" line per repository of the release
    repos=$(printf '%s\n' "$release_json" | sed 's/"loader":{[^}]*}//' | tr '{}' '\n\n' | while IFS= read -r line; do
        name=$(printf '%s' "$line" | sed -n 's/.*"name":"\([A-Za-z0-9_-]*\)".*/\1/p')
        path=$(printf '%s' "$line" | sed -n 's/.*"path":"\([A-Za-z0-9_/-]*\)".*/\1/p')
        commit=$(printf '%s' "$line" | sed -n 's/.*"commit":"\([0-9a-f]\{40\}\)".*/\1/p')
        if [ -n "$name" ] && [ -n "$path" ]; then printf '%s %s %s\n' "$name" "$path" "$commit"; fi
    done)
    if [ "$(sed -n 1p "$cache/runner/release" 2>/dev/null)" != "$runner_release" ]; then
        for entry in $(printf '%s\n' "$repos" | tr ' ' ':'); do
            name=${entry%%:*}
            commit=${entry##*:}
            [ ${#commit} -eq 40 ] || continue
            pack=$cache/incoming/$commit.pack
            [ -f "$pack" ] && continue
            ( printf '0032want %s\n00000009done\n' "$commit" \
                | curl -fsS --proto "$proto" --retry 2 -m 1800 -H 'Content-Type: application/x-git-upload-pack-request' \
                    --data-binary @- -o "$pack.part" "$service/public/git/runner/$name.git/git-upload-pack" \
                && mv -f "$pack.part" "$pack" ) >/dev/null 2>>"$logs/download.log" &
            prefetch_pids="$prefetch_pids $!"
            prefetch_count=$((prefetch_count + 1))
        done
    fi
fi

# --- What there is to do --------------------------------------------------------------------
need_rust=
rust_release=$rust_version
if [ "$(cat "$rust_own/.toolchain-version" 2>/dev/null)" = "$rust_version $triple" ] \
    && [ -x "$rust_own/bin/cargo" ] && [ -x "$rust_own/bin/rustc" ]; then
    rust_llvm_link "$rust_own" || fail "Cannot link LLVM for rust-objcopy in $rust_own"
    row 1 ok ready "${c_dim}in rust/${c_r0}"
elif rust_check --probe "$rust_version"; then
    rust=$rust_sysroot
    rust_release=$rc_version
    name_1="Rust $rust_release"
    row 1 ok ready "${c_dim}installed, $(short "$rust")${c_r0}"
else
    need_rust=1
    [ "$rust_reason" = 'No installed Rust was found on PATH.' ] \
        || notice "$rust_reason Rust $rust_version goes into this folder."
    rust_mb=0
    for part in $part_names; do rust_mb=$((rust_mb + $(part_field $part 2) / 1048576)); done
    row 1 wait 'to download' "${c_dim}$rust_mb MB from static.rust-lang.org${c_r0}"
fi
# The loader ZIP's SHA-256 names the source version, independent of the runner's commit.
# Offline, or with arguments, use the version recorded with the source already here.
archive=$(printf '%s' "$release_json" | sed -n 's/.*"loader":{"name":"\([A-Za-z0-9_.-]*\)","bytes":\([0-9]*\),"sha256":"\([0-9a-f]\{64\}\)"[^}]*}.*/\1 \2 \3/p')
loader_version=$(printf '%s' "$archive" | cut -s -d' ' -f3)
# Update the active source folder in place, replacing only changed files so cargo recompiles
# only their crates (target/ stays inside it: makepad finds its std two levels above itself).
loader_built_sha256=$(cat "$src/.loader-sha256" 2>/dev/null)
[ -n "$loader_version" ] || loader_version=$loader_built_sha256
[ -n "$loader_version" ] || loader_version=$(cat "$src/.version" 2>/dev/null)
cur_row=2
[ -n "$loader_version" ] || fail "No answer from $svchost, and no loader is here yet. Check the connection, then run this again."
loader_short=$(printf '%s' "$loader_version" | cut -c1-10)
built="$loader_version $rust_release $triple"
need_build=
[ -x "$makepad" ] && [ -x "$src/target/host/makepad-launcher" ] && [ "$loader_built_sha256" = "$loader_version" ] \
    && [ "$(awk 'NR == 1 { print $2, $3 }' "$src/.built" 2>/dev/null)" = "$rust_release $triple" ] || need_build=1
need_zip=
[ "$(cat "$src/.version" 2>/dev/null)" != "$loader_version" ] \
    && [ "$loader_built_sha256" != "$loader_version" ] && need_zip=1
if [ -n "$need_zip" ]; then
    [ -n "$archive" ] || fail "The release service did not answer; the loader's source is not here yet. Check the connection, then run this again."
    loader_name=$(printf '%s' "$archive" | cut -d' ' -f1)
    loader_bytes=$(printf '%s' "$archive" | cut -d' ' -f2)
    loader_sha256=$(printf '%s' "$archive" | cut -d' ' -f3)
    zip=$downloads/$loader_name
    row 2 wait 'to download' "${c_dim}$loader_name, $((loader_bytes / 1024)) KB${c_r0}"
else
    row 2 ok 'up to date' "${c_dim}$loader_short${c_r0}"
fi
draw

# --- 1 + 2. Rust's pieces and the loader's source, all at once ------------------------------
cfg=$downloads/fetch.curl
rm -f "$cfg"
rust_total=0
rust_base=0
rust_pieces=0
# pieces PART SIZE adds the pieces of PART not yet here to the curl config
pieces() {
    idx=0 from=0
    while [ $from -lt "$2" ]; do
        to=$((from + piece - 1))
        [ $to -lt "$2" ] || to=$(($2 - 1))
        file=$downloads/$1.tar.gz.$idx
        have=0
        [ -f "$file" ] && have=$(size_of "$file")
        if [ "$have" != $((to - from + 1)) ]; then
            [ ! -f "$cfg" ] || echo next >> "$cfg"
            printf 'url = "%s"\noutput = "%s"\nrange = %s-%s\nproto = =https\nfail\nlocation\nretry = 3\n' \
                "$dist/$1-$rust_version-$triple.tar.gz" "$(cfgq "$file")" $from $to >> "$cfg"
            rust_pieces=$((rust_pieces + 1))
        fi
        idx=$((idx + 1))
        from=$((to + 1))
    done
}
if [ -n "$need_rust" ]; then
    for part in $part_names; do
        size=$(part_field $part 2)
        rust_total=$((rust_total + size))
        if [ -f "$downloads/$part.tar.gz" ] && [ "$(size_of "$downloads/$part.tar.gz")" = "$size" ]; then
            rust_base=$((rust_base + size))
        else
            rm -f "$downloads/$part.tar.gz"
            pieces $part "$size"
        fi
    done
fi
if [ -n "$need_zip" ]; then
    rm -f "$zip.part"
    [ ! -f "$cfg" ] || echo next >> "$cfg"
    printf 'url = "%s"\noutput = "%s"\nproto = %s\nfail\nretry = 3\n' \
        "$service/public/app/runner/$runner_release/$loader_name" "$(cfgq "$zip.part")" "$proto" >> "$cfg"
fi
# the summed size of the Rust pieces here
pieces_sum() { ls -ln "$downloads" | awk '$9 ~ /\.tar\.gz\.[0-9]+$/ { s += $5 } END { print s + 0 }'; }
# fetched: the bytes here of each download (r_b Rust, z_b the loader's source)
fetched() {
    r_b=0 z_b=0
    [ -z "$need_rust" ] || r_b=$((rust_base + $(pieces_sum)))
    [ -z "$need_zip" ] || [ ! -f "$zip.part" ] || z_b=$(size_of "$zip.part")
}
fetch_t0=$(now)
if [ -f "$cfg" ]; then
    if [ -n "$need_rust" ] && [ -n "$need_zip" ]; then fetch_what="Downloading Rust in $rust_pieces pieces, and the loader's source"
    elif [ -n "$need_rust" ]; then fetch_what="Downloading Rust from static.rust-lang.org in $rust_pieces pieces"
    else fetch_what="Downloading the loader's source from $svchost"
    fi
    cur_row=2
    [ -z "$need_rust" ] || cur_row=1
    job fetch curl -sS -Z --parallel-immediate --parallel-max 16 -K "$cfg" 2>"$logs/download.log"
    # each download's speed: bytes a second over the last few seconds
    fetched
    f_t=$fetch_t0 f_n=0 r_prev=$r_b z_prev=$z_b r_speed=0 z_speed=0
    while :; do
        f_done=
        [ ! -f "$jobs/fetch.done" ] || f_done=1
        fetched
        f_now=$(now)
        if [ "$f_now" -gt "$f_t" ]; then
            f_dt=$((f_now - f_t))
            r_i=$(((r_b - r_prev) / f_dt)); [ "$r_i" -ge 0 ] || r_i=0
            z_i=$(((z_b - z_prev) / f_dt)); [ "$z_i" -ge 0 ] || z_i=0
            if [ "$f_n" -eq 0 ]; then r_speed=$r_i z_speed=$z_i; else r_speed=$(((r_speed * 3 + r_i) / 4)) z_speed=$(((z_speed * 3 + z_i) / 4)); fi
            f_n=$((f_n + 1)) f_t=$f_now r_prev=$r_b z_prev=$z_b
        fi
        f_left=0
        if [ -n "$need_rust" ]; then
            amount "$r_b" "$rust_total" "$r_speed"
            row 1 spin downloading "$amount" "$r_b" "$rust_total"
            [ "$r_speed" -le 0 ] || [ $(((rust_total - r_b) / r_speed)) -le "$f_left" ] || f_left=$(((rust_total - r_b) / r_speed))
        fi
        if [ -n "$need_zip" ]; then
            amount "$z_b" "$loader_bytes" "$z_speed"
            if [ "$z_b" -ge "$loader_bytes" ]; then row 2 doing downloaded "$amount" "$z_b" "$loader_bytes"; else row 2 spin downloading "$amount" "$z_b" "$loader_bytes"; fi
            [ "$z_speed" -le 0 ] || [ $(((loader_bytes - z_b) / z_speed)) -le "$f_left" ] || f_left=$(((loader_bytes - z_b) / z_speed))
        fi
        foot="$fetch_what."
        if [ "$f_n" -ge 3 ] && [ "$f_left" -gt 0 ]; then mmss "$f_left"; foot="$fetch_what · about $mmss left."; fi
        draw
        [ -z "$f_done" ] || break
        tick
    done
    [ "$(cat "$jobs/fetch.done")" = 0 ] || fail "The download stopped. Run makepad.sh again to continue it." "$logs/download.log"
    rm -f "$cfg"
    stamp downloaded
fi

# --- Rust: join the pieces, check, unpack the three parts side by side ----------------------
if [ -n "$need_rust" ]; then
    cur_row=1
    row 1 doing joining "${c_dim}the pieces of each archive${c_r0}"
    foot='Joining the pieces.'
    draw
    for part in $part_names; do
        archive_file=$downloads/$part.tar.gz
        [ -f "$archive_file" ] && continue
        idx=0
        : > "$archive_file.joined" || fail "Could not join the pieces of $part."
        while [ -f "$archive_file.$idx" ]; do
            cat "$archive_file.$idx" >> "$archive_file.joined" || fail "Could not join the pieces of $part."
            idx=$((idx + 1))
        done
        idx=0
        while [ -f "$archive_file.$idx" ]; do rm -f "$archive_file.$idx"; idx=$((idx + 1)); done
        mv -f "$archive_file.joined" "$archive_file"
    done
    foot='Checking each archive against its published SHA-256.'
    checked=0
    for part in $part_names; do
        row 1 doing checking "$checked of 3 archives ${c_dim}· SHA-256${c_r0}"
        draw
        if [ "$(sha256 "$downloads/$part.tar.gz")" != "$(part_field $part 3)" ]; then
            rm -f "$downloads/$part.tar.gz"
            fail "Checksum mismatch for $part.tar.gz - downloaded again on the next run"
        fi
        checked=$((checked + 1))
    done
    stage=$cache/rust.unpack
    rm -rf "$stage"
    mkdir -p "$stage" || fail "Cannot create $stage"
    foot='Unpacking Rust into rust/, the three archives side by side.'
    for part in $part_names; do
        top=$part-$rust_version-$triple
        inner=$part
        [ $part != rust-std ] || inner=rust-std-$triple
        mkdir "$stage/$part" || fail "Cannot create $stage/$part"
        job unpack-$part tar -xzf "$downloads/$part.tar.gz" -C "$stage/$part" --strip-components=2 "$top/$inner" 2>"$logs/unpack-$part.log"
    done
    while :; do
        unpacked=0
        for part in $part_names; do [ ! -f "$jobs/unpack-$part.done" ] || unpacked=$((unpacked + 1)); done
        row 1 spin unpacking "$unpacked of 3 archives" "$unpacked" 3
        draw
        [ $unpacked -lt 3 ] || break
        tick
    done
    ready=$stage/ready
    mkdir "$ready" || fail "Cannot create $ready"
    for part in $part_names; do
        [ "$(cat "$jobs/unpack-$part.done")" = 0 ] || fail "Unpacking $part failed." "$logs/unpack-$part.log"
        rm -f "$stage/$part/manifest.in"
        # the parts share bin/, lib/ and share/: move each file into place
        ( cd "$stage/$part" && find . -type d -print | while IFS= read -r dir; do
            mkdir -p "$ready/$dir" || exit 1
        done && find . ! -type d -print | while IFS= read -r file; do
            mv -f "$file" "$ready/$file" || exit 1
        done ) \
            2>>"$logs/unpack-$part.log" || fail "Unpacking $part failed: it could not be moved into place." "$logs/unpack-$part.log"
    done
    rust_llvm_link "$ready" || fail "Cannot link LLVM for rust-objcopy in $ready"
    printf '%s %s' "$rust_version" "$triple" > "$ready/.toolchain-version"
    rm -rf "$rust_own"
    mv "$ready" "$rust_own" || fail "Cannot move Rust into $rust_own"
    rm -rf "$stage"
    for part in $part_names; do rm -f "$downloads/$part.tar.gz"; done
    mmss $(($(now) - fetch_t0))
    row 1 ok ready "${c_dim}downloaded and unpacked in $mmss${c_r0}"
    draw
    stamp rust
fi

# --- The loader's source: check and unpack --------------------------------------------------
if [ -n "$need_zip" ]; then
    cur_row=2
    row 2 doing checking "${c_dim}$loader_name · SHA-256${c_r0}"
    foot="Checking the loader's source against its SHA-256."
    draw
    if [ "$(size_of "$zip.part" 2>/dev/null)" != "$loader_bytes" ]; then
        rm -f "$zip.part"
        fail "The loader's source has the wrong size - downloaded again on the next run"
    fi
    if [ "$(sha256 "$zip.part")" != "$loader_sha256" ]; then
        rm -f "$zip.part"
        fail "Checksum mismatch for $loader_name - downloaded again on the next run"
    fi
    unpack=$cache/loader/.unpack
    rm -rf "$unpack"
    mkdir -p "$unpack" || fail "Cannot create $unpack"
    row 2 doing unpacking "${c_dim}$loader_name${c_r0}"
    foot="Unpacking the loader's source."
    draw
    if command -v unzip >/dev/null 2>&1; then unzip -q "$zip.part" -d "$unpack" >>"$logs/download.log" 2>&1
    elif command -v bsdtar >/dev/null 2>&1; then bsdtar -xf "$zip.part" -C "$unpack" >>"$logs/download.log" 2>&1
    elif command -v python3 >/dev/null 2>&1; then python3 -m zipfile -e "$zip.part" "$unpack" >>"$logs/download.log" 2>&1
    else fail "Makepad needs unzip - install it, then run this again"
    fi || fail "Unpacking the loader's source failed. Run this again." "$logs/download.log"
    new=$unpack/$runner_release
    [ -f "$new/Cargo.toml" ] || fail "The loader's source has no $runner_release/Cargo.toml. Run this again later; the release service is being updated."
    if [ ! -f "$src/Cargo.toml" ]; then
        rm -rf "$src"
        mv "$new" "$src" || fail "Cannot move the loader's source into $src"
    else
        # an update: files that changed are replaced (with the time of now, so cargo sees them
        # as changed), files the new version no longer has are removed, the rest stay as they are
        row 2 doing updating "${c_dim}to $loader_short, only the files that changed${c_r0}"
        foot="Updating the loader's source: only the files that changed are replaced."
        draw
        # one `diff -rq` names what differs (one process, not one per file)
        LC_ALL=C diff -rq -x target -x .version -x .built -x .loader-sha256 "$new" "$src" > "$unpack/changes" 2>&1
        [ $? -le 1 ] || fail "Cannot compare the loader's source with $src - $(head -1 "$unpack/changes")"
        while IFS= read -r line; do
            case "$line" in
                "Files $new/"*" differ")
                    rel=${line#"Files $new/"}
                    rel=${rel%%" and $src/"*}
                    mv -f "$new/$rel" "$src/$rel" && touch "$src/$rel" || fail "Cannot replace $src/$rel" ;;
                "File $new/"*" while file $src/"*)
                    # a file on one side and a folder on the other: the new one takes its place
                    rel=${line#"File $new/"}
                    rel=${rel%%" is a "*}
                    rm -rf "$src/$rel" && mv -f "$new/$rel" "$src/$rel" \
                        && find "$src/$rel" -exec touch {} + || fail "Cannot replace $src/$rel" ;;
                "Only in $new"*)
                    dir=${line#"Only in $new"}
                    dir=${dir%%": "*}
                    rel=${dir#/}${dir:+/}${line#*": "}
                    rel=${rel#/}
                    mkdir -p "$(dirname "$src/$rel")" && mv -f "$new/$rel" "$src/$rel" \
                        && find "$src/$rel" -exec touch {} + || fail "Cannot add $src/$rel" ;;
                "Only in $src"*)
                    dir=${line#"Only in $src"}
                    dir=${dir%%": "*}
                    rel=${dir#/}${dir:+/}${line#*": "}
                    rm -rf "$src/${rel#/}" || fail "Cannot remove $src/${rel#/}" ;;
                # Something was not updated, so this is not the new version.
                *) fail "Cannot update the loader's source in $src - diff said: $line" ;;
            esac
        done < "$unpack/changes" || fail "Cannot update the loader's source in $src"
    fi
    atomic_line "$src/.version" "$loader_version" || fail "Cannot write $src/.version"
    rm -rf "$unpack"
    rm -f "$zip.part"
    row 2 ok ready "${c_dim}$loader_short${c_r0}"
    draw
    stamp source
fi

# --- 3. Compile the loader --------------------------------------------------------------------
# The compile sees no Rust settings from elsewhere on this computer and writes only into
# cache/. Its flags are the runner's own (apps/commercial/runner build_work.rs RUSTFLAGS): this
# computer's CPU and rustc's parallel frontend, which stable rustc takes under RUSTC_BOOTSTRAP=1,
# with an empty allow list so that no #![feature] builds. Cargo's plain output stays in the log; the
# build's row counts its Compiling lines against the crates cargo tree lists.
unset RUSTUP_TOOLCHAIN RUSTFLAGS CARGO_ENCODED_RUSTFLAGS CARGO_BUILD_TARGET CARGO_BUILD_RUSTFLAGS CARGO_TARGET_DIR RUSTC_WRAPPER RUSTDOC
export PATH="$rust/bin:$PATH" CARGO_HOME="$cache/cargo-home" RUSTC="$rust/bin/rustc" CARGO_INCREMENTAL=0
build_loader() {
    cd "$src" && CARGO_TERM_COLOR=never CARGO_TERM_PROGRESS_WHEN=never RUSTFLAGS="-C target-cpu=native -Z threads=8 -Z allow-features=" RUSTC_BOOTSTRAP=1 \
        exec "$rust/bin/cargo" build --profile host --offline --locked -p makepad-run --bin makepad --bin makepad-launcher
}
# build_progress: the build's row (its bar, N / M crates and the crate compiling now, cut to 16)
# and, below the rule, the time so far and, from a tenth of the crates on, about how long is left.
# Plain output also gets the time so far and how long the crate has taken.
build_progress() {
    build_state=$(awk '
        $1 == "Compiling" && $3 ~ /^v[0-9]/ { compiled++; current = $2 }
        END { printf "%d %s", compiled, current }
    ' "$build_log")
    b_now=$(now)
    if [ "$build_state" != "$build_previous" ]; then
        crate_t0=$b_now
        build_previous=$build_state
    fi
    compiled=${build_state%% *}
    current=${build_state#* }
    [ "$compiled" -le "$crates" ] || compiled=$crates
    [ "${#current}" -le 17 ] || current="$(printf '%s' "$current" | cut -c1-16)…"
    b_n=$compiled
    while [ "${#b_n}" -lt "${#crates}" ]; do b_n=" $b_n"; done
    mmss $((b_now - build_t0))
    b_sofar=$mmss
    foot="Compiling Makepad's loader for this processor: $b_sofar so far"
    if [ "$compiled" -gt 0 ] && [ $((compiled * 10)) -ge "$crates" ]; then
        mmss $(((b_now - build_t0) * (crates - compiled) / compiled))
        foot="$foot, about $mmss left"
    fi
    foot="$foot."
    b_info="$b_n / $crates crates  ${c_dim}$current${c_r0}"
    [ -n "$tty_out" ] || b_info="$b_info · $b_sofar so far · $((b_now - crate_t0)) s on crate"
    row 3 spin compiling "$b_info" "$compiled" "$crates"
}
if [ -n "$need_build" ]; then
    cur_row=3
    build_log=$logs/build.log
    rm -f "$build_log"
    row 3 doing counting "${c_dim}the crates to compile${c_r0}"
    foot="Listing the loader's crates."
    draw
    # Resolve the archive's path crates offline before compiling. Cargo writes the lock for
    # --locked below; the archive need not ship one. Count only this host's normal/build
    # dependencies, not other targets or the workspace's dev-dependency placeholders.
    if ! (cd "$src" && CARGO_TERM_COLOR=never "$rust/bin/cargo" tree --offline --target "$triple" \
        -p makepad-run --edges normal,build --prefix none --format '{p}') >"$logs/crates.log" 2>"$build_log"; then
        fail "Cannot list the loader's crates from its source." "$build_log"
    fi
    crates=$(awk '!seen[$1 FS $2]++ { count++ } END { print count + 0 }' "$logs/crates.log")
    [ "$crates" -gt 0 ] || fail "The loader's source lists no crates." "$build_log"
    # room for the build (about 2 GB of target/), said before it starts rather than as a cargo error
    free_kb=$(df -Pk "$cache" | awk 'NR == 2 { print $4 }')
    [ "${free_kb:-0}" -ge 3000000 ] || fail "Compiling Makepad's loader needs about 3 GB free on this disk. Free some space, then run this again."
    build_t0=$(now)
    crate_t0=$build_t0
    build_previous=
    # for this computer's processor ([profile.host]: opt-level 3, 16 codegen units, LTO within each crate; target-cpu=native)
    job build build_loader >"$build_log" 2>&1
    while :; do
        b_done=
        [ ! -f "$jobs/build.done" ] || b_done=1
        build_progress
        draw
        [ -z "$b_done" ] || break
        tick
    done
    if [ "$(cat "$jobs/build.done")" != 0 ]; then
        row 3 fail failed "${c_dim}$compiled of $crates crates${c_r0}"
        fail "The loader did not compile." "$build_log"
    fi
    atomic_line "$src/.built" "$built" || fail "Cannot write $src/.built"
    atomic_line "$src/.loader-sha256" "$loader_version" || fail "Cannot write $src/.loader-sha256"
    mmss $(($(now) - build_t0))
    if [ "$compiled" -ge "$crates" ]; then
        row 3 ok compiled "${c_dim}in $mmss · $crates crates${c_r0}"
    else
        row 3 ok compiled "${c_dim}in $mmss · $compiled of $crates crates, the rest kept${c_r0}"
    fi
    draw
    stamp built
else
    row 3 ok 'up to date' "${c_dim}for this processor${c_r0}"
    draw
fi

# This script stays in the folder for the next start.
if [ -n "$script_dir" ] && [ "$script_dir" != "$root" ]; then
    cp "$0" "$root/makepad.sh.next" 2>/dev/null && chmod 755 "$root/makepad.sh.next" && mv -f "$root/makepad.sh.next" "$root/makepad.sh" || :
fi

# --- 4. Start it --------------------------------------------------------------------------------
# makepad run runs cargo/rustc for build scripts and unit graphs (until it does that itself), so the
# toolchain stays on PATH for it and every app it starts. With no arguments `makepad` runs the
# runner: its launch record names the runner's repositories, the release service's latest
# licence-free `runner` release, which the service serves read-only over git
# ($service/public/git/runner/<repository>.git, one branch per release). The repository at
# `makepad` is the workspace, the others mount at their folders inside it. makepad run keeps its
# stores in $RELAX_HOME/cache and, with --cached, starts the last one at once while it
# fetches the new release for the next start. Offline, the release recorded last.
export MAKEPAD_LOADER_ROOT="$root" MAKEPAD_LOADER_SERVICE="$service"
if [ $# -gt 0 ]; then
    row 4 wait 'not started' "${c_dim}the loader runs with your arguments${c_r0}"
    ready 'Running the loader with your arguments.'
    screen_end
    stamp start-loader
    exec "$makepad" "$@"
fi
cur_row=4
# the runner's packs downloaded alongside the build are complete before makepad run looks for them
if [ -n "$prefetch_pids" ]; then
    foot="Downloading the Runner's code from $svchost."
    while :; do
        p_left=0
        # a pack download that ended is gone, or a zombie until it is waited for
        for pid in $prefetch_pids; do
            case "$(ps -p "$pid" -o stat= 2>/dev/null)" in '' | *Z*) ;; *) p_left=$((p_left + 1)) ;; esac
        done
        row 4 spin fetching "$((prefetch_count - p_left)) of $prefetch_count repositories ${c_dim}· the Runner's code${c_r0}" $((prefetch_count - p_left)) "$prefetch_count"
        draw
        [ "$p_left" -gt 0 ] || break
        tick
    done
    wait $prefetch_pids || :
fi
if [ -n "$runner_release" ]; then
    [ -n "$repos" ] || fail "The runner release names no repositories."
    { printf '%s\n' "$runner_release"; printf '%s\n' "$repos"; } > "$cache/runner/release.next" && mv -f "$cache/runner/release.next" "$cache/runner/release"
else
    [ -f "$cache/runner/release" ] || fail "No answer from $svchost, and the Runner was never set up here."
    notice "No answer from $svchost: the Runner fetched before starts."
    runner_release=$(sed -n 1p "$cache/runner/release")
    repos=$(sed -n '2,$p' "$cache/runner/release")
fi
git_base=$service/public/git/runner
# The runner's launch record ($RELAX_HOME/apps/runner.launch, one `key<TAB>value` line each):
# what `makepad` with no arguments runs, from here and from anywhere else (makepad launch.rs).
launch_dir=$root/apps
mkdir -p "$launch_dir" || fail "Cannot create $launch_dir"
record=$launch_dir/runner.launch
printf 'title\tMakepad\narg\trun\narg\t--quiet\narg\t--cached\n' > "$record.part" || fail "Cannot write $record"
rm -f "$record.mounts"
root_repo=
for entry in $(printf '%s\n' "$repos" | tr ' ' ':'); do
    name=${entry%%:*}
    path=${entry#*:}
    path=${path%%:*}
    # the release's commit pinned: makepad run takes the pack downloaded above (or its store) as it is,
    # without asking the service again
    commit=${entry##*:}
    pin=
    [ ${#commit} -ne 40 ] || pin=@$commit
    case "$path" in
        makepad) root_repo="$git_base/$name.git#$runner_release$pin" ;;
        makepad/*) printf 'arg\t--mount\narg\t%s\n' "${path#makepad/}=$git_base/$name.git#$runner_release$pin" >> "$record.mounts" ;;
        *) rm -f "$record.mounts"; fail "The runner release has a repository outside its workspace: $path" ;;
    esac
done
[ -n "$root_repo" ] || fail "The runner release has no repository at makepad."
printf 'arg\t%s\n' "$root_repo" >> "$record.part"
[ ! -f "$record.mounts" ] || { cat "$record.mounts" >> "$record.part"; rm -f "$record.mounts"; }
printf 'arg\tmakepad-runner\n' >> "$record.part"
mv -f "$record.part" "$record" || fail "Cannot write $record"
row 4 doing starting "${c_dim}Makepad's loader opens its window${c_r0}"
ready 'The Runner opens in a moment.'
screen_end
stamp start-loader
# The loader stays in the foreground until the runner closes; its output is appended to the log.
printf '  Makepad is open in its own window; closing it ends this one.\n  %sIts log: %s%s\n' "$c_dim" "$(short "$logs/makepad.log")" "$c_r0"
"$makepad" >>"$logs/makepad.log" 2>&1
code=$?
stamp "loader-exit-$code"
[ "$code" = 0 ] || fail "Makepad stopped with an error (exit $code). Run this again to start it; the log says why: $logs/makepad.log"
exit 0
