#!/bin/sh
# Fetch the night-sky star panorama the engine's analytic sky can layer
# over its point stars: NASA/GSFC Scientific Visualization Studio
# "Deep Star Maps 2020" (svs.gsfc.nasa.gov/4851), celestial-coordinate
# equirectangular panorama from Gaia DR2 / Hipparcos. Public domain;
# credit NASA/GSFC SVS.
#
# The SVS publishes the map as linear EXR only, so this downloads the 4k
# EXR (~36 MB, resumable) and converts it to the 4096x2048 sRGB PNG the
# renderer loads from local/sky/starmap_2020_4k.png (override with
# MAKEPAD_STAR_MAP). Run from the repo root.
set -e

DIR=local/sky
PNG=$DIR/starmap_2020_4k.png
EXR=$DIR/starmap_2020_4k.exr
URL=https://svs.gsfc.nasa.gov/vis/a000000/a004800/a004851/starmap_2020_4k.exr

mkdir -p "$DIR"

attribution() {
    cat > "$DIR/ATTRIBUTION.txt" <<'TXT'
starmap_2020_4k.png: NASA/Goddard Space Flight Center Scientific
Visualization Studio "Deep Star Maps 2020" (svs.gsfc.nasa.gov/4851),
celestial-coordinate equirectangular panorama from Gaia DR2 / Hipparcos.
Public domain; credit NASA/GSFC SVS. Converted to sRGB PNG from the 4k
EXR by tools/download_stars.sh.
TXT
}

if [ -f "$PNG" ] && [ "$(wc -c < "$PNG")" -gt 2000000 ]; then
    attribution
    echo "already there: $PNG"
    exit 0
fi

if ! [ -f "$EXR" ] || [ "$(wc -c < "$EXR")" -lt 35000000 ]; then
    echo "downloading $URL"
    curl -L -C - --fail -o "$EXR" "$URL"
fi
SIZE=$(wc -c < "$EXR")
if [ "$SIZE" -lt 35000000 ]; then
    echo "download incomplete ($SIZE bytes) — re-run to resume" >&2
    exit 1
fi

# Linear EXR -> sRGB 8-bit PNG. OpenCV decodes EXR at float precision;
# the sRGB transfer keeps the faint stars sips' straight conversion loses.
OPENCV_IO_ENABLE_OPENEXR=1 python3 - "$EXR" "$PNG" <<'PY'
import sys
try:
    import cv2, numpy as np
except ImportError:
    sys.exit("needs python3 with opencv (pip install opencv-python numpy)")
img = cv2.imread(sys.argv[1], cv2.IMREAD_UNCHANGED)
if img is None:
    sys.exit("could not decode " + sys.argv[1])
lin = np.clip(img.astype(np.float64), 0.0, 1.0)
srgb = np.where(lin <= 0.0031308, lin * 12.92,
                1.055 * np.power(lin, 1 / 2.4) - 0.055)
cv2.imwrite(sys.argv[2], (srgb * 255.0 + 0.5).astype(np.uint8))
print("wrote", sys.argv[2], img.shape[1], "x", img.shape[0])
PY

attribution
echo "done: $PNG (+ ATTRIBUTION.txt)"
