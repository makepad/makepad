#!/bin/sh
# Renders devhub icon previews (Dock size, tray size, light and dark menu bar) from the sources
# apps/devhub/resources/icon_1024.png and tray.svg (macOS QuickLook, authoring only).
#   sh tools/devhub/icon-preview.sh [out-dir]    (default local/agent_state/devhub/icon)
set -eu
REPO=$(cd "$(dirname "$0")/../.." && pwd)
OUT=${1:-$REPO/local/agent_state/devhub/icon}
RES=$REPO/apps/devhub/resources
mkdir -p "$OUT"
tmp=$(mktemp -d "${TMPDIR:-/tmp}/devhub-icon.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
b64=$(base64 < "$RES/icon_1024.png" | tr -d '\n')
glyph() { # fill colour, x, y, scale
  sed -n '/<g /,/<circle cx="18" cy="19"/p' "$RES/tray.svg" | sed "s/#000/$1/g" | sed "1s|^|<g transform=\"translate($2 $3) scale($4)\">|;\$s|\$|</g>|"
}
cat > "$tmp/p.svg" <<SVG
<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="1200" height="520" viewBox="0 0 1200 520">
  <rect width="1200" height="520" fill="#6b7a90"/>
  <image x="30" y="30" width="256" height="256" xlink:href="data:image/png;base64,$b64"/>
  <image x="320" y="190" width="96" height="96" xlink:href="data:image/png;base64,$b64"/>
  <image x="440" y="226" width="60" height="60" xlink:href="data:image/png;base64,$b64"/>
  <image x="520" y="250" width="36" height="36" xlink:href="data:image/png;base64,$b64"/>
  <rect x="30" y="330" width="560" height="60" fill="#f1f1f1"/>
  $(glyph '#111' 60 342 1.0)
  $(glyph '#111' 130 336 1.5)
  $(glyph '#111' 220 330 2.0)
  <rect x="30" y="420" width="560" height="60" fill="#1c1c1e"/>
  $(glyph '#f5f5f5' 60 432 1.0)
  $(glyph '#f5f5f5' 130 426 1.5)
  $(glyph '#f5f5f5' 220 420 2.0)
</svg>
SVG
qlmanage -t -s 1200 -o "$tmp" "$tmp/p.svg" >/dev/null 2>&1
mv "$tmp/p.svg.png" "$OUT/preview.png"
cp "$RES/icon_1024.png" "$OUT/icon_1024.png"
printf '%s\n' "$OUT/preview.png"
