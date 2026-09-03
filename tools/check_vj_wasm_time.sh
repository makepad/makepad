#!/bin/bash
set -euo pipefail

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

# Keep cap-linted dependency artifacts from an ordinary build from satisfying
# a later primary-package lint invocation without actually running Clippy.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$repo_root/target/vj-wasm-time-gate}"

tree=$(cargo tree \
    -p makepad-vj \
    --target wasm32-unknown-unknown \
    -e normal \
    --prefix none \
    --offline \
    --format '{p}|{f}')
workspace=$(cargo metadata --format-version 1 --no-deps --offline)

package_args=()
feature_args=()
seen='|'
while IFS='|' read -r package features; do
    [[ "$package" == *" ($repo_root/"* ]] || continue
    [[ "$features" != " (*)" ]] || continue

    name=${package%% *}
    manifest_dir=${package##*\(}
    manifest_dir=${manifest_dir%\)}
    manifest="$manifest_dir/Cargo.toml"

    # Nested standalone workspaces have their own lint policy. This gate is
    # the root workspace's complete library intersection with VJ's wasm graph.
    [[ "$workspace" == *"\"manifest_path\":\"$manifest\""* ]] || continue
    [[ "$name" != makepad-vj ]] || continue
    [[ "$seen" != *"|$name|"* ]] || continue
    seen="$seen$name|"

    package_args+=(-p "$name")
    if [[ -n "$features" ]]; then
        IFS=',' read -r -a package_features <<< "$features"
        for feature in "${package_features[@]}"; do
            feature_args+=("$name/$feature")
        done
    fi
done <<< "$tree"

required=(
    makepad-platform
    makepad-asset-store
    makepad-network
    makepad-asset-importer
    makepad-chat-ui
    makepad-asset-chat
    makepad-system-speech
    makepad-sqlite
    makepad-widgets
    makepad-audio-encode
    makepad-render
    makepad-tsdf
    makepad-svg
    makepad-asset-data
    makepad-filesystem-watcher
    makepad-video-flow
    makepad-archive-org
    makepad-error-log
    makepad-splat
)
for name in "${required[@]}"; do
    if [[ "$seen" != *"|$name|"* ]]; then
        echo "missing VJ wasm dependency: $name" >&2
        exit 1
    fi
done

clippy_lints=(
    -A clippy::all
    -D clippy::disallowed_types
    -D clippy::disallowed_methods
)

echo "checking VJ wasm dependency libraries"
cargo clippy \
    "${package_args[@]}" \
    --lib \
    --no-default-features \
    --features "$(IFS=,; echo "${feature_args[*]}")" \
    --target wasm32-unknown-unknown \
    --offline \
    --no-deps \
    -- \
    "${clippy_lints[@]}"

echo "checking makepad-vj"
cargo clippy \
    -p makepad-vj \
    --bin makepad-vj \
    --no-default-features \
    --target wasm32-unknown-unknown \
    --offline \
    --no-deps \
    -- \
    "${clippy_lints[@]}"
