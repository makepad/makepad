## Zune-png

A fast, correct and safe png decoder

## Limitations

- This decoder (currently) expands images with less than 8 bpp to be 8 bits(one byte)
  automatically.
  This may or may not be desired depending on your use cases.

## Features

- Fast deflate decoder
- Vectorized filters and bit manipulation
- Memory friendly (few allocations)
- Zero unsafe outside of platform specific intrinsics
- Support for animated image decoding up until the post-processing.

## Usage

First, include this in your Cargo.toml

```toml
[dependencies]
zune-png = "0.2.0"
```

Then you can access the decoder in your library/binary.

```rust
use zune_png::PngDecoder;
// decode bytes
let decoder = PngDecoder::new(b"bytes").decode().unwrap();
```

## Debug vs release

The decoder heavily relies on platform specific intrinsics, namely AVX2 and SSE to gain speed-ups in decoding,
but they [perform poorly](https://godbolt.org/z/vPq57z13b) in debug builds. To get reasonable performance even
when compiling your program in debug mode, add this to your `Cargo.toml`:

```toml
# `zune-png` package will be always built with optimizations
[profile.dev.package.zune-png]
opt-level = 3
```

## Benchmarks

The updated benchmarks comparing this decoder with other Rust and C decoders can be
found [here](https://etemesi254.github.io/assets/criterion/report/index.html) with
the `png` prefix. Benchmarks are updated regularly to keep up with optimizations added.
