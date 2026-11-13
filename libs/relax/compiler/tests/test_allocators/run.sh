#!/bin/bash
# usage: run.sh <makepad binary>: `makepad test ta-app` runs tests/first.rs and tests/second.rs, each
# with its own `#[global_allocator]`, tests/alloc.rs too under a sysroot crate's name, tests/core.rs,
# std.rs, test.rs and proc_macro.rs named likewise, and tests/plain_alloc.rs with none, out of the package's one
# test program: each target's allocations go through its own allocator only. Twice on one fresh
# cache (cold, then restored), so cached code never carries one target's allocator into another.
# Prints "test_allocators: ok" or "test_allocators: FAILED".
H=$(cd "$(dirname "$0")" && pwd)
C=$(mktemp -d) || exit 1
for pass in cold warm; do
    OUT=$(cd "$H" && RELAX_CACHE_DIR=$C RELAX_STD=$H/../../std "$1" test ta-app 2>&1)
    rc=$?
    echo "$OUT"
    ok=$(echo "$OUT" | grep -c "^test result: ok. 1 passed")
    if [ $rc != 0 ] || [ "$ok" != 8 ]; then
        echo "test_allocators: FAILED ($pass: rc $rc, results $ok)"
        rm -rf -- "$C"
        exit 1
    fi
done
rm -rf -- "$C"
echo "test_allocators: ok"
