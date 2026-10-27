#!/bin/bash
# Regression test: the program's stderr never fails because the session transcript cannot be
# written (10-23: ENOSPC on a full tmpfs TMPDIR panicked eprintln! and aborted a fleet Sandbox).
# The transcript is made unwritable past FILE_KB with a file-size limit (EFBIG, SIGXFSZ ignored);
# relax-stderr-flood then writes LINES lines (~100 bytes each) with its own eprintln!. Passes when
# the run exits 0, prints "flood done" and the terminal (this script's stderr capture) received
# every line. Linux and macOS. Run from the workspace root after
# `cargo build --release -p makepad-run`. TARGET (default target/release), LINES (default 40000),
# FILE_KB (default 1024: the transcript fails after ~1 MB, the flood is ~4 MB).
set -u
T=${TARGET:-target/release}
LINES=${LINES:-40000}
FILE_KB=${FILE_KB:-1024}
dir=$(mktemp -d "${TMPDIR:-/tmp}/stderr_full.XXXXXX") || exit 1
mkdir -p "$dir/tmp" "$dir/home"
# a warm-up run without the limit, so the JIT cache (bigger than FILE_KB) is written before the
# limit applies; the limited run then only has the transcript to fail
TMPDIR="$dir/tmp" MAKEPAD_RUN_HOME="$dir/home" "$T/makepad" run --quiet relax-provided-fixture --bin relax-stderr-flood -- 10 \
    > "$dir/warm.out" 2> "$dir/warm.err"
# the "terminal" is a fifo whose reader starts outside the limit (a capture file under it would fail too)
mkfifo "$dir/err.fifo" "$dir/out.fifo"
cat "$dir/err.fifo" > "$dir/run.err" &
err_pid=$!
cat "$dir/out.fifo" > "$dir/run.out" &
out_pid=$!
(
    trap '' XFSZ
    ulimit -f "$FILE_KB"
    TMPDIR="$dir/tmp" MAKEPAD_RUN_HOME="$dir/home" "$T/makepad" run --quiet relax-provided-fixture --bin relax-stderr-flood -- "$LINES" \
        > "$dir/out.fifo" 2> "$dir/err.fifo"
    echo $? > "$dir/rc"
)
wait $err_pid $out_pid
rc=$(cat "$dir/rc")
got=$(grep -c '^flood line ' "$dir/run.err")
done_line=$(grep -c "^flood done $LINES" "$dir/run.out")
echo "stderr_full: rc=$rc, terminal got $got of $LINES lines, stdout done=$done_line (logs $dir)"
grep -E 'panicked|failed printing|session log' "$dir/run.err" | head -5
# case 2 (no limit): a panic's message reaches the terminal and the kept transcript, in order
TMPDIR="$dir/tmp" MAKEPAD_RUN_HOME="$dir/home" "$T/makepad" run --quiet relax-provided-fixture --bin relax-stderr-flood -- 1000 panic \
    > "$dir/panic.out" 2> "$dir/panic.err"
prc=$?
kept=$(sed -n 's/^makepad-run: session log kept at //p' "$dir/panic.err" | tail -1)
p_term=$(grep -c 'flood panic after 1000 lines' "$dir/panic.err")
p_file=0; p_lines=0
if [ -n "$kept" ] && [ -f "$kept" ]; then
    p_file=$(grep -c 'flood panic after 1000 lines' "$kept")
    p_lines=$(grep -c '^flood line ' "$kept")
fi
echo "stderr_full panic: rc=$prc, message on the terminal $p_term, in the kept transcript $p_file ($p_lines lines before it)"
[ "$prc" != 0 ] && [ "$p_term" -ge 1 ] && [ "$p_term" = "$p_file" ] && [ "$p_lines" = 1000 ] || { echo "STDERR-FULL-FAIL (panic case)"; exit 1; }
if [ "$rc" = 0 ] && [ "$got" = "$LINES" ] && [ "$done_line" = 1 ]; then
    echo "STDERR-FULL-OK"
    exit 0
fi
echo "STDERR-FULL-FAIL"
exit 1
