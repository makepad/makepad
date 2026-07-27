# map2 — why the map falls apart when you zoom in, and what's missing to fix it

Analysis of `makepad-example-map` render quality at high zoom (Amsterdam, z16–17),
compared against the osm.org (openstreetmap-carto / Mapnik) reference render of the
same area. Screenshot symptoms: roads that change width mid-street, butt-capped
stubs ending in the middle of the screen, a canal that breaks and shifts sideways,
wobbly/lumpy road edges, dark casing bars crossing junctions and tile seams.

TL;DR: the dataset is *not* the main problem. The renderer magnifies z14 geometry
that was tessellated once, in absolute-Mercator f32 coordinates, with widths, AA,
tolerance and layer-ordering all baked in at tile-build time. Almost every visible
artifact traces back to one of five root causes below. Two of them are genuinely
missing bits in the vector API (round joins, GPU-side stroke width); the other
three are map-widget architecture and need no API change at all.

---

## 1. What the pipeline does today

- Data source: `noord-holland-shortbread-1.0.mbtiles` — Geofabrik Shortbread 1.0
  vector tiles, **maxzoom 14** (verified from the metadata table). `streets` and
  `buildings` layers only exist at z14. View zoom goes to 17
  (`view.rs` `max_zoom: 17.0`), but `request_zoom_level()` clamps tile requests to
  z14 (`view.rs:1648`). So everything past z14 is a **4–8× magnification of z14
  tiles**.
- Tile build (`tile.rs build_tile_buffers_from_body`): MVT → fake Overpass JSON →
  re-projected to **absolute web-Mercator world pixels at the tile zoom** as `f32`
  (`lon_lat_to_world`, `geometry.rs:817`). Strokes are merged per style
  (`merge_stroke_polylines`), clipped to tile+8px (`ROAD_CLIP_PADDING`, `tile.rs:27`),
  then tessellated on the CPU (`libs/svg/src/tessellate.rs`) into two cached GPU
  buffers per tile: one fill geometry, one stroke geometry. Stroke widths, the 1px
  AA fringe, the 0.25px flatten tolerance and the dash period are all in
  **tile-zoom world units**, fixed forever.
- Draw (`view.rs draw_walk`): two loops — all tiles' fills, then all tiles'
  strokes — each tile as one draw call with `map_scale = 2^(view_zoom − tile_z)`
  and a shared `map_offset`, transformed in the vertex shader
  (`DrawMapVector`, `view.rs:21`).

---

## 2. Why the osm.org reference looks so much better with "the same" data

It's the same OSM *source*, but not the same render-time inputs:

1. **osm-carto re-renders every zoom level from the full database.** At z16 it has
   z16-selected geometry, z16-tuned widths, z16 label sizes. Our z16 is a scaled
   photograph of a z14 rendering. This is the single biggest difference.
2. **Global layer compositing.** Mapnik draws *all* road casings in the map, then
   *all* road fills, per style layer, across the whole image. We interleave
   casing/center per tile (see §3.3), so tiles stamp over each other at seams.
3. **Screen-space-constant styling.** Carto widths/dashes/halos are defined in
   output pixels per zoom. Ours are frozen world units that scale with
   magnification.
4. **Full-detail data past z14.** Shortbread z14 is already simplified and
   quantized to a 4096 extent. However: 4096 quanta over a 256-world-px tile is
   1/16 world px = **0.5 screen px at z17**. The *data* would support a crisp z17
   render — the artifacts we see (2–8 px scale) are renderer-made, not data-made.

So "the dataset is supposedly the same" is true enough that it's not the excuse:
the ceiling imposed by shortbread-z14 at z17 is ~0.5px; we're leaving ~10× that on
the floor.

---

## 3. Root causes, ranked by visual damage

### 3.1 f32 absolute-Mercator coordinates (precision) — the lumpy edges & offset seams

Geometry is stored as world pixels at tile zoom. Amsterdam at z14:
`x_world ≈ 0.5136 × 4,194,304 ≈ 2,154,000`, which sits in `[2^21, 2^22)` →
**f32 ULP = 0.25 world px**. Every projected vertex is snapped to a 0.25px grid
*before* tessellation; at z17 (×8) that's a **2 screen px quantization** on every
vertex. All the join/normal math in the tessellator then runs on these coarse
inputs (short segments get visibly wrong normals — extra wobble in extrusions).

It gets worse in the shader (`view.rs:28`): `pos * map_scale + map_offset` in f32.
At z17, `pos * scale ≈ 1.7e7 ∈ [2^24, 2^25)` → **ULP = 2 screen px** on both terms
of a catastrophic cancellation. Net: final vertex positions carry up to ~2–4 px of
position-dependent error, and `map_offset` (same magnitude) rounds while panning →
pan jitter and per-vertex "shimmer" of edges.

