# Spool v2 notes — single-machine bottleneck analysis

Written 2026-08-02 during the first planet spool (94GB pbf → 430GB store),
which gated a 10-worker bake fleet behind ~3h of serial work. A re-spool
is likely (quarterly planet refreshes, schema changes), so the observed
profile and the improvement ideas are recorded here.

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
cluster at the end and stall the counter.

## Ideas, by expected value

1. **Spiral-ordered finalization (the structural fix).** The spool's
   whole reason to exist is the global sort — but its OUTPUT (per-z14
   tile buckets) doesn't need to finalize globally. Finalize buckets in
   the same NL-outward cell order the bake spiral uses and drop
   PER-REGION markers (`SPOOL_COMPLETE.cell-001` …). The slab driver /
   mapfleet then starts slicing NL while the spool is still assembling
   Australia. This removes the fleet-idle gate entirely — worth more
   than every micro-optimization combined. Requires: bucket-complete
   tracking per cell (a bucket is complete when all passes that write
   into it have flushed past it — needs pass-2/3/4 writers to emit
   watermark offsets in tile-id space).

2. **Offset-sorted member resolution in pass 4.** Relation members
   resolve via random reads into the node/way stores. Batch each
   relation block's member ids, SORT BY STORE OFFSET, fetch
   semi-sequentially, then reassemble. Classic OSM-tooling optimization;
   typically 2-5x on this pass.

3. **Block cache on the compressed node store.** Pass 4 decompresses
   store blocks per lookup; an LRU of decompressed blocks (a few GB)
   turns the hot-region lookups (cities) into memory hits.

4. **Wider pass-4 parallelism.** ~11/16 cores busy suggests a lock or an
   ordered-emission serialization in the assembly loop. Profile before
   assuming; combine with (2), which also improves parallel scaling by
   removing latency stalls.

5. **Early relation filtering.** If assembly resolves members before
   deciding a relation type is unused by the schema, hoist the tag
   filter above member resolution. (Verify — may already be the case.)

6. **Distribution: don't.** Cross-references (ways→nodes anywhere on the
   planet) make a distributed spool a data-shuffle project with little
   win over one big-RAM machine + the fixes above. The fleet's place is
   after the sort, and with (1) it starts almost immediately anyway.

## Interaction with pipeline-v2

The long-term streaming pipeline (slice → bake → q11 → shard append,
no intermediate sqlite) composes with (1): per-region spool completion
feeds per-region streaming slices, so planet-refresh wall-time
approaches max(spool-of-densest-region, fleet-bake) instead of
spool + bake.
