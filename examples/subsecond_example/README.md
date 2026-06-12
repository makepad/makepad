## `makepad-example-subsecond-example`

Minimal example to validate **subsecond Rust hotpatching** (Dioxus subsecond) in Makepad apps.

### What to edit

Open `examples/subsecond_example/src/main.rs` and change the string returned by `hotpatch_message()`.

The marker should refresh automatically after the hotpatch is applied.

### Run on iOS Simulator (hotpatch)

From the repo root:

```bash
dx serve --hotpatch --ios --package makepad-example-subsecond-example
```

Edit `hotpatch_message()` and save — you should see `dx` report **Hot-patching** and the marker update without restarting the app.

### Run on macOS (hotpatch)

```bash
dx serve --hotpatch --macos --package makepad-example-subsecond-example
```

### Notes

- Even though this example doesn’t explicitly use camera/microphone, iOS can terminate the app if the process enumerates capture devices without usage strings. We include them via `Dioxus.toml` `[permissions]` to avoid TCC crashes.
- If you see `Hot-patching: ...` in the `dx` logs, the client handshake is working.
- The click counter is script state. It should keep its current value across Rust hotpatches, so you can confirm the patch does not force a full `script_mod!` reload.
