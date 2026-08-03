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
