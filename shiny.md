# shiny.md — cheap light, shadow & GI for the map renderer (and the XR engine)

Status: plan, 2026-07-30. Companion docs: `mapplan.md`, `blur.md` (gauss/post
stack), `overlay.md`. Investigation notes at the bottom are from a code sweep
of `widgets/src/map`, `draw/src/shader`, `xr/`, and all seven platform
backends — file:line references are from the current working tree.

## Direction (the rules this plan follows)

1. **One global light.** A single sun (direction + color + hemisphere
   ambient) shared by every consumer: building walls, terrain hillshade,
   water, trees, and the XR engine. No light lists, no point lights.
2. **Tricks, not a deferred renderer.** No G-buffer, no MRT, no SSAO
   depth-reconstruction pass. The map is 2.5D heightfield-shaped geometry —
   almost everything a deferred pipeline would compute per-pixel can be
   computed *at tile bake time* or with a one-liner in the existing pixel
   shader.
3. **The tile meshgen is the renderer's precompute stage.** We are geometry
   bound when zoomed out; drawing the scene geometry a second time (shadow
   pass, depth prepass) is the one guaranteed way to trash perf. The bake
   worker already touches every polygon with full semantic knowledge
   (heights, footprints, landcover, water) — AO, one-bounce GI and shadow
   *geometry* get baked there, costing the GPU nothing.
4. **Every feature has an on/off switch** and is free when off: bake-time
   features simply don't emit geometry/colors (a rebake applies the change),
   shader features sit behind float uniform gates (the exact pattern
   `DrawPbr` already uses with its `u_enable_*` uniforms,
   `draw/src/shader/draw_pbr.rs:117-123`), and optional passes are simply
   not allocated/scheduled when disabled.
5. **Map and XR share the light rig and the philosophy**, not a framework.
   The XR engine (`DrawPbr`) already has a real BRDF + IBL; what it lacks is
   the same things the map lacks — a unified sun definition, AO, and (later,
   optional) one sun shadow map.

## What exists today (short version)

- Map "lighting" is entirely CPU-baked into vertex colors at tile bake:
  fixed NW sun `(-0.55,-0.835)`, wall shade `0.62 + 0.20*(facing+1)`
  (`widgets/src/map/tile.rs:3051,3111-3118`), Gouraud Blinn-Phong on
  tree/lamp balls (`tile.rs:2086-2164`), CPU hillshade worker with its own
  copy of the same sun (`libs/geodata/src/terrain_shade.rs:142`). No
  shadows, no AO, nothing dynamic.
- The map has no 3D camera: screen-space affine + tilt y-compression +
  hand-authored depth ladder (`widgets/src/map/view.rs:63-279`). Depth
  cannot be inverted back to world space — classic SSAO/shadow-mapping math
  does not apply to the map path.
- Vertex format `VectorVertex` = 19 floats
  (`draw/src/vector/triangulate.rs:4`); **`param1..param3` are unused (0.0)
  on all shape-0 geometry** (walls, roofs, balls) and survive to the pixel
  shader as varyings — three free channels per vertex, no format growth.
- The XR engine is `DrawPbr` (`draw/src/shader/draw_pbr.rs`): Cook-Torrance
  GGX + IBL env maps + baked-AO-texture support (`u_occlusion_strength`),
  per-feature uniform gates already in place. Five-plus copy-pasted ad-hoc
  sun rigs exist across `xr/src/obj/*`, `draw_cube.rs`, gamemaker.
- Working multi-pass substrate exists (GaussStack: 13 chained BGRA8 passes,
  `widgets/src/window.rs:449-680`) — but see Appendix A: depth textures are
  not sampleable on ANY backend, float render targets are dead code, MRT
  works only on D3D11. Reinforces rule 2.

---

## The one light: `SceneSun`

A single POD struct, one definition, no per-shader copies of the numbers:

```rust
// draw/src/scene_sun.rs (new, tiny)
pub struct SceneSun {
    pub dir: Vec3f,          // world/map-space, normalized, z-up for map
    pub color: Vec3f,        // sun tint * intensity
    pub sky: Vec3f,          // hemisphere ambient from above (cool)
    pub ground: Vec3f,       // hemisphere ambient from below (warm bounce)
    pub shadow_alpha: f32,   // how dark baked shadows draw
}
impl SceneSun { pub fn from_time_of_day(hours: f32, latitude: f32) -> Self }
```

