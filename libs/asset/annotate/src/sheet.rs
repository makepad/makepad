//! Turntable sheet preparation: published PNG -> the RGB the vision tower
//! wants.
//!
//! Sheets are published at 1024x1024 (a 4x4 grid of 16 turntable views). Fed
//! in at that size the tower emits 1024 image tokens per asset; box-filtered
//! down to 512 it emits 256, which is a quarter of the prefill for cells that
//! are still 128px across. The working size is a knob because it is one of the
//! things prompt iteration trades against.

/// Decoded interleaved RGB8.
pub struct Rgb {
    pub w: usize,
    pub h: usize,
    pub pixels: Vec<u8>,
}

/// Decode a PNG to RGB8, compositing any alpha over the sheet's own dark
/// background so a transparent sheet does not arrive as white-on-white.
pub fn decode_png(bytes: &[u8]) -> Result<Rgb, String> {
    use makepad_zune_png::makepad_zune_core::bytestream::ZCursor;
    let mut dec = makepad_zune_png::PngDecoder::new(ZCursor::new(bytes));
    dec.decode_headers().map_err(|e| format!("png headers: {e:?}"))?;
    let (w, h) = dec.dimensions().ok_or("png has no dimensions")?;
    let channels = dec.colorspace().ok_or("png has no colorspace")?.num_components();
    let out = dec.decode_raw().map_err(|e| format!("png decode: {e:?}"))?;
    if out.len() < w * h * channels {
        return Err(format!("png payload short: {} < {}", out.len(), w * h * channels));
    }
    // The renderer's background, used as the matte for transparent pixels.
    const BG: [u8; 3] = SHEET_BG;
    let mut pixels = vec![0u8; w * h * 3];
    for i in 0..w * h {
        let s = &out[i * channels..];
        let (rgb, a) = match channels {
            1 => ([s[0], s[0], s[0]], 255u16),
            2 => ([s[0], s[0], s[0]], s[1] as u16),
            3 => ([s[0], s[1], s[2]], 255u16),
            4 => ([s[0], s[1], s[2]], s[3] as u16),
            n => return Err(format!("unsupported png channel count {n}")),
        };
        for c in 0..3 {
            pixels[i * 3 + c] = if a == 255 {
                rgb[c]
            } else {
                ((rgb[c] as u16 * a + BG[c] as u16 * (255 - a)) / 255) as u8
            };
        }
    }
    Ok(Rgb { w, h, pixels })
}

/// Box-filter downscale by an integer factor. Sheets are square powers of two
/// and the targets are too, so an integer box filter is both exact and the
/// right filter: it averages whole source cells instead of point-sampling, and
/// point-sampling is what loses thin geometry like fence posts and lane lines.
pub fn downscale(src: &Rgb, target: usize) -> Rgb {
    if target == 0 || src.w <= target || src.h <= target || src.w % target != 0 || src.h % target != 0
    {
        return Rgb { w: src.w, h: src.h, pixels: src.pixels.clone() };
    }
    let fx = src.w / target;
    let fy = src.h / target;
    let n = (fx * fy) as u32;
    let mut pixels = vec![0u8; target * target * 3];
    for y in 0..target {
        for x in 0..target {
            let mut acc = [0u32; 3];
            for sy in y * fy..(y + 1) * fy {
                let row = sy * src.w * 3;
                for sx in x * fx..(x + 1) * fx {
                    let p = row + sx * 3;
                    acc[0] += src.pixels[p] as u32;
                    acc[1] += src.pixels[p + 1] as u32;
                    acc[2] += src.pixels[p + 2] as u32;
                }
            }
            let d = (y * target + x) * 3;
            for c in 0..3 {
                pixels[d + c] = (acc[c] / n) as u8;
            }
        }
    }
    Rgb { w: target, h: target, pixels }
}

/// The turntable renderer's flat background colour.
pub const SHEET_BG: [u8; 3] = [0x14, 0x18, 0x28];