This alone explains the wobbly Nassaukade edges and most of the "doesn't fit
together" impression. It is the classic web-mercator-in-float problem; every GPU
map renderer (MapLibre etc.) solves it with **tile-local coordinates** (0..256 —
exact in f32) plus a per-tile translation computed in f64 on the CPU. Our shader
already takes per-draw-call `map_scale`/`map_offset` uniforms — the fix is purely
to make geometry tile-local and compute `offset + tile_origin × scale` in f64 per
tile. No vector-API change needed.

### 3.2 Overzoom: everything styled in baked tile-zoom units

Because tiles stop at z14 and buffers are cached per tile only (never per view
zoom), at z17 all of these are magnified ×8:

- **Stroke widths** (`view.rs:139` style table, applied at `tile.rs` build time):
  residential casing 2.0 world px → 16 screen px. Roads look bloated and toy-like.
- **AA fringes**: strokes are expanded by `aa/2 = 0.5` world px per side
  (`tessellate.rs stroke`, `hw = w/2 + aa/2`) → roads are drawn 8 screen px wider
  than styled at z17 (the fragment AA itself stays crisp thanks to
  `fwidth`-based coverage in `draw_vector.rs:251` — the *edge position* is what's
  wrong, not the sharpness).
- **Fill fringe bloat**: fills push their AA fringe outward by `woff = 0.5` world
  px (`tessellate.rs fill`, non-GPU-expand path) → every polygon (water, buildings,
  landuse) grows ~4 screen px at z17. Small courtyards close up; unrelated fills
  touch.
- **Flatten tolerance** `DEFAULT_FLATTEN_TOLERANCE = 0.25` world px
  (`geometry.rs:12`) → 2px chords on curves.
- **Vertex thinning**: `project_way_points_with_nodes` drops any point closer than
  0.5 world px to the previous one (`tile.rs:580`, `< 0.25` squared) → up to 4
  screen px of curve detail discarded at z17.
- **Dash periods** (rail `shape_id 10`, tunnels `11`): `v_stroke_dist` is baked in
  world units, so dashes stretch ×8 (`view.rs get_stroke_mask` / `draw_vector.rs
  dash()` — the shader never sees `map_scale` for distances).
- **Clip padding**: `ROAD_CLIP_PADDING = 8` world px becomes a 64-screen-px
  overlap strip along every tile edge at z17, which supersizes the seam problem
  below.

### 3.3 Per-tile painter's order breaks casing/center layering at seams

Within a tile, `tile.rs` draws correctly: pass 1 all casings, pass 2 all centers
(`tile.rs:474–548`). But both passes live in **one geometry buffer per tile**, and
tiles are drawn one draw call after another (`view.rs:517`). In the ±8 world px
overlap strip that both neighbors tessellate, the later tile's **casing paints on
top of the earlier tile's road center** — a casing-colored plug across every road
at every tile boundary, 64 screen px long at z17. The same mechanism produces the
dark bars across the canal (waterway casing) in the screenshot. The
`ROAD_CENTER_OVERLAY_*` constants (`tile.rs:29–32`) are an existing partial hack
around the intra-tile version of this; they can't help across tiles.

Related: merged stroke chains end with **butt caps** at junctions where degree ≠ 2
or the style changes (`merge_stroke_polylines` only chains degree-2 endpoints), so
T-junctions get notches and overlapping wedges, magnified ×8.

### 3.4 Mixed-zoom fallback tiles drawn side by side

`fill_draw_tile_keys` (`view.rs:1100`) substitutes any ready ancestor/descendant
for a missing tile and draws it in the same frame with a different `map_scale`.
A z13 parent next to a z14 tile has different simplification, ×2 widths, and
different f32 rounding → the visibly *offset* canal segment (Bloemgracht) and the
Rozengracht width jump, with duplicate labels at two sizes. There is no fade, no
"children fully cover parent → skip parent" logic, and stale parents can persist
because eviction is soft (`tiles.len() > 640`).

### 3.5 Water drawn twice, cased

Shortbread ships Amsterdam canals as `water_polygons` **and** `water_lines`. We
fill the polygon, then stroke the centerline on top with a **dark casing**
(`MapWaterwayRule` canal: casing `#4a8fc3` 1.5 → 12 screen px at z17). Carto keeps
waterway centerlines invisible where a water polygon exists (or same-color, no
casing). The tube-on-polygon look plus its butt caps at tile seams is a large part
of the "canals look broken" impression.

---

## 4. Actual missing bits in the vector API

The user hypothesis ("missing bits in the vector API") is partially right. In
`libs/svg/src/tessellate.rs` / `draw/src/vector` / `DrawVector`:

1. **`LineJoin::Round` is not implemented.** `calculate_joins` flags Round the
   same as Bevel (`tessellate.rs:333–338`) and `stroke()` only ever emits
   `emit_bevel_join` (`tessellate.rs:419`). The NanoVG `roundJoin` arc emission
   was never ported. Every bend on a road/canal is a bevel; magnified, that reads
   as faceted/lumpy.
2. **No inner-bevel correction** (NanoVG `chooseBevel` for `PT_INNERBEVEL`).
   `emit_bevel_join` extrudes the inner side with raw segment normals
   (`tessellate.rs:704–754`), so sharp angles fold geometry over itself. Invisible
   with opaque strokes at 1×, visible as edge lumps under magnification.
