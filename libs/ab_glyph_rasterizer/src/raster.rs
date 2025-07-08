// Forked/repurposed from `font-rs` code: https://github.com/raphlinus/font-rs
// Copyright 2015 Google Inc. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//
// Modifications copyright (C) 2020 Alex Butler
//
// Cubic bezier drawing adapted from stb_truetype: https://github.com/nothings/stb
#[cfg(all(feature = "libm", not(feature = "std")))]
use crate::nostd_float::FloatExt;
#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use crate::geometry::{lerp, Point};

type DrawLineFn = unsafe fn(&mut Rasterizer, Point, Point);

/// Coverage rasterizer for lines, quadratic & cubic beziers.
pub struct Rasterizer {
    width: usize,
    height: usize,
    a: Vec<f32>,
    draw_line_fn: DrawLineFn,
}

impl Rasterizer {
    /// Allocates a new rasterizer that can draw onto a `width` x `height` alpha grid.
    ///
    /// ```
    /// use ab_glyph_rasterizer::Rasterizer;
    /// let mut rasterizer = Rasterizer::new(14, 38);
    /// ```
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            a: vec![0.0; (width + 1) * height],
            draw_line_fn: optimal_draw_line_fn(),
        }
    }

    /// Resets the rasterizer to an empty `width` x `height` alpha grid. This method behaves as if
    /// the Rasterizer were re-created, with the advantage of not allocating if the total number of
    /// pixels of the grid does not increase.
    ///
    /// ```
    /// # use ab_glyph_rasterizer::Rasterizer;
    /// # let mut rasterizer = Rasterizer::new(14, 38);
    /// rasterizer.reset(12, 24);
    /// assert_eq!(rasterizer.dimensions(), (12, 24));
    /// ```
    pub fn reset(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        self.a.truncate(0);
        self.a.resize((width + 1) * height, 0.0);
    }

    /// Clears the rasterizer. This method behaves as if the Rasterizer were re-created with the same
    /// dimensions, but does not perform an allocation.
    ///
    /// ```
    /// # use ab_glyph_rasterizer::Rasterizer;
    /// # let mut rasterizer = Rasterizer::new(14, 38);
    /// rasterizer.clear();
    /// ```
    pub fn clear(&mut self) {
        for px in &mut self.a {
            *px = 0.0;
        }
    }

    /// Returns the dimensions the rasterizer was built to draw to.
    ///
    /// ```
    /// # use ab_glyph_rasterizer::*;
    /// let rasterizer = Rasterizer::new(9, 8);
    /// assert_eq!((9, 8), rasterizer.dimensions());
    /// ```
    pub fn dimensions(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// Adds a straight line from `p0` to `p1` to the outline.
    ///
    /// ```
    /// # use ab_glyph_rasterizer::*;
    /// # let mut rasterizer = Rasterizer::new(9, 8);
    /// rasterizer.draw_line(point(0.0, 0.48), point(1.22, 0.48));
    /// ```
    pub fn draw_line(&mut self, p0: Point, p1: Point) {
        unsafe { (self.draw_line_fn)(self, p0, p1) }
    }

    #[inline(always)] // must inline for simd versions
    fn draw_line_scalar(&mut self, p0: Point, p1: Point) {
        if (p0.y - p1.y).abs() <= f32::EPSILON {
            return;
        }
        let (dir, p0, p1) = if p0.y < p1.y {
            (1.0, p0, p1)
        } else {
            (-1.0, p1, p0)
        };
        let dxdy = (p1.x - p0.x) / (p1.y - p0.y);
        let mut x = p0.x;
        let y0 = p0.y as usize; // note: implicit max of 0 because usize
        if p0.y < 0.0 {
            x -= p0.y * dxdy;
        }
        for y in y0..self.height.min(p1.y.ceil() as usize) {
            let linestart = y * (self.width + 1);
            let dy = ((y + 1) as f32).min(p1.y) - (y as f32).max(p0.y);
            let xnext = x + dxdy * dy;
            let d = dy * dir;
            let (x0, x1) = if x < xnext { (x, xnext) } else { (xnext, x) };
            let x0floor = x0.floor();
            let x0i = x0floor as i32;
            let x1ceil = x1.ceil();
            let x1i = x1ceil as i32;
            if x1i <= x0i + 1 {
                let xmf = 0.5 * (x + xnext) - x0floor;
                let linestart_x0i = linestart as isize + x0i as isize;
                if linestart_x0i < 0 {
                    continue; // oob index
                }
                self.a[linestart_x0i as usize] += d - d * xmf;
                if x0i < self.width as i32 {
                    self.a[linestart_x0i as usize + 1] += d * xmf;
                }
            } else {
                let s = (x1 - x0).recip();
                let x0f = x0 - x0floor;
                let a0 = 0.5 * s * (1.0 - x0f) * (1.0 - x0f);
                let x1f = x1 - x1ceil + 1.0;
                let am = 0.5 * s * x1f * x1f;
                let linestart_x0i = linestart as isize + x0i as isize;
                if linestart_x0i < 0 {
                    continue; // oob index
                }
                self.a[linestart_x0i as usize] += d * a0;
                if x0i < self.width as i32 {
                    if x1i == x0i + 2 {
                        self.a[linestart_x0i as usize + 1] += d * (1.0 - a0 - am);
                    } else {
                        let a1 = s * (1.5 - x0f);
                        self.a[linestart_x0i as usize + 1] += d * (a1 - a0);
                        for xi in x0i + 2..x1i - 1 {
                            self.a[linestart + xi as usize] += d * s;
                        }
                        let a2 = a1 + (x1i - x0i - 3) as f32 * s;
                        self.a[linestart + (x1i - 1) as usize] += d * (1.0 - a2 - am);
                    }
                }
                if x1i < self.width as i32 {
                    self.a[linestart + x1i as usize] += d * am;
                }
            }
            x = xnext;
        }
    }

    /// Adds a quadratic Bézier curve from `p0` to `p2` to the outline using `p1` as the control.
    ///
    /// ```
    /// # use ab_glyph_rasterizer::*;
    /// # let mut rasterizer = Rasterizer::new(14, 38);
    /// rasterizer.draw_quad(point(6.2, 34.5), point(7.2, 34.5), point(9.2, 34.0));
    /// ```
    pub fn draw_quad(&mut self, p0: Point, p1: Point, p2: Point) {
        let devx = p0.x - 2.0 * p1.x + p2.x;
        let devy = p0.y - 2.0 * p1.y + p2.y;
        let devsq = devx * devx + devy * devy;
        if devsq < 0.333 {
            self.draw_line(p0, p2);
            return;
        }
        let tol = 3.0;
        let n = 1 + (tol * devsq).sqrt().sqrt().floor() as usize;
        let mut p = p0;
        let nrecip = (n as f32).recip();
        let mut t = 0.0;
        for _i in 0..n - 1 {
            t += nrecip;
            let pn = lerp(t, lerp(t, p0, p1), lerp(t, p1, p2));
            self.draw_line(p, pn);
            p = pn;
        }
        self.draw_line(p, p2);
    }

    /// Adds a cubic Bézier curve from `p0` to `p3` to the outline using `p1` as the control
    /// at the beginning of the curve and `p2` at the end of the curve.
    ///
    /// ```
    /// # use ab_glyph_rasterizer::*;
    /// # let mut rasterizer = Rasterizer::new(12, 20);
    /// rasterizer.draw_cubic(
    ///     point(10.3, 16.4),
    ///     point(8.6, 16.9),
    ///     point(7.7, 16.5),
    ///     point(8.2, 15.2),
    /// );
    /// ```
    pub fn draw_cubic(&mut self, p0: Point, p1: Point, p2: Point, p3: Point) {
        self.tessellate_cubic(p0, p1, p2, p3, 0);
    }

    // stb_truetype style cubic approximation by lines.
    fn tessellate_cubic(&mut self, p0: Point, p1: Point, p2: Point, p3: Point, n: u8) {
        // ...I'm not sure either ¯\_(ツ)_/¯
        const OBJSPACE_FLATNESS: f32 = 0.35;
        const OBJSPACE_FLATNESS_SQUARED: f32 = OBJSPACE_FLATNESS * OBJSPACE_FLATNESS;
        const MAX_RECURSION_DEPTH: u8 = 16;

        let longlen = p0.distance_to(p1) + p1.distance_to(p2) + p2.distance_to(p3);
        let shortlen = p0.distance_to(p3);
        let flatness_squared = longlen * longlen - shortlen * shortlen;

        if n < MAX_RECURSION_DEPTH && flatness_squared > OBJSPACE_FLATNESS_SQUARED {
            let p01 = lerp(0.5, p0, p1);
            let p12 = lerp(0.5, p1, p2);
            let p23 = lerp(0.5, p2, p3);

            let pa = lerp(0.5, p01, p12);
            let pb = lerp(0.5, p12, p23);

            let mp = lerp(0.5, pa, pb);

            self.tessellate_cubic(p0, p01, pa, mp, n + 1);
            self.tessellate_cubic(mp, pb, p23, p3, n + 1);
        } else {
            self.draw_line(p0, p3);
        }
    }

    /// Run a callback for each pixel `index` & `alpha`, with indices in `0..width * height`.
    ///
    /// An `alpha` coverage value of `0.0` means the pixel is not covered at all by the glyph,
    /// whereas a value of `1.0` (or greater) means the pixel is totally covered.
    ///
    /// ```
    /// # use ab_glyph_rasterizer::*;
    /// # let (width, height) = (1, 1);
    /// # let mut rasterizer = Rasterizer::new(width, height);
    /// let mut pixels = vec![0u8; width * height];
    /// rasterizer.for_each_pixel(|index, alpha| {
    ///     pixels[index] = (alpha * 255.0) as u8;
    /// });
    /// ```
    pub fn for_each_pixel<O: FnMut(usize, f32)>(&self, mut px_fn: O) {
        if self.width == 0 || self.height == 0 {
            return;
        }
        for y in 0..self.height {
            let mut acc = 0.0;
            let scanline_offset = y * (self.width + 1);
            for x in 0..self.width {
                acc += self.a[scanline_offset + x];
                let coverage = acc.abs().min(1.0);
                let pixel_idx = y * self.width + x;
                px_fn(pixel_idx, coverage);
            }
        }
    }

    /// Run a callback for each pixel x position, y position & alpha.
    ///
    /// Convenience wrapper for [`Rasterizer::for_each_pixel`].
    ///
    /// ```
    /// # use ab_glyph_rasterizer::*;
    /// # let mut rasterizer = Rasterizer::new(1, 1);
    /// # struct Img;
    /// # impl Img { fn set_pixel(&self, x: u32, y: u32, a: u8) {} }
    /// # let image = Img;
    /// rasterizer.for_each_pixel_2d(|x, y, alpha| {
    ///     image.set_pixel(x, y, (alpha * 255.0) as u8);
    /// });
    /// ```
    pub fn for_each_pixel_2d<O: FnMut(u32, u32, f32)>(&self, mut px_fn: O) {
        let width32 = self.width as u32;
        self.for_each_pixel(|idx, alpha| px_fn(idx as u32 % width32, idx as u32 / width32, alpha));
    }
}

