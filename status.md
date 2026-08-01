# Map pipeline status — 2026-08-01 afternoon

## Where we are

The single-origin pipeline is settled end to end:

```
europe-260726.osm.pbf (34.7GB)
  └─ pbf-base → europe-base-br.mbtiles        55.6GB  virgin base, z2-14, brotli q11 (KEEP — bake input)
       └─ map_bake → europe-base-br-v2.mbtiles 63.7GB  + field-101 cascade streams (current app archive)
            └─ transmux → europe-base-br.mkmap/        root.mkidx + 107 tiles-NNN.mkshard, all <510MB
```

Baked-faces stream (field 101, `BAKED_FACES_VERSION=2`):
- **Input signature**: hash of ring-construction input (way geometry, dz, tier keys,
  joint sets — NO colors). HIT skips the whole i_overlay tier ring build.
- **Theme-independent**: proven one bake serves light/night/circuit
  (`baked_faces_theme_independent` test). Theme contract: themes only RECOLOR.
- z14 tiles carry buckets {14,16}; z10-13 their native bucket; threshold 100ms.
- Sidecars (never sharded): nl-bridge-dz.mbtiles (required at bake AND runtime),
  europe.searchdb (8.1GB, flat positioned-read format), europe-major.graph (1GB).

Perf arc today (worst Amsterdam z14, rz16 3D): 383ms → **153ms**; z12 city 661 → 50ms;
z13 166 → 13ms. All optimizations byte-parity-proven against the runtime cascade.

## Milestone 2026-08-01 late: v3 SHIPPED — app runs fully from shards

- europe-base-br-v3.mbtiles 74.4GB: buckets 14/15/16 + z10-13 native, v3
  streams = painter-cascade + BAKED BUILDING SHADOWS (own signature; night
  and future dynamic-sun themes just don't submit them).
- europe-base-br.mkmap: 128 shards, all <510,000,000B, 253,525 sampled
  byte-identical. App reads THESE (TileArchiveReader sniffs the path).
- Perf (headless via shards, shadows included): docklands 1.6-3.1s -> 116-161ms,
  worst AMS rz16 259ms, Paris 166ms, z12 54ms. In-app: user sees nothing >150ms.
- 291GB of superseded archives deleted (v1 faces chain, shortbread+osm-detail,
  old NL builds). Fallbacks kept: base archive + v2.

## Remaining to full remote (task #25 second half)
- [ ] HTTP range client for .mkmap (fetch root.mkidx once, ranged GETs, LRU disk cache)
- [ ] searchdb/graph/bridge-dz through the same fetch layer (they are flat
      offset-addressed files already; bridge-dz small enough to ship whole)
- [ ] Upload: rclone the .mkmap dir + sidecars to Cloudflare (only after any
      future rebake — every re-cut shifts all shard offsets)
- [ ] On-device measure once the phone client exists; next perf tier if needed:
      emit-stage ribbon bake (worst tiles ~100ms of emit remain)