Consumers:

| consumer | how it receives the sun |
|---|---|
| tile bake worker (`tile.rs`) | passed into `build_tile_buffers_*` with the style — replaces the hardcoded `(-0.55,-0.835)` and ball light |
| terrain shade worker (`terrain_shade.rs`) | replaces its private sun `(-0.5,-0.62,0.6)` |
| `DrawMapVector` | 3 uniforms (`sun_dir`, `sun_color`, `sun_ambient`) — only read when a dynamic-sun gate is on |
| `DrawPbr` / `DrawCube` / gamemaker / xr objs | feeds the existing `u_light_dir`/`u_light_color`/`u_ambient` uniforms from ONE place (`xr/src/scene/xr_env.rs:161-168` becomes the setter, the copy-pasted rigs in `gltf.rs`, `physics_view.rs`, `cube.rs`, `game_view.rs` read from it) |

This is deliberately *not* a new uniform-buffer mechanism. The two free
custom uniform-buffer slots (`platform/src/draw_vars.rs:27,147`) exist but
have zero production users — adopting them is a cleanup we can do later once
validated, not a dependency of this plan. Phase 1 uses plain per-shader
uniforms sourced from the one Rust struct.

---

## The tricks

Each trick lists: where it computes, GPU cost, its switch, and XR reuse.
Ordered by (visual win / effort).

### T1 — Material channel + optional per-pixel sun (the foundation)

At bake time, write a **material id into `param3`** and the **surface normal
into `param1/param2`** for shape-0 geometry (walls: 2D outward normal, z=0;
roofs: up; balls: baked normal). The shape-id branches that hijack params
(gradients 0.5..19.5, patterns 29.5..32.5, expanded strokes) don't apply to
shape 0, so this is safe (`widgets/src/map/view.rs:214-235`).

Materials: `0` legacy/none, `1` wall, `2` roof, `3` water, `4` canopy,
`5` green-area, `6` shadow-decal, `7` route-glow. The pixel shader gets one
`match` on `v_param3` (shader-enum rule, CLAUDE.md #20) dispatching the
per-material tricks below.

Two lighting tiers, both driven by `SceneSun`:

- **Baked (default, cost 0):** exactly today's path — the bake worker
  multiplies sun + hemisphere + AO into vertex color. Changing time-of-day
  triggers the existing restyle path (tiles already rebuild per zoom
  bucket; a sun change is just another restyle key).
- **Dynamic sun (toggle `dynamic_sun`, cost ≈ 2 dot products/px):** bake
  writes *unlit* albedo into color and the shader evaluates
  `albedo * (ambient_hemi(n) + sun_color * max(dot(n, sun_dir), 0))`.
  Buys a live time-of-day slider with zero rebakes. Off ⇒ shader gate
  short-circuits to `v_color` untouched.

Effort: bake plumbing ~1 day, shader ~half day.

### T2 — Baked AO + one-bounce GI in the meshgen (the "cheap GI")

All computed in the tile worker from footprint geometry it already holds.
Baked into vertex colors (or, with T1 dynamic sun, into a per-vertex
`ao` scalar folded into albedo). GPU cost: **zero**.

- **Street-canyon AO:** for each wall base vertex, sample distance to the
  nearest opposing building footprint (the worker has all footprints per
  tile + neighbor aprons). Narrow streets → darker wall bases and slightly
  darker ground fill between. A coarse per-tile distance grid (e.g. 64×64,
  computed once per bake) makes this O(verts), not O(verts×polys).
- **Wall vertical AO gradient:** bottom verts darker than top
  (`ao = mix(0.75, 1.0, height_frac)`) — the single cheapest "grounded
  buildings" win, ~5 lines in `append_wall_quad`.
- **Roof-edge / parapet AO:** darken roof fill vertices within ~1.5 m of
  the outline (earcut already visits the ring).
- **Sky-visibility ambient:** wall verts in canyons see less sky → scale
  the `sky` hemisphere term by the same distance grid.
- **One-bounce ground GI:** walls receive `ground` bounce tinted by the
  dominant landcover under them (grass → greenish bounce, water → cool,
  street → warm gray). The worker already rasterizes landcover for the
  drape (`examples/map/src/main.rs:1759-1951`) — reuse that class mask.
- **Tree contact shadow:** small dark disc under each canopy (bake emits a
  ground decal, material 6).
- **Emissive-road bounce (dark themes):** when the theme marks roads
  emissive (see T5b / Circuit City), roads become GI *emitters* in the
  same bake: ground vertices and wall-base vertices within a few meters
  of a glowing road get an amber bounce tint falling off with distance —
  reuse the canyon distance grid, keyed by road class. This spill is what
  makes glowing streets read as *light sources* instead of painted lines
  (visible in the reference images), and it costs the GPU nothing.

Switches: one bake flag each (`bake_ao`, `bake_bounce`), rebake applies.
Effort: ~2 days including the distance grid.

XR reuse: the same idea is `u_occlusion` in DrawPbr (already implemented
for glTF AO textures); gamemaker terrain/cubes can bake per-vertex AO with
the same helper math (extract the canyon/contact AO functions into a small
shared `draw/src/bake_light.rs`).

### T3 — Shadow geometry for buildings + terrain shadow bake (the "shadows")

No shadow map, no second scene pass — **shadows are polygons**, generated
at bake time, drawn once as ordinary fills:

- For each building, project the roof polygon along the sun's ground
  direction by `height * shadow_len(sun_elevation)`; the shadow volume's
  ground silhouette = union(footprint, projected roof, connecting quads
  per silhouette edge).
- **Union all shadow polys per tile** with the existing boolean/dissolve
  machinery (the road union mesh architecture) so overlapping shadows
  don't double-darken; clip to the tile square (Sutherland-Hodgman
  clipper already exists in the bake path).
