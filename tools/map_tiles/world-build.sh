#!/bin/zsh
# World (planet) build chain — runs DETACHED, survives session death.
# Waits for the planet download, then: spool -> base -> bake -> shards.
# Progress: tail -f local/maps/world-build.log ; status per phase below.
set -e
cd "$(dirname "$0")/../.."
PBF=local/maps/pbf/planet-latest.osm.pbf
STORE=local/maps/world-detail.store
DETAIL_TMP=local/maps/world-detail-tmp.mbtiles
BASE=local/maps/world-base-br.mbtiles
BAKED=local/maps/world-base-br-v1.mbtiles
MKMAP=local/maps/world-base-br.mkmap
LOG=local/maps/world-build.log
BIN=./target/release/mptiles-run
BAKE=./target/release/mpbake-run

phase() { echo "==== $(date '+%F %T') PHASE: $1 ====" | tee -a "$LOG"; }

phase "wait-for-download"
# Download done = curl gone AND file over 70GB AND size stable 60s.
while pgrep -f "planet-latest.osm.pbf" | grep -qv $$; do sleep 60; done
SIZE1=$(stat -f %z "$PBF"); sleep 60; SIZE2=$(stat -f %z "$PBF")
if [ "$SIZE1" != "$SIZE2" ] || [ "$SIZE1" -lt 70000000000 ]; then
  echo "download incomplete/unstable ($SIZE1 vs $SIZE2) — aborting" | tee -a "$LOG"
  exit 1
fi
echo "planet pbf: $SIZE1 bytes" | tee -a "$LOG"

phase "spool (pbf-detail)"
"$BIN" pbf-detail "$PBF" "$DETAIL_TMP" --store "$STORE" --sort-memory-mib 2048 2>&1 | tee -a "$LOG"

phase "base (pbf-base)"
"$BIN" pbf-base "$PBF" "$BASE" --store "$STORE" 2>&1 | tee -a "$LOG"

phase "prune spool"
rm -rf "$STORE" "$DETAIL_TMP"
df -h . | tail -1 | tee -a "$LOG"

phase "bake (buckets 15-18, v4 streams)"
"$BAKE" "$BASE" "$BAKED" \
    --zooms 10,11,12,13,14 --buckets 15,16,17,18 --threshold-ms 100 2>&1 | tee -a "$LOG"

phase "decode-sanity"
"$BIN" verify-mbtiles "$BAKED" --stride 37 2>&1 | tee -a "$LOG"

phase "transmux to .mkmap shards"
rm -rf "$MKMAP"
"$BIN" transmux "$BAKED" "$MKMAP" 2>&1 | tee -a "$LOG"
"$BIN" mkmap-verify "$BAKED" "$MKMAP" 97 2>&1 | tee -a "$LOG"

phase "done"
