# Experimental incremental diffuse GI

`Renderer::set_gi_mode(GiMode::Fast)` enables a world-space, incremental multi-bounce
diffuse-light cache on the clustered forward path. `GiMode::Off` is the
default, including Quest. Off allocates no GI field and schedules no GI work.
This is not the existing progressive GPU path tracer: only its CPU BVH
builder is reused; the bounded trace and lighting shaders are independent.
Static probe placement also uses CPU BVH queries on the worker pool, only
when preparing a scene/grid, never for per-frame lighting.

In Sandbox use Settings → Indirect light, or Shift+F7 while the game has keyboard
input. `MAKEPAD_GI=fast` opts in at startup. An asset-independent fixture is
available with `SANDBOX_WORLD=gi`: click to capture mouse look, WASD to walk,
Shift to run, Space to jump, Escape to release. It uses the ordinary player
rig and world collisions, with no weapon. Launch without `SANDBOX_TARGET`
or `SANDBOX_DIST`: those deliberately pin the diagnostic camera.
F6 moves its door; Shift+F7 compares GI on/off; F5 compares multi-bounce feedback
with one-bounce lighting (allow several probe sweeps to settle).
`MAKEPAD_GI_DEMO_FREEZE=1` freezes the fixture's spotlight for comparisons.
F4 cycles cells, confidence, probe-weight colours, irradiance, probe state,
raw nearest-probe irradiance, and normal rendering. These are display-only;
they never enter feedback. Raw probe mode selects the largest nominal
trilinear weight without visibility, facing or interpolation. It can select
exterior probes behind a wall: use it to inspect cached SH, not as lighting.
The room has no direct sun by default: `MAKEPAD_GI_TEST_SUN=1` restores the
former faint directional source. Its roof edge casts a real line across the
floor even with GI off. `MAKEPAD_GI_TEST_NO_SPOT_SHADOW=1` isolates spot shadows
(it also removes that light from GI bounce lighting; do not compare GI energy).

Build from `apps/sandbox` with `cargo build --release -p makepad-sandbox`,
then launch `apps/sandbox/target/release/makepad-sandbox --remote` from the
repository root with those environment variables. Close agent-owned test
instances through `/gq`.

## Work and quality controls

```rust
renderer.set_gi_config(makepad_render::GiConfig {
    grid: [12, 8, 12],
    spacing: 1.5,
    probes_per_frame: 32,
    ray_distance: 32.0,
    strength: 1.0,
    anchor: None, // Some(world_center) for a fixed room volume
    feedback: 0.85, // 0 = one bounce; bounded to 0..0.95
});
renderer.set_gi_mode(makepad_render::GiMode::Fast);
```

With no selected box emitters, the defaults give 1,152 probes, 64 rays per
probe, at most 2,048 new static rays per rendered frame, and one full relight
refresh in 36 frames. Up to two static box emitters add 12 targeted ray slots
each; the batch shrinks to keep the same ray-slot budget (26 probes with one
source, 23 with two, versus 32 without). One whole probe is the minimum batch,
so very small configured budgets can exceed their nominal ray count. Initial
tracing reduces its batch for larger BVHs (see traversal budget below). Decreasing
`probes_per_frame` reduces update cost at the expense of response latency;
decreasing the grid reduces coverage. Spacing controls spatial resolution.
Strength blends indirect diffuse with ordinary hemisphere ambient (0–1).
On scene/grid initialization, the first three complete probe sweeps are hidden,
then GI blends in over 1.2 seconds using a smoothstep ramp. Ordinary direct
lighting and hemisphere ambient remain available during warmup. This changes
presentation only: transport still uses full feedback strength and the same
per-frame ray budget. At defaults the warmup is 108 rendered frames without
selected emitters, 135 with one, or 153 with two; larger BVHs or smaller budgets
take longer. F4 diagnostics bypass the presentation
fade. `GiStats::startup_sweeps` and `display_blend` report readiness separately
from the number of statically traced probes. Sandbox's asset-free GI/clustered
fixtures also keep their world when the unrelated asset library arrives.
The forward shader samples up to eight probes per surface with trilinear
weights and bilinear angular visibility (up to 64 nearest texture fetches
before the optional local-blocker checks),
independent of the update budget. Lowering the ray budget does not eliminate
that cost. Two ping-pong fields and two ray caches occupy about 3.66 MiB at
these defaults without emitters. Widened ray caches and producer metadata add
about 0.86 MiB per selected source at 1,152 probes, excluding geometry and
driver allocations. No extra forward sampler or texture read is introduced
by emitter integration.
The receiver's visibility bias is limited to a quarter of its distance to
each probe. Facing weights use the unbiased receiver, with the direction's
length bounded below by a quarter of a spacing: a probe almost on a surface
must not create a point-sized weight spike at its projection.
Blend confidence is normalized against reachable valid probe support, not the
original eight-corner weight sum: disabling a probe inside a solid, or rejecting
one behind an exact blocker, must not darken the surface beside it. The segment
test runs once per probe and feeds both the lighting sum and its nominal
confidence denominator. Moment visibility and facing still attenuate
confidence; a fully blocked neighborhood receives no light. Within the field,
low confidence attenuates indirect light
instead of injecting unoccluded hemisphere ambient. Outside the field and
with GI off the usual ambient fallback is unchanged.
Interpolation selects its cell a tenth of a spacing into the surface's
outward hemisphere; occlusion still tests the original receiver with the
smaller, distance-limited visibility bias.

