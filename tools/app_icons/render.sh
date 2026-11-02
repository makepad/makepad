#!/bin/sh
# Renders app icon sources (resources/icon.svg) to the resources/icon_1024.png
# that the Builder turns into macOS AppIcons and Windows icon resources.
# Uses macOS QuickLook (WebKit) for SVG rendering, which paints on opaque white;
# the icon_mask example then cuts the icon back to its transparent rounded
# square. Authoring only, nothing at build time depends on it.
#   sh tools/app_icons/render.sh apps/notes/resources/icon.svg [...]
# With ICON_COPY_TO=<dir> each result is also copied to <dir>/<svg folder with / as _>.png (a build
# node's results are fetched from its target folder, e.g. ICON_COPY_TO=target/icons).
set -eu
[ "$#" -gt 0 ] || { printf 'usage: render.sh path/to/icon.svg...\n' >&2; exit 1; }
tmp=$(mktemp -d "${TMPDIR:-/tmp}/app-icons.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
# Build the mask tool once; calling cargo per icon waits on the shared build lock.
mask=$(cargo metadata --format-version 1 --no-deps 2>/dev/null | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')/release/examples/icon_mask
[ -x "$mask" ] || cargo build -q --release -p makepad-win-resource --example icon_mask
for svg in "$@"; do
    dir=$(dirname -- "$svg")
    qlmanage -t -s 1024 -o "$tmp" "$svg" >/dev/null 2>&1
    out="$tmp/$(basename -- "$svg").png"
    [ -f "$out" ] || { printf 'render failed: %s\n' "$svg" >&2; exit 1; }
    mv "$out" "$dir/icon_1024.png"
    "$mask" "$dir/icon_1024.png" >/dev/null
    if [ -n "${ICON_COPY_TO:-}" ]; then
        mkdir -p "$ICON_COPY_TO"
        name=$(printf '%s' "$dir" | tr / _)
        cp "$dir/icon_1024.png" "$ICON_COPY_TO/$name.png"
    fi
    printf '%s\n' "$dir/icon_1024.png"
done