/// ```
/// let rasterizer = ab_glyph_rasterizer::Rasterizer::new(3, 4);
/// assert_eq!(
///     &format!("{:?}", rasterizer),
///     "Rasterizer { width: 3, height: 4 }"
/// );
/// ```
impl core::fmt::Debug for Rasterizer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Rasterizer")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish()
    }
}

#[cfg(all(feature = "std", any(target_arch = "x86", target_arch = "x86_64")))]
#[target_feature(enable = "avx2")]
unsafe fn draw_line_avx2(rast: &mut Rasterizer, p0: Point, p1: Point) {
    rast.draw_line_scalar(p0, p1)
}

#[cfg(all(feature = "std", any(target_arch = "x86", target_arch = "x86_64")))]
#[target_feature(enable = "sse4.2")]
unsafe fn draw_line_sse4_2(rast: &mut Rasterizer, p0: Point, p1: Point) {
    rast.draw_line_scalar(p0, p1)
}

/// Return most optimal `DrawLineFn` impl.
///
/// With feature `std` on x86/x86_64 will use one-time runtime detection
/// to pick the best SIMD impl. Otherwise uses a scalar version.
fn optimal_draw_line_fn() -> DrawLineFn {
    unsafe {
        // safe as write synchronised by Once::call_once or no-write
        static mut DRAW_LINE_FN: DrawLineFn = Rasterizer::draw_line_scalar;

        #[cfg(all(feature = "std", any(target_arch = "x86", target_arch = "x86_64")))]
        {
            static INIT: std::sync::Once = std::sync::Once::new();
            INIT.call_once(|| {
                // runtime detect optimal simd impls
                if is_x86_feature_detected!("avx2") {
                    DRAW_LINE_FN = draw_line_avx2
                } else if is_x86_feature_detected!("sse4.2") {
                    DRAW_LINE_FN = draw_line_sse4_2
                }
            });
        }

        DRAW_LINE_FN
    }
}
