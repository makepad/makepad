# Makepad map renderer repair plan

Date: 2026-07-27

## Goal

Bring `makepad-example-map` to an OpenStreetMap Standard-like quality bar, especially when
overzooming the local Shortbread tiles from source zoom 14 to view zooms 15–17:

- show the building footprints and other data that are already present in the MBTiles;
- preserve polygon and line geometry instead of exposing source-resolution shortcuts;
- keep road widths, joins, caps, dashes, and antialiasing well behaved at every view zoom;
- use deterministic cartographic layer ordering;
- remove cracks, spikes, overdraw fragments, and tile-boundary artifacts;
- add enough diagnostics and fixtures that later style work cannot silently regress geometry.
- retain the raw Simple 3D Buildings attributes needed for optional 2.5D extrusion.

The OSM.org screenshot is the visual reference, not a requirement to reproduce every
OpenStreetMap Carto feature or pixel. Shortbread is intentionally a leaner schema, so it is
the compact visual base rather than the completeness source. A second all-tag detail archive
is required for benches, playgrounds, uncommon tags, and features outside Shortbread.

Current execution scope is conversion tooling and data validation only. Do not launch
`makepad-example-map`; visual/runtime work below is the follow-up plan.

## What the investigation found

### 1. The buildings are present; most are being hidden

The local source is not missing Amsterdam's building data. For the tile around the supplied
street-level screenshot, the generated cache contains:

| Tile `z14/x8414/y5384` | Ways/rings tagged with the layer |
| --- | ---: |
| `buildings` | 6,880 |
| `water_polygons` | 99 |
| `land` | 69 |
| `sites` | 50 |
| all ways | 10,581 |

The tile's feature runs arrive in this order:

```text
water lines → piers → bridges → street polygons → streets → labels
→ buildings → water polygons → land → sites
```

The fill builder in [`tile.rs`](widgets/src/map/tile.rs) preserves encounter order and
increments `fill_zbias` for every polygon. That paints `water_polygons`, `land`, and `sites`
after `buildings`. Shortbread explicitly describes `land` as a base layer drawn first and
`sites` as above land but below buildings. The current order is the reverse of that contract.

This explains the large pale blocks and isolated dark building fragments in the close-up:
residential land/site polygons cover the building footprints, with only pieces left visible
outside the covering polygon.

### 2. Fine geometry is deliberately discarded before tessellation

`project_way_points_with_nodes` converts each point to global `f32` world coordinates and
drops any point less than `0.5` source-tile units from the last retained point:

```rust
if dx * dx + dy * dy < 0.25 {
    continue;
}
```

That shortcut is almost invisible at source zoom 14, which agrees with the observation that
the startup-level view looks acceptable. At view zoom 17 the same discarded detail is up to
four logical pixels wide.

For cached tile `z14/x8414/y5384`, the current filter sees:

| Segment result | Count |
| --- | ---: |
| retained | 58,273 |
| non-zero segments below `0.5` units and discarded | 22,512 |
| already collapsed to zero length | 9,125 |
| discarded non-zero building segments alone | 17,975 |

This is a primary cause of clipped corners, diagonal shortcuts, malformed narrow buildings,
and small notches becoming conspicuous when overzoomed.

### 3. Overzoom scales baked vector styling instead of only map positions

Shortbread's maximum tile zoom is intentionally 14; higher client zooms are expected to use
overzoomed z14 data. The current renderer tessellates fills and strokes once at tile zoom and
then applies:

```text
map_scale = 2^(view_zoom - tile_zoom)
```

to the entire finished mesh. At view zoom 17 this is `8×`, so it also magnifies:

- road casing and center widths by `8×`;
- the fill and stroke antialias fringe from roughly one unit to roughly eight units;
- butt-cap and join defects;
- source-level clipping and simplification errors.

A production vector renderer scales centerline/polygon positions with the map, but evaluates
style widths and antialias coverage in screen space using zoom-dependent style stops. This is
why OSM's roads remain controlled at street level while the current roads become broad masks
over surrounding detail.

### 4. Several Shortbread layers are dropped or interpreted as the wrong primitive

