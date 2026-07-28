# makepad-geodata

Bulk open-geodata fetching, per-layer overlay database building, live-source
syncing (rain radar), and a structured query surface — for the map stack.
Netherlands-first. Companion docs: `datasources.md` (source survey, licenses)
and `gps.md` (interaction layer) at the repo root.

The two consumers of every layer database:
1. **The renderer** — standard gzipped MVT vector tiles / PNG raster tiles.
2. **An LLM (via the map app)** — the `features` sidecar table + `query`
   module answer "what is at/near this location" with structured JSON, and
   raster layers carry class tables so values are nameable.

Design rules, in order:

1. **Bulk downloads only.** Every static source is a single downloadable
   file. No API paging, no WFS spidering, no tile scraping. Live sources
   (radar) poll their official file APIs at most once per data refresh.
   Search-style APIs (Wikipedia, Overpass, NDW live traffic, GTFS-RT) are
   *not* this crate's job — the map app queries those by viewport at runtime.
2. **One .mbtiles per layer**, never merged into the base map archive:
   `local/overlays/nl-<layer>.mbtiles` — independently rebuildable,
   shippable, deletable.
3. **Reuse the existing stack.** SQLite read *and* write via
   `makepad-mbtile-reader` (GeoPackages are just SQLite; the reader grew
   `open_sqlite` / `schema_entries` / `for_each_row` /
   `for_each_row_in_range`, the writer grew extra-table support). Mercator
   math from `makepad-map-nav`. New here: RD New <-> WGS84 polynomials, WKB,
   MVT encoding, TIFF subset reader, PNG codec, tiling.
4. **Library first.** The `geodata` binary is a thin CLI; the map app embeds
   the same machinery (`fetch_source` for periodic re-syncs, `RadarSync` for
   live radar, `query::LayerDb` for the LLM/tap-inspect surface).

## Politeness (enforced in `fetch.rs` / `radar.rs`, not left to callers)

- descriptive User-Agent with contact address; one transfer at a time; fixed
  pause after every network hit
- cached files are not even revalidated before `recheck_days`; afterwards
  If-Modified-Since makes an unchanged file cost one 304
- interrupted downloads resume; per-source `--limit-rate` for small servers
- radar: `min_poll_secs` gate armed *before* the request (a failing API
  cannot get hammered), request pacing + one 429 backoff-retry

## CLI

```
geodata list                          layers + sources + ready/planned
geodata fetch <layer|all> [--force]   download / revalidate bulk sources
geodata build <layer|all>             build nl-<layer>.mbtiles
geodata status                        cache + outputs
geodata query <layer> <lon> <lat> [--radius m] [--limit n]   features JSON
geodata radar-sync [forecast|reflectivity]                   poll rain radar
```

Defaults: cache `local/overlays/cache/`, output `local/overlays/`.

## Database contract

Written by `MbtilesWriter` (64 KB pages, deterministic block-major rowids,
passes `PRAGMA integrity_check`). Per file:

- `tiles` — gzipped MVT 2.1 (vector, `format=pbf`) or PNG (raster,
  `format=png`).
- `features` — the query sidecar (vector layers): columns `cell, layer,
  name, min_lon, min_lat, max_lon, max_lat, attrs (JSON), ring (JSON|null)`.
  rowid = `(z12 grid cell << 24) | seq` so bbox queries become b-tree range
  scans. Polygon layers opt into `ring` (simplified exterior) for exact
  point-in-polygon ("which buurt am I in").
- `metadata` — mbtiles standard keys + `attribution`, `license`, TileJSON
  `json.vector_layers` (field names/types per MVT layer), and for rasters
  `geodata_encoding` (`terrarium` | `class-index`) + `geodata_classmap`
  (JSON class -> label/color) so both shader and query side interpret pixels.

Raster encodings: **terrarium** RGB (elevation, e = h+32768) — read by the
renderer's future hillshade/3D and by map_nav's EV grade baking; **class
index** gray8 (noise dB bands, flood depth bands) — colormapped in the
shader, named via the classmap.

## Layers (all implemented)

