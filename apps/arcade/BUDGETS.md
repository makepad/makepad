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
| skinned character vertex | 16 floats / 64 B | 6 floats / **24 B** | **−62 %**, and this one is re-uploaded *every frame* |
| shadow mesh vertex | 16 floats / 64 B | 6 floats / **24 B** | **−62 %** |
| terrain vertex | 16 floats / 64 B | unchanged | uploaded once per terrain revision, so the win is small; see below |

The Knight is the prize: 3716 verts × 64 B = **238 KB every frame** (skinning
is CPU-side, so the whole buffer is re-uploaded) → **89 KB**. On a
bandwidth-bound tiler that is the single biggest saving available.

The instance saving came from moving `sun_color`, `sun_sky`, `sun_ground` and
`fog_color` **off the instance stream into shader uniforms**. They are
identical for every instance in a batch, so as instance fields they were 12
floats of pure duplication per cube. `fog_density` stays per-instance because
shadows switch it off individually. At the demo's instance counts that is
48 B × every cube in the frame, every frame.

### How the packing works

**Vertex attributes in this engine are f32-only** — there is no u8/u16/i16
attribute type. Compression therefore means bit-packing into f32 lanes and
unpacking in the shader, which the engine already supports: `unpack2f16` and
`unpack4u8` are builtins on every backend (Metal/GLSL/HLSL/WGSL), and
`geom.VectorVertexPacked` is the house precedent.

`geom.GameMeshVertex` (`draw/geometry_gen.rs`) is the shared packed layout:

| field | packing | floats |
|---|---|---|
| position | 3 × f32, kept exact | 3 |
| normal | octahedral, 2 × f16 in one lane | 1 |
| uv | 2 × f16 in one lane | 1 |
| colour | 4 × unorm8 in one lane | 1 |
| **total** | | **6 floats / 24 B** |

Two gotchas worth keeping: the pod struct must use **flat `f32` fields, not
`Vec3f`** — std140 pads a vec3 to 16 bytes and the Rust repr(C) size then
fails the POD size assertion. And in the shader language `let` bindings are
immutable and helper fns cannot be forward-referenced, so the octahedral
decode uses a branchless `step(0,v)*2-1` for its sign rather than reassignment
or a shared helper (the `sign()` builtin returns 0 at 0, which would collapse
the fold on an axis-aligned normal).

Terrain still uses PbrVertex: it uploads once per terrain revision rather than
per frame, so the saving does not justify re-verifying gamemaker's 257×257
fixture. It is a mechanical follow-up if wanted.

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

## step_world weight (measured 2026-08-03, release, aarch64)

Measured with `cargo run -p makepad-game-sim --release --example weigh`, which
wraps the global allocator so the byte counts include everything the tick
touches, not just what the harness allocates.

The tick used to clone two things per tick purely to dodge a borrow: the whole
`Terrain` (its `heights: Vec<f32>` **and** `colors: Vec<Vec4f>`) and every
static/kinematic `Entity` (208 bytes each). Terrain dominated — a 257² field is
1.3 MB/tick, i.e. **79 MB/s of memcpy at 60 Hz on a world with seven entities
in it**. Splitting the struct borrow removes the terrain copy entirely; the
statics snapshot has to stay a copy (movers must sweep against *last* tick's
kinematic poses — that ordering is load-bearing) but now copies a 48-byte
`Solid` view instead of the full entity.

| scene | ms/tick before → after | B/tick before → after |
|---|---|---|
| demo (arcade) | 0.002 → 0.003 | 15,140 → 4,796 (−68%) |
| demo + terrain 65 | 0.003 → 0.004 | 99,640 → 4,796 (−95%) |
| racing-ish (129 terrain) | 0.007 → 0.002 (−71%) | 362,316 → 8,576 (−98%) |
| terrain 129 only | 0.005 → 0.000 | 335,804 → 896 (−99.7%) |
| terrain 257 only | 0.019 → 0.001 (−95%) | 1,323,964 → 896 (−99.93%) |
| large (500 static) | 0.063 → 0.056 (−11%) | 591,386 → 82,382 (−86%) |
| stress (2000 static) | 0.583 → 0.457 (−22%) | 2,353,936 → 327,812 (−86%) |

Allocations/tick fell from 6–14 to 3–11; the residual is the statics snapshot,
the box3d reconcile and touch collection.

**Leak check**: `--soak` runs 10 simulated minutes (36,000 ticks) of a busy
world with projectiles spawning and expiring throughout. RSS is flat at 3.8 MB
from warmup to the end (+0.4% drift, 242 entities alive) — the tick path does
not leak.

**Result-neutrality** is gated by `libs/game/sim/tests/mover_golden.rs`: the
golden world-state hash is byte-identical before and after this optimisation
(verified by reverting the source and re-running), and it covers the terrain,
sweep, platform-carry, attach, projectile-lifetime and auto-face paths that
`rigid_dynamics.rs` doesn't reach.

## Memory and binary, whole app (measured 2026-08-03)

| | value |
|---|---|
| sim core only, all 7 scenes incl. 2000-static stress | 13.4 MB RSS |
| sim soak, busy world, steady state | 3.8 MB RSS |
| `hello_world` (baseline makepad + widgets + headless) | 195 MB RSS, 13.5 MB binary |
| `makepad-arcade` (headless) | 948 MB RSS, 25.1 MB binary |

The engine core is genuinely small; the weight is above it. Two findings worth
acting on, both outside the sim/render/script crates:

1. **~750 MB of Arcade's RSS is not the sim** (13 MB) and not the framebuffer
   (unchanged when the headless size changes). It needs a profiler pass to
   attribute properly — candidates are the script isolates (each one
   re-evaluates the *entire* widgets DSL, `widget_async.rs:317`, and a game
   isolate needs none of those prototypes), the glyph/texture atlases, and the
   offscreen pass chain.
2. **The voice stack links unconditionally.** `makepad-converse` is a plain
   dependency of `apps/arcade`, not feature-gated, so Kokoro TTS is compiled in
   and initialised (`tts: backend Kokoro` appears in every boot log) even at the
   `chatbox` tier where it can never be used. `voice`/`local-llm` gate the
   *models*, not the crate. Gating this is the obvious binary-size win for a
   Quest build; the binary carries whisper/kokoro/silero symbols today.
