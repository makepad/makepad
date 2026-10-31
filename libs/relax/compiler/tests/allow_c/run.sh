#!/bin/sh
# makepad run --allow-c on the fixture (a path crate whose build script compiles C); prints the same
# four lines as `cargo run`. Usage: run.sh <makepad binary> [run flags]
set -eu
here=$(cd "$(dirname "$0")" && pwd)
m=$1
shift
cd "$here"
export CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0
exec nice -n 15 "$m" run --allow-c "$@" allow-c-fixture
