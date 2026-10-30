#!/usr/bin/env bash
# Build and run the SDF bake selftest on the node this runs on (a devhub lease):
# prints SDFBAKE-OK / SDFBAKE-FAIL and exits with its code. `--bench` adds GPU ms.
set -euo pipefail
cargo build --release -p makepad-example-sdf-bake -j8
exec "${CARGO_TARGET_DIR:-target}/release/makepad-example-sdf-bake" "${1:---selftest}"
