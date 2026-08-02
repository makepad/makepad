#!/bin/zsh
# World (planet) build chain — runs DETACHED, survives session death.
# Waits for the planet download, then: spool -> base -> bake -> shards.
# Progress: tail -f local/maps/world-build.log ; status per phase below.
set -e
set -o pipefail
# Single-instance lock: a raced duplicate corrupted the spool once.
LOCK="/tmp/$(basename $0).lock"
if ! mkdir "$LOCK" 2>/dev/null; then echo "another instance holds $LOCK — exiting"; exit 1; fi
trap 'rmdir "$LOCK"' EXIT
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

phase "spool-complete"
# The slab driver (world-slabs.sh) takes over from here: Europe-outward
# pbf-base --bbox slabs, per-slab bake, weave into the serving shard set.
touch local/maps/world-detail.store/SPOOL_COMPLETE
