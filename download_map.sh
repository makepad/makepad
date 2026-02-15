#!/bin/bash
set -e

URL="https://download.geofabrik.de/europe/netherlands/noord-holland-shortbread-1.0.mbtiles"
OUT="noord-holland-shortbread-1.0.mbtiles"

echo "Downloading $OUT ..."
curl -L -o "$OUT" "$URL"
echo "Done: $OUT"
