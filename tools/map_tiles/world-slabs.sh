#!/bin/zsh
# NL-outward spiral slab driver: waits for the world spool, then per cell
# (tools/map_tiles/world-cells.txt, NL first): slice -> bake -> weave the
# grown shard set. The app watches the woven dir and shows NL first, live,
# rings appearing as they land. Resume-safe: baked cells are skipped.
set -e
set -o pipefail
# Single-instance lock: a raced duplicate corrupted the spool once.
LOCK="/tmp/$(basename $0).lock"
if ! mkdir "$LOCK" 2>/dev/null; then echo "another instance holds $LOCK — exiting"; exit 1; fi
trap 'rmdir "$LOCK"' EXIT
cd "$(dirname "$0")/../.."
STORE=local/maps/world-detail.store
PBF=local/maps/pbf/planet-latest.osm.pbf
CELLS=tools/map_tiles/world-cells.txt
OUT=local/maps/world-cells
MKMAP=local/maps/world.mkmap
LOG=local/maps/world-slabs.log
BIN=./target/release/mptiles-run
BAKE=./target/release/mpbake-run
mkdir -p "$OUT"

log() { echo "$(date '+%F %T') $1" | tee -a "$LOG"; }

log "waiting for spool"
while [ ! -f "$STORE/SPOOL_COMPLETE" ]; do sleep 120; done
log "spool complete — spiral begins"

N=0
WOVEN=""
while IFS= read -r BBOX; do
    N=$((N+1))
    CELL=$(printf "cell-%03d" $N)
    BAKED="$OUT/$CELL-baked.mbtiles"
    if [ -f "$BAKED" ]; then
        WOVEN="$WOVEN $BAKED"
        continue  # resume: previous output honored, no duplicate work
    fi
    RAW="$OUT/$CELL-base.mbtiles"
    log "$CELL slice bbox=$BBOX"
    if ! "$BIN" pbf-base "$PBF" "$RAW" --store "$STORE" --bbox "$BBOX" >> "$LOG" 2>&1; then
        log "$CELL empty/failed slice — skipping"
        rm -f "$RAW"
        continue
    fi
    log "$CELL bake"
    "$BAKE" "$RAW" "$BAKED" \
        --zooms 10,11,12,13,14 --buckets 15,16,17,18 --threshold-ms 100 >> "$LOG" 2>&1
    rm -f "$RAW"
    WOVEN="$WOVEN $BAKED"
    log "$CELL weave ($(echo $WOVEN | wc -w | tr -d ' ') cells)"
    rm -rf "$MKMAP.next"
    "$BIN" transmux ${=WOVEN} "$MKMAP.next" >> "$LOG" 2>&1
    rm -rf "$MKMAP.prev"
    [ -d "$MKMAP" ] && mv "$MKMAP" "$MKMAP.prev"
    mv "$MKMAP.next" "$MKMAP"
    log "$CELL LIVE — world now $(du -sh $MKMAP | awk '{print $1}')"
done < "$CELLS"
log "spiral complete"
