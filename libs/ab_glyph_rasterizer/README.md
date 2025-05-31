ab_glyph_rasterizer
[![crates.io](https://img.shields.io/crates/v/ab_glyph_rasterizer.svg)](https://crates.io/crates/ab_glyph_rasterizer)
[![Documentation](https://docs.rs/ab_glyph_rasterizer/badge.svg)](https://docs.rs/ab_glyph_rasterizer)
===================
Coverage rasterization for lines, quadratic & cubic beziers.
Useful for drawing .otf font glyphs.

Inspired by [font-rs](https://github.com/raphlinus/font-rs) &
[stb_truetype](https://github.com/nothings/stb/blob/master/stb_truetype.h).

## Example

```rust
let mut rasterizer = ab_glyph_rasterizer::Rasterizer::new(106, 183);

// draw a 300px 'ę' character
rasterizer.draw_cubic(point(103.0, 163.5), point(86.25, 169.25), point(77.0, 165.0), point(82.25, 151.5));
rasterizer.draw_cubic(point(82.25, 151.5), point(86.75, 139.75), point(94.0, 130.75), point(102.0, 122.0));
rasterizer.draw_line(point(102.0, 122.0), point(100.25, 111.25));
rasterizer.draw_cubic(point(100.25, 111.25), point(89.0, 112.75), point(72.75, 114.25), point(58.5, 114.25));
rasterizer.draw_cubic(point(58.5, 114.25), point(30.75, 114.25), point(18.5, 105.25), point(16.75, 72.25));
rasterizer.draw_line(point(16.75, 72.25), point(77.0, 72.25));
rasterizer.draw_cubic(point(77.0, 72.25), point(97.0, 72.25), point(105.25, 60.25), point(104.75, 38.5));
rasterizer.draw_cubic(point(104.75, 38.5), point(104.5, 13.5), point(89.0, 0.75), point(54.25, 0.75));
rasterizer.draw_cubic(point(54.25, 0.75), point(16.0, 0.75), point(0.0, 16.75), point(0.0, 64.0));
rasterizer.draw_cubic(point(0.0, 64.0), point(0.0, 110.5), point(16.0, 128.0), point(56.5, 128.0));
rasterizer.draw_cubic(point(56.5, 128.0), point(66.0, 128.0), point(79.5, 127.0), point(90.0, 125.0));
rasterizer.draw_cubic(point(90.0, 125.0), point(78.75, 135.25), point(73.25, 144.5), point(70.75, 152.0));
rasterizer.draw_cubic(point(70.75, 152.0), point(64.5, 169.0), point(75.5, 183.0), point(105.0, 170.5));
rasterizer.draw_line(point(105.0, 170.5), point(103.0, 163.5));
rasterizer.draw_cubic(point(55.0, 14.5), point(78.5, 14.5), point(88.5, 21.75), point(88.75, 38.75));
rasterizer.draw_cubic(point(88.75, 38.75), point(89.0, 50.75), point(85.75, 59.75), point(73.5, 59.75));
rasterizer.draw_line(point(73.5, 59.75), point(16.5, 59.75));
rasterizer.draw_cubic(point(16.5, 59.75), point(17.25, 25.5), point(27.0, 14.5), point(55.0, 14.5));
rasterizer.draw_line(point(55.0, 14.5), point(55.0, 14.5));

// iterate over the resultant pixel alphas, e.g. save pixel to a buffer
rasterizer.for_each_pixel(|index, alpha| {
    // ...
});
```

Rendering the resultant pixel alphas as 8-bit grey produces:

![reference_otf_tailed_e](https://user-images.githubusercontent.com/2331607/78987793-ee95f480-7b26-11ea-91fb-e9f359d766f8.png)

## no_std
no_std environments are supported using `alloc` & [`libm`](https://github.com/rust-lang/libm).
```toml
ab_glyph_rasterizer = { default-features = false, features = ["libm"] }
```