- Emit as one fill batch, material 6, drawn right after ground fills /
  before casings, semi-transparent `shadow_alpha` (soft edge: 1-2 m
  analytic AA fringe — the tessellator's fringe support does this
  already).
- In tilt/3D mode shadows follow terrain for free: they are ground-plane
  geometry, so the existing terrain lift in the vertex shader displaces
  them like every other ground fill.
- **Terrain cast shadows:** horizon-march in the hillshade worker
  (`terrain_shade.rs`) — mountains shadow valleys. Pure CPU, reuses the
  elevation grid it already walks; ~30 lines.
- **Bridge/deck shadows:** stamp a darkened decal under bridge decks
  (deck outline projected by `bridge_dz` height) — same mechanism.

Costs: bake CPU per tile (bounded: silhouettes are the footprints we
already simplify for walls; LOD-gate to z≥14 and/or 3D mode); GPU cost is
just extra ground-fill vertices — measure, but shadow unions are far
smaller than the road union meshes we already draw. Sun direction changes
→ rebake (same as T1 baked tier; time-of-day is a slow variable).

Switches: `bake_shadows` flag + `shadow_alpha` uniform (instant fade-out
without rebake: alpha 0 ⇒ invisible, and the bake flag stops emitting on
the next restyle).

Effort: ~2-3 days (silhouette projection + union wiring + LOD gates),
terrain march ~half day.

### T4 — Shiny water (pixel shader, zero geometry)

Water fills get material 3 and a per-pixel effect in `fill_pattern()` /
the material match (`view.rs:281-315` is the precedent — pattern fills
already do procedural per-pixel work):

- Procedural micro-normal from 2-3 octaves of value noise scrolled by
  `draw_pass.time` (time already reaches every shader,
  `platform/src/draw_pass.rs:358-389`).
- Sun glint: `pow(max(dot(n, half_vec),0), k) * sun_color` with the fixed
  view direction the ball bake already assumes `(0, 0.62, 0.79)` — in a
  2.5D map a constant view vector is correct enough.
- Fresnel-ish sky tint: mix water color toward `sky` by tilt amount.
  Subtle sparkle at high zoom, calm sheen when zoomed out (scale noise
  frequency by `map_scale` so it's zoom-stable, same trick as dash
  periods).

Cost: ~10-15 ALU on water pixels only. Switch: `water_fx` uniform gate
(off ⇒ flat color, branch coherent). Effort: ~1 day of shader tuning.

