#!/bin/zsh
# Overnight Europe flow: pbf-base -> single brotli mbtiles -> transmux into
# Cloudflare-ready .mkmap shards (each file < 510,000,000 bytes).
#
# Run from the repo root. Reuses the completed pbf-detail store; does NOT
# re-ingest the PBF. Expect roughly 3.5-5h at q11 on 16 cores plus ~1h for
# the transmux (estimates from the NL run; see nl-base-br.log).
set -e
setopt no_nomatch

PBF=local/maps/europe-260726.osm.pbf
STORE=local/maps/native-detail-europe.store
OUT=local/maps/europe-base-br.mbtiles
MKMAP=local/maps/europe-base-br.mkmap
BIN=./target/release/makepad-map-tiles
LOG=local/maps/europe-base-br.log

ulimit -n 8192
cargo build --release -p makepad-map-tiles

echo "== pbf-base (brotli q11, all zooms, whole store) ==" | tee "$LOG"
"$BIN" pbf-base "$PBF" "$OUT" \
    --store "$STORE" \
    --brotli-quality 11 \
    --sort-memory-mib 512 2>&1 | tee -a "$LOG"

echo "== decode-sanity ==" | tee -a "$LOG"
"$BIN" verify-mbtiles "$OUT" --stride 200 2>&1 | tee -a "$LOG"

echo "== transmux to .mkmap shards ==" | tee -a "$LOG"
rm -rf "$MKMAP"
"$BIN" transmux "$OUT" "$MKMAP" 2>&1 | tee -a "$LOG"

echo "== shard sizes (all must be < 510000000 bytes) ==" | tee -a "$LOG"
ls -l "$MKMAP" | tee -a "$LOG"
echo "done" | tee -a "$LOG"
