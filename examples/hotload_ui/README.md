# hotload_ui

This example is a small dynamic-linking experiment around `makepad-widgets-dll`.

Build or run it from this directory so Cargo picks up the local `.cargo/config.toml`:

```bash
cargo run
```

If you build it from the workspace root instead, pass the same Rust flag explicitly:

```bash
cargo run -p makepad-example-hotload-ui --config 'build.rustflags=["-C","prefer-dynamic"]'
```
