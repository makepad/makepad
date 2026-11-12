#!/bin/sh
# makepad check on the fixture without --allow-c: it reads the script outputs of the native build
# a `--allow-c` run left in the cache. Usage: run.sh <makepad binary> [check flags]
set -eu
here=$(cd "$(dirname "$0")" && pwd)
m=$1
shift
cd "$here"
export CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0
exec nice -n 15 "$m" check "$@" cached-check-fixture