| Shortbread layer/data | Current behavior | Required behavior |
| --- | --- | --- |
| `buildings` | filled, then commonly overpainted; no high-zoom outline | above land/sites, with a thin outline from z15 |
| `land` | mapped to generic landuse/leisure | base land pass, first |
| `sites` | coerced into generic landuse/leisure | dedicated pass above land and below buildings |
| `street_polygons` | normalized as a highway, then explicitly excluded from strokes; never filled | road/pedestrian area fill with correct bridge/tunnel order |
| `bridges` | normalized as a highway, then explicitly excluded | structure/casing and road pass above water/normal roads |
| `dam_polygons`, `pier_polygons` | not normalized, therefore dropped | structure fills |
| `dam_lines`, `pier_lines` | treated as generic waterways | structure strokes, not rivers |
| rail features in `streets` | `rail=true` is ignored and the feature becomes a highway | railway styling based on `rail`/`kind` |
| `boundaries` | dropped | zoom-dependent boundary strokes |
| `place_labels`, `boundary_labels`, water labels | dropped | point/line labels with hierarchy |
| `addresses`, most `pois` | dropped | house numbers/POIs at close zoom, subject to collision rules |
| aerialways, ferries, public transport | broadly coerced to highways or dropped | separate semantic styles |

The style API currently supports only building/water/landuse/leisure fills plus a few line
families, so the decoder is forced to invent OSM-like tags and loses the source schema's
meaning.

### 5. The MVT path is needlessly lossy and obscures topology

Local tile handling currently performs:

```text
MVT integer geometry
→ tile-local lon/lat
→ generated Overpass-shaped JSON rounded to 8 decimals
→ JSON parse
→ Web Mercator global coordinates
→ f32
→ generic WayData
→ reconstruct feature/ring identity from private string tags
```

This:

- discards the typed MVT layer and geometry model too early;
- creates a new node for every path point;
- duplicates polygon closure points;
- converts exact tile integers through two coordinate systems and text;
- makes paint order depend on protobuf layer order;
- forces polygon identity and ring order through `__mp_*` string tags;
- makes it difficult to distinguish true endpoints from tile-clipped endpoints.

MVT already provides the exact tile-local integer coordinates, extent, geometry type, layer,
feature id, ring order, tags, and per-layer feature order needed by the renderer.

### 6. Polygon and tile-edge handling needs hardening

The current ring grouping assumes that the first ring's winding establishes the exterior
winding and that subsequent ring order is valid. That is normally valid for conforming MVT,
but the data is first reconstructed through the JSON path and then globally quantized.
There is also a silent 500-ring truncation named `EARCUT_MAX_RINGS`, although the active fill
primitive is the sweep tessellator.

Fills are not clipped by the map code to a consistent tile-local buffered rectangle. MVT
geometry is allowed to extend outside its tile as a render buffer, so neighboring tiles can
paint overlapping, independently tessellated fragments. Artificial tile-cut edges also get
the same magnified AA fringe as real polygon boundaries. This can create seams or wedges even
after feature ordering is fixed.

### 7. Coordinate precision is a contributor, but should be isolated rather than assumed

The more strongly evidenced primary faults are paint order, point removal, semantic mapping,
and scaled primitives. Precision is still measurable:

- MVT extent 4096 provides a `1/16` tile-unit grid at z14.
- Global z14 Amsterdam X coordinates are around 2.15 million, where `f32` spacing is `0.25`.
- In a sample of the problem tile, converting projected positions to global `f32` produces a
  median `0.5` and maximum `1.0` logical-pixel position error after `8×` overzoom, before the
  GPU multiply/large-offset cancellation.

The fix is useful regardless of how visible this individual error is: keep geometry
tile-local and compute only the small tile-to-viewport translation in `f64` on the CPU.
Validate this with an A/B capture rather than treating it as the sole explanation.

### 8. Overzoom also requests far too many source tiles

`visible_tile_keys` computes the center in request-zoom coordinates but subtracts the
viewport's screen-pixel half-size without dividing by the overzoom scale. At view z17 with
request z14, it considers an area eight times too wide and eight times too tall. This causes
excess decoding, tessellation, GPU geometry, and cache growth. It does not directly deform
shapes, but it will make the correct high-detail renderer unnecessarily expensive.

There is also a duplicate push in `fill_ready_descendants`, and local MBTiles batches scan an
entire zoom level instead of querying only the requested `(x, y)` rows.

