# XR Headless Test HOWTO

These XR UI loopback tests are integration-test crates gated with:

```rust
#![cfg(headless)]
```

So run them with both:

- `MAKEPAD=headless`
- `RUSTFLAGS='--cfg headless'`

`MAKEPAD=headless` enables the headless backend in Makepad itself.
`RUSTFLAGS='--cfg headless'` makes Cargo compile the integration test file at all.

The test harnesses already use headless `no-draw` mode internally via
`Cx::headless_no_draw_event_loop_for_draw_cycles(...)`.
That means they still run the event loop and draw-time code paths, but skip actual GPU/UI raster work.

Use `--release` for the loopback multiplayer tests.

## Shooter Loopback

Build only:

```sh
RUSTFLAGS='--cfg headless' MAKEPAD=headless cargo test -p makepad-xr --test ui_shooter_loopback --no-run
```

Single-app sanity:

```sh
RUSTFLAGS='--cfg headless' MAKEPAD=headless cargo test --release -p makepad-xr --test ui_shooter_loopback single_headless_shooter_app_emits_projectiles_from_synthetic_xr_updates -- --nocapture
```

Two-app loopback replication:

```sh
RUSTFLAGS='--cfg headless' MAKEPAD=headless cargo test --release -p makepad-xr --test ui_shooter_loopback two_headless_shooter_apps_emit_and_replicate_projectiles_over_loopback -- --nocapture
```

Desktop-like observer path:

```sh
RUSTFLAGS='--cfg headless' MAKEPAD=headless cargo test --release -p makepad-xr --test ui_shooter_loopback two_headless_shooter_apps_replicate_projectiles_to_desktop_like_observer -- --nocapture
```

Desktop-like wall-bounce regression:

```sh
RUSTFLAGS='--cfg headless' MAKEPAD=headless cargo test --release -p makepad-xr --test ui_shooter_loopback two_headless_shooter_apps_replicate_wall_bounces_to_desktop_like_observer -- --nocapture
```

OpenXR aim-point replication:

```sh
RUSTFLAGS='--cfg headless' MAKEPAD=headless cargo test --release -p makepad-xr --test ui_shooter_loopback two_headless_shooter_apps_emit_and_replicate_projectiles_from_openxr_aim_point -- --nocapture
```

Ignored long-hold regressions:

```sh
RUSTFLAGS='--cfg headless' MAKEPAD=headless cargo test --release -p makepad-xr --test ui_shooter_loopback two_headless_shooter_apps_keep_replicating_projectiles_during_long_hold -- --ignored --nocapture
```

```sh
RUSTFLAGS='--cfg headless' MAKEPAD=headless cargo test --release -p makepad-xr --test ui_shooter_loopback two_headless_shooter_apps_repro_frozen_observer_projectiles -- --ignored --nocapture
```

## Shared Object Loopback

Build only:

```sh
RUSTFLAGS='--cfg headless' MAKEPAD=headless cargo test -p makepad-xr --test ui_shared_object_loopback --no-run
```

Single-app shared-cube sanity:

```sh
RUSTFLAGS='--cfg headless' MAKEPAD=headless cargo test --release -p makepad-xr --test ui_shared_object_loopback single_headless_shared_cube_app_grabs_moves_and_releases_cube -- --nocapture
```

Two-app shared-cube loopback:

```sh
RUSTFLAGS='--cfg headless' MAKEPAD=headless cargo test --release -p makepad-xr --test ui_shared_object_loopback two_headless_shared_cube_apps_take_over_and_release_cube_over_loopback -- --nocapture
```

## Notes

- These multiplayer tests bind localhost UDP/TCP ports. If you run them in a restricted sandbox, socket bind can fail.
- `--nocapture` is useful because test failures include detailed app state dumps.
- The shooter harness uses bounded headless draw cycles, so peer discovery still needs some real-time slack; if you add new long-running cases, prefer copying the existing `run_test_app_with_limits(...)` pattern.
