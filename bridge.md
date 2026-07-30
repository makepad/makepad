# bridge.md — road elevation & grade separation for the 3D car-follow view

How we get overpasses, bridges, tunnels and stacked interchanges to render with
real depth in the perspective/ortho car-follow GPS view. Companion to `map.md`
(renderer), `gps.md` (interaction), `datasources.md` §2 (elevation sources; this
doc expands its "Road elevation / grade separation" subsection). Researched and
endpoint-checked 2026-07-29.

## 1. The data reality: OSM will not store road z

- **OSM has no plans to add elevation to road geometry — it's a settled
  position, not an oversight.** The data model is strictly 2D lat/lon. `ele=*`
  exists but is for summits/POIs; the wiki is explicit that OSM is not an
  elevation database. No active proposal changes this.
- **Overture landed in the same place.** Their transportation `level` property
  is relative stacking order only, documented as "an approximation for
  rendering, not a precise indication of elevation". Geometry is 2D.
- The freshest ecosystem effort (OSM2World's Prototype-Fund project, June 2026 —
  see §3) plans *around* the missing z, not toward adding it. At most it may
  propose new helper *tags*.

Conclusion: waiting for upstream z is not a strategy. Everyone derives it;
so will we — at tile/nav build time, baked into our own tiles.

## 2. What road geometry does carry (all already in our PBF)

| Tag | Meaning | Use for us |
|---|---|---|
| `layer=*` | relative stacking, −5..5, per way | the load-bearing input: crossing order |
| `bridge=*` / `tunnel=*` | way is elevated / buried; ways are split at the structure ends | where decks start/end; abutment anchors |
| `incline=*` | signed grade on a way (%, or up/down) | constraint on ramps where mapped |
| `maxheight=*` | legal clearance *under* a structure | lower bound on deck z at that crossing |
| `height=*`, `ele=*` on bridge ways | rare, occasionally present | direct evidence when it exists |
| `bridge:structure`, `man_made=bridge` areas | structure type / deck outline polygon | rendering style; polygon for AHN sampling (§5) |

`layer` is relative and only meaningful *at crossings* — two `layer=1` bridges
in different towns share nothing. Treat it as a partial order between ways whose
geometries intersect, never as an absolute height.

## 3. Prior art