### 9. The raw Europe extract and the map-ready tile source are different artifacts

The current Geofabrik Europe extract is a 34,701,836,396-byte OSM PBF. It is the right
archival/source dataset, but it is not directly usable by the map widget: producing a good
z0–14 pyramid also requires Shortbread schema interpretation, zoom-dependent filtering,
generalisation, clipping, and MVT encoding. Treating that pipeline as a mere container
conversion would recreate the same data-interpretation bugs in a different tool.

Geofabrik currently publishes experimental Shortbread MBTiles for individual European
regions, but not one current Europe-wide MBTiles file. VersaTiles publishes the same kind of
ready-made Shortbread MVT pyramid for the full planet. The pinned 2026-06-08 archive is
66,534,652,244 bytes, covers z0–14, and stores Brotli-compressed tiles in an indexed
`.versatiles` container.

The practical local pipeline has two independently tiled products:

```text
Geofabrik europe-260726.osm.pbf
    canonical raw OSM source, retained and checksum-verified
        → native Rust multi-pass converter at source z14
        → compressed disk-backed node/way indexes
        → buffered integer clipping and external tile-block sorting
        → gzip MVT with original IDs, object types, and every source tag
        → indexed local/maps/europe-osm-detail.mbtiles

VersaTiles osm.20260608.versatiles
    official prebuilt Shortbread z0–14 pyramid
        → select the Europe bounding rectangle block-by-block
        → transcode selected Brotli MVT payloads to gzip MVT
        → stream them into local/maps/europe-shortbread.mbtiles
```

The base archive supplies inexpensive low-zoom cartography and established semantic layers.
It must never be described as a complete conversion of raw OSM. The detail profile separately:

- processes every tagged node and every way;
- copies every source tag without a whitelist;
- carries the original OSM feature ID and explicit node/way/relation type;
- disables feature combining, feature limits, simplification, and low-zoom generalisation;
- preserves every tagged way as a line and also writes closed ways as polygons, so conversion
  does not make an irreversible `area` interpretation;
- accepts tagged relations and writes those for which spatial geometry can be assembled.
- retains `building:part`, explicit/minimum height, level counts, roof geometry,
  materials, and colours verbatim for a later 2.5D mesh builder.

MVT cannot encode an untagged support node or a relation that has no standalone spatial
geometry. The verified PBF therefore remains the canonical lossless artifact and is never
deleted after conversion. “All data available” means every tagged spatial element is
demand-streamable from detail tiles, while the PBF retains the complete original graph,
including relation membership.

### 10. MBTiles can be both standard and efficient for this app

MBTiles 1.3 uses a SQLite `metadata` interface and a `tiles` interface with TMS-oriented Y
rows. Its conventional `(zoom_level, tile_column, tile_row)` index is optional. The existing
minimal reader did not use its parsed index and `get_tiles_at_zoom` scanned an entire zoom,
which is not viable for Europe.

The new pure-Rust writer keeps the standard tables and metadata, but orders tile-table rowids
deterministically by zoom, 256×256 block, local Y, and local X. A private metadata marker lets
the Makepad reader compute the rowid and descend the table B-tree directly in `O(log n)`.
Other SQLite/MBTiles readers still see an ordinary, valid MBTiles database; legacy archives
remain readable through the compatibility scan path.

The native raw-detail converter uses the same deterministic Makepad block rowids as the
Shortbread extractor. The reader computes the rowid and descends the tiles-table B-tree
directly. It also understands conventional third-party
`(zoom_level, tile_column, tile_row)` indexes. Both layouts therefore provide direct
`O(log n)` cold lookup; an index-free legacy archive alone uses the compatibility scan.

The writer is streaming:

- it never holds the Europe tile payload set in memory;
- it writes 64 KiB SQLite table, interior, and overflow pages directly;
- it converts XYZ input to standard TMS rows;
- it preserves gzip-compressed MVT blobs without decoding them again in the app;
- it validates monotonic row order and refuses malformed coordinates;
- release tests exercise overflow chains, multi-level B-trees, direct lookup, and a complete
  synthetic VersaTiles-to-MBTiles conversion.

