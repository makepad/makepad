//! Pixel conversions between what guests send (JPEG), what the hub's image
//! models take and give (PNG) and what the phone shows (JPEG). Workers only:
//! never on a UI thread.

use makepad_zune_core::bit_depth::BitDepth;
use makepad_zune_core::bytestream::ZCursor;
use makepad_zune_core::colorspace::ColorSpace;
use makepad_zune_core::options::{DecoderOptions, EncoderOptions};

/// Tightly packed RGB8.
#[derive(Clone)]
pub struct Rgb {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

/// The picture as texels (0xAARRGGBB), as the screen's textures take them.
pub fn bgra(rgb: &Rgb) -> Vec<u32> {
    rgb.data.chunks_exact(3).map(|p| 0xff00_0000 | (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32).collect()
}

fn to_rgb(pixels: Vec<u8>, width: usize, height: usize) -> Rgb {
    let n = width * height;
    let components = if n == 0 { 3 } else { (pixels.len() / n).max(1) };
    let data = match components {
        3 => pixels,
        1 => pixels.iter().flat_map(|&l| [l, l, l]).collect(),
        c => pixels.chunks(c).flat_map(|p| [p[0], p[1.min(c - 1)], p[2.min(c - 1)]]).collect(),
    };
    Rgb { width, height, data }
}

/// Largest picture decoded, in pixels: a phone sends at most 1280 on its
/// long side; anything far past that is not a phone page's upload.
const MAX_PIXELS: usize = 16 * 1024 * 1024;

pub fn decode_jpeg(bytes: &[u8]) -> Result<Rgb, String> {
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut decoder = makepad_zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    decoder.decode_headers().map_err(|e| format!("jpeg: {e:?}"))?;
    let (width, height) = decoder.dimensions().ok_or("jpeg: no dimensions")?;
    if width * height > MAX_PIXELS {
        return Err(format!("jpeg: {width}x{height} is too large"));
    }
    let pixels = decoder.decode().map_err(|e| format!("jpeg: {e:?}"))?;
    Ok(to_rgb(pixels, width, height))
}

pub fn decode_png(bytes: &[u8]) -> Result<Rgb, String> {
    let options = DecoderOptions::default().png_set_strip_to_8bit(true);
    let mut decoder = makepad_zune_png::PngDecoder::new_with_options(std::io::Cursor::new(bytes), options);
    decoder.decode_headers().map_err(|e| format!("png: {e:?}"))?;
    let info = decoder.info().cloned().ok_or("png: no header")?;
    let (width, height) = (info.width, info.height);
    if width * height > MAX_PIXELS {
        return Err(format!("png: {width}x{height} is too large"));
    }
    let pixels = decoder.decode_raw().map_err(|e| format!("png: {e:?}"))?;
    Ok(to_rgb(pixels, width, height))
}

pub fn encode_png(rgb: &Rgb) -> Result<Vec<u8>, String> {
    let options = EncoderOptions::default()
        .set_width(rgb.width)
        .set_height(rgb.height)
        .set_depth(BitDepth::Eight)
        .set_colorspace(ColorSpace::RGB);
    let mut out = Vec::new();
    makepad_zune_png::PngEncoder::new(&rgb.data, options).encode(&mut out).map_err(|e| format!("png encode: {e:?}"))?;
    Ok(out)
}

pub fn encode_jpeg(rgb: &Rgb, quality: u8) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, quality)
        .encode(&rgb.data, rgb.width as u16, rgb.height as u16, jpeg_encoder::ColorType::Rgb)
        .map_err(|e| format!("jpeg encode: {e:?}"))?;
    Ok(out)
}

/// Scaled (box filter down, nearest up) and center-cropped to exactly
/// `width` x `height`: what a model's fixed input size wants.
pub fn fit(rgb: &Rgb, width: usize, height: usize) -> Rgb {
    let scale = (width as f64 / rgb.width as f64).max(height as f64 / rgb.height as f64);
    let (sw, sh) = (rgb.width as f64 * scale, rgb.height as f64 * scale);
    let (ox, oy) = ((sw - width as f64) / 2.0, (sh - height as f64) / 2.0);
    sample(rgb, width, height, (1.0 / scale, 1.0 / scale), (ox, oy))
}

/// Scaled (box filter down, nearest up) to `width` x `height`, without a crop.
pub fn resize(rgb: &Rgb, width: usize, height: usize) -> Rgb {
    sample(rgb, width, height, (rgb.width as f64 / width as f64, rgb.height as f64 / height as f64), (0.0, 0.0))
}

fn sample(rgb: &Rgb, width: usize, height: usize, step: (f64, f64), offset: (f64, f64)) -> Rgb {
    let mut data = vec![0u8; width * height * 3];
    let taps = (step.0.ceil().max(1.0) as usize, step.1.ceil().max(1.0) as usize);
    for y in 0..height {
        for x in 0..width {
            let sx0 = (x as f64 + offset.0) * step.0;
            let sy0 = (y as f64 + offset.1) * step.1;
            let mut acc = [0u32; 3];
            let mut n = 0u32;
            for ty in 0..taps.1 {
                for tx in 0..taps.0 {
                    let sx = ((sx0 + tx as f64) as usize).min(rgb.width - 1);
                    let sy = ((sy0 + ty as f64) as usize).min(rgb.height - 1);
                    let p = (sy * rgb.width + sx) * 3;
                    for c in 0..3 {
                        acc[c] += rgb.data[p + c] as u32;
                    }
                    n += 1;
                }
            }
            let o = (y * width + x) * 3;
            for c in 0..3 {
                data[o + c] = (acc[c] / n) as u8;
            }
        }
    }
    Rgb { width, height, data }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: usize, h: usize) -> Rgb {
        let data = (0..w * h).flat_map(|i| [(i % w * 255 / w) as u8, (i / w * 255 / h) as u8, 128]).collect();
        Rgb { width: w, height: h, data }
    }

    #[test]
    fn resizing_keeps_the_edges_instead_of_cropping_them() {
        let rgb = Rgb { width: 6, height: 2, data: [200, 0, 0, 0, 0, 200].repeat(2).into_iter().flat_map(|r| [r, 0, 0]).collect() };
        let resized = resize(&rgb, 3, 2);
        assert_eq!((resized.width, resized.height), (3, 2));
        let red: Vec<_> = resized.data.chunks_exact(3).map(|p| p[0]).collect();
        assert_eq!(red, [100, 0, 100].repeat(2), "each edge is averaged with its neighbour, not cropped away");
    }

    #[test]
    fn round_trips_and_fit() {
        let g = gradient(64, 48);
        let png = decode_png(&encode_png(&g).unwrap()).unwrap();
        assert_eq!((png.width, png.height), (64, 48));
        assert_eq!(png.data, g.data, "PNG is lossless");
        let jpeg = decode_jpeg(&encode_jpeg(&g, 90).unwrap()).unwrap();
        assert_eq!((jpeg.width, jpeg.height), (64, 48));
        let off = jpeg.data.iter().zip(&g.data).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
        assert!(off < 24, "JPEG stays close: {off}");
        let f = fit(&g, 32, 32);
        assert_eq!((f.width, f.height, f.data.len()), (32, 32, 32 * 32 * 3));
        // a 4:3 source cropped to a square keeps its centre columns: red rises left to right
        assert!(f.data[0] > 20 && f.data[(31) * 3] < 230 && f.data[31 * 3] > f.data[0]);
    }
}
