# Arcade budgets

What a generated game can spend and still hold frame time. This feeds Fable's
system prompt so generated games stay inside the envelope by construction
rather than being profiled after the fact.

**Status: measured on an M-class Mac (release), extrapolated conservatively
for Quest. The Quest column is an ESTIMATE until it is run on device.**

## Simulation

| thing | measured | notes |
|---|---|---|
| 100 movers + 50 rigid bodies + 65×65 terrain | **0.038 ms/tick** | M1a, release. The 60 Hz budget is 16.6 ms, so the sim is ~0.2 % of it |
| entity lookup | O(log n) | binary search over the sorted-id Vec (M0r) |
| script per tick | ≤ 2 ms | one cumulative 500k-instruction pool shared by on_tick + timers + touch events |

The sim is nowhere near the limiting factor. Draw calls and CPU skinning are.

## Vertex and instance bandwidth

Quest is bandwidth-bound before it is ALU-bound, so this is the number that
matters most on device.

| stream | before | after | note |
|---|---|---|---|
| cube instance | 44 floats / **176 B** | 32 floats / **128 B** | **−27 %**, measured from the compiled shader (`RenderStats::instance_floats`), not counted by hand |
| mesh vertex (PbrVertex) | 16 floats / 64 B | unchanged | see "not yet landed" below |

The instance saving came from moving `sun_color`, `sun_sky`, `sun_ground` and
`fog_color` **off the instance stream into shader uniforms**. They are
identical for every instance in a batch, so as instance fields they were 12
floats of pure duplication per cube. `fog_density` stays per-instance because
shadows switch it off individually. At the demo's instance counts that is
48 B × every cube in the frame, every frame.

### Packed vertices — designed, not yet landed

The constraint found while investigating: **vertex attributes in this engine
are f32-only**. There is no u8/u16/i16 attribute type, so "compress the
vertex" means bit-packing into f32 lanes and unpacking in the shader. The
engine already provides `unpack2f16` and `unpack4u8` as builtins on every
backend (Metal/GLSL/HLSL/WGSL), and `geom.VectorVertexPacked` is the house
precedent — `uv: f32` holding two f16s, `color: f32` holding four unorm8s.

A packed skinned/terrain vertex would therefore be:

| field | packing | floats |
|---|---|---|
| position | 3 × f32 (kept exact) | 3 |
| normal | octahedral, 2 × f16 in one lane | 1 |
| uv | 2 × f16 in one lane | 1 |
| colour | 4 × unorm8 in one lane | 1 |
| **total** | | **6 floats / 24 B** vs 16 floats / 64 B — **−62 %** |

The Knight is the biggest single vertex load in the app: 3716 verts, and
because skinning is CPU-side it is **re-uploaded every frame** — 238 KB/frame
today, 89 KB/frame packed. Joint indices/weights are not in the uploaded
stream at all (skinning happens before upload), so they cost nothing here.

Blocker for a later session: the packed layout needs a new pod vertex type
registered as `geom.*`, which also needs a placeholder `GeometryGen` — and
`GeometryGen::from_triangle_*` plus its `shared()` helper live inside `draw/`
and are not reachable from `libs/game/render`. Either export them or register
the type from `draw/geometry_gen.rs` alongside `PbrVertex`.

## Rendering

One draw call per shape per pass, plus one per skinned character. Particles and
shadows join the existing alpha batch, so **neither adds a draw call**.

| thing | desktop | Quest (est.) | why |
|---|---|---|---|
| entities | 2000 | 600 | instance packing is cheap; fill rate is not |
| skinned characters | 8 | 2–3 | CPU skinning: ~3.7k verts each, re-uploaded every frame |
| projected shadows (`shadow_budget`) | 24 | 8 | 0.6 µs for 24, ~2 µs for 64–128 — the CPU cost is trivial, the fill cost of large ground quads is not |
| particles (`ParticleSystem::cap`) | 2000 | 500 | see below |

### Particle cost (measured, per frame, step + instance build)

| cap | live | step | instances | total |
|---|---|---|---|---|
| 500 | 500 | 0.8 µs | 0.7 µs | **1.5 µs** |
| 1000 | 1000 | 3.1 µs | 2.1 µs | **5.2 µs** |
| 2000 | 2000 | 3.1 µs | 2.8 µs | **5.9 µs** |
| 4000 | 4000 | 11.5 µs | 14.5 µs | **26 µs** |

CPU cost stays negligible even at 4000. The real limit is **overdraw**: every
particle is an alpha-blended quad, and a Quest fills pixels far more slowly
than it runs this loop. Hence the 500 cap there — it is a fill-rate budget, not
a CPU one.

### CPU light bake (bake.rs), measured release

| stage | cost | when it runs |
|---|---|---|
| AO (per static, 5 face samples × 8 rays) | **15 µs** | world edits only — sun-independent |
| sun visibility (1 ray per static + per probe) | **34 µs** | world edits **and** whenever the sun swings past 0.03 rad |
| probe lattice sky term | **61 µs** | world edits only |

Measured on the demo world (12 statics, 13 occluders, 605 probes). Debug
builds are ~50× slower (5.7 ms for the probe pass) — measure in release.

The split matters: AO is the expensive half and does not depend on the sun, so
a day/night cycle only pays the 34 µs sun pass. A ray that starts above the
heightfield's highest point and travels upward skips the terrain march
entirely, which is what keeps the probe pass in microseconds.

Bake output costs **zero** bandwidth and zero GPU: it is folded into the
instance colours the renderer was already sending.

### Shadow tiers

Casters are ranked by camera distance. The nearest `shadow_budget` get a
projected silhouette; everything else gets a blob. Both cost one instance, so
the budget buys fidelity rather than draw calls. Rigid bodies and anything
person-sized (≥ 0.5 units tall) count as heroes; smaller movers always get
blobs.

## Setting the budgets

```rust
renderer.set_shadow_budget(8);          // standalone XR
particles.set_cap(500);                 // standalone XR
```

Lowering these on one device is safe: particles and shadows are tier-3 Local
(game.md), so two devices in the same room may draw different numbers of them
and the simulation cannot diverge — particles never touch the world RNG, which
`particles_never_advance_the_world_rng` asserts.

## Rules of thumb for generated games

- A racing game with 4 cars, a track of ~200 static pieces and dust particles
  sits at a few percent of frame budget on desktop.
- Prefer one emitter attached to a moving entity over per-frame bursts: an
  emitter costs one request, bursts cost one per call.
- Characters are the expensive thing. Two or three on Quest, not eight.
- Terrain above ~129 cells starts to matter for eval time, not draw time (the
  isolate's wall-clock budget is 64 ms and it is a hard bail, not a yield).
