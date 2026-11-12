#!/bin/sh
# makepad run --allow-c on a package whose build script builds a static archive with cc and ar it
# runs itself and links it: prints the line `cargo run` prints. Usage: run.sh <makepad binary>
set -u
here=$(cd "$(dirname "$0")" && pwd)
m=$1
cd "$here"
export CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0
out=$(nice -n 15 "$m" run --allow-c allow-c-static-archive-fixture 2>&1)
if printf '%s\n' "$out" | grep -qx 'fixture_add(2, 3) = 1005'; then
    echo "allow_c_static_archive: ok"
else
    printf '%s\n' "$out"
    echo "allow_c_static_archive: FAILED"
    exit 1
fi