3. **No GPU-side stroke width ("gpu_expand_stroke").** Fills have a
   `gpu_expand_fill` mode that anchors vertices on the edge and encodes the
   outward normal for shader-side expansion (`tessellate.rs:875–955`,
   `fill_gpu()` in `draw_vector.rs`). Strokes have no equivalent: width and AA
   fringe are baked into vertex positions. This is *the* API bit that would make
   zoom-independent road widths possible without re-tessellating — emit
   centerline-anchored vertices + normal + side, resolve `width_px` and fringe in
   the vertex shader from a uniform. (`v_stroke_dist`/`u`-side encoding already
   exists, so the vertex layout barely changes.)
4. **Single cap style per path.** Map rendering needs *butt* at tile-clip cuts and
   *round* at true dead ends, ideally per polyline end. The API takes one
   `LineCap` for everything.
5. **No draw-layer separation in accumulated geometry.** `append_tessellated_geometry`
   flattens everything into one interleaved buffer; there's no way to draw "index
   range of pass 1 across N geometries, then pass 2". (Workaround without API
   change: build separate casing/center `Geometry` objects per tile — doubles
   buffer count but works today. Nicer API: per-layer index ranges within one
   buffer, drawable as sub-range draw calls.)
6. **Dash/dot distances are geometry-space.** `dash()`/`dots()` use
   `v_stroke_dist` raw (`draw_vector.rs:124–139`); a scaled consumer like
   `DrawMapVector` needs `v_stroke_dist × map_scale` so patterns are stable in
   screen px. One-line fix in the map shader, but worth a note in the API docs.
7. **Not missing, but unused by the map**: `cur_tolerance` / `cur_fill_aa` /
   `cur_stroke_aa` hooks on `DrawVector` exist precisely to bake fringes/tolerance
   for a known display scale — the tile builder ignores them and uses fixed
   world-unit constants.

Explicitly *not* API problems: the f32 precision issue (§3.1 — fixable with
tile-local coords + existing per-draw uniforms), the per-tile layer ordering
(§3.3 — fixable with two buffers), and overzoom styling (§3.2 — fixable with
re-tessellation per zoom bucket even without item 3).

---

## 5. Recommended plan (ordered by impact ÷ effort)

1. **Tile-local coordinates.** Tessellate everything relative to the tile origin
   (subtract `tile_key.{x,y} × 256` before building paths; also do the projection
   math in f64 until that subtraction). In `draw_walk`, compute each tile's
   `map_offset` in f64 as `global_offset + tile_origin × scale`, cast at the end.
   Fixes §3.1 completely — both CPU-side tessellation precision and shader
   cancellation — with zero API changes. Biggest single win at high zoom.
2. **Split stroke buffers into casing + center geometries per tile**; draw three
   loops (fills → all casings → all centers). Kills the seam plugs and junction
   wedges of §3.3, and makes the `ROAD_CENTER_OVERLAY_*` hack removable.
3. **Re-tessellate per view-zoom bucket** (key tile cache by `(tile, render_zoom)`,
   build in the existing worker pool): widths from a per-zoom ramp (carto-style
   ~×1.6/zoom), `aa`/tolerance/thinning thresholds divided by the render scale so
   they stay ~1 device px. Alternatively (more work, better): implement
   **gpu_expand_stroke** (§4.3) and drive width per frame from a uniform — then
   one tessellation serves all zooms and continuous zoom is perfectly crisp.
4. **Overzoom tile subdivision**: synthesize z15–17 tiles by clipping the z14
   data (the MVT decode already yields per-feature geometry), so clip padding,
   world-unit constants and per-tile precision keep their intended meaning at
   every rendered zoom. Combines naturally with (3).
5. **Tessellator quality**: port NanoVG round joins + `chooseBevel` inner-bevel;
   use Round caps for true dead ends, Butt only at clip cuts (needs §4.4 or a
   per-part cap flag in `append_stroke_pass`).
6. **Water**: suppress `water_lines` casing (or the whole stroke) where
   `water_polygons` cover the same area at z12+; at minimum draw waterway strokes
   *under* polygon fills or in the polygon color.
7. **Mixed-zoom fallback hygiene** (§3.4): skip a parent when all four children
   are ready, evict replaced parents immediately, optional 100ms fade.
8. **Style/label polish toward the reference**: per-zoom style curves (min-zoom
   per road class, width ramps), label font size scaling with zoom + halo
   (currently fixed `font_scale 0.92`, no halo — `view.rs:1444`), building fill
   contrast + outline, dashed footpath/cycleway styling.

### Verification

Use the headless render harness (`MAKEPAD=headless` render-to-png) with a fixture
at a fixed center/zoom over cached tiles: render z14 (native) and z17 (overzoom)
crops and eyeball/golden-diff them per step above. Step 1 should visibly remove
edge wobble; step 2 removes seam bars; step 3 brings road weights in line with the
osm-carto reference.
