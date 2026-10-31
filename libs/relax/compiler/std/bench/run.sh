#!/bin/sh
set -eu
HOST=${1:?usage: run.sh mac|165}
BENCH=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT=$(CDPATH= cd -- "$BENCH/../../../../.." && pwd)
case "$HOST" in
 mac) TARGET="$ROOT/target/bench"; RELAX="$ROOT/target/release/makepad" ;;
 165) TARGET="$HOME/fastrust-std/bench/target"; RELAX="$HOME/fastrust-std/target/release/makepad" ;;
 *) echo 'usage: run.sh mac|165' >&2; exit 2 ;;
esac
mkdir -p "$TARGET" "$BENCH/logs-$HOST"
export MAKEPAD_RUN_STD="$ROOT/libs/relax/compiler/std"
# The shims need their cfgs (check: relax_check, check_os: relax_shim).
nice -n 10 rustc --edition 2021 -C opt-level=3 "$BENCH/real/main.rs" -o "$TARGET/std-bench-real" >"$BENCH/logs-$HOST/build-A.log" 2>&1
RUSTFLAGS="${RUSTFLAGS:-} --cfg relax_check --cfg relax_shim" nice -n 10 cargo build --offline --release -j 4 --manifest-path "$BENCH/shim/Cargo.toml" --target-dir "$TARGET" >"$BENCH/logs-$HOST/build-B.log" 2>&1
python3 "$BENCH/runner.py" "$HOST" "$TARGET" "$RELAX"
