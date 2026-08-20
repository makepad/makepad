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
    const BG: [u8; 3] = [0x14, 0x18, 0x28];
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