The pinned real archive also passes a no-write planning scan. The Europe rectangle selects
483 of 4,853 blocks, 24,564,277 tiles, and 24.39 GiB of Brotli-compressed source payload.
This scan reads only the container indexes; it does not create an output file or load tile
payloads into memory. Gzip MBTiles output will be larger than that Brotli payload total, and
the all-tag detail build needs substantially more temporary SSD space.

### 11. The all-tag compiler is now native Rust and restartable at the expensive boundary

Tilemaker 3.1.0 source was pinned at commit
`e16203e4e2fb38a11580621fc0503ef463ab849f` and inspected only as an architectural
reference; it was not built or run and is not a runtime/build dependency. The replacement
Rust path performs five bounded-memory stages:

```text
relations → bounded relation-way membership bitset, then persist it
nodes     → one-time projection + compressed coordinate chunks + direct group index
ways      → direct node resolution + compressed relation-way references
relations → member stitching + multipolygon outer/hole assignment
spools    → external block sort → gzip MVT → streaming pure-Rust MBTiles
```

The PBF advertises `Sort.Type_then_ID`, which makes these sequential passes possible without
a continent-sized in-memory object graph. Geometry is projected once to integer z14 MVT
coordinates, exact consecutive duplicates alone are removed, and line/polygon clipping uses
a consistent 64-unit buffer. Tile records are spooled per 256×256 block with only 32 open
files and externally sorted with a configurable memory ceiling.

Every original tag stays a string. This matters for 2.5D data because OSM heights can carry
units and level/roof attributes are not interchangeable. The output metadata names the
supported 2.5D keys, while the renderer can normalize them once during mesh preparation:

```text
building, building:part
height, min_height
building:levels, building:min_level
roof:shape, roof:height, roof:levels
roof:direction, roof:orientation, roof:angle
building:material, building:colour
roof:material, roof:colour
```

After passes 1–4, an atomic `spool.complete.json` marker and source audit are written. If the
final MBTiles pass is interrupted, rerunning with the same source/store validates the source
size and zoom and restarts only external sorting/MBTiles writing. Incomplete earlier stores
are refused rather than guessed at.

A real Monaco Geofabrik smoke conversion produced 109 z14 tiles from 41,524 nodes, 6,209
ways, and 348 relations. SQLite `quick_check` passed, a cold direct lookup succeeded, all six
MVT layers decoded, and the largest tile contained actual height, building-level,
building-part, roof, material, and colour keys. Re-running the final pass from completed
scratch produced a byte-identical MBTiles file.

The completed Europe source audit contains 3,792,033,607 nodes, 464,000,109 ways,
4,793,955,052 way-node references, and 1,840,402,362 tags. It specifically finds:

| Europe PBF feature tag | Count |
| --- | ---: |
| `amenity=bench` | 2,696,309 |
| `leisure=playground` | 618,114 |
| `building` | 251,162,480 |
| `building:part` | 3,126,026 |
| `height` | 5,457,826 |
| `min_height` | 177,798 |
| `building:levels` | 25,386,641 |
| `building:min_level` | 139,202 |
| `roof:shape` | 7,431,109 |
| `roof:height` | 349,899 |
| `roof:levels` | 4,559,236 |

The optimized release converter was then benchmarked on the 47,228,875-byte Luxembourg
extract: 4,208,128 nodes, 6,313,908 way-node references, and 1,617,855 tile records became
3,953 validated tiles in 12.3 seconds. Pass 3 sustained about 1.29 million resolved
references/second. Scaling each measured pass by the audited Europe counts gives a
hardware-specific preflight estimate of roughly 2–3 hours, with output remaining
byte-identical before and after the optimization pass.

### 12. Why the OpenStreetMap.org reference looks substantially richer

The Standard layer on openstreetmap.org is OpenStreetMap Carto, a Mapnik raster rendering
stack over a PostgreSQL/PostGIS database imported and preprocessed by `osm2pgsql`. Its
project layers contain explicit SQL feature selection, CartoCSS styling, and deliberate
render order. Buildings have a dedicated source/style and gain an outline at close zoom;
shops, roads, tunnels, landcover, and labels occupy intentionally separate passes.

That is materially different from the current Makepad path:

```text
OSM Standard:
raw OSM → rendering-specific PostGIS schema → per-layer SQL/style → Mapnik raster tile

current Makepad:
Shortbread z14 MVT → generic OSM-like tag coercion → encounter-order meshes
                    → scale the complete z14 primitive when overzooming
```

