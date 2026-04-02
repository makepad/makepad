# XR Remote parity notes

`examples/xr_remote` is the host/Quest split variant of `examples/xr`.

## What currently matches `examples/xr`

- `src/shared_scene.rs::test_scene` mirrors the desktop example's cube/pedestal
  test scene.
- `src/shared_scene.rs::tree_scene` mirrors the desktop example's fractal tree
  scene.
- `src/scene.rs` keeps software-rasterized `TEST_SCENE_BOXES` and
  `TREE_SCENE_BOXES` aligned with those same local-scene layouts so the stream
  preview and encoded eye frames use the same scene IDs as Quest local-scene
  mode.
- The replicated marker path is shared between host and Quest local-scene mode.

## Known parity gaps versus `examples/xr`

`xr_remote` does not yet mirror the full desktop XR sample surface area.
The current remote example is intentionally narrower:

- Missing scene parity: `block_scene`, `ico_box_scene`, `ico_shoot_scene`,
  `helmet_scene`, and `refraction_scene`.
- Missing physics control parity: the remote example does not currently expose
  the desktop example's physics time-scale controls or reset button.
- Missing debug/control-strip parity: the desktop-only depth toggles, TSDF/debug
  counters, and wrist menu controls are not present in the remote example.

For the streaming fix work, the practical lane-3 baseline is:

1. keep the cube/test scene and tree scene aligned between `examples/xr`,
   `shared_scene.rs`, and `scene.rs`;
2. preserve the replicated marker behavior in Quest local-scene mode; and
3. treat the extra desktop-only scenes/physics controls as follow-up parity
   work instead of silently assuming they already exist in `xr_remote`.

## Regression checks used for this lane

```sh
cargo check -p makepad-example-xr
cargo check -p makepad-example-xr-remote
```

Both checks should stay green when editing the shared-scene parity files.
