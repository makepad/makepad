# XR Remote parity notes

`examples/xr_remote` is the host/Quest split variant of `examples/xr`.
The Quest side is now a remote video client only; it no longer renders a local
fallback copy of the scene graph.

## What currently matches `examples/xr`

- `src/host_scene.rs::test_scene` mirrors the desktop example's cube/pedestal
  test scene on the Mac host.
- `src/host_scene.rs::tree_scene` mirrors the desktop example's fractal tree
  scene on the Mac host.
- `src/scene.rs` keeps software-rasterized `TEST_SCENE_BOXES` and
  `TREE_SCENE_BOXES` aligned with those same host-side scene layouts so the CPU
  preview/encode path matches the GPU scene path.

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
   `host_scene.rs`, and `scene.rs`; and
2. treat the extra desktop-only scenes/physics controls as follow-up parity
   work instead of silently assuming they already exist in `xr_remote`.

## Regression checks used for this lane

```sh
cargo check -p makepad-example-xr
cargo check -p makepad-example-xr-remote
```

Both checks should stay green when editing the shared-scene parity files.


## Shell + adb offload proof (Quest ↔ Mac host)

Fresh shell/adb-only validation on 2026-04-02 confirmed that the Quest client can
reconnect to the Mac host at `192.168.2.23` without passing
`MAKEPAD_XR_REMOTE_HOST` manually.

### Host-side commands

```sh
cargo run --release -p makepad-example-xr-remote
lsof -nP -p <host-pid> | egrep 'UDP \*:44511|TCP \*:44510|TCP \*:41548|UDP \*:4154[67]'
```

Observed host evidence from the release run:

- `xr_remote host: GPU readback pipeline active (Mac VT encoder)`
- `xr_remote host: dual-eye media client connected, forcing keyframe`
- `xr_remote remote-log [info] quest-client ... startup mode=stereo`
- `xr_remote remote-log [info] quest-client ... left/right first complete frame cfg1`
- `xr_remote remote-log [info] quest-client ... left/right prepared`
- `xr_remote remote-log [info] quest-client ... left/right configured`

Observed host sockets during the same run:

- `UDP *:44511`
- `TCP *:44510 (LISTEN)`
- `UDP *:41546`
- `UDP *:41547`
- `TCP *:41548 (LISTEN)`
- `TCP 192.168.2.23:41548->192.168.2.120:<ephemeral> (ESTABLISHED)`

### Quest-side commands

```sh
adb -s 192.168.2.120:5555 logcat -c
adb -s 192.168.2.120:5555 shell am force-stop dev.makepad.makepad_example_xr_remote
adb -s 192.168.2.120:5555 shell am start -W -n   dev.makepad.makepad_example_xr_remote/.MakepadAppXr
adb -s 192.168.2.120:5555 shell netstat -tn | grep 192.168.2.23
adb -s 192.168.2.120:5555 logcat -d | rg 'makepad_example_xr_remote|VrRuntimeService|UseScenePermissionRssdk'
```

Observed Quest evidence from the relaunch:

- `Starting: Intent { cmp=dev.makepad.makepad_example_xr_remote/.MakepadAppXr }`
- `pid_after_start=19016`
- `192.168.2.120:56480 -> 192.168.2.23:41548 ESTABLISHED`
- `VrRuntimeService: RuntimeIPC: IPC_SYSTEM_EVENT_CLIENT_CONNECTED_EXT`
- `UseScenePermissionRssdk(..., true) -> ALLOWED`

### Current warnings seen during relaunch

These did **not** prevent the host-rendered path from connecting, but they are the
main follow-up items still visible in shell/adb logs:

- `Package ... is retrieving an HzOS SDK Manager (VolumetricWindowManager), but the client's manifest does not specify a minimum HzOS SDK version`
- `ACameraManager: openCamera ... cannot open camera "50" from background`