So the reference's house coverage is not explained by precision or palette. It comes from
retaining the building source, giving it a dedicated paint pass, applying zoom-specific
styling, and rendering primitives for the requested display zoom. The local Shortbread
tiles already contain thousands of those footprints; the renderer presently hides or
deforms many of them.

## Implementation plan

### Dataset and container pipeline — utilities ready; large conversions not started

- [x] Pin the Geofabrik Europe PBF and VersaTiles snapshot instead of downloading mutable
`latest` URLs.
- [x] Add resumable downloads, publisher checksum verification, partial filenames, and
atomic final renames to `download_map.sh`.
- [x] Download and MD5-verify `local/maps/europe-260726.osm.pbf` (34,701,836,396 bytes).
- [x] Download and SHA-256-verify `local/maps/osm.20260608.versatiles`
(66,534,652,244 bytes).
- [x] Extend `makepad-mbtile-reader` with a pure-Rust streaming SQLite/MBTiles writer and
deterministic direct tile lookup.
- [x] Add `makepad-map-tiles`, a Rust extractor that reads VersaTiles v02 block/tile indexes,
selects Europe through z14, transcodes Brotli MVT to standards-compliant gzip PBF, and writes
MBTiles.
- [x] Implement the all-tag raw-PBF compiler entirely in Rust. It retains IDs/types/all tags,
emits both line and polygon interpretations for closed ways, assembles spatial relations,
clips in integer tile coordinates, externally sorts bounded tile-block spools, and performs
no geometry simplification.
- [x] Add a parallel streaming PBF audit command and an indexed-MBTiles direct probe command
to `makepad-map-tiles`; neither loads the Europe dataset or tile table into memory.
- [x] Audit the 2.5D building family explicitly while copying all source tags verbatim:
heights/min-heights, levels/min-levels, building parts, roof geometry, materials, and colours.
- [x] Extend `makepad-mbtile-reader` to perform direct lookup through standard third-party
MBTiles composite indexes as well as Makepad's deterministic rowids.
- [x] Change the local map loader to use direct requested-tile lookups for Makepad-authored
files while retaining the legacy full-zoom compatibility path.
- [x] Treat MBTiles itself as the local disk cache: do not duplicate local tiles into
millions of generated JSON files, sort visible requests into on-disk block order, and keep
B-tree page reuse in a bounded 32-page cache.
- [x] Add and run a no-write planning scan against the real tiled source; it validates the
v02 indexes and reports the exact Europe selection before committing disk space.
- [x] Smoke-convert 23 real Amsterdam tiles, pass `PRAGMA quick_check`, and directly fetch
gzip tile `14/8414/5384` through the Makepad rowid path.
- [x] Convert a real raw Monaco PBF through every native pass; validate 109 gzip-MVT tiles,
all six detail layers, direct lookup, actual 2.5D tag keys, dynamic source bounds, and a
byte-identical final-pass restart.
- [ ] Produce `local/maps/europe-shortbread.mbtiles`, run SQLite integrity/schema checks, and
verify representative Amsterdam z14 tiles and layer counts.
- [ ] Produce `local/maps/europe-osm-detail.mbtiles` on a volume with sufficient SSD
workspace; verify the tile index, metadata, and representative `amenity=bench` and
`leisure=playground` features against the source PBF.
- [ ] Add a PBF-versus-detail audit report: tagged node/way/spatial-relation totals, feature
IDs, tag-key/value samples, and explicit counts for benches, playgrounds, addresses, and
buildings. A mismatch must fail conversion validation rather than be treated as styling.
- [ ] UI launch and screenshot comparison are deliberately deferred; the current task stops
at converter/database validation.
- [ ] If a fully native *curated Shortbread pyramid* compiler is later desired, scope its
zoom-dependent schema mapping and generalisation as a separate product. The completed native
all-tag detail compiler intentionally avoids those irreversible cartographic decisions.

### Prepared commands

Run these independently from the repository root:

