# XR Loopback Test HOWTO

The XR UI loopback tests drive the app on a HAL headless session: the native
client runs its event loop and draw-time code with no OS window and no GPU.
Each harness opens `backend_id::HEADLESS` itself and asserts that the process
runs with `MAKEPAD_HAL_BACKEND=headless`, so set it on every run.

Use `--release` for the loopback multiplayer tests.

## Shooter Loopback

Build only:

```sh
cargo test --release -p makepad-xr --test ui_shooter_loopback --no-run
```

Single-app sanity:

```sh
MAKEPAD_HAL_BACKEND=headless cargo test --release -p makepad-xr --test ui_shooter_loopback single_headless_shooter_app_emits_projectiles_from_synthetic_xr_updates -- --nocapture
```

Two-app loopback replication:

```sh
MAKEPAD_HAL_BACKEND=headless cargo test --release -p makepad-xr --test ui_shooter_loopback two_headless_shooter_apps_emit_and_replicate_projectiles_over_loopback -- --nocapture
```

Desktop-like observer path:

```sh
MAKEPAD_HAL_BACKEND=headless cargo test --release -p makepad-xr --test ui_shooter_loopback two_headless_shooter_apps_replicate_projectiles_to_desktop_like_observer -- --nocapture
```

Desktop-like wall-bounce regression:

```sh
MAKEPAD_HAL_BACKEND=headless cargo test --release -p makepad-xr --test ui_shooter_loopback two_headless_shooter_apps_replicate_wall_bounces_to_desktop_like_observer -- --nocapture
```

OpenXR aim-point replication:

```sh
MAKEPAD_HAL_BACKEND=headless cargo test --release -p makepad-xr --test ui_shooter_loopback two_headless_shooter_apps_emit_and_replicate_projectiles_from_openxr_aim_point -- --nocapture
```

Ignored long-hold regressions:

```sh
MAKEPAD_HAL_BACKEND=headless cargo test --release -p makepad-xr --test ui_shooter_loopback two_headless_shooter_apps_keep_replicating_projectiles_during_long_hold -- --ignored --nocapture
```

```sh
MAKEPAD_HAL_BACKEND=headless cargo test --release -p makepad-xr --test ui_shooter_loopback two_headless_shooter_apps_repro_frozen_observer_projectiles -- --ignored --nocapture
```

## Shared Object Loopback

Build only:

```sh
cargo test --release -p makepad-xr --test ui_shared_object_loopback --no-run
```

Single-app shared-cube sanity:

```sh
MAKEPAD_HAL_BACKEND=headless cargo test --release -p makepad-xr --test ui_shared_object_loopback single_headless_shared_cube_app_grabs_moves_and_releases_cube -- --nocapture
```

Two-app shared-cube loopback:

```sh
MAKEPAD_HAL_BACKEND=headless cargo test --release -p makepad-xr --test ui_shared_object_loopback two_headless_shared_cube_apps_take_over_and_release_cube_over_loopback -- --nocapture
```

## Notes

- These multiplayer tests bind localhost UDP/TCP ports. If you run them in a restricted sandbox, socket bind can fail.
- `--nocapture` is useful because test failures include detailed app state dumps.
- The shooter harness runs a bounded number of headless draw cycles, so peer discovery still needs some real-time slack; if you add new long-running cases, prefer copying the existing `run_test_app_with_limits(...)` pattern.
