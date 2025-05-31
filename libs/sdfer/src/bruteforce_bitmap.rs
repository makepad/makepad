//! Bitmap-based "closest opposite-color pixel" bruteforce search.
//!
//! This algorithm is a commonly implemented one, but a naive and inefficient one.
//!
//! It was most notably popularized by Valve in their 2007 SIGGRAPH entry,
//! on using 2D texture-based SDFs to render text (or any vector shapes) in 3D environments,
//! ["Improved Alpha-Tested Magnification for Vector Textures and Special Effects"](https://steamcdn-a.akamaihd.net/apps/valve/2007/SIGGRAPH2007_AlphaTestedMagnification.pdf).

use crate::{Bitmap, Image2d, Unorm8};

// HACK(eddyb) only exists to allow toggling precision for testing purposes.
#[cfg(sdfer_use_f64_instead_of_f32)]
type f32 = f64;

pub fn sdf(bitmap: &Bitmap, sdf_size: usize, spread: usize) -> Image2d<Unorm8> {
    let (w, h) = (bitmap.width(), bitmap.height());

    assert!(w.is_power_of_two() && h.is_power_of_two() && sdf_size.is_power_of_two());
    let scale = w.max(h) / sdf_size;
    assert_ne!(scale, 0);

    let spread = spread * scale;

    let width = w / scale;
    let height = h / scale;
    Image2d::from_fn(width, height, |x, y| {
        let (x, y) = (x * scale + scale / 2, y * scale + scale / 2);
        let inside = bitmap.get(x, y);
        // FIXME(eddyb) this could use a spiral search, and maybe better bitmap
        // access, e.g. "which bits in a block are different than the center x,y".
        let dist = (((y.saturating_sub(spread)..=(y + spread))
            .flat_map(|y2| (x.saturating_sub(spread)..=(x + spread)).map(move |x2| (x2, y2)))
            .filter(|&(x2, y2)| x2 < w && y2 < h && bitmap.get(x2, y2) != inside)
            .map(|(x2, y2)| x2.abs_diff(x).pow(2) + y2.abs_diff(y).pow(2))
            .min()
            .unwrap_or(usize::MAX) as f32)
            .sqrt()
            / (spread as f32))
            .clamp(0.0, 1.0);
        let signed_dist = if inside { -dist } else { dist };

        // [-1, +1] -> [0, 1]
        Unorm8::encode((signed_dist + 1.0) / 2.0)
    })
}