XR reuse: the same noise+glint function is directly usable in a DrawPbr
material variant (gamemaker water).

### T4b — Shiny buildings: specular sheen from the one sun (zero geometry)

Reference: the dark-miniature-city look (matte black volumes with glossy
highlights, see user ref image). With T1's normal in `param1/2` and
material ids for wall/roof, this is pure pixel shader on building pixels:

- **Blinn-Phong from `SceneSun`** with the fixed 2.5D view vector the
  ball bake already assumes (`(0, 0.62, 0.79)`):
  `spec = pow(max(dot(n, h), 0), k) * sun_color * gloss`. Because the
  view vector is constant but the *map rotates under it*, highlights
  sweep across facades during heading rotation — reads as real
  reflection for free.
- **Walls:** normal is `(nx, ny, 0)` + a small constant z-tilt so sun-side
  facades catch a streak; multiply spec by the height fraction
  (`param4`-derived) so it blooms toward rooflines — the fresnel-ish
  vertical gradient in the reference.
- **Roofs:** normal is up, so plain spec is flat per face — instead use a
  screen-position-dependent half-vector (`h` nudged by `v_world * tiny`)
  = a fake linear-gradient environment reflection; big roofs get a soft
  sheen ramp instead of uniform gray, and it slides during pan/tilt.
- **Gloss per material/theme:** `gloss` is a style value — near-zero in
  the day carto theme (subtle), high in a dark theme where the
  matte-plus-sheen look lives. Optional per-building gloss jitter (seed
  from the free `stroke_dist` slot) so towers vary like the reference.
- Pairs with T6's dark palette + T5b's glow: dark theme + `building_sheen`
  + amber `route_glow` on major roads + T6b tilt-shift ≈ the reference
  image as a selectable map style, no HDR anywhere.

Cost: ~8-10 ALU on building pixels only. Switch: `building_sheen` gate
(off ⇒ v_color passthrough, today's look). Effort: ~1 day of tuning with
screenshots. XR reuse: this is literally DrawPbr's direct-specular term —
nothing to port; the shared win is the same `SceneSun` feeding both.

### T5 — Foliage / "green" pixel effect (zero geometry)

- Canopy balls (material 4): per-pixel value-noise modulation of the
  baked Gouraud color (leaf clumping), slight hue jitter per tree
  (seed from `stroke_dist` slot), sun-side rim brightening using the
  baked normal from T1.
- Green areas / parks (material 5): low-frequency two-tone noise
  (grass patchiness) + the T2 bounce tint. Kills the flat solid-green
  polygon look at a few ALU per pixel.

Switch: `foliage_fx` gate. Effort: ~1 day. XR reuse: tree shader in
`xr/src/obj/tree.rs` adopts the same noise (it already does N·L + rim).

### T5b — Tron route glow (shader + geometry, no HDR/bloom)

The nav route (car-path) gets a glowing, energy-line look using only stroke
geometry and the blend mode we already have:

- **Geometry:** draw the route stroke with a deliberately wide raster
  carrier — the tessellator already supports exactly this (the analytic
  fringe sentinel keeps visible coverage at the true width while the
  band is wider, see the `2e6` carrier comment in
  `draw/src/vector/triangulate.rs`). Emit the route at core width +
  ~10-16 px halo band, material 7.
- **Shader:** per-pixel signed distance from the centerline is already in
  `v_tcoord.x`. Compose `bright saturated core + halo * exp(-k*d)`.
- **The no-HDR glow trick:** the whole pipeline blends premultiplied
  alpha — output `rgb = glow_color * falloff, a = 0` in the halo region
  and it becomes **pure additive** over whatever is underneath (roads
  brighten instead of being covered). Core stays normal premultiplied so
  it reads as a solid line. No float targets, no bloom pass, no pipeline
  change.
- **Motion:** `stroke_dist` carries zoom-stable along-path distance (the
  dash machinery proves it) — animate a brightness pulse marching toward
  the destination with `draw_pass.time`. Instant "the route is alive"
  read, zero geometry churn.
- In 3D mode the route lifts with terrain like any other stroke, and the
  additive halo glowing over building bases actually helps legibility.

Cost: route pixels only, a handful of ALU; the wider carrier adds a few
hundred verts along the route. Switch: `route_glow` gate (off ⇒ current
flat route stroke). Effort: ~1 day. XR reuse: the same
core+exp-falloff+additive pattern works for any emissive line/edge in
DrawPbr scenes (tron grid floors, laser paths in gamemaker).

### Showcase style: "Circuit City" (the reference-image preset)

The user's reference renders (dark miniature Berlin: matte silver or
near-black buildings, every road a glowing amber filament, light spilling
onto the ground, tilt-shift DOF) decompose exactly into this plan's
toggles — worth shipping as a named map style to prove the system:

| ingredient in the reference | plan feature |
|---|---|
| all roads as glowing amber lines (class-weighted width/brightness) | T5b generalized: the additive core+halo material on *road strokes*, not just the nav route — a style flag marks road classes emissive |
| amber light pooling on ground / wall bases near roads | T2 emissive-road bounce (baked) |
| matte silver vs glossy black building volumes | palette + T4b `building_sheen` (gloss high, fresnel edge lift) |
| buildings grounded by soft darkening | T2 contact/canyon AO |
| dark reflective water | T4 with sky term ≈ 0, glint from road-glow color |
| miniature depth-of-field | T6b tilt-shift |
| overall dark ground, minimal labels | theme/palette (exists) |

No feature beyond what's already specced — Circuit City is a *style
preset* (`ShinyConfig` + palette), not new machinery. It's also the ideal
tuning vehicle for phases 2-3 because every trick is visible at once.

### T6 — Night mode & emissive (later, optional)

Bake: window strips on walls as emissive vertex color regions when the
palette is dark (wall UVs derivable from edge length + height fraction —
`param4` already varies bottom→top), street-lamp halo discs on ground.
Optional bloom via the existing GaussStack path — the scene is drawn ONCE
into the capture pass and blurred with full-screen quads, so it does not
violate rule 3; still BGRA8-only until float RTs are repaired (Appendix A),
which caps bloom quality — acceptable for a subtle pass.
Switches: `night_mode` (restyle), `bloom` (pass not scheduled when off).

### T6b — Tilt-shift / miniature mode (for fun, optional, GaussStack reuse)

The diorama look: a sharp focus band across the screen, blur growing
toward top and bottom. Everything needed already exists in the window's
Gauss machinery:

- Render the map scene once into the GaussStack capture pass (same
  single-scene-draw as T6 bloom — no geometry re-render, rule 3 holds),
  build the existing 6-level mip chain (`widgets/src/window.rs:583-616`).
- Composite with one fullscreen quad that computes a **per-pixel blur
  amount** = `|screen_y - focus_y| / band_height`, clamped, optionally
  shaped by tilt so the focus band sits on the tilt pivot row (where the
  camera "focuses") and far ground blurs more — that's physically what a
  real tilt-shift lens does to a city, which is exactly why it reads as
  miniature.
- Per-pixel variable blur = manual trilinear across the bound mip chain:
  pick `level = blur * 5.0`, sample `mip[floor]` and `mip[ceil]`, mix by
  fract. The 7-texture scene+mip0..5 sampling pattern with a mix factor
  is already production code (`widgets/src/gauss_view.rs:113-119`,
  `glass_panel.rs`) — this is the same shader with the mix factor turned
  from a uniform into a per-pixel function of screen y.
- Saturation boost (+10-15%) inside the same composite quad — half the
  miniature illusion is candy colors.
- Labels/UI draw after the composite as today, so text stays sharp.

Cost: the GaussStack mip chain (a handful of small fullscreen quads —
already shipped and smooth for glass) + one composite quad. Switch:
`tilt_shift` — a pass-level flag; when off the capture pass isn't
scheduled and the map renders directly to the window exactly as today.
Auto-suggest: only meaningful in 3D tilt mode; gate it to tilt > ~30°.

### T7 — XR engine: shared sun, baked AO, and the ONE real pass (optional)

- Adopt `SceneSun` as the single source for all XR/gamemaker light rigs
  (pure cleanup, no visual change until tuned).
- Extract T2's AO helpers for static-scene bakes (gamemaker terrain,
  placed cubes).
