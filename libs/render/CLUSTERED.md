# Clustered forward lighting

`Renderer` defaults to clustered local lights. Geometry submission, static slabs,
instancing, and GPU skinning are unchanged: one forward material pass, with
separate sun-shadow passes. No G-buffer, geometry prepass, compute shader, depth
readback, or per-object light selection is required by clustering.

This follows the CPU-assigned clustered-forward approach described in
[Filament's rendering documentation](https://google.github.io/filament/main/filament.html).
It is a first lighting increment, not Filament feature or visual parity.

## Lighting and shadow policy

| Local lighting | Sun mode | World lighting work |
| --- | --- | --- |
| Clustered (default) | Realtime | CSM; no world atlas or CPU probe bake |
| Clustered | OnChange | Sun-only atlas and legacy CPU probes |
| Legacy (`MAKEPAD_CLUSTERED=off`) | Either | Original eight-light selection and lamp atlas |

Baked per-model AO is retained in every mode. Clustered lights reach primitives,
terrain, diffuse/PBR/custom models, and skinned characters per fragment. PBR
materials evaluate local GGX specular as well as diffuse. Explicitly prelit
materials retain their existing unlit/baked-color semantics.

Local lights can request budgeted realtime shadows; see [Entity lights](ENTITY_LIGHTS.md)
for Splash headlights, point/spot fixtures and shadow budgets. Generic lights
default to unshadowed and can shine through walls. The sun retains its own tier.
Engine-authored lights use `(1 - distance / radius)^2`; imported glTF punctual
lights use their separate photometric mode. Full linear HDR, exposure/tone
mapping, IBL and unified PBR materials remain follow-ups. Clustering does not
itself add MSAA.

## Budgets and stereo

`Renderer::set_cluster_config(ClusterConfig { ... })` controls XY tiles, depth
slices, lights per cluster, and total uploaded lights. Defaults are 16 × 9 × 24,
32 lights/cluster, and 1024 total. Hard limits are 32 × 24 × 32, 64 lights/cluster,
and 4096 total. `set_clustered_lighting(bool)` switches the complete legacy/new
lighting policy and invalidates old baked state. Policy survives realm changes.

Flat views use frustum tiles with logarithmic depth. The backend currently
late-latches XR eye matrices without exposing them to this host. XR therefore
uses a conservative world-space 3D grid over light influence, shared by both
eyes—not clusters derived from an unrelated flat camera. Large, dispersed worlds
make that fallback less selective. Exact stereo/per-eye frustum clustering needs
an eye-view snapshot API before it can replace this fallback.

Smaller `lights_per_cluster` bounds fragment cost but may visibly omit lights;
finer grids cost CPU/memory but can reduce false-positive light references. Start
device tuning with the counters, not a claimed universal Quest preset. This path
has not yet been benchmarked on a headset.

`RenderStats::clustered` / `Renderer::cluster_stats()` report build/upload-queue
CPU microseconds, uploaded bytes, occupancy, references, rejected lights, and
overflow. They do **not** measure GPU time. Per-cluster overflow retains lights
with the strongest conservative contribution estimate; equal scores retain the
earlier input. The global cap keeps the first valid input lights. Counts are
explicit because neither cap promises all lights survive. Overflow logs are
throttled; `MAKEPAD_CLUSTER_STATS=1` also logs frame counters periodically.

## Texture ABI

One nearest-sampled RGBA32F texture contains cluster headers, packed light
indices (four scalar indices per texel), then three texels per light:
position/radius, RGB/angular falloff, and normalized emission direction/mode. Headers
store scalar index offset/count. CPU list storage and texture upload vectors are
reused. Integer-valued floats are exactly representable within the hard caps.
The shader mixin and CPU packing live together in `src/clustered.rs`.

Local shadows use a separate nearest-sampled metadata texture (one light header,
four camera rows per face) and a depth atlas, preserving the light-data stride.
Inherited texture bindings are resolved by name for derived PBR materials.

## Reproduce

Build the sandbox in its own workspace, then launch from the checkout root:

```sh
# In apps/sandbox:
cargo build --release -p makepad-sandbox
# From the checkout root:
SANDBOX_WORLD=clustered MAKEPAD_CLUSTER_STATS=1 ./apps/sandbox/target/release/makepad-sandbox --remote
```

The fixture supplies 256 moving lights and a single large floor, independent of
downloaded models. `MAKEPAD_CLUSTER_DEMO_LIGHTS=N` changes the count;
`MAKEPAD_CLUSTER_DEMO_FREEZE=1` fixes light motion for A/B images. Add
`MAKEPAD_CLUSTERED=off` for the old path. That comparison deliberately shows
missing lights in the old eight-slot path; it is **not** equal-quality GPU work.
F8 switches sun modes. Finish owned test instances via the printed remote port's
`/gq` endpoint.

```sh
cargo test --release -p makepad-render
cargo test --release -p makepad-render clustered::tests::benchmark_build_and_pack -- --ignored --nocapture
```

Coverage includes more than eight overlapping lights, depth/tile boundaries,
rotated and orthographic cameras, XR/world-grid lookup, empty/invalid inputs,
bounded overflow, texture packing, material shader compilation/binding order,
and mode/realm lifecycle. The ignored benchmark measures CPU build+pack only.
