#!/bin/bash
# Run under the repository's build-slot wrapper: run.sh <absolute makepad binary> [output dir].
# Compare rustc and Relax, with each feature setting and serial/parallel prescans.
set -euo pipefail
fixture=$(cd "$(dirname "$0")" && pwd)
compiler=$(cd "$fixture/../.." && pwd)
relax_bin=${1:?usage: run.sh <absolute makepad binary> [output dir]}
output=${2:-$compiler/local/agent_state/cfg_macros}
mkdir -p "$output"
output=$(cd "$output" && pwd)
export CARGO_NET_OFFLINE=true
export MAKEPAD_RUN_STD="$compiler/std"
export MAKEPAD_RUN_CACHE=off
cd "$fixture"
for mode in off on; do
    flags=(--no-default-features)
    if [ "$mode" = on ]; then flags+=(--features cfg-macros-defs/tweaker); fi
    cargo build --release --offline -j 6 --target-dir "$output/rustc" -p cfg-macros-app "${flags[@]}" > "$output/rustc-$mode.log" 2>&1 || {
        cat "$output/rustc-$mode.log"
        exit 1
    }
    "$output/rustc/release/cfg-macros-app" > "$output/rustc-$mode.out"
    for jobs in 1 6; do
        MAKEPAD_RUN_JOBS=$jobs MAKEPAD_RUN_CACHE_DIR="$output/relax-$mode-$jobs" \
            "$relax_bin" run --quiet cfg-macros-app "${flags[@]}" \
            > "$output/relax-$mode-$jobs.out" 2> "$output/relax-$mode-$jobs.log" || {
                cat "$output/relax-$mode-$jobs.log"
                exit 1
            }
        diff -u "$output/rustc-$mode.out" "$output/relax-$mode-$jobs.out"
        printf 'cfg_macros: feature=%s jobs=%s matches rustc\n' "$mode" "$jobs"
    done
done