- **Single-cascade sun shadow map over the play area** — the one place a
  real extra pass is justified (small scene, XR far plane is 15 m,
  gamemaker's handoff already ranks it). Default OFF, desktop/tethered
  quality tier first; Quest only after measuring. Requires platform work
  (Appendix A): either pack depth into a BGRA8 color target (works on all
  backends today, 8-tap manual PCF) or do the 1-line Metal/Vulkan
  usage-flag fixes for true depth sampling. Mitigate the
  one-draw-call-per-mesh cost by only re-rendering the shadow map when
  the scene is dirty (`InitWith` preserves the target across frames).

---

## Toggle architecture (the on/off requirement)

One config struct, three enforcement layers:

```rust
pub struct ShinyConfig {
    // bake-time (apply via restyle/rebake, zero GPU when off)
    pub bake_ao: bool,          // T2
    pub bake_bounce: bool,      // T2 GI
    pub bake_shadows: bool,     // T3
    pub terrain_shadows: bool,  // T3
    // draw-time (float uniform gates, DrawPbr u_enable_* pattern)
    pub dynamic_sun: bool,      // T1
    pub water_fx: bool,         // T4
    pub building_sheen: bool,   // T4b
    pub foliage_fx: bool,       // T5
    pub route_glow: bool,       // T5b
    // pass-level (pass not allocated/scheduled when off)
    pub bloom: bool,            // T6
    pub tilt_shift: bool,       // T6b
    pub xr_shadow_map: bool,    // T7
    pub sun: SceneSun,
}
```

- **Bake flags** join the tile restyle key (next to `render_bucket`):
  flipping one marks tiles stale exactly like a zoom-bucket change; the
  stale tile stays drawable while rebaking, so toggling is glitch-free.
- **Shader gates** are `uniform(0.0/1.0)` floats — uniform branches are
  coherent and effectively free; when off, the shader path is identical to
  today's. (Precedent: `draw_pbr.rs:117-123` + CPU-side gate computation
  at `:986-1043`.)
- **Pass flags**: an off pass is simply never allocated/begun (precedent:
  `MAKEPAD_NO_GAUSS`, `gauss_view.rs:45-50`). Textures drop to the 1×1
  fallback binding so shaders stay valid (precedent: `terrain_fallback`,
  `view.rs:4634-4647`).
- Exposed as DSL properties on `MapView` (`shiny.bake_shadows: true`,
  `shiny.sun_hours: 14.5`) and runtime setters (`set_shiny(cx, cfg)`)
  for a time-of-day slider; XR side reads the same struct via `XrEnv`.
- **Capability gating:** a tiny `shiny_caps(cx)` check downgrades
  automatically per backend (e.g. `xr_shadow_map` forced off on WebGL
  until Appendix A items land). Everything in T1-T5 is plain
  geometry + pixel shader and runs on every backend unconditionally.

## Phases

| phase | contents | effort | GPU cost when on |
|---|---|---|---|
| **1** | `SceneSun` + T1 material/normal channel + T2 vertical-wall AO + roof-edge AO | ~2 days | 0 (baked) |
| **2** | T3 building shadow geometry + terrain shadow march + tree contact discs | ~3 days | ground-fill verts only, LOD-gated |
| **3** | T2 full (canyon distance grid, sky visibility, landcover bounce) + T4 water + T4b building sheen + T5 foliage + T5b route glow | ~5 days | few ALU/px on affected materials |
| **4** | T1 dynamic-sun tier + time-of-day UI + XR rig unification (T7 first half) | ~2 days | 2 dot/px when enabled |
| **5** (opt) | T6 night/bloom, T6b tilt-shift, T7 shadow map + its platform fixes | sized separately; tilt-shift ~1 day (pure GaussStack reuse) | 1 extra small pass each |

Each phase lands behind its toggles, default matching today's look until
tuned (i.e. `bake_ao` etc. can default on only after screenshot review).

## Perf budget & guardrails

- **Zero extra scene-geometry passes in the map path.** The only new
  geometry is T3's unioned ground shadows — budget: shadow verts ≤ 15% of
  fill verts per tile, enforced with the same simplification/LOD gates as
  walls (`wall_min_edge` pattern); shadows OFF below z14 and optionally
  2D mode.
- Bake-time additions ride the existing worker; watch tile rebuild time
  (the typed-MVT decode made this fast — keep it under ~50 ms/tile
  added).