Opaque built-in surface shaders apply stable, world-seeded, zero-mean
one-code-value dithering before 8-bit scene output when GI is enabled. This
reduces visible quantization contours in dark gradients, not actual
irradiance undersampling. It is not applied to GI data, transparent output,
or the GI-Off baseline, and does not clamp HDR values to one.

The first pass caches static triangle hits. Later frames relight those hits
without retracing static geometry. Relighting intersects a bounded set of
moving oriented-box proxies and evaluates current shadowed lights. A gather
pass stores first-order spherical-harmonic irradiance and directional
distance moments in an 8×8 octahedral map per probe. Each distance bin filters
a wrapped 3×3 neighborhood of existing rays, using solid-angle and cosine^16
weights. It stores E[d] and E[d²], not a filtered distance squared. Distances
are capped at two spacings for this visibility field (beyond the queried cell),
so distant sky hits cannot inflate variance arbitrarily. This removes angular
wedges without new rays, fields, or per-pixel reads; gather does 576 instead of
64 distance reads per updated probe, an extra 16,384 reads/frame at defaults.
Transport ray distances and cached hits are unchanged.
Invalid rays contribute neither distance nor SH normalization weight. Empty
angular footprints have an explicit no-data marker; bilinear lookup excludes
those samples as well. Exhausted traces invalidate a probe separately from
solid/backface classification, rather than being interpreted as sky or geometry.
The main forward pass interpolates these moments with visibility and normal
weighting. Direct lighting and reflections retain their existing
render paths. Probes are shared between eyes, not reconstructed from screen
depth. Compute shaders and hardware ray-tracing support are not required.

Thin moving blockers and actual static box primitives also receive an explicit
probe-to-receiver OBB segment test, both for display and feedback. This rejects
dark exterior probes as well as bright leaking probes: distance moments alone
can give substantial weight to probes behind a room wall. Arbitrary mesh bounds
are never treated as solid static boxes. Static box selection and cell lists
are prepared on the worker pool when the scene/grid changes. CPU broad-phase lists select up to four
nearby blockers per grid cell, including relocated-probe bounds. Crowded cells
fall back to the complete bounded list, never drop an occluder. Lists and OBB
transforms share the irradiance texture (40 texels per row); no additional
scene sampler is required. They refresh every frame, independently of probe
sweeps. Only the moving subset is intersected by transport rays; static triangle
hits remain cached. Relighting reads the previous frame's topology and its
matching blocker count with history; spawning/removing an actor must not scan
unwritten records. Gather and display use the current topology/count.
Empty cells skip segment tests. A nearby blocker adds up to 24 texture reads
and eight slab tests; a crowded cell can be materially more expensive. This
is a quality-path cost, not a free fix or a headset performance claim. Exact
receiver visibility admits 32 static boxes intersecting the padded volume,
nearest to its center first, plus the existing 32 moving proxies. Thus overflow
scans at most 64 boxes (up to 512 slab tests / 1,536 row fetches for eight probes),
not the whole world. `static_blockers` and `static_blocker_fallbacks` report
admission. Additional static boxes retain triangle/moment visibility and may
still leak; no triangles are dropped. The default field dimensions are unchanged;
small fields reserve at least 64 rows for the combined bank.

Relighting also samples the previous frame's irradiance at each surface hit,
allowing subsequent sweeps to propagate additional diffuse bounces. Diffuse
reflectance is capped at 0.9, feedback defaults to 0.85, and radiance remains
bounded; this is damped approximate transport, not a radiometric reference.
Gather reads one field and writes the other, copying untouched rows in the
same pass. Small coefficient changes retain 35% history; large changes and
visibility changes update immediately. No recursive rays are added, but hit
shading now includes probe sampling, and copying the field adds bandwidth.
The 64 static directions remain fixed: updates do not converge angular detail.
Turning a source off takes multiple sweeps to remove higher-bounce energy.

### Bright static box emitters

