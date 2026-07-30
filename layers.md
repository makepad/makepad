# Overlay data layers — work log & status

Status 2026-07-28. This documents the overlay-layer track: the `libs/geodata`
crate, the nine built layer databases, the live rain-radar sync, and the LLM
query surface. Companions: `datasources.md` (source survey + licenses, the
research this implements), `gps.md` (interaction layer), `libs/geodata/README.md`
(the crate's own contract-level docs). The map renderer is a separate track —
nothing in here touches `widgets/src/map`.

## TL;DR

All planned NL layers are **built, integrity-checked, and query-verified** in
`local/overlays/nl-<layer>.mbtiles` (one file per layer, never merged into the
base Europe archive). Every vector layer is simultaneously:
1. **renderable** — standard gzipped MVT 2.1 tiles the renderer's existing
   decoder already parses, and
2. **LLM-queryable** — a grid-indexed `features` sidecar table + `query::LayerDb`
   (point / radius / bbox → structured JSON) for "reason with the map" tool calls.

Raster layers use terrarium RGB (elevation) or gray8 class-index encoding with
a `geodata_classmap` metadata table (class → label + suggested color), so the
shader colormaps and the query side can *name* values.

| layer | content | zooms | size | build | verified by |
|---|---|---|---|---|---|
| `nature` | Natura 2000 + Ramsar wetland polygons | 6–12 | 6.2 MB | 0.6 s | polygons + rings in sidecar |
| `chargers` | 66,583 EV charging locations (operator, kW) | 8–14 | 26 MB | 2.9 s | Krasnapolsky charger @128 m from Dam |
| `demographics` | CBS 500 m + 100 m grid stats (442k cells) | 8–13 | 275 MB | 9.4 s | population at point |
| `wijkbuurt` | gemeente/wijk/buurt polygons + kerncijfers | 6–13 | 91 MB | 4.8 s | Dam → Amsterdam → Burgwallen-NZ → Nieuwe Kerk e.o. |
| `transit` | all NL stops + 1,457 non-bus route shapes | 7–14 | 14 MB | 6.1 s | streamed from 234 MB GTFS |
| `buildings-age` | **all 11,407,303 BAG buildings**, bouwjaar+status | 13–14 | 1.6 GB | 91 s | Royal Palace → bouwjaar 1655 |
| `terrain` | GLO-30 elevation, terrarium, NL bbox | 6–12 | 330 MB | 57 s | 18/20 cells; 2 sea-only 404s by design |
| `noise` | RIVM 10 m Lden → 5 dB classes | 6–13 | 34 MB | 23 min | A10 tile: 65–75 dB spine over 50–55 dB city |
| `flood` | JRC 1-in-100y river flood depth classes | 6–11 | 2.7 MB | 3.6 min | class histogram plausible |

Plus the live source: **rain radar** (`RadarSync`) — pulled a real KNMI +2h
nowcast minutes after publication; poll-gated, cache-pruning, app-embeddable.

CLI quick reference (`cargo run -p makepad-geodata --bin geodata` or the
release binary):

```
geodata list | fetch <layer|all> | build <layer|all> | status
geodata query <layer> <lon> <lat> [--radius m] [--limit n]
geodata radar-sync [forecast|reflectivity]
```

## Architecture

`libs/geodata` is a **library first** (the map app will embed it for radar
sync, periodic source refresh, and the query surface), with a thin `geodata`
CLI. Workspace member; depends only on `makepad-mbtile-reader`,
`makepad-map-nav`, `flate2`, `serde_json`.

Data flow per layer:

```
bulk source file  ──fetch.rs──▶  local/overlays/cache/  (+ .meta.json)
      (GPKG / zip / json.gz / GeoTIFF, verified URLs, polite)
                 ──layers/<id>.rs──▶  parse + reproject to WGS84
                 ──tiler.rs / spool.rs──▶  clipped MVT features per (z,x,y)
                                      └──▶ sidecar.rs features records
                 ──MbtilesWriter──▶  local/overlays/nl-<id>.mbtiles
                                      ├─ tiles     (gzip MVT | PNG)
                                      ├─ features  (query sidecar)
                                      └─ metadata  (license, classmap, TileJSON)
```

Key decisions and why:

- **One .mbtiles per layer.** Independently rebuildable/shippable/deletable;
  the base-map archive (owned by the import-pipeline track) is never touched.
- **Bulk downloads only.** No API paging/spidering — search-shaped APIs
  (Wikipedia, Overpass, NDW live traffic, GTFS-RT) will be integrated in the
  map app, queried by viewport context at runtime. Radar is the one live
  source here and polls its official file API at most once per data refresh.
- **Reuse over reimplementation** (per the "don't duplicate the map engine's
  math" rule): mercator projection comes from `makepad_map_nav::geo`; SQLite
  read *and* write go through `makepad-mbtile-reader` — GeoPackages are just
  SQLite files, so the reader ingests PDOK/CBS/BAG data directly. Genuinely
  new code only where nothing existed: RD New ↔ WGS84 polynomials, WKB
  parsing, MVT encoding, a GeoTIFF subset reader, a PNG codec, clipping/tiling.

## Politeness (the "don't get banned" layer)

Enforced inside `fetch.rs`/`radar.rs` so no layer module can violate it:

- descriptive User-Agent with contact address; one transfer at a time; 1 s
  pause after every network hit
- a cached file is not even *revalidated* before its `recheck_days` age
  (BAG 45 d, CBS 90 d, chargers 2 d, DEM ~forever); afterwards
  If-Modified-Since revalidation makes an unchanged file cost one 304, zero bytes
- interrupted downloads resume (`.part` + `-C -`); optional `--limit-rate`
  for small government origin servers
- radar: the poll gate is armed *before* the request (a failing API cannot be
  hammered — this was found and fixed when the shared KNMI anonymous key
  429'd), plus request pacing and a single 429 backoff-retry
- every cached file carries a `.meta.json` (url, license, fetch time)

## Database contract (what the renderer AI and app consume)

Written by `MbtilesWriter`: 64 KB pages, deterministic block-major rowids
(direct-seek tile lookup), passes `PRAGMA integrity_check` — verified on all
nine outputs. Standard SQLite tooling reads everything.

- `tiles` — gzip MVT 2.1, extent 4096 (vector; `format=pbf`) or PNG 256 px
  (raster; `format=png`). The renderer's gzip sniff + MVT decode work as-is;
  MVT winding follows the spec (exterior CW in y-down tile coords).
- `metadata` — mbtiles standard keys, `attribution` + `license` (for the
  attribution screen), TileJSON-style `json.vector_layers` (field names and
  types per MVT layer — the renderer can style data-driven without guessing),
  `geodata_encoding` + `geodata_classmap` for rasters, `geodata_built_unix`.
- `features` — the query sidecar (vector layers): `cell, layer, name,
  min_lon, min_lat, max_lon, max_lat, attrs (JSON), ring (JSON|null)` with
  rowid = `(z12 grid cell << 24) | seq`. Bbox queries become pruned b-tree
  range scans — no SQL engine, no extra index. Polygon layers opt into
  storing a simplified exterior ring (≤96 pts, ~10 m tolerance) for exact
  point-in-polygon.

## Per-layer notes

**nature** — PDOK Natura 2000 + wetlands GPKGs (CC0, ~15 MB). MVT layers
`natura2000` (naam_n2k, sitecodes, status…) and `wetlands`; every feature also
tagged `kind`. Rings stored.

**chargers** — NDW national OCPI aggregate (`charging_point_locations_ocpi.json.gz`,
open data, refreshed ~daily; recheck 2 d makes this the natural app-side
periodic re-sync candidate). Attrs: name, operator, city, evses, connectors,
max_kw (from `max_electric_power` or volts×amps fallback).

**demographics** — CBS Vierkantstatistieken 500 m (z8–z11) + 100 m (z12–z13)
grids; 139/134 columns of per-cell stats carried through with CBS suppression
sentinels (negatives) dropped. Square cells mean bbox containment is exact —
no rings needed.

**wijkbuurt** — CBS Wijk- en Buurtkaart 2025: `gemeenten` (z6–8), `wijken`
(z9–10), `buurten` (z11–13) with kerncijfers. Rings stored → exact
administrative lookup for any coordinate.

**transit** — OVapi static GTFS (CC0, no key). CSVs are streamed out of the
234 MB zip (`unzip -p`, never extracted). `stops` points (stations from z8,
stops from z10) + `routes` lines for rail/tram/metro/ferry (bus shapes
excluded deliberately: they follow roads already on the map and triple the
size). Live vehicle positions are explicitly app-side (GTFS-RT), not here.

**buildings-age** — PDOK `bag-light.gpkg` (CC0, 7.8 GB, monthly). The one
layer that can't fit the in-memory tiler: `SpoolTiler` clips features in one
pass and spools compact records to per-(zoom, 256×256-block) disk files — NL
is only 5 blocks at z13/z14 — then loads one block at a time in writer rowid
order. 11.4M polygons → 20,229 tiles + an 11.4M-row sidecar in 91 s, peak
memory one block. MVT layer `bag`: bouwjaar, status.

**terrain** — Copernicus GLO-30 DSM COGs from anonymous AWS S3, 20 one-degree
cells for NL; all-ocean cells 404 upstream and are treated as sea level (2 of
20 did — as expected). Terrarium encoding is the shared contract: renderer
hillshade/3D terrain later, and `map_nav` samples the same file to bake
per-edge climb/descent for EV routing (the elevation thread from
`datasources.md`).

**noise** — RIVM "Geluid in Nederland" Lden all-sources 10 m GeoTIFF (CC0),
EPSG:28992 — sampled through the WGS84→RD inverse polynomial. dB values are
binned into 5 dB classes (1: <45 … 8: ≥75) at encode time; the classmap
metadata carries labels + colors. 23-minute build (≈800M bilinear samples of
a deflate-tiled national raster through the pure-Rust TIFF reader) — fine for
a yearly-refresh batch job.

**flood** — JRC Europe river flood hazard RP100 depth GeoTIFF (CC BY, WGS84
~90 m), binned to depth classes (<0.5 m … ≥4 m). Small and cheap. Follow-ups
noted: JRC's `spurious_depth_areas` mask, and PDOK's official ROR zone
polygons (CC0) as a vector companion.

**radar (live)** — `radar.rs`: KNMI Data Platform datasets `radar_forecast`
(+2 h nowcast, whole animation in one ~0.5 MB HDF5 file every 5 min; the
map-app default) and `radar_reflectivity_composites` (5-min frames, keeps
~1 h). `RadarSync::sync()` is safe to call arbitrarily often — network at
most once per `min_poll_secs` (240 s default), downloads only missing files,
prunes old ones, `state()` never touches the network. API key: config →
`KNMI_API_KEY` env → the documented shared anonymous key (dev-only; its
quota is shared — register a free personal key for real use).

## The query surface (LLM goal)

```rust
let mut db = query::LayerDb::open("local/overlays/nl-wijkbuurt.mbtiles")?;
db.query_point(4.8926, 52.3731, 10)?;       // exact point-in-polygon
db.query_radius(4.8926, 52.3731, 400., 5)?; // nearest-first + distances
db.query_bbox(min_lon, min_lat, max_lon, max_lat, 100)?;
```

Live-verified answers (via `geodata query`):
- Dam Square → gemeente **Amsterdam** (pop 934,526) → wijk
  **Burgwallen-Nieuwe Zijde** (4,345) → buurt **Nieuwe Kerk e.o.** (830,
  9,827 /km²) — exact containment via stored rings.
- Chargers near Dam: **Krasnapolsky at 128 m** (Eneco, 22 kW), with distances.
- Royal Palace → **bouwjaar 1655**, "Pand in gebruik" — out of 11.4M buildings.
- Population grid cell at Dam: 1,735 inhabitants (vk500).

The intended wiring: the map app exposes these as LLM tool calls (and as
tap-to-inspect UI). Every layer answers from the same three query shapes, and
attrs come back as JSON with the source's own column names.

## Infrastructure added to shared crates (all additive)

`libs/mbtile_reader`:
- `open_sqlite` / `schema_entries` / `for_each_row` — generic SQLite table
  reading; what lets GeoPackages be ingested with zero new dependencies.
- `for_each_row_in_range` — pruned b-tree descent for rowid ranges; the
  engine under the sidecar's spatial queries.
- Writer: `begin_extra_table` / `write_extra_row` (+ `WriterValue`, float and
  NULL record support) — arbitrary extra tables alongside `tiles`/`metadata`.
- All pre-existing writer/reader tests still pass; new outputs pass
  `PRAGMA integrity_check`.

`libs/geodata` internals worth knowing:
- `geo.rs` — RD New ↔ WGS84 both directions (Schreutelkamp/Strang van Hees
  polynomials), round-trip tested < 1 m at Amsterdam/Rotterdam/Maastricht/
  Groningen; `tile_order_key` replicating the writer's rowid order.
- `tiff.rs` — GeoTIFF subset: tiled + striped, deflate/LZW/uncompressed,
  predictors 1/2/3 (incl. the floating-point predictor), int/float samples,
  ModelPixelScale/Tiepoint georeferencing, GDAL nodata, block cache. Proven
  against Copernicus COGs, the JRC Europe raster, and RIVM's RD raster.
- `png.rs` — encoder + decoder for gray8/RGB8 (hand-rolled CRC32, round-trip
  tested). `raster.rs` — sampler → 256 px tile pyramid.
- `mvt.rs` — spec-correct MVT 2.1 encoder with key/value dedup.
- `spool.rs` — the country-scale streaming tiler described above.

Test suite: 7 unit tests in geodata (projections, round-trips, ordering, PNG,
ISO-8601), 9 in mbtile_reader. Byte-level MVT/gzip verification and SQLite
integrity checks run against every produced artifact.

## Open follow-ups (in rough priority order)

1. **Radar HDF5 → raster decode** so frames render (and a small
   `RadarFrame::to_grid()` for querying "rain at my location in 30 min");
   OPERA/MeteoGate (CC-BY COGs) after that for Europe-wide radar.
2. **Raster point-query helper** in `query.rs` (PNG-decode a tile, return
   elevation / class + classmap label) — completes the LLM surface for
   terrain/noise/flood ("how high / how loud / what flood depth here").
3. **EV grade baking**: map_nav's `nav-build` samples `nl-terrain.mbtiles`
   per edge (the datasources.md EV thread).
4. Flood vector companion (PDOK ROR, CC0) + JRC spurious-areas mask.
5. Scale-out to Europe per country as sources allow (the machinery is
   region-agnostic; NL-specific parts are just the RD transform and source
   URLs).
6. Minor: neighbor-tile point buffering, Douglas-Peucker simplification,
   BigTIFF, replacing shelled-out `unzip`.

## Verification ledger

- All 9 mbtiles: `PRAGMA integrity_check` = ok (validates the hand-rolled
  SQLite writer end-to-end, incl. extra tables and >1 GB files).
- MVT: tile payloads gzip-wrapped (renderer sniff), protobuf structure
  byte-checked, winding per spec.
- Geo math: RD origin exact; RD↔WGS84 round trip < 1 m ×4 cities; Amsterdam
  sanity; ISO-8601 epoch math vs hand-computed constants.
- Content spot-checks: Royal Palace 1655; Dam admin hierarchy; charger
  distances; A10 noise spine; terrain cell coverage 18/20 with sea handling.
- Radar: real nowcast downloaded; poll gate verified (second call → zero
  network); 429 backoff exercised against the live saturated anonymous tier.