```sh
./download_map.sh raw       # checksum-pinned 34.7 GB Europe OSM PBF
./download_map.sh tiles     # checksum-pinned 66.5 GB planet Shortbread source
./download_map.sh plan      # validate indexes and estimate Europe selection; writes nothing
./download_map.sh audit     # bounded-memory source feature/tag manifest
./download_map.sh convert   # streaming Europe Shortbread MBTiles build

# Put the bounded native scratch store on a spacious fast SSD.
DETAIL_STORE=/external/ssd/native-detail-europe.store \
DETAIL_SORT_MEMORY_MIB=256 \
./download_map.sh detail

./download_map.sh verify
```

`convert` and `detail` write to distinct `.partial.mbtiles` files, validate before atomic
rename, and refuse to overwrite stale partial results. The raw PBF is retained after both.

### Phase 0 — Lock down evidence and visual regressions

- [ ] Add a small raw-MVT fixture and synthetic fixtures for:
  - a polygon with a hole;
  - a multipolygon with two exteriors and holes;
  - rings touching/continuing through a tile buffer;
  - short building edges below `0.5` tile units;
  - road junctions, dead ends, bridges, and tunnels.
- [ ] Add decoder assertions for layer name, geometry type, extent, feature id, original
  feature order, tags, ring count, and point count.
- [ ] Add a diagnostic layer counter. A valid feature must be rendered, intentionally
  filtered by a named style rule, or counted as rejected with a reason.
- [ ] Add temporary debug toggles for one-layer-at-a-time rendering, tile boundaries, ring
  winding, and primitive wireframes.
- [ ] Define fixed Amsterdam camera bookmarks at z14, z15, z16, and z17, including the
  Bloemgracht/Rozengracht area from the screenshots.
- [ ] Capture baselines at device scales 1× and 2× through the release Studio `RunItem` flow.

### Phase 1 — Restore the data already present

- [ ] Introduce an explicit semantic paint order independent of MVT protobuf order and
`HashMap` iteration. A starting order is:

  ```text
  background/ocean
  → base land
  → water polygons
  → sites/landuse overlays
  → buildings
  → dams/piers/street areas
  → tunnels
  → ordinary waterways/rail/roads
  → bridges and bridge roads
  → boundaries
  → labels/icons
  ```

- [ ] Preserve the source feature index as a stable secondary sort key. Shortbread documents
some layers, especially streets, as already sorted for rendering.
- [ ] Render buildings after `land` and `sites`; add a restrained building outline at z15+.
- [ ] Give `sites`, `street_polygons`, piers, dams, and bridges explicit fill/stroke rules.
- [ ] Interpret `streets.rail` and `kind` before synthesizing a highway classification.
- [ ] Keep bridge, tunnel, link, and source z-order attributes through all batching stages.
- [ ] Stop grouping ordered strokes through an unordered `HashMap` unless the grouping key
also retains the complete paint order.

This phase should immediately reveal the thousands of building footprints already in the
local tile and remove the largest apparent “missing data” problem.

### Phase 2 — Replace the MVT-to-JSON round trip

- [ ] Decode local PBF directly into a typed model, for example:

  ```rust
  DecodedTile {
      layers: Vec<DecodedLayer {
          name,
          extent,
          features: Vec<DecodedFeature {
              id,
              source_order,
              geometry_type,
              tags,
              paths: Vec<Vec<IVec2>>,
          }>,
      }>,
  }
  ```

- [ ] Keep MVT coordinates as integers until geometry preparation; convert once to
tile-local `f32` coordinates such as `x * 256 / extent`.
- [ ] Keep the network/Overpass source as a separate adapter that emits the same typed
intermediate model rather than pretending local MVT is Overpass JSON.
- [ ] Cache raw source payloads or decoded typed data, not generated 8-decimal JSON.
- [ ] Version or remove `local/tilecache_v4` when this lands so stale lossy geometry cannot
mask the fix.

### Phase 3 — Make topology and clipping exact

- [ ] Remove the `< 0.5` radial point filter. Initially remove only exact consecutive
duplicates in integer tile coordinates.
- [ ] If simplification is needed later, use a topology-safe, view/style-aware tolerance and
never simplify building corners merely because they were subpixel at source z14.
- [ ] Group MVT polygon rings directly according to the MVT rule: positive tile-coordinate
area starts an exterior, following negative-area rings are its holes.
- [ ] Validate and report malformed rings rather than silently losing them.
- [ ] Replace the 500-ring truncation with a documented budget/fallback that never turns a
valid multipolygon into a different shape.
- [ ] Do all clipping in tile-local coordinates with a consistent render buffer.
- [ ] Distinguish true line endpoints from clipped endpoints so real dead ends can use the
intended cap while tile cuts remain seamless.
- [ ] Prevent AA on artificial internal tile cuts, or overlap/scissor buffered fill bodies in
a way that guarantees no transparent seam between neighboring tiles.
- [ ] Add cross-tile tests asserting that the same source edge meets at exactly the same
screen coordinate during fractional zoom and pan.

