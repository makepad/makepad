# Entity lights

Sandbox Splash can attach named point lights and spotlights to any entity:

```javascript
let car = game.car({pos: vec3(0, 1, 0)})
game.headlights(car, true)
game.light(car, {
    name: "headlight_left"
    kind: "spot"
    pos: vec3(-0.7, 0.1, -2.1)
    dir: vec3(0, -0.08, -1)
    color: #fff0d1
    intensity: 4
    range: 30
    inner_angle: 12
    outer_angle: 25
    shadows: true
})
game.headlights(car, false)
game.light_remove(car, "headlight_left")
```

`game.light(owner, options)` upserts one name, preserving omitted settings.
The default name is `light`; default kind is `point`. It returns false and logs
invalid data, without replacing the previous fixture. `game.light_remove`
returns whether it removed one. `game.headlights` installs/toggles
`headlight_left` and `headlight_right` without resetting custom tuning.
Both `game.car` and `game.racecar` install enabled headlights by default;
`headlights: false` creates them disabled.

Position is body-local and follows entity scale. Direction follows the visible
entity rotation (including rigid-body pitch/roll), not scale. Forward is -Z.
Range is world units and does not scale. Cone angles are half-angles in degrees;
require `0 <= inner <= outer` and `0.5 <= outer <= 85`. Color is linear RGB in
0..1; intensity is a nonnegative artistic multiplier, not lumens/candelas.
Intensity is limited to 10000 and range to 0.05..10000. Names are 1..64 bytes;
there are at most 16 authored lights per entity.

Shadow casters include the owner: position the emitter outside its solid mesh.
This is surface lighting and occlusion, not a volumetric beam/fog effect.

Lights live in `Entity::lights`, clone and disappear with their owner, and are
transformed every frame. Hidden physics bodies may own visible fixtures.
There is no geometry rebuild or light bake on a light edit. Authored values
replicate reliably in protocol version 7; movement uses the existing entity
pose stream. Every peer must run the same protocol version. This does not add
new rigid-body orientation fields to the network protocol.

Named model/bone sockets are not part of this API. Use body-local offsets;
imported model emitters are a separate modeling API.

## Local shadows

In clustered mode, `shadows: true` requests realtime occlusion. Shadows are
off by default for generic lights and on for the headlight helper. The renderer
draws a depth atlas before the forward pass, including static models,
primitives, terrain/voxels, moving models/parts and skinned casters. Transparent
light transmission and alpha-cutout shadow silhouettes are not implemented;
dynamic primitive casters retain the existing box approximation.

```rust
renderer.set_local_shadow_config(makepad_render::LocalShadowConfig {
    max_faces: 2,
    resolution: 256,
});
```

Default budget: eight 512px faces. Limits: 0..16 faces, 128..1024px per face
(rounded up to a power of two). A spotlight uses one face; a point uses six.
The allocator prioritizes active lights by brightness/range/distance to the
camera. Allocation is all-or-nothing per light; an excluded shadow request is
omitted, not changed into a wall-leaking unshadowed light. This can cause popping
at the budget boundary. `RenderStats::clustered.shadows` reports selected lights,
faces, omitted requests, caster draws and CPU encoding time, not GPU duration.

Use a small spotlight budget on constrained devices; no Quest GPU performance
claim is implied by these defaults. Sun CSM remains independent. The legacy
`MAKEPAD_CLUSTERED=off` path does not support these local shadow/cone semantics.

## Verification fixture

After a release sandbox build, launch from the checkout root:

```sh
SANDBOX_WORLD=clustered MAKEPAD_CLUSTER_DEMO_HEADLIGHTS=1 \
  ./apps/sandbox/target/release/makepad-sandbox --remote
```

A moving body carries two lights toward a freestanding blocker and a rear wall.
`MAKEPAD_CLUSTER_DEMO_FREEZE=1` pins the body for comparison;
`MAKEPAD_HEADLIGHT_UNSHADOWED=1` removes the two shadow requests for an A/B run.
`MAKEPAD_HEADLIGHT_POINT=1` adds a blue, six-face point light.
`MAKEPAD_HEADLIGHT_SHADOW_FACES=N` overrides the atlas budget in this fixture.
Finish owned test instances through their printed `/gq` endpoint.