| layer | content | source (license) | zooms | output |
|---|---|---|---|---|
| `nature` | Natura 2000 + wetlands polygons, rings in sidecar | PDOK (CC0) | 6-12 | 5.9 MB |
| `chargers` | 66k EV charging locations, operator/power | NDW OCPI (open) | 8-14 | 25 MB |
| `demographics` | CBS 500m+100m grid stats per cell | CBS (CC BY) | 8-13 | 262 MB |
| `wijkbuurt` | gemeente/wijk/buurt polygons + kerncijfers, rings | CBS/Kadaster (CC BY) | 6-13 | 87 MB |
| `transit` | all NL stops + rail/tram/metro/ferry shapes | OVapi GTFS (CC0) | 7-14 | 13 MB |
| `buildings-age` | **all 11.4M BAG buildings** with bouwjaar+status | PDOK bag-light (CC0, 7.8 GB) | 13-14 | 1.5 GB |
| `terrain` | GLO-30 elevation, terrarium | Copernicus (attr) | 6-12 | raster |
| `noise` | RIVM 10m Lden binned to 5 dB classes | RIVM (CC0) | 6-13 | raster |
| `flood` | JRC RP100 river flood depth classes | JRC (CC BY) | 6-11 | raster |

Live source: **rain radar** (`radar.rs`) — KNMI `radar_forecast` (+2h
nowcast, one file per 5 min, the map-app default) and
`radar_reflectivity_composites` (5-min frames, keeps ~1 h). `RadarSync::sync`
is safe to call every frame; it polls at most once per `min_poll_secs` and
returns cached `RadarFrame`s (raw KNMI HDF5 — decode to raster is the next
step). API key: config > `KNMI_API_KEY` env > shared anonymous key (register
a free personal key for real use; the anonymous quota is shared).

## Query surface (the LLM tool-call backend)

```rust
let mut db = query::LayerDb::open(path)?;
db.query_point(lon, lat, limit)          // exact point-in-polygon w/ rings
db.query_radius(lon, lat, meters, limit) // nearest-first with distances
db.query_bbox(min_lon, min_lat, max_lon, max_lat, limit)
```

Verified examples: Dam Square resolves gemeente Amsterdam -> wijk
Burgwallen-Nieuwe Zijde -> buurt Nieuwe Kerk e.o. (with populations); nearest
charger 128 m (operator + kW); Royal Palace bouwjaar 1655 out of 11.4M
buildings.

## Module map

```
src/fetch.rs      polite bulk downloader (curl), cache + .meta.json
src/radar.rs      RadarSync: KNMI radar poll/cache/prune, app-embeddable
src/geo.rs        RD New <-> WGS84 polynomials (round-trip tested < 1 m);
                  writer-order tile key; mercator via makepad-map-nav
src/wkb.rs        GeoPackage blob + ISO/EWKB parser
src/gpkg.rs       GeoPackage feature iteration (schema-driven, srs transform)
src/mvt.rs        MVT 2.1 encoder (extent 4096, key/value dedup)
src/tiler.rs      geometry -> clipped tile features (shared); in-memory
                  Tileset for <= few-million-feature layers
src/spool.rs      SpoolTiler: country-scale layers via per-block disk spool
                  (BAG: 11.4M polygons in one pass, peak memory = one block)
src/sidecar.rs    features table builder (grid rowids, JSON attrs, rings)
src/query.rs      LayerDb: point/radius/bbox queries over the sidecar
src/tiff.rs       GeoTIFF subset: tiled/striped, deflate/LZW/none,
                  predictors 1/2/3, int/float samples, geo tags, nodata
src/png.rs        PNG encode/decode (gray8 + rgb8) with hand-rolled CRC32
src/raster.rs     sampler -> 256px PNG tile pyramid (terrarium/class-index)
src/layers/       one module per layer + registry
src/main.rs       CLI
```

## Known limits / follow-ups

- Radar HDF5 -> renderable raster decode is not in yet (files are cached and
  indexed; decode next). OPERA/MeteoGate is the Europe-wide radar follow-up.
- Flood: JRC `spurious_depth_areas` mask not applied; PDOK ROR official zone
  polygons (CC0) planned as vector companion.
- Points are not duplicated into neighbor-tile buffers (edge icons may clip).
- No Douglas-Peucker (dedup + area culling only); BigTIFF unsupported;
  `unzip` is shelled out.
- Raster query helper (`sample elevation/class at lon/lat` via PNG decode)
  belongs in `query.rs` next, so the LLM can ask "how high / how loud /
  flood depth here".
