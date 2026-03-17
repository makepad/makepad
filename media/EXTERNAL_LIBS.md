# External AV1 C libraries

`makepad-media` keeps AV1 C libraries outside the main repository.

Default sibling layout:

- `../makepad-media-libs/dav1d`
- `../makepad-media-libs/svt-av1`

Relative to `makepad/media`, the default lookup path is `../../makepad-media-libs`.

Override with:

```bash
MAKEPAD_MEDIA_LIBS=/path/to/makepad-media-libs
```

Build with external AV1 libs enabled:

```bash
cargo build -p makepad-media --features dav1d,svt-av1
```

Without the external directory, `makepad-media` still builds, but the optional C-backed AV1 paths stay unavailable.
