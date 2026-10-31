#!/usr/bin/env bash
# Build and run the depth-mix selftest on the node this runs on (a devhub
# lease): prints DEPTHMIX-OK / DEPTHMIX-FAIL and exits with its code.
set -euo pipefail
cargo build --release -p makepad-example-depth-mix -j8
exec "${CARGO_TARGET_DIR:-target}/release/makepad-example-depth-mix" --selftest
