# SDF ("Signed Distance Field") generation algorithms

[![Crates.io](https://img.shields.io/crates/v/sdfer.svg)](https://crates.io/crates/sdfer)
[![Docs](https://docs.rs/sdfer/badge.svg)](https://docs.rs/sdfer)

## Algorithms

Currently, this library contains these SDF generation algorithms:
- [`sdfer::bruteforce_bitmap`](https://docs.rs/sdfer/0.2.0/sdfer/bruteforce_bitmap): bitmap-based "closest opposite-color pixel" bruteforce search
  - this is the algorithm popularized by Valve in their 2007 SIGGRAPH submission,
    ["Improved Alpha-Tested Magnification for Vector Textures and Special Effects"](https://steamcdn-a.akamaihd.net/apps/valve/2007/SIGGRAPH2007_AlphaTestedMagnification.pdf)
  - prohibitively expensive, as the lack of precision (per pixel) leads to needing
    much larger input images (Valve paper gives 4096x4096 -> 64x64 as an example)
- [`sdfer::esdt`](https://docs.rs/sdfer/0.2.0/sdfer/esdt): "Euclidean Subpixel Distance Transform"
  - this is a Rust port of the original JS implementation, from the
    [`@use-gpu/glyph`](https://www.npmjs.com/package/@use-gpu/glyph) `npm` package
  - the <https://acko.net/blog/subpixel-distance-transform/> blog post explains
    how the older EDT ("Euclidean Distance Transform") algorithm was modified to
    better use the information present in e.g. grayscale AA rasterization of glyphs
    (where "grayscale AA" is really an alpha channel encoding of per-pixel coverage)
  - scales (roughly) linearly with the number of output pixels (which are 1:1 with
    input pixels, so no oversized rasterization required either), making it far more
    viable than most other algorithms, for on-demand runtime glyph->SDF conversion  
    (e.g. to avoid pixel-snapping text during scrolling/panning, to allow some
    amount of smooth pinch-zooming before needing larger rasterizations, etc.)


<!-- FIXME(eddyb) write an explanation of pros-vs-cons, especially for ESDT -->

## License

Licensed under the MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT).

> <sub>_**Note**: this is MIT-only, instead of the common dual-licensing, mainly due to the
[ESDT algorithm implementation](src/esdt.rs) being a port of the JS code from the
[`@use-gpu/glyph`](https://www.npmjs.com/package/@use-gpu/glyph) `npm` package,
which itself is MIT-licensed._</sub>

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this crate by you, shall be licensed as MIT, without any additional
terms or conditions.
