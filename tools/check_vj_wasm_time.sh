#!/bin/bash
set -euo pipefail

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

# Keep cap-linted dependency artifacts from an ordinary build from satisfying
# a later primary-package lint invocation without actually running Clippy.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$repo_root/target/vj-wasm-time-gate}"

tree=''
add_tree() {
    tree+=$'\n'"$(cargo tree \
        -p "$1" \
        "${@:2}" \
        --target wasm32-unknown-unknown \
        -e normal \
        --prefix none \
        --offline \
        --format '{p}|{f}')"
}

# Use each web app's shipped feature set; the union below preserves the
# feature-resolved wasm graph for every demo while linting shared crates once.
add_tree makepad-vj --no-default-features
add_tree makepad-app-route --no-default-features --features demo
add_tree makepad-files --no-default-features --features demo
add_tree makepad-sheets
add_tree makepad-app-finance --no-default-features --features demo
workspace=$(cargo metadata --format-version 1 --no-deps --offline)

package_args=()
feature_args=()
checked_names=()
seen='|'
while IFS='|' read -r package features; do
    [[ "$package" == *" ($repo_root/"* ]] || continue
    [[ "$features" != *" (*)"* ]] || continue

    name=${package%% *}
    manifest_dir=${package##*\(}
    manifest_dir=${manifest_dir%\)}
    manifest="$manifest_dir/Cargo.toml"

    # Nested standalone workspaces have their own lint policy. This gate is
    # the root workspace's complete library union for the web-demo wasm graphs.
    [[ "$workspace" == *"\"manifest_path\":\"$manifest\""* ]] || continue
    case "$name" in
        makepad-vj|makepad-app-route|makepad-files|makepad-sheets|makepad-app-finance)
            continue
            ;;
    esac
    if [[ -n "$features" ]]; then
        IFS=',' read -r -a package_features <<< "$features"
        for feature in "${package_features[@]}"; do
            feature_args+=("$name/$feature")
        done
    fi
    [[ "$seen" != *"|$name|"* ]] || continue
    seen="$seen$name|"

    package_args+=(-p "$name")
    checked_names+=("$name")
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
        echo "missing web-demo wasm dependency: $name" >&2
        exit 1
    fi
done

clippy_lints=(
    -A clippy::all
    -D clippy::disallowed_types
    -D clippy::disallowed_methods
)

echo "checking wasm dependency libraries: ${checked_names[*]}"
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

check_app() {
    echo "checking $1"
    cargo clippy \
        -p "$1" \
        "${@:2}" \
        --target wasm32-unknown-unknown \
        --offline \
        --no-deps \
        -- \
        "${clippy_lints[@]}"
}

# apps/vj is deliberately omitted while its worker loops are changed elsewhere.
check_app makepad-app-route --no-default-features --features demo
check_app makepad-files --no-default-features --features demo
check_app makepad-sheets
check_app makepad-app-finance --no-default-features --features demo
