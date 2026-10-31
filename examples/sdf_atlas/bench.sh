#!/usr/bin/env bash
# Time the bake-shaped kernel, fast and precise (prints ATLAS-BENCH ...).
set -euo pipefail
cargo build --release -p makepad-example-sdf-atlas -j8
exec "${CARGO_TARGET_DIR:-target}/release/makepad-example-sdf-atlas" --bench
