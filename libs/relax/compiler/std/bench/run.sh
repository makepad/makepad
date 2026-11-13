#!/bin/sh
# usage: run.sh <label> [relax executable]
# <label> names this machine's results (logs-<label>/, results-<label>.md), e.g. mac or linux.
# The Relax executable defaults to this checkout's target/release/relax; the rustc builds go to
# target/bench (BENCH_TARGET_DIR overrides it).
set -eu
USAGE='usage: run.sh <label> [relax executable]'
HOST=${1:?$USAGE}
case "$HOST" in
 *[!A-Za-z0-9_-]*) echo "$USAGE" >&2; exit 2 ;;
esac
BENCH=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT=$(CDPATH= cd -- "$BENCH/../../../../.." && pwd)
TARGET=${BENCH_TARGET_DIR:-$ROOT/target/bench}
RELAX=${2:-$ROOT/target/release/relax}
mkdir -p "$TARGET" "$BENCH/logs-$HOST"
export RELAX_STD="$ROOT/libs/relax/compiler/std"
nice -n 10 rustc --edition 2021 -C opt-level=3 "$BENCH/real/main.rs" -o "$TARGET/std-bench-real" >"$BENCH/logs-$HOST/build-A.log" 2>&1
python3 "$BENCH/runner.py" "$HOST" "$TARGET" "$RELAX"
