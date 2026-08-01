#!/bin/zsh
# Europe mid-zoom faces bake: adds native-bucket cascade streams for
# z10-13 tiles on top of the finished z14 bake (input keeps its streams —
# zoom sets must not overlap, see map_bake --zooms). Then re-transmux the
# .mkmap shards from the new archive.
set -e
cd "$(dirname "$0")/../.."
IN=local/maps/europe-base-br-faces.mbtiles
OUT=local/maps/europe-base-br-faces-mz.mbtiles
MKMAP=local/maps/europe-base-br.mkmap
LOG=local/maps/europe-midzoom.log
BIN=./target/release/makepad-map-tiles
BAKE=./target/release/makepad-map-bake

phase() { echo "==== $(date '+%F %T') PHASE: $1 ====" | tee -a "$LOG"; }

phase "mid-zoom faces bake (zooms 10-13, native buckets, threshold 100ms)"
"$BAKE" "$IN" "$OUT" \
    --bridge-dz local/maps/nl-bridge-dz.mbtiles \
    --zooms 10,11,12,13 --threshold-ms 100 2>&1 | tee -a "$LOG"

phase "decode-sanity"
"$BIN" verify-mbtiles "$OUT" --stride 200 2>&1 | tee -a "$LOG"

phase "transmux to .mkmap shards"
rm -rf "$MKMAP"
"$BIN" transmux "$OUT" "$MKMAP" 2>&1 | tee -a "$LOG"
ls -l "$MKMAP" | head -3 | tee -a "$LOG"

phase "done"
echo "europe mid-zoom bake complete" | tee -a "$LOG"
