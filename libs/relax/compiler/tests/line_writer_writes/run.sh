#!/bin/bash
# usage: run.sh <makepad binary>: a LineWriter writes each completed line once, its buffered start with its end
# (real std's write_all); prints the writes as cargo run does, then "line_writer_writes: ok" or
# "line_writer_writes: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
export RELAX_CACHE=off
ERR=$(mktemp)
OUT=$(cd "$H" && RELAX_STD=$H/../../std "$1" run --quiet lww-app 2>"$ERR")
echo "$OUT"
if [ "$OUT" = 'line writer writes ["LINE a=1 b=2\n", "part 3\n", "two\n", "lines 1\n", "tail"]' ]; then rm -f "$ERR"; echo "line_writer_writes: ok"; else tail -5 "$ERR"; rm -f "$ERR"; echo "line_writer_writes: FAILED"; exit 1; fi
