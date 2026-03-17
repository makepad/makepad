# makepad-media

Optional media implementation workspace for `makepad-platform`.

`makepad-platform` stays host/API only. Codec/container/runtime implementations live here.

## Crates

- `makepad-media` — single media crate with:
  - media packet/types + H264 packet helpers
  - MP4 demux/mux helpers
  - software AV1 runtime (`dav1d` decode, `svt-av1` encode)
  - plugin bridge (`makepad_media::install()`)

## App integration

```rust
use makepad_media;

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_media::install();
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }
}
```

Without `install()`, `makepad-platform` media host stays active but no external codec plugin is registered.

## Moved examples

Video examples are in this workspace:

- `examples/video_formats`
- `examples/camera_av1_capture`

Build from the Makepad workspace root:

```bash
cargo run --manifest-path tools/cargo_makepad/Cargo.toml -- android --abi=aarch64 build -p makepad-example-video-formats --release
cargo run --manifest-path tools/cargo_makepad/Cargo.toml -- android --abi=aarch64 build -p makepad-example-camera-av1-capture --release
```