- Pixel tricks apply per-material, not full-screen; water/foliage pixels
  are a minority of the frame.
- Measure with the existing `local/map_perf.log` draw_walk instrumentation
  and the F3 PerfGraph; verify in a standalone release window (studio
  RunView streaming lies about frame pacing).

## Verification

- Studio-bridge screenshots at the usual spots (Rozengracht z16/z17 3D,
  Innsbruck relief for terrain shadows, IJ waterfront for water) — the
  headless path cannot exercise render-to-texture (Appendix A) and the
  map can't headless-render anyway, so bridge screenshots + A/B toggle
  flips are the test: **every feature must screenshot-diff to zero when
  toggled off.**
- Bake determinism: same tile + same `ShinyConfig` ⇒ identical buffers
  (extend the existing ignored probe tests in `tile.rs` with an AO/shadow
  probe).

## Non-goals / rejected

- **Deferred rendering, G-buffer, MRT** — geometry-bound + MRT broken on
  4/5 runtimes. Rejected.
- **Screen-space SSAO for the map** — the map's depth ladder is
  hand-authored and not invertible to world space; a depth/height prepass
  would mean re-rendering all geometry. T2's baked AO replaces it at zero
  GPU cost. (Desktop-XR SSAO stays a possible later add since
  `XrSceneView` already renders with a real camera — but it's not in this
  plan.)
- **Shadow maps for the map** — same re-render objection; T3's shadow
  geometry wins for heightfield-shaped content.
- **Multiple lights / point lights** — one sun, per direction.

---

## Appendix A — platform facts (for phase 5 and future work)

Sweep results worth keeping (exact sites, current tree):

1. **No backend can sample a pass depth texture.** Metal allocates depth
   without `ShaderRead` (`metal.rs:2476`), GL uses a renderbuffer
   (`opengl.rs:3009-3035`), D3D11's SRV is commented out
   (`d3d11.rs:1727,1763`), Vulkan lacks `SAMPLED` (`vulkan.rs:4209`),
   WebGL never implements depth targets (`web_gl.js:906`). Fix costs:
   1-liners on Metal/Vulkan; D3D11 needs typeless format + SRV; GL needs
   renderbuffer→texture rewrite; web is real work.
2. **BUG (fix regardless of this plan):** GL/GLES offscreen passes never
   attach their depth renderbuffer — the only `glFramebufferRenderbuffer`
   call sits inside a comment (`opengl.rs:869-910`, line 908). Any
   depth-tested content in an offscreen pass (SsaaStack, XrSceneView,
   gamemaker RTT) renders without depth testing on Linux/Android GLES.
3. **Float render targets are dead code**: `RenderRGBAf16/f32` exist but
   Metal hardcodes `BGRA8Unorm` in the per-shader pipeline
   (`metal.rs:1708` — needs pipeline variants keyed on attachment
   formats) and GL uses unsized internal formats (`opengl.rs:2986-2996`).
   D3D11/Vulkan would work. Blocks HDR/bloom quality, not this plan.
4. **MRT** is fully wired in shader codegen (8 outputs) but works only on
   D3D11; GL lacks `glDrawBuffers`, Vulkan hardcodes one blend
   attachment, Metal same pipeline issue as (3), WebGL hardcodes one
   target.
5. **No compute shaders, no sRGB, no MSAA on offscreen passes, no RT
   mipmap generation, no texelFetch, no sample_compare/PCF hardware**
   anywhere. Post-processing = fullscreen fragment quads over BGRA8
   chains (GaussStack proves 13-deep works), `sample_rt()` handles the GL
   y-flip, `keep_camera_matrix = true` for custom cameras, cube-face RTT
   works everywhere except web, `DrawPassClearColor::InitWith` enables
   temporal accumulation.
6. Pass ordering follows *allocation order* fallbacks
   (`os/cx_shared.rs:60-120`): allocate producer passes before consumers.
7. In XR, the CPU never sees view/projection (identity in
   `xr_scene_state`, `xr_root.rs:151-159`); eye matrices live only in
   pass uniforms — any CPU-side sun-frustum fitting for T7 must use the
   head pose.
8. 16 texture slots per draw call, 7 proven in production; map uses 2
   (gradient, terrain). Room for a shadow map + noise LUT if ever needed.
