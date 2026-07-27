# GPS layer: search, current position, routing

## Status 2026-07-27 — basic navigator SHIPPED (M0 + M1 + M3 + sim-M2/M4)

Built and verified end-to-end (search "centraal" → route → simulated drive →
"You have arrived" banner):

- `libs/map_nav` — geo, `region.search` (builder+query, prefix/category/proximity),
  `region.graph` (CSR, per-mode speeds, oneway, turn restrictions, snap grid, A*),
  maneuver generation, `NavSession` (map-matching, off-route → reroute). 26 unit tests.
- `tools/map_tiles nav-build <pbf> <basename> [--bbox]` — one scan → both artifacts
  (Noord-Holland: 7.8s, 89MB graph / 107MB search, 1.39M directed edges, 1.8M docs
  incl. full addresses). `nav-probe` for CLI search/route checks.
  NH pbf at `local/maps/noord-holland-latest.osm.pbf`, artifacts `local/maps/noord-holland.*`.
- MapView M0: `MapViewAction` (ViewportChanged/Tapped/LongPressed/MarkerClicked),
  camera API + `fly_to` (zoom-out-in arc), `overlay.rs` (route casing+fill with
  traveled dim, pin markers, position puck w/ accuracy + heading), `mbtiles_path`
  DSL property (per-app tile archive override). Map now respects the `handled`
  flag so floating panels win hits (EventOrder::Up).
- `examples/map` is the navigator app: debounced worker-thread search, result list,
  long-press / "Set position here" → puck, route bar (Drive/Bike/Walk), Start →
  simulated turn-by-turn with banner + follow cam + recenter.

Not yet: real GPS (CoreLocation), heading-up camera, voice, CH scaling, v2 pbf
index extras (opening hours, admin hierarchy), typo tolerance. Plan below is the
original roadmap.

Plan for the interaction layer on top of the local map stack: a search engine over the
map data (places, shops, streets, addresses), a live "current position" puck, and
turn-by-turn routing — a proper offline GPS app. This work sits between the renderer
(being improved in `widgets/src/map/`) and the OSM import pipeline (`tools/map_tiles/`),
and deliberately touches both only through small, agreed contracts so all three tracks
can proceed in parallel.

## Current state (what we build on)

- **Renderer**: `MapView` (`widgets/src/map/view.rs`) renders Shortbread vector tiles
  from a local mbtiles file. Camera = `center_norm` (normalized web mercator `Vec2d`)
  + fractional `zoom`. No rotation. Pan/scroll-zoom gestures work. There is **no public
  camera API, no widget actions, no tap reporting, no fly-to animation** — the widget is
  render-only today. `examples/map` is a bare demo with an empty `handle_actions`.
- **Data**: `noord-holland-shortbread-1.0.mbtiles`, z0–14. The z14 layers carry real
  search fodder: `pois` (name, amenity, shop, cuisine, tourism…), `addresses`
  (housenumber/housename — **no street name**), `street_labels` (street names),
  `place_labels` (cities/towns + population), `public_transport` (stations),
  `buildings`, `sites`, `land`. The `streets` layer has `kind`, `oneway`,
  `oneway_reverse`, `bridge`, `tunnel`, `link` — enough for a stopgap routing graph,
  but no turn restrictions and no stable node ids.
- **Import pipeline**: `tools/map_tiles` already parses raw `.osm.pbf` (`osmpbf` dep)
  and builds a full-tag "detail archive" that retains OSM ids + complete tag maps.
  This is the natural source of truth for search v2 and the routing graph.
- **Reusable primitives**:
  - Zoom-constant screen-space symbols: `ICON_SHAPE_ID` path in the map vertex shader
    + `append_icon_mesh` (`tile.rs`) — exactly what markers and the position puck need.
  - `DrawVector` immediate-mode path API (`move_to`/`line_to`/`stroke`) — route overlay.
  - `DrawRotatedText` + the label collision machinery — overlay labels.
  - Background work pattern: `TagThreadPool` + `ToUIReceiver` (used for tile decode) —
    reuse for search queries, index build, and route computation.
  - `MbtilesReader` (`libs/mbtile_reader`): sync, cheap to open, direct tile lookup.
  - Overlay injection point: end of `MapView::draw_walk`, after `place_and_draw_labels`.