Bright panels can occupy only a few of the 64 directions, causing large,
stationary energy errors between neighboring probes. Fast GI splits out up to
two actual static emissive box primitives, selected stably by brightness,
surface area, and distance to the local volume center. This is not an AABB
approximation for arbitrary emissive meshes. All their triangles remain in
the static BVH as occluders.

During placement, the worker divides each visible box face into 2x2 tiles
(at most three visible faces / twelve samples). For each tile it analytically
integrates the full-sphere DC/L1 coefficients of its emitted radiance. Solid
angles use spherical triangles; first moments use spherical polygon boundary
integrals. See [PBRT's spherical geometry](https://www.pbr-book.org/4ed/Geometry_and_Transformations/Spherical_Geometry)
for the solid-angle construction. The result is exact for this **unoccluded,
truncated SH representation**, not exact clamped-cosine irradiance.

One GPU ray to each tile center estimates static visibility and is cached
alongside the existing hits. Every relight checks the same segment against
current moving OBBs. Blocked or exhausted target traces contribute no emission;
there is no unshadowed fallback. Partial visibility within a tile is approximate
and can still produce coarse changes. These samples add emitted radiance only,
outside the original 64-ray normalization, distance moments, and probe
classification. Triangle source IDs suppress the separately integrated emission
in ordinary hit shading, while preserving reflected light.

Sources outside the two-box budget and non-box/moving emitters keep the ordinary
64-ray estimator. At a probe where the selected emitter cannot be integrated
safely or is not entirely within the trace range, its header is inactive and
ordinary emission remains enabled. Source selection, target geometry, and
integrals rebuild only with scene/grid preparation, not every frame. The data
shares the producer-only positions texture; the displayed probe field is still
40 texels wide. `GiStats::selected_emitters`, `emitter_fallbacks` (relevant
unselected sources), and `emitter_probe_fallbacks` (selected source/probe pairs
using the original estimator) report admission separately.

This removes the selected panel's sparse-direction energy error, not all
possible GI undersampling: reflected lighting, arbitrary emitters, probe
interpolation, and partial-tile occlusion remain approximate. The optional
mode still defaults Off, including Quest; these changes are not a headset
performance claim.

`anchor: Some(center)` fixes the volume independently of camera motion. The
GI room uses `(0,2,0)`, so walking does not invalidate its field. Unanchored
volumes still reset on snapped-grid changes. Realm changes clear the anchor.
Strength and feedback changes preserve geometry/history; other configuration
changes invalidate it.

Local shadow quality is independent: `Renderer::set_soft_local_shadows(true)`
enables approximate contact-hardening shadows; the baseline uses four bilinearly
weighted comparison taps. The soft path uses five bilinear blocker queries
(20 depth reads) and a dense, continuously weighted filter (up to 64 reads).
Its support grows with caster/receiver separation, capped at four shadow
texels. This costs more shading work, but adds no shadow render targets,
GI rays, temporal noise, or dependency on TAA.
`Renderer::set_local_shadow_source_radius(radius)` sets the apparent emitter
radius in world units, default 0.4, clamped to 0–2. This is currently shared
by local lights, not a per-light physical emitter shape.
Sandbox enables the softer filter when opting into Fast GI,
and retains it when toggling GI off for an otherwise identical comparison.
Depth bias scales with the projected shadow-texel size and the filter's
support, preventing adjacent floor/wall samples from making a false dark seam.
This is bounded shadow-map filtering, not physically traced area-light penumbrae.
The GI fixture allocates one 1024² spotlight tile; small atlases now omit
unused columns. Its atlas is smaller than the default eight-slot 512² atlas,
but its one shadow renders four times as many texels. This reduces spatial
aliasing, not a claim that shadow-map aliasing is eliminated. Ordinary engine
shadow defaults remain unchanged.

BVH preparation runs on the platform worker pool; the UI never waits for
completion. A busy pool is reported and retried. Static render/paint/model
revisions and terrain/voxel mesh changes invalidate the prepared scene.
Light changes and moving proxy transforms do not invalidate static hits.
Placement surveys 26 directions per probe, at most four relocation iterations
plus final classification. The target surface clearance is 0.2 spacings; total
displacement is bounded to 0.45 spacings. Probes that cannot safely exit solids
remain inactive. The static BVH stays resident on CPU for grid changes; its
estimated payload and placement time are reported separately. The position
texture is producer-only, not another forward-shading texture. Placement is
bounded and heuristic, not a proof of free space for arbitrary thin meshes.

Hard admission limits are 100,000 static triangles, 4,096 static instances,
64 MiB of copied source data, 32 in-range moving proxies and eight selected
local lights. Static traversal is bounded by min(tree nodes, 2048), with a
256-node minimum cap. Initial batches shrink by `256 / cap`, retaining the
old default worst-case node-visit budget (minimum dispatch: one 64-ray probe).
For a large tree the default becomes four probes / 256 rays per startup frame,
then returns to 32 probes / 2048 relight rays after the static sweep. Over-budget
geometry/proxies disable GI rather than silently removing blockers;
exhausted rays are invalid, not sky misses. Baseline lighting remains the
fallback. Geometry textures and the clamped grid fit 2,048-pixel dimensions.

## Backend requirements

Native backends need RGBA float render targets. WebGL2 additionally requires
`EXT_color_buffer_float`; without it, no GI preparation or update is
scheduled. Data textures use nearest sampling, so float-linear filtering is
not needed. The WebGL backend now preserves RGBA32F/RGBA16F target formats
instead of substituting RGBA8. Driver-level WebGL and headset validation
are separate from Rust cross-compilation and JavaScript unit tests.

Full rigid PBR uses 16 fragment textures and one vertex-only morph texture.
The engine's combined texture-binding capacity accommodates that seventeenth
binding; it is not a requirement for 17 fragment texture units.

## Current limitations

- Experimental low-frequency diffuse GI, not Lumen/path-tracing parity.
  No glossy GI, traced reflections or HDR output
  overhaul. The existing renderer's light/color units are retained.
- Static triangles use averaged vertex/base-color samples and approximate
  diffuse material response. Normal maps, textured emission, alpha-cutout
  holes and transparent transmission are not traced accurately.
- Doors, cars and characters use conservative box proxies. Animated mesh
  silhouettes and soft-body deformation are not exact GI geometry.
- Only local lights with allocated realtime shadows contribute reflected
  light; other direct lights remain direct-only. Sun bounce outside known
  CSM coverage is suppressed. Small emitters may be missed by sparse rays.
- Non-box static geometry and boxes beyond the local exact budget still use
  coarse distance moments, not exact receiver rays. Blocker tests enforce
  visibility to admitted static boxes and moving OBB proxies, not exact animated
  silhouettes. Residual angular undersampling and low-frequency variation
  remain; there is no screen-space denoiser or rotated-ray accumulation.
- Crossing a snapped grid boundary currently resets the field; it refills
  incrementally. Overlap reuse and prioritized dirty regions are follow-ups.
  Static edits currently rebuild the bounded scene, not individual chunks.
- Large worlds may exceed admission limits. This first version does not
  silently crop distant geometry that could otherwise block light.

## Diagnostics

`Renderer::gi_stats()` exposes preparation time, scheduled trace/relight
rays, memory estimates, selected lights, mover overflow, queue/build state,
and asynchronous GPU pass timings. `ready_probes` counts updated slots, not
GPU-confirmed valid probes. Memory estimates exclude driver allocations.
`set_gi_debug(GiDebug::...)` provides the F4 views to other hosts. Probe-state
colours: green valid, red solid/backface-invalid, magenta traversal exhausted,
gray uninitialized. Placement statistics count CPU classifications only; GPU
relighting can invalidate additional probes inside movers. Statistics also
include placement time, relocated/inactive counts, retained CPU BVH bytes,
traversal cap, cells with blockers, and full-list overflow cells.

`MAKEPAD_GI_STATS=1` logs producer-pass timings and Sandbox scene-pass
timings separately. Scene timings include GI sampling but exclude shadow
and GI producer passes. Compare fixed cameras and resolutions after warm-up;
do not call the GI producer timings the feature's total cost. No Quest
performance claim follows from desktop measurements.

The CPU alternative can be reproduced with the release
`makepad-raytrace` example `gi_cpu_budget`. On an M3 Max, 2,048 closest-hit
rays measured about 5–13 ms for its 3.7k–98k triangle scenes, before relighting.
That measured implementation was rejected as the ongoing GI tracing path.

## Technique references

- [Production DDGI](https://arxiv.org/html/2009.10796v2): previous-field
  feedback, adaptive history and power-cosine distance moments inform this
  transport update. Our bounded nine-ray filter is an approximation.
  This implementation now includes bounded static relocation/classification,
  but not the paper's rotated rays or multi-resolution volumes.
- [NVIDIA PCSS](https://developer.download.nvidia.com/assets/gamedev/docs/PCSS_Integration.pdf):
  blocker search and penumbra estimation; our capped filter is approximate.
- [Antialiased moment shadow maps](https://momentsingraphics.de/GDCEurope2016.html):
  an alternative to evaluate for the quality tier. Filterable moments can
  move filtering work into the shadow atlas, with precision, light-leak,
  memory and prefiltering trade-offs. Not implemented here.
