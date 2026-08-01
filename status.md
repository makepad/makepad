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

## Active problem (RIGHT NOW)

User at zoom 15 sees 600-2100ms tiles: **bucket 15 was never baked** ("transient
band" assumption was wrong). Headless confirms rz15 = runtime cascade, 260-430ms
+ contention. Fix in flight:
1. NL smoke bake with `--buckets 14,15,16` running (verify HIT at rz15)  ← waiting
2. Then rerun `tools/map_tiles/europe-finalcut.sh` with buckets 14,15,16 (~2.5h)
   → new v2 archive + re-shard (transmux is cheap, ~7min)
3. Repoint app (apps/route/src/main.rs:118-119), rebuild, verify rz15 in-app

Also open from tonight: night theme MISSed in-app even though the recolor test
passes — cascade-dump lever (`touch /tmp/mp_cascade_dump`) is in the build to
diff light-vs-night tier structure. Re-check AFTER the bucket-15 rebake (the
in-app test was on the pre-theme-independent archive, likely just v1-vs-v2
stream rejection — may already be fixed).

## Sharding: done vs still to do

### Done (producer side)
- [x] `.mkmap` format: root.mkidx (header, metadata, brotli dict, Hilbert-range
      directory → (shard, offset, len)) + tiles-NNN.mkshard, hard cap 510,000,000B
      per shard (Cloudflare), content-hash dedup of identical tiles.
- [x] transmux from any finished mbtiles (verbatim compressed bytes, no re-encode).
- [x] Verification pass: every tile resolved through the index, 253k sampled
      byte-identical, cap audit. 107 shards / ~52GB for current v2.

### To do (consumer side — task #25, nothing started)
- [ ] **mkmap reader** in libs (mmap-first): open root.mkidx, mmap shards,
      resolve key → (shard, offset, len) → decode. Replaces MbtilesReader on desktop.
- [ ] **MapView integration**: mbtiles_path accepts .mkmap dir (or auto-detect),
      same for detail path; keep MbtilesReader as fallback for sidecars/tests.
- [ ] **HTTP range client** (phone/web): fetch root.mkidx once, then ranged GETs
      into shards via CxWebSocket/http; LRU disk cache of fetched tiles
      (RAM-frugal, lossless — mobile-server memory doc applies).
- [ ] **Sidecar strategy**: searchdb + graph are already flat offset-addressed
      files — same range-fetch model, but need their readers taught to go through
      the same fetch/cache layer. bridge-dz is small (<100MB NL) — ship whole.
- [ ] **Upload**: after bucket-15 rebake verifies → rclone to Cloudflare R2/pages.
      NOTE: re-shard = full re-cut (offsets shift), so upload only after the
      archive is final. Nothing uploaded yet.

## Cleanup once bucket-15 cut verifies (~120GB reclaimable)
- europe-base-br-faces.mbtiles (59.7GB, v1 streams — superseded)
- europe-base-br-faces-mz.mbtiles (63.9GB, v1+midzoom — superseded)
- europe-shortbread.mbtiles (33GB) + europe-osm-detail.mbtiles (58.7GB) — old
  two-archive design, only fallback
- old NL archives (nl-base-br, br2, slimfrag… ~12GB)
Keep: europe-base-br.mbtiles (bake input), the newest v2, the .mkmap dir, sidecars.

## Perf: next tier after this (optional, all measured candidates)
- emit stage 56ms on worst tile (fringe carriers + stroke passes) — ribbon bake
  or anchored GPU-width strokes.
- detail-merge 28-40ms (tag string materialization — interning).
- mid-zoom z12 stroke-prep 205ms→33ms done, but z12 emit still ~20ms.
- Phone target: worst tile ≤150ms on-device ≈ current numbers ÷ my-box-factor;
  measure on real hardware once the mkmap client exists.