### Phase 4 — Use zoom-correct vector primitives

- [ ] Separate position scaling from style scaling. Overzoom map coordinates by `2^dz`, but
evaluate stroke width from the current view zoom.
- [ ] Add zoom stops/interpolation to road, rail, waterway, casing, outline, and label styles.
Do not obtain a z17 road by multiplying a z14 baked width by eight.
- [ ] Keep fill and stroke AA approximately one device pixel at every view zoom and DPI.
- [ ] Prefer GPU-side screen-space extrusion for line width/AA, or cache tessellation by a
small style-zoom bucket if the vertex format cannot yet represent centerline plus extrusion.
- [ ] Use the existing GPU-expandable fill approach, or an equivalent map-specific variant,
so fill fringes do not grow with `map_scale`.
- [ ] Rework road passes as deterministic casing/center layers with correct round joins,
true-end caps, intersections, bridge decks, and tunnel dashes.
- [ ] Preserve a stable line metric across features/tiles for dash phase.
- [ ] Remove the current center-overlay width workaround once the proper stroke primitive
makes seams unnecessary.

### Phase 5 — Reach the OSM-like cartographic quality bar

- [ ] Use the official Shortbread styles (especially VersaTiles Neutrino/Colorful) as the
semantic source-layer and zoom-stop reference; use OpenStreetMap Carto as the visual
hierarchy/palette reference.
- [ ] Cover every Shortbread layer intentionally. Keep a checked-in coverage table with
`rendered`, `label-only`, or `deliberately omitted` for each layer.
- [ ] Tune the light theme first:
  - visible, separately outlined buildings;
  - distinct but quiet residential/commercial/industrial land;
  - water without heavy magnified borders;
  - clear road class hierarchy;
  - correctly layered bridges, tunnels, rail, paths, piers, and pedestrian areas;
  - parks and sites that do not cover buildings.
- [ ] Then derive the dark theme from the same semantic order and zoom stops.
- [ ] Extend labels in this order: street labels, place hierarchy, water names, boundary
labels, house numbers at close zoom, then POIs/icons.
- [ ] Add road shields and multiline/ref handling only after base collision behavior is
stable.
- [ ] Keep label size in screen pixels and make repeat/collision distances zoom-aware.

### Phase 6 — Recover performance after correctness

- [ ] Fix visible-source-tile bounds by dividing viewport size by
`2^(view_zoom - request_zoom)`.
- [x] Query requested MBTiles rows directly for both Makepad-authored files and conventional
indexed MBTiles instead of reading all tiles at a zoom for every batch.
- [ ] Remove the duplicate descendant push.
- [ ] Separate decoded-tile caching from style/geometry caching so theme changes do not
redecode PBF.
- [ ] Keep one reader/file handle alive in the tile I/O worker across batches; retain a
strictly bounded page cache and cancel queued batches made stale by a newer camera epoch.
- [ ] Key prepared geometry by tile, style epoch, and only the zoom bucket actually needed.
- [ ] Measure decode, topology preparation, tessellation, upload, draw, and label placement
independently in release builds.
- [ ] Establish memory and frame-time budgets after the correct building count is visible;
do not regain speed by silently dropping geometry.

## Acceptance criteria

### Data and ordering

- The Shortbread base and all-tag detail archive have distinct metadata/source-kind markers;
no validation may infer raw-OSM completeness from Shortbread.
- Every tagged spatial feature selected from the PBF retains its original OSM ID, type, and
complete tag map in the detail archive. Benches and playgrounds are explicit audit fixtures.
- Building footprints/parts retain explicit height and minimum-height strings, level
fallbacks, roof geometry, materials, and colours; the renderer can extrude them without a
second database or lossy conversion.
- Closed ways remain available as source lines and polygon candidates; conversion does not
silently discard either interpretation.
- Every valid `buildings` polygon in the fixture tile is emitted or has a logged rejection
reason; land/site passes cannot cover the building pass.
- `street_polygons`, bridges, piers/dams, rail, and boundaries appear in their documented
semantic order.
- The renderer's output is deterministic across runs regardless of hash seed or worker order.

