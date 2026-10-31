#!/usr/bin/env bash
# Build and run the atlas selftest on the node this runs on (a devhub lease):
# prints ATLAS-OK / ATLAS-FAIL and exits with its code.
set -euo pipefail
cargo build --release -p makepad-example-sdf-atlas -j8
exec "${CARGO_TARGET_DIR:-target}/release/makepad-example-sdf-atlas" --selftest
