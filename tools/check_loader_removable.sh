#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"
unexpected=$(find apps/fab/loaders -mindepth 1 -maxdepth 1 -type d ! -name gltf -print)
if test -n "$unexpected"; then
    echo "unexpected out-of-tree loader directory: $unexpected" >&2
    exit 1
fi

if rg -n 'external-loaders|makepad-fab-loader-external|loaders/external' \
    apps/fab Cargo.toml .gitignore; then
    echo "an external loader seam remains in the application" >&2
    exit 1
fi

cargo check -p makepad-fab

echo "loader removability gate: ok"
