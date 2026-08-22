# DirectComposition test harness

Manual walkthrough of `docs/pr/makepad-direct-composition.md`'s test plan.

```bash
cargo run -p makepad-example-direct-composition
```

The left window is the default HWND path. The lab and twin windows on the right set `window.direct_composition` at create time. Click **Run** on a card and watch the lab.

T17–T20 are `cargo` commands, not UI scenes:

```bash
cargo test -p makepad-platform --lib window::tests
cargo test -p makepad-platform --lib dcomp::tests
cargo check -p makepad-platform --target aarch64-linux-android
cargo run --release --manifest-path tools/windows_strip/Cargo.toml
```