## Architecture

One new pure-logic crate plus additions to the widget and the import tool:

```
libs/map_nav/            NEW — no UI deps, fully unit-testable
  src/search/            index format, builder, query engine, ranking
  src/graph/             routing graph format, builder helpers, A* query
  src/nav/               NavSession state machine (map-matching, instructions, reroute)
  src/geo.rs             shared: fixed-point coords, mercator helpers, haversine

tools/map_tiles/         import pipeline (other agent) gains two subcommands:
  search-index           pbf (or tiles) -> region.search
  route-graph            pbf -> region.graph

widgets/src/map/
  view.rs                camera API, actions, fly-to, overlay draw hook
  overlay.rs             NEW — markers, route polyline, position puck
  (search UI + nav HUD live in the example app first, promoted to widgets later)

examples/map/            grows into the actual GPS app (search box, nav HUD)
```

Rationale: `libs/map_nav` mirrors what `libs/mbtile_reader` is for tiles — a dependency
shared by the import tool (which *builds* artifacts) and the app/widget (which *queries*
them). File formats live in one place; the import agent and we only need to agree on
the artifact contracts (below).

All heavy work (index build, search queries, route computation, reroutes) runs on the
existing thread-pool pattern and posts results back over `ToUIReceiver`. The UI thread
never blocks on any of this.

Coordinate conventions: `f64` lon/lat at every public API boundary; normalized web
mercator internally (matches `center_norm`); `u32` fixed-point (norm-mercator × 2³²)
in file formats.

## Milestone 0 — MapView interaction API (prerequisite for everything)

The widget must become programmable before search or nav can exist. Small, mechanical,
should land first and unblocks all later milestones. Coordinate with the renderer agent
since it touches `view.rs`, but it's additive.

1. **Actions**: add `MapViewAction` and emit via `cx.widget_action`:
   - `ViewportChanged { center_lon, center_lat, zoom }` (end of gesture + end of fly-to)
   - `Tapped { lon, lat, abs }` (finger up without drag)
   - `LongPressed { lon, lat, abs }` (for "set position here" / "route to here")
2. **Camera API** on `MapView`/`MapViewRef`:
   - `set_center(lon, lat)`, `set_zoom(z)`, `center() -> (f64, f64)`, `zoom() -> f64`
   - `screen_to_lonlat(abs) -> (f64, f64)` and inverse (the math already exists
     privately in `geometry.rs`)
   - write `center_lon/center_lat/zoom` back after gestures (today they're stale
     input-only seeds)
3. **`fly_to(lon, lat, zoom)`**: animated camera. Copy the existing timer pattern
   (`zoom_settle_timer` / tile-fade). Ease center in mercator space + zoom with a
   zoom-out-then-in arc when the target is far (keeps tiles loadable mid-flight and
   reads like every mapping app). ~150 lines including easing.
4. **Marker layer** (`overlay.rs`): `set_markers(Vec<MapMarker { lon, lat, icon, color, id }>)`,
   drawn with the zoom-constant symbol path; hit-test markers *before* the map's finger
   grab and emit `MarkerClicked { id }`. Add a pin SVG to `ICON_SVGS`.
5. **Overlay draw hook**: draw order = tiles → labels → route polyline → markers →
   position puck (puck always on top).

Deliverable: `examples/map` can tap the map, log lon/lat, drop a pin, and `fly_to` it.

## Milestone 1 — Search

### Two data stages (don't block on the import agent)

