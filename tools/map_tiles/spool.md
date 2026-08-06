# Spool v2 notes — single-machine bottleneck analysis

Written 2026-08-02 during the first planet spool (94GB pbf → 430GB store),
which gated a 10-worker bake fleet behind ~3h of serial work. Updated the
same night after the v2 implementation round: every viable idea below is
now built; dispositions inline.

## Observed profile (M-series, 16 cores, NVMe)

| pass | work | time | bound by |
|---|---|---|---|
| 1 scan | pbf block index | ~3 min | I/O |
| 2 nodes | ~10B node coords → compressed store | ~25 min | CPU+write |
| 3 ways | ~1.05B ways resolved | ~35 min | mixed |
| 4 relations | ~10M+ assembled | **>60 min, the tail** | random-read latency |
| 5 finalize | store manifest/marker | ~minutes | I/O |

CPU during pass 4: ~1100% of 1600% — cores idle while member lookups
random-read the store. The heavyweight coastline/boundary multipolygons
cluster at the end and stall the counter. (Both symptoms addressed: the
stores are fully RAM-resident and the spiral sort fronts the monsters.)

## Ideas, by expected value

1. **Spiral-ordered finalization (the structural fix).** DONE — as a
   *streaming frontier* rather than per-region markers. Pass 4 collects
   all relation jobs, bbox-scans them in parallel against the RAM
   stores, sorts by distance from the NL anchor (bbox NEAREST point),
   and the spool writer publishes `store/spool-frontier.txt`: the spiral
   key of the first unfinished relation (atomic tmp+rename, spool
   flushed first, f64::MAX when done). Soundness: a relation that could
   touch a cell has bbox-nearest <= the cell far-corner distance, so
   `frontier > far_corner` means every such relation is fully on disk.
   pbf-base accepts a live store for --bbox slices behind that same gate
   (plus torn-tail-tolerant block reads — a torn tail can only belong to
   uncovered relations), and mapfleet's claim loop blocks per cell on
   the frontier instead of waiting for the whole spool. The fleet starts
   on NL while pass 4 is still assembling the far side of the planet.

2. **Offset-sorted member resolution in pass 4.** MOOT — pass 4 now
   loads FlatNodeStore/FlatWayStore fully into RAM; there are no store
   random-reads left to sort.

3. **Block cache on the compressed node store.** MOOT — same reason.
   The paged NodeStore/WayStore were deleted outright (git has them).

4. **Wider pass-4 parallelism.** DONE — assembly and bbox-scan workers
   now clamp to cores-2 max 14 (was cores-3 max 12).

5. **Early relation filtering.** VERIFIED — the untagged skip runs
   before member collection in the collect pass; nothing to hoist.

6. **Distribution: don't.** Unchanged — the fleet's place is after the
   sort, and with (1) it starts almost immediately anyway.

7. **Pass-granular resume.** DONE — passes 2 and 3 write
   `spool.passN.json` stamps (exact per-block spool lengths + counters +
   stats snapshot). A restart with an incomplete store rolls the spool
   back to the newest stamp (truncate stamped blocks, delete unstamped
   ones, drop the stale frontier) and reruns only the remaining passes.
   Covered by spool unit tests (torn-tail rollback + resume) and a
   crash-drill on the Luxembourg smoke store.

8. **Largest-first relation scheduling.** SUPERSEDED by the spiral
   sort — the planet-scale monsters (coastlines, country boundaries)
   have bboxes that contain or near the NL anchor, so their nearest-
   point key is ~0 and they start at hour zero anyway.

9. **Segment-bbox prefilter in clip_ring (the tail's real cost).**
   DONE — implemented as recursive bisection (bisect_polygon): rings
   split against tile-midpoint halves, so a ring clips against
   O(log tiles) spans instead of fully against every tile. Geometric
   equivalence covered by test.

## Interaction with pipeline-v2

The long-term streaming pipeline (slice → bake → q11 → shard append,
no intermediate sqlite) composes with (1): the frontier feeds per-cell
streaming slices, so planet-refresh wall-time approaches
max(spool-of-densest-region, fleet-bake) instead of spool + bake.

## Smoke rig

`local/maps/pbf/smoke/` holds a Luxembourg pbf + store + 4-cell config
that exercises the whole chain in ~2 minutes: spool with stamps and
frontier, crash/resume drill, live-frontier pbf-base gate (near cell
slices, far cell refused), mapfleet with a real worker box, weave with
multi-source verification.