### Geometry

- Short building edges and corners remain present from z14 through z17.
- Holes and multipolygon islands render correctly.
- No spikes, diagonal shortcuts, transparent cracks, or doubled fringes appear at tile
boundaries.
- Continuous zoom and pan do not make shared edges separate or jump.
- A precision A/B confirms whether tile-local coordinates produce a visible improvement; the
architecture keeps them local either way.

### Primitives and style

- Road widths follow explicit zoom stops and do not grow by `2^dz`.
- Fill and line AA remains about one device pixel at 1× and 2× DPI.
- Junctions have continuous casings/centers; true endpoints and clipped endpoints have the
correct caps.
- Buildings are individually legible at z15–17 with a subtle outline comparable in hierarchy
to the OSM.org reference.

### Runtime

- At view z17/source z14, only intersecting z14 tiles plus the chosen prefetch ring are
requested.
- Opening the Europe archive must not enumerate its tiles. A cold visible batch reads only
the metadata B-tree, the requested tile B-tree paths, and those tiles' payload/overflow pages;
steady-state caches remain bounded independently of archive size.
- Conversion validation uses release CLI tests, publisher checksums, SQLite checks, direct
tile probes, and PBF-versus-MVT audits; it does not launch the example app.
- The corrected dense Amsterdam view remains interactive in release mode without geometry
loss as a performance shortcut.

## Recommended landing order

1. Diagnostics, fixture counts, and fixed camera screenshots.
2. Explicit paint order and source-layer coverage; confirm that buildings reappear.
3. Direct typed MVT decode; delete the JSON round trip and `< 0.5` point filter.
4. Tile-local topology, clipping, and cross-tile seam tests.
5. Screen-space line/fill primitives and zoom-dependent style stops.
6. Complete Shortbread styling and label hierarchy.
7. Tile selection, MBTiles query, cache, and profiling cleanup.

Do not start by palette-tuning the current meshes. Correct data interpretation, topology, and
screen-space primitive behavior first; otherwise style changes will merely hide defects that
reappear at another zoom or DPI.

## References

- [Shortbread schema 1.0](https://shortbread-tiles.org/schema/1.0/) — maxzoom/overzoom,
building coverage, documented layer meaning and feature sorting.
- [Shortbread map styles](https://shortbread-tiles.org/styles/) — schema-native reference
styles.
- [Mapbox Vector Tile 2.1 specification](https://github.com/mapbox/vector-tile-spec/blob/master/2.1/README.md)
— integer tile coordinates, extent buffers, geometry commands, and polygon winding/order.
- [OpenStreetMap Standard layer](https://wiki.openstreetmap.org/wiki/Standard_tile_layer) and
[OpenStreetMap Carto](https://github.com/openstreetmap-carto/openstreetmap-carto) — the
OSM.org raster rendering/style reference.
- [OpenStreetMap Carto building style](https://github.com/openstreetmap-carto/openstreetmap-carto/blob/master/style/buildings.mss)
— buildings from z14 and outlines from z15 in the reference style.
- [Geofabrik Europe extract](https://download.geofabrik.de/europe.html) — pinned raw OSM PBF
source and checksums.
- [VersaTiles downloads](https://download.versatiles.org/) and
[container overview](https://docs.versatiles.org/basics/versatiles_server.html) — official
Shortbread planet archive and byte-range-indexed storage model.
- [MBTiles 1.3 specification](https://github.com/mapbox/mbtiles-spec/blob/master/1.3/spec.md)
— required metadata/tiles interfaces, gzip PBF convention, TMS row orientation, and optional
tile index.
- [tilemaker](https://github.com/systemed/tilemaker) — source-only architectural reference
for disk-backed PBF conversion; the implemented converter does not build or invoke it.
- [Simple 3D Buildings](https://wiki.openstreetmap.org/wiki/Simple_3D_Buildings) — OSM
building-part, height/level, roof, material, and colour semantics retained by the detail
archive.