- **v1 — index built from the tiles we already ship.** Scan all z14 tiles
  (Noord-Holland: 5,780 tiles ≤ ~400 KB — a seconds-scale, one-time background job on
  first launch, cached to disk). Extract named/categorized features from `pois`,
  `place_labels`, `street_labels`, `public_transport`, `buildings`, `sites`, `land`.
  The MVT decoder already produces per-feature tag maps at tile-build time; v1 adds an
  `MvtSink` that collects (name, tags, centroid) instead of tessellating. Known v1
  limits: no full addresses (Shortbread `addresses` lacks street names), features can
  appear in multiple tiles (dedupe by name_key + quantized location), no house-number
  interpolation.
- **v2 — index emitted by the import pipeline from the pbf.** Full `addr:street` +
  `addr:housenumber` + `addr:city` + postcode geocoding, OSM ids, opening hours /
  website / phone in result details, admin hierarchy for display names
  ("Albert Heijn — Zaandam"). Same file format, better builder. This is the contract
  to agree with the import agent; v1 exists so the search UI, ranking, and UX are
  finished before v2 data arrives.

### Index format (`region.search`, owned by `libs/map_nav`)

Design goals: mmap/stream-friendly (match the disk-streamable ethos of the tile store),
zero-copy query, no runtime deps. Layout:

- **Doc table**: fixed-size records — coord (2×u32 fixed-point), category (u16 enum:
  city, town, street, supermarket, restaurant, cafe, station, …), static rank (u8:
  population for places, category weight for POIs), offsets into a string pool
  (display name, secondary line).
- **Token index**: all names normalized (reuse/extend `normalize_label_key` in
  `label.rs`: lowercase, unaccent, strip punctuation) and tokenized. Sorted token
  table → binary search gives exact *and prefix* ranges for free. Each token has a
  postings list of doc ids, pre-sorted by static rank so top-k short-circuits.
- **Category synonyms**: small static table in code mapping query words to categories
  ("supermarkt"/"supermarket"/"groceries" → shop=supermarket; "pizza" →
  amenity=restaurant|fast_food + cuisine=pizza), so category queries return nearby
  instances rather than name matches. NL + EN synonyms first.
- Typo tolerance (trigram fallback) is a later add — the format reserves a section id
  for it; v1 is prefix-only, which covers the autocomplete-style UX.

### Query pipeline (background thread, debounced ~150 ms)

1. Normalize + tokenize; last token treated as a prefix.
2. Intersect postings (rarest token first); or category expansion when a token hits
   the synonym table.
3. Score: match quality (exact > prefix, full-name > partial) + static rank +
   proximity boost (distance to current position if set, else viewport center) —
   `log2(distance)` penalty so "pizza" means "pizza near me".
4. Top 10 → `SearchResults` action to the UI thread.

### Search UI (`examples/map` first)

Floating panel over the map: `TextInput` + results list (name, category icon,
secondary line, distance). Enter/click → `fly_to` + drop marker; category queries can
pin all top-N results. Escape/map-tap dismisses. Promote to a reusable
`MapSearchPanel` widget once the UX settles.

Deliverable: type "centraal", get Amsterdam Centraal ranked above street matches,
fly there. Type "supermarkt", get the nearest ten with pins.

## Milestone 2 — Current position

### Position provider

`PositionSample { lon, lat, accuracy_m, heading_deg: Option, speed_mps: Option, time }`
behind a `PositionSource` abstraction with three implementations, in build order:

1. **Manual** — long-press → "set my position here". Trivial, immediately useful.
2. **Simulated** — plays back a polyline (later: a computed route) at a given speed
   with synthetic heading + configurable GPS noise. This is the workhorse: it's how
   nav (M4) gets developed and tested on a desk, and it feeds the same code path a
   real receiver would.
