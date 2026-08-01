#!/bin/zsh
# Europe final-cut bake: v2 input-signature streams for ALL zoom levels in
# one run from the virgin base archive (z14 at buckets 14+16, z10-13 at
# their native bucket, threshold 100ms). Replaces the v1 -faces/-faces-mz
# chain, then re-cuts the .mkmap shards. This is the upload candidate.
set -e
cd "$(dirname "$0")/../.."
IN=local/maps/europe-base-br.mbtiles
OUT=local/maps/europe-base-br-v3.mbtiles
MKMAP=local/maps/europe-base-br.mkmap
LOG=local/maps/europe-finalcut.log
BIN=./target/release/makepad-map-tiles
BAKE=./target/release/makepad-map-bake

phase() { echo "==== $(date '+%F %T') PHASE: $1 ====" | tee -a "$LOG"; }

# v3: buckets carry baked building shadows. Buckets 14,15,16: rz15 is NOT a transient band — users cruise at z15 and
# an unbaked bucket there ran the full 460ms cascade per tile in-app.
phase "v2 faces bake (zooms 10-14, buckets 14+15+16 at z14 / native below, threshold 100ms)"
"$BAKE" "$IN" "$OUT" \
    --bridge-dz local/maps/nl-bridge-dz.mbtiles \
    --zooms 10,11,12,13,14 --buckets 14,15,16 --threshold-ms 100 2>&1 | tee -a "$LOG"

phase "decode-sanity"
"$BIN" verify-mbtiles "$OUT" --stride 200 2>&1 | tee -a "$LOG"

phase "transmux to .mkmap shards"
rm -rf "$MKMAP"
"$BIN" transmux "$OUT" "$MKMAP" 2>&1 | tee -a "$LOG"
ls -l "$MKMAP" | head -3 | tee -a "$LOG"

phase "done"
echo "europe final-cut complete" | tee -a "$LOG"