- **OSM2World** is the reference implementation of derive-at-build: an elevation
  connector graph is initialized at terrain height, then a constraint pass
  enforces equal z at shared nodes, minimum vertical clearance where a higher
  `layer` crosses a lower one, tunnels below terrain, and grade
  smoothing/clamping (heat-equation smoothing was merged as PR #176).
- **OSM2World "OSM-3D-Terrain"** (Prototype Fund, roadmap 2026-06-03) is
  productizing exactly this into 3D tiles with LOD — and explicitly flags that
  elevation adjustments must not create discontinuities at tile boundaries.
  Same problem class as ours; worth tracking their output formats.
- **MapLibre / the open vector-map world**: nobody renders true grade-separated
  interchanges yet. Terrain drape squashes bridges onto the valley floor;
  Terrain3D work in MapLibre Native doesn't address elevated roads. Doing this
  properly is a visible differentiator for our renderer.
- Commercial nav stacks survey slope/z per link themselves — same conclusion
  from the other direction: the community map gives topology and stacking,
  measured/derived z comes from elsewhere.

## 4. The derivation recipe (tile/nav build, Europe-wide)

Runs offline in `tools/map_tiles` / `nav-build`; the renderer only ever sees
baked results.

1. **Init**: every road/rail vertex gets z = DEM height at (x,y)
   (Mapterhorn PMTiles / AHN, per `datasources.md` §2).
2. **Never sample the DEM mid-bridge or mid-tunnel.** Bridge/tunnel ways are
   split at the structure boundary, so the endpoints are the abutments:
   interpolate z between abutments (linear; spline if the deck is long).
   Sampling under the deck is the classic artifact — the bridge sagging into
   the river valley — and it equally poisons the per-edge climb/descent numbers
   baked into `region.graph` for EV routing (datasources.md, EV thread).
3. **Constraint pass** over crossing pairs and shared nodes:
   - shared node ⇒ equal z (ramps connect continuously);
   - `layer(a) > layer(b)` at a geometric crossing ⇒
     `z_a ≥ z_b + clearance`; default clearance ≈ 5.5 m per layer step
     (≥ 4.7 m legal clearance + ~0.8 m deck structure), overridden upward by
     `maxheight` where tagged;
   - `tunnel` ⇒ z ≤ terrain − cover (default ~2 m below surface at portals,
     deeper mid-tunnel is fine);
   - respect `incline=*` where mapped.
4. **Smooth + clamp**: grade smoothing along each way chain (moving
   average or heat-equation style), then clamp per class — motorway ≤ ~6%,
   links ≤ ~10%, service/other ≤ ~15%. Approach ramps to a lifted deck extend
   into the adjacent non-bridge ways; ease in/out (cosine) so decks don't pop.
5. **Tile-boundary continuity**: solve on a buffered neighborhood (same trick
   as our existing tile pipeline uses for geometry clipping) so a vertex near
   the edge gets the same z in both tiles.
6. **Bake**: per-vertex **Δz above DEM** (not absolute z), quantized (dm is
   plenty), on road/rail geometry in our tiles. Δz-above-DEM keeps the field
   independent of DEM version/resolution mismatches at render time and is 0 for
   ~99% of vertices ⇒ compresses to almost nothing.

## 5. NL upgrade: measured deck heights instead of solved ones

Where surveyed data exists, replace the constraint solution:

- **RWS DTB** (Digitaal Topografisch Bestand, open on PDOK): cm-grade surveyed
  x/y/z on lines/points/areas covering exactly the RWS-managed motorway network
  — i.e. precisely where the multi-level interchanges are. Z is in the PDOK
  atom shapefiles and (since recently) the WFS as well. Join to OSM ways by
  proximity+bearing, sample z along the DTB lines.
- **AHN** (open LiDAR): the **DSM keeps bridge decks, the DTM removes them** —
  so DSM − DTM over a bridge polygon (`man_made=bridge` from OSM, or BGT
  "overbruggingsdeel") ≈ deck height directly. The LAZ point clouds classify
  structures explicitly if we want it from the source.
- **Kadaster 3D Basisvoorziening** (open, 3dfier-generated): ready-made 3D
  terrain + LoD1 bridge decks as meshes — the ingest-meshes-instead-of-solving
  option, and a validation set for our solver either way.

Europe-wide the solver result stands; NL gets true-to-survey interchanges.
Validation loop: run the solver in NL, diff against DTB/AHN, tune clearance and
clamp constants until the residuals are boring, then trust it elsewhere.

## 6. Renderer integration

- Tiles carry Δz per road vertex; the renderer adds it on top of the same
  terrain drape it already uses (see map terrain/3D displacement architecture
  notes). Decks lift, tunnels sink, everything else stays glued to the drape.
- 2D/top-down mode keeps using `layer` draw order exactly as today — Δz only
  engages in tilted/3D camera modes, so this is purely additive.
- Depth: elevated decks get real depth-tested geometry; the existing
  bridge-casing styling (open item in the carto-quality list) becomes the deck
  side/edge treatment in 3D.
- What sells the effect: a drop shadow / AO blob under decks onto whatever is
  below, and portal treatment at tunnel mouths (clip or fade the tunnel line
  under terrain, keep a dimmed dashed hint in nav mode when the route goes
  through it).
- Car-follow camera: route position includes Δz, so the chase camera rides over
  the flyover instead of clipping through it; on stacked interchanges the
  route's own level is unambiguous because it comes from the graph edge, not
  from GPS altitude (which is too noisy to pick a deck).

## 7. Build order

- **M0 — fake it visually**: no DEM needed. Δz = `layer` × 5.5 m on
  bridge/tunnel ways with cosine ease-in/out extending into approach ways.
  Gets stacked interchanges reading correctly in one step; wrong absolute
  heights, right relative ones.
- **M1 — solve it**: recipe of §4 in the tile build, bake Δz-above-DEM,
  renderer consumes it. Requires the DEM ingest that hillshade (path B) needs
  anyway.
- **M2 — measure it (NL)**: DTB/AHN override per §5 + solver validation.
- **M3 — polish**: deck shadows/AO, tunnel portals, casing-as-deck-edge,
  `region.graph` climb/descent switched to the same z source.

## Sources (checked 2026-07-29)

- OSM wiki: [Altitude](https://wiki.openstreetmap.org/wiki/Altitude),
  [Key:ele](https://wiki.openstreetmap.org/wiki/Key:ele),
  [Key:layer](https://wiki.openstreetmap.org/wiki/Key:layer),
  [Key:maxheight](https://wiki.openstreetmap.org/wiki/Key:maxheight)
- OSM2World: [OSM-3D-Terrain Prototype-Fund roadmap (2026-06-03)](https://osm2world.org/blog/2026/06/03/ptf-roadmap-2026-osm-3d-terrain/),
  [elevation smoothing PR #176](https://github.com/tordanik/OSM2World/pull/176)
- Overture: [shape & connectivity](https://docs.overturemaps.org/schema/concepts/by-theme/transportation/shape-connectivity/),
  [segments & connectors](https://docs.overturemaps.org/guides/transportation/segments-and-connectors/)
- NL: [RWS DTB on PDOK](https://www.pdok.nl/introductie/-/article/digitaal-topografisch-bestand-dtb-),
  [DTB open data (RWS)](https://rijkswaterstaat.nl/zakelijk/open-data/digitaal-topografisch-bestand),
  [AHN DTM](https://data.overheid.nl/dataset/47567-actueel-hoogtebestand-nederland--ahn--dtm),
  [AHN DSM (AHN4)](https://data.overheid.nl/dataset/36461-actueel-hoogtebestand-nederland-dsm--ahn4-),
  [Kadaster 3D Basisvoorziening](https://www.kadaster.nl/zakelijk/producten/geo-informatie/3d-producten/3d-basisvoorziening)
- Renderer gap: [MapLibre Terrain3D roadmap](https://maplibre.org/roadmap/maplibre-native/terrain3d/)
