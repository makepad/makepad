#!/bin/zsh
# Overnight Europe flow: pbf-base (streets-merged emission) -> verify ->
# painter-cascade faces bake (v2-faces-1, threshold 150ms, buckets 14+16;
# rz15 intentionally falls back to the runtime cascade — the signature
# guard is exact-bucket, so there is no cross-bucket reuse; rz17+ was
# always runtime) -> transmux into Cloudflare-ready .mkmap shards (each
# < 510,000,000 bytes) -> stat-verify every shard under the cap.
#
# Run from the repo root. Reuses the completed pbf-detail store; does NOT
# re-ingest the PBF. Logs progressively to local/maps/europe-overnight.log.
set -e
setopt no_nomatch

PBF=local/maps/europe-260726.osm.pbf
STORE=local/maps/native-detail-europe.store
OUT=local/maps/europe-base-br.mbtiles
FACES=local/maps/europe-base-br-faces.mbtiles
MKMAP=local/maps/europe-base-br.mkmap
BIN=./target/release/makepad-map-tiles
BAKE=./target/release/makepad-map-bake
LOG=local/maps/europe-overnight.log

ulimit -n 8192

phase() {
  echo "" | tee -a "$LOG"
  echo "==== $(date '+%F %T') PHASE: $1 ====" | tee -a "$LOG"
}

: > "$LOG"
phase "disk headroom"
df -g . | tee -a "$LOG"

phase "pbf-base (brotli q11, all zooms, whole store, z14 streets merge)"
"$BIN" pbf-base "$PBF" "$OUT" \
    --store "$STORE" \
    --brotli-quality 11 \
    --sort-memory-mib 512 2>&1 | tee -a "$LOG"

phase "decode-sanity (base)"
"$BIN" verify-mbtiles "$OUT" --stride 200 2>&1 | tee -a "$LOG"

phase "faces bake (threshold 150ms, buckets 14,16, worker-side brotli q10, all cores)"
# nl-bridge-dz passed so NL tiles bake with the dz configuration the app
# runs with (outside its bounds it is inert). NOTE: dz joins are stale
# against the merged street indices until the dz rebake; the bake and the
# app fail those joins closed IDENTICALLY, so signatures still match.
"$BAKE" "$OUT" "$FACES" \
    --bridge-dz local/maps/nl-bridge-dz.mbtiles \
    --threshold-ms 150 --buckets 14,16 2>&1 | tee -a "$LOG"

phase "decode-sanity (faces)"
"$BIN" verify-mbtiles "$FACES" --stride 200 2>&1 | tee -a "$LOG"

phase "transmux faces archive to .mkmap shards"
rm -rf "$MKMAP"
"$BIN" transmux "$FACES" "$MKMAP" 2>&1 | tee -a "$LOG"

phase "shard cap verification (all must be < 510000000 bytes)"
ls -l "$MKMAP" | tee -a "$LOG"
OVER=$(find "$MKMAP" -type f -size +509999999c | wc -l | tr -d ' ')
echo "shards over cap: $OVER" | tee -a "$LOG"
if [ "$OVER" != "0" ]; then
  echo "SHARD CAP EXCEEDED" | tee -a "$LOG"
  exit 1
fi

phase "done"
ls -la "$OUT" "$FACES" | tee -a "$LOG"
echo "europe overnight complete" | tee -a "$LOG"