/// Lift the subject out of the renders' shadows, leaving the background alone.
///
/// The sheets are lit dimly against a near-black navy field, and unlit faces
/// land in the 20-60 range where hue is mostly quantisation noise. Read at
/// that exposure the model names shadows rather than materials — a brown
/// arched door came back "blue", a grey road's shading came back "dark blue
/// edges". A gamma lift on subject pixels only fixes the naming without
/// touching geometry, and the background is a known flat colour so "subject"
/// is an exact test rather than a guess.
pub fn lift_exposure(img: &mut Rgb, gamma: f32) {
    if gamma <= 1.0 {
        return;
    }
    // Lookup table: 256 powf calls instead of one per subpixel.
    let mut lut = [0u8; 256];
    for (i, v) in lut.iter_mut().enumerate() {
        *v = (((i as f32 / 255.0).powf(1.0 / gamma)) * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    // Anything within this distance of the background is background. The
    // renderer writes it flat, so a tight threshold cannot eat dark geometry
    // that merely sits near it in colour.
    const TOL: i32 = 6;
    for px in img.pixels.chunks_exact_mut(3) {
        let is_bg = (0..3).all(|c| (px[c] as i32 - SHEET_BG[c] as i32).abs() <= TOL);
        if is_bg {
            continue;
        }
        for c in 0..3 {
            px[c] = lut[px[c] as usize];
        }
    }
}

/// Zoom every turntable cell onto its subject: per cell, find the bounding
/// box of non-background pixels, expand it to a square with a margin, and
/// resample that crop to fill the cell.
///
/// A mini-character occupies ~15% of its cell; fed to the tower at 512 the
/// whole person is ~60px and faces are quantisation noise — the model names
/// guesses (a bare hand came back "a wooden staff", a black suit "a grey
/// tunic"). Zoomed, the same patch budget lands on the subject. Generic by
/// construction: the pass knows only "background vs not", never what the
/// subject is — no VRAM cost, unlike feeding a larger sheet.
///
/// Deliberately NOT used for construction-kit pieces: their relative size
/// in the cell is a signal (`size:` line) that zooming would destroy.
pub fn zoom_to_subject(src: &Rgb, grid: usize, margin_frac: f32) -> Rgb {
    if grid == 0 || src.w % grid != 0 || src.h % grid != 0 {
        return Rgb { w: src.w, h: src.h, pixels: src.pixels.clone() };
    }
    // The background is read from the sheet itself (its corners are always
    // empty), not from SHEET_BG: served thumbnails have gone through
    // renderer generations with slightly different fields, and a constant
    // that misses by a few counts silently turns the zoom into a no-op.
    const TOL: i32 = 12;
    let corner = |x: usize, y: usize| {
        let p = (y * src.w + x) * 3;
        [src.pixels[p], src.pixels[p + 1], src.pixels[p + 2]]
    };
    let bg = corner(0, 0);
    let (cw, ch) = (src.w / grid, src.h / grid);
    let mut out = Rgb { w: src.w, h: src.h, pixels: src.pixels.clone() };
    let is_bg = |p: &[u8]| (0..3).all(|c| (p[c] as i32 - bg[c] as i32).abs() <= TOL);
    for gy in 0..grid {
        for gx in 0..grid {
            let (x0, y0) = (gx * cw, gy * ch);
            // Subject bbox within this cell.
            let (mut minx, mut miny, mut maxx, mut maxy) = (cw, ch, 0usize, 0usize);
            let mut any = false;
            for y in 0..ch {
                let row = (y0 + y) * src.w * 3;
                for x in 0..cw {
                    let p = row + (x0 + x) * 3;
                    if !is_bg(&src.pixels[p..p + 3]) {
                        any = true;
                        minx = minx.min(x);
                        miny = miny.min(y);
                        maxx = maxx.max(x);
                        maxy = maxy.max(y);
                    }
                }
            }
            if !any {
                continue;
            }
            // Square crop around the subject, clamped inside the cell.
            let bw = (maxx - minx + 1) as f32;
            let bh = (maxy - miny + 1) as f32;
            let side = (bw.max(bh) * (1.0 + margin_frac)).min(cw.min(ch) as f32);
            let cx = (minx + maxx) as f32 * 0.5;
            let cy = (miny + maxy) as f32 * 0.5;
            let half = side * 0.5;
            let sx0 = (cx - half).clamp(0.0, cw as f32 - side);
            let sy0 = (cy - half).clamp(0.0, ch as f32 - side);
            // Bilinear upsample of the crop to fill the cell.
            for y in 0..ch {
                let fy = sy0 + (y as f32 + 0.5) / ch as f32 * side - 0.5;
                let iy = fy.floor().max(0.0) as usize;
                let ty = (fy - iy as f32).clamp(0.0, 1.0);
                let iy1 = (iy + 1).min(ch - 1);
                for x in 0..cw {
                    let fx = sx0 + (x as f32 + 0.5) / cw as f32 * side - 0.5;
                    let ix = fx.floor().max(0.0) as usize;
                    let tx = (fx - ix as f32).clamp(0.0, 1.0);
                    let ix1 = (ix + 1).min(cw - 1);
                    let at = |px: usize, py: usize, c: usize| {
                        src.pixels[((y0 + py) * src.w + x0 + px) * 3 + c] as f32
                    };
                    let d = ((y0 + y) * src.w + x0 + x) * 3;
                    for c in 0..3 {
                        let top = at(ix, iy, c) * (1.0 - tx) + at(ix1, iy, c) * tx;
                        let bot = at(ix, iy1, c) * (1.0 - tx) + at(ix1, iy1, c) * tx;
                        out.pixels[d + c] =
                            (top * (1.0 - ty) + bot * ty).round().clamp(0.0, 255.0) as u8;
                    }
                }
            }
        }
    }
    out
}

/// Serialise as binary P6 PPM, the interchange format the batch executor
/// reads. Deliberately the dumbest possible container: no compression, no
/// dependency, and trivially reimplemented by any other executor.
pub fn to_ppm(img: &Rgb) -> Vec<u8> {
    let mut out = format!("P6\n{} {}\n255\n", img.w, img.h).into_bytes();
    out.extend_from_slice(&img.pixels);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: usize, h: usize, c: [u8; 3]) -> Rgb {
        let mut pixels = Vec::with_capacity(w * h * 3);
        for _ in 0..w * h {
            pixels.extend_from_slice(&c);
        }
        Rgb { w, h, pixels }
    }

    #[test]
    fn ppm_header_and_payload() {
        let ppm = to_ppm(&solid(2, 3, [1, 2, 3]));
        assert!(ppm.starts_with(b"P6\n2 3\n255\n"));
        assert_eq!(ppm.len(), 11 + 2 * 3 * 3);
    }

    #[test]
    fn downscale_averages_whole_cells() {
        // a 4x4 split black/white left/right averages to mid grey at 2x2
        let mut src = solid(4, 4, [0, 0, 0]);
        for y in 0..4 {
            for x in 2..4 {
                let p = (y * 4 + x) * 3;
                src.pixels[p..p + 3].copy_from_slice(&[255, 255, 255]);
            }
        }
        let small = downscale(&src, 2);
        assert_eq!((small.w, small.h), (2, 2));
        assert_eq!(&small.pixels[0..3], &[0, 0, 0]);
        assert_eq!(&small.pixels[3..6], &[255, 255, 255]);
    }

    #[test]
    fn downscale_is_a_noop_when_it_cannot_divide_evenly() {
        let src = solid(10, 10, [7, 7, 7]);
        let out = downscale(&src, 3);
        assert_eq!((out.w, out.h), (10, 10));
        let same = downscale(&src, 20);
        assert_eq!((same.w, same.h), (10, 10));
    }

    #[test]
    fn exposure_lifts_the_subject_and_leaves_the_background() {
        let mut img = solid(3, 1, SHEET_BG);
        // one dark subject pixel and one already-bright one
        img.pixels[3..6].copy_from_slice(&[40, 30, 20]);
        img.pixels[6..9].copy_from_slice(&[250, 250, 250]);
        lift_exposure(&mut img, 1.8);
        // background untouched, so it cannot start reading as a colour
        assert_eq!(&img.pixels[0..3], &SHEET_BG);
        // shadowed subject lifted well clear of the noise floor
        assert!(img.pixels[3] > 90, "{}", img.pixels[3]);
        // highlights stay put instead of clipping
        assert!(img.pixels[6] >= 250);
        // hue order is preserved: r was brightest, it stays brightest
        assert!(img.pixels[3] > img.pixels[4] && img.pixels[4] > img.pixels[5]);
    }

    #[test]
    fn exposure_is_a_noop_at_unit_gamma() {
        let mut img = solid(2, 1, [40, 30, 20]);
        let before = img.pixels.clone();
        lift_exposure(&mut img, 1.0);
        assert_eq!(img.pixels, before);
    }

    #[test]
    fn thin_geometry_survives_the_box_filter() {
        // one bright column in 4 must still tint the 1px output, which is what
        // point sampling would have dropped entirely
        let mut src = solid(4, 4, [0, 0, 0]);
        for y in 0..4 {
            let p = (y * 4 + 1) * 3;
            src.pixels[p..p + 3].copy_from_slice(&[200, 200, 200]);
        }
        let out = downscale(&src, 1);
        assert_eq!((out.w, out.h), (1, 1));
        assert_eq!(out.pixels[0], 50);
    }
}
