# XR Loopback Test HOWTO

The XR UI loopback tests drive the app on a HAL headless session: the native
client runs its event loop and draw-time code with no OS window and no GPU.
Each harness opens `backend_id::HEADLESS` itself and sets
`MAKEPAD_HAL_BACKEND=headless` for its process before the client starts.

Use `--release` for the loopback multiplayer tests.

```sh
cargo test --release -p makepad-xr --test ui_shooter_loopback -- --nocapture
```
