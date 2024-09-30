## Build & Run

```sh
cargo build --release --manifest-path ../../c-api/Cargo.toml
meson builddir --buildtype release
ninja -C builddir
builddir/outline
```
