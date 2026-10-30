#!/usr/bin/env bash
# The kernel precision probes (ATLAS_PROBE=1..5) after one build: which op differs from the CPU.
set -uo pipefail
cargo build --release -p makepad-example-sdf-atlas -j8 || exit 1
for p in 1 2 3 4 5 6 7; do
  echo "probe $p"
  ATLAS_PROBE=$p "${CARGO_TARGET_DIR:-target}/release/makepad-example-sdf-atlas" --selftest | grep kernel
done