3. **Platform** — real location services. Makepad's platform layer has no geolocation
   API yet, so this is a new `Cx` service: `cx.start_location_updates()` →
   `Event::LocationUpdate`. macOS (CoreLocation via the existing objc bindings) first
   since that's the dev machine; iOS shares CoreLocation; Android (FusedLocation via
   JNI), web (`navigator.geolocation`) after. Permission plumbing per platform.
   Schedule late — everything above it works against Manual/Simulated.

### Rendering + camera behavior

- **Puck**: accuracy circle drawn in *map* space (scales with zoom), blue dot +
  white ring + heading wedge drawn zoom-constant (`ICON_SHAPE_ID` path). Interpolate
  between 1 Hz fixes at frame rate (short lerp, mild dead-reckoning by speed/heading)
  so the puck glides instead of teleporting.
- **Follow mode**: camera tracks the puck. Any user pan/zoom breaks follow; a
  recenter button (standard GPS UX) re-engages it. Follow state lives in the app,
  driven via the M0 camera API — the widget stays dumb.

Deliverable: simulated drive around Amsterdam with a gliding puck, follow mode, and
working recenter button.

## Milestone 3 — Routing

### Graph build (import pipeline, contract with the import agent)

Go straight to the **pbf-derived graph** — `osmpbf` is already a dependency of
`tools/map_tiles`, and the tile-derived alternative (stitching clipped z14 `streets`
geometry) has no turn restrictions, lossy connectivity at tile borders, and
crossing-vs-junction ambiguity. Not worth building twice. (If the interaction track
ever gets far ahead of the import track, a tile-stitched graph behind the same file
format is the documented fallback — `bridge`/`tunnel`/`layer` disambiguate most
crossings — but it's plan B, not the path.)

Builder (`route-graph` subcommand, logic in `libs/map_nav::graph::build`):

1. Pass 1 over ways: keep `highway=*` filtered by access profile; count node usage.
2. Vertices = nodes used ≥2× + way endpoints. Edges = way segments between vertices,
   carrying: length (m), speed class (from `highway` kind, `maxspeed` when present),
   oneway, access flags per profile (car / bike / foot — Netherlands, so bike is not
   optional), and the full segment geometry for drawing.
3. Turn restrictions from relations (`no_left_turn`, `only_straight_on`, …) stored as
   (via-node, from-edge, to-edge) ban/only lists.
4. Serialize `region.graph`: CSR adjacency, u32 vertex ids, fixed-point coords, packed
   edge attrs, geometry pool, plus a uniform **grid spatial index over edges** for
   nearest-edge snapping.

Scale check: Noord-Holland is on the order of a million edges — plain in-memory graph,
loads in well under a second.

### Query engine (`libs/map_nav::graph::query`)

- Snap start/goal: grid lookup → project point onto candidate edges → virtual split.
- **Bidirectional A*** with a per-profile cost function (time-based; distance mode as
  a flag). Province-scale queries land in tens of ms on a worker thread — no
  preprocessing needed at this size. When we go country/Europe scale, add Contraction
  Hierarchies *in the builder* (query side barely changes); the file format reserves a
  section for shortcut edges now so the format doesn't break later.
- Turn restrictions handled by edge-based expansion at restricted via-nodes only
  (cheap, standard trick — avoids paying edge-based-graph cost everywhere).
- Output `Route { polyline, length_m, duration_s, legs: Vec<Maneuver> }`.

### Route rendering

Route polyline via `DrawVector` in the overlay hook: casing + fill in a distinct
color, simplified by zoom (`geometry.rs` already has simplification helpers), the
already-traveled portion dimmed once nav is active. Start/end markers from M0.

Deliverable: long-press → "route here" from current position; route line + distance
and time appear.

## Milestone 4 — Turn-by-turn navigation

### Instruction generation (`libs/map_nav::nav`)

Walk the route's vertices: where bearing delta + road-class change warrant it, emit
`Maneuver { kind, at: coord, street_name, distance_from_start }` with kinds
Depart / Continue / TurnSlightLeft…SharpRight / Roundabout { exit_n } / Uturn /
Arrive. Roundabout exit counting from the graph topology. Street names come from way
tags (already in the graph edges).

### NavSession state machine

Owned by the app, fed `PositionSample`s, emits actions:

- **Map-matching**: project position onto route polyline near last known progress,
  gated by heading agreement; monotonic progress (never snaps backward on noise).
- **Progress**: distance to next maneuver, remaining distance/time, ETA (recomputed
  from live speed with per-class fallback speeds).
- **Off-route**: > ~30 m from route (accuracy-scaled) for > ~5 s → `Rerouting` state,
  fire a background route query from current position, swap route on arrival. The
  simulated source with noise (M2) is the test harness for exactly this.
- Maneuver announcements at distance thresholds (e.g. 500 m / 100 m / now) → actions
  the UI turns into banner changes. Optional later: voice via `libs/tts` (kokoro) —
  the announcement action is designed so a TTS consumer just subscribes.

### Nav UI (`examples/map`)

- Top banner: maneuver arrow icon + "300 m — turn left onto Prinsengracht".
- Bottom bar: ETA, remaining km/min, end-navigation button.
- Camera: follow mode centered ahead of the puck (position ~1/3 from bottom), zoom
  scaled by speed. **North-up in v1.** Heading-up requires camera rotation, which the
  renderer doesn't have — the transform is affine so it's feasible, but it's the
  renderer agent's call; flagged as a coordinated stretch goal, not a v1 dependency.

Deliverable: simulated drive follows a computed route with live banner instructions,
and going off-route visibly triggers a reroute.

## Milestone 5 — Real-world hardening (post-MVP, ordered by value)

1. macOS CoreLocation backend (first real-GPS platform), then iOS/Android/web.
2. Search v2 index from pbf (full addresses, postcodes, admin hierarchy, details).
3. Camera rotation + heading-up nav (with renderer agent).
4. Voice guidance via `libs/tts`.
5. Typo tolerance (trigram section), multi-stop routes, avoid-motorway/ferry flags.
6. Europe scale: CH preprocessing, sharded search index, both keyed by region files.

## Artifact contracts (the import-agent interface, agree early)

| Artifact | Producer | Consumer | Contents |
|---|---|---|---|
| `region.mbtiles` | import pipeline | renderer | exists today (Shortbread tiles) |
| `region.search` | `map_tiles search-index` (v1: app self-builds from tiles) | search engine | doc table + token index + string pool, format owned by `libs/map_nav` |
| `region.graph` | `map_tiles route-graph` | router | CSR graph + turn restrictions + edge geometry + snap grid, format owned by `libs/map_nav` |

Formats versioned with a magic + version header; builders and readers live in the same
crate so they can't drift.

## Testing

- **Unit** (`libs/map_nav`, no UI): tokenizer/normalizer, ranking ("dam" must rank
  Dam square over side streets), postings intersection; graph snap edge cases (near
  junctions, dual carriageways); A* against hand-verified routes (a handful of
  Noord-Holland pairs sanity-checked against an online router, asserted within a
  tolerance); turn-restriction respect; maneuver generation on synthetic geometries
  (roundabout exit counts).
- **NavSession sim tests**: drive a noisy simulated position along a route, assert
  instruction sequence, ETA monotonicity, and that a forced detour triggers exactly
  one reroute.
- **Headless render tests** (existing `MAKEPAD=headless` PNG harness +
  `examples/*/tests` pattern): marker/route/puck overlay snapshots, search panel
  open-with-results snapshot.

## Suggested execution order

M0 is small and unblocks everything — do it first. After M0, search (M1) and position
(M2) are independent and can proceed in parallel; both are pure-add. M3 starts with
the `region.graph` contract conversation with the import agent, then builder + A*
(testable entirely without UI), then the overlay. M4 builds strictly on M2+M3. The
riskiest unknowns are deliberately early: the MapView API touch (renderer-agent
coordination) is M0, and the graph contract conversation opens at the start of M3.
