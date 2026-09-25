//! RGBA images: PNG decode/encode through the in-repo zune-png, and an
//! area-weighted resampler for deriving icon sizes the source lacks.
use makepad_zune_png::makepad_zune_core::bit_depth::BitDepth;
use makepad_zune_png::makepad_zune_core::bytestream::ZCursor;
use makepad_zune_png::makepad_zune_core::colorspace::ColorSpace;
use makepad_zune_png::makepad_zune_core::options::{DecoderOptions, EncoderOptions};
use makepad_zune_png::{PngDecoder, PngEncoder};

pub const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const MAX_DIMENSION: usize = 4096;

/// Straight (not premultiplied) 8-bit RGBA, rows top to bottom.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Image, String> {
        if width == 0 || height == 0 || rgba.len() != width as usize * height as usize * 4 {
            return Err(format!("image {width}x{height} has {} bytes of RGBA", rgba.len()));
        }
        Ok(Image { width, height, rgba })
    }

    pub fn decode_png(png: &[u8]) -> Result<Image, String> {
        let options = DecoderOptions::default()
            .set_max_width(MAX_DIMENSION)
            .set_max_height(MAX_DIMENSION)
            .png_set_strip_to_8bit(true);
        let mut decoder = PngDecoder::new_with_options(ZCursor::new(png), options);
        decoder.decode_headers().map_err(|e| format!("PNG header: {e:?}"))?;
        let (width, height) = decoder.dimensions().ok_or("PNG without dimensions")?;
        let decoded = decoder.decode().map_err(|e| format!("PNG decode: {e:?}"))?;
        let samples = match decoded.u8() {
            Some(samples) => samples,
            None => return Err("PNG decoded to an unsupported sample type".into()),
        };
        let pixels = width * height;
        if pixels == 0 || samples.len() % pixels != 0 {
            return Err(format!("PNG {width}x{height} decoded to {} samples", samples.len()));
        }
        let mut rgba = Vec::with_capacity(pixels * 4);
        match samples.len() / pixels {
            4 => rgba = samples,
            3 => samples.chunks_exact(3).for_each(|c| rgba.extend_from_slice(&[c[0], c[1], c[2], 255])),
            2 => samples.chunks_exact(2).for_each(|c| rgba.extend_from_slice(&[c[0], c[0], c[0], c[1]])),
            1 => samples.iter().for_each(|&g| rgba.extend_from_slice(&[g, g, g, 255])),
            n => return Err(format!("PNG with {n} components per pixel")),
        }
        Image::new(width as u32, height as u32, rgba)
    }

    pub fn encode_png(&self) -> Result<Vec<u8>, String> {
        let options = EncoderOptions::new(self.width as usize, self.height as usize, ColorSpace::RGBA, BitDepth::Eight);
        let mut sink = Vec::new();
        PngEncoder::new(&self.rgba, options).encode(&mut sink).map_err(|e| format!("PNG encode: {e:?}"))?;
        Ok(sink)
    }

    /// Area-weighted (box) resample in premultiplied alpha, so transparent
    /// pixels do not bleed their colour into the edge of the shape.
    pub fn resample(&self, width: u32, height: u32) -> Image {
        if width == self.width && height == self.height {
            return self.clone();
        }
        let (sw, sh, dw, dh) = (self.width as usize, self.height as usize, width as usize, height as usize);
        let premultiplied: Vec<[f32; 4]> = self.rgba.chunks_exact(4).map(|p| {
            let a = p[3] as f32 / 255.0;
            [p[0] as f32 * a, p[1] as f32 * a, p[2] as f32 * a, p[3] as f32]
        }).collect();
        let columns = taps(sw, dw);
        let rows = taps(sh, dh);
        let mut horizontal = vec![[0f32; 4]; dw * sh];
        for y in 0..sh {
            for (x, taps) in columns.iter().enumerate() {
                let out = &mut horizontal[y * dw + x];
                for &(sx, w) in taps {
                    let p = premultiplied[y * sw + sx];
                    for c in 0..4 { out[c] += p[c] * w; }
                }
            }
        }
        let mut rgba = Vec::with_capacity(dw * dh * 4);
        for taps in &rows {
            for x in 0..dw {
                let mut p = [0f32; 4];
                for &(sy, w) in taps {
                    let q = horizontal[sy * dw + x];
                    for c in 0..4 { p[c] += q[c] * w; }
                }
                let a = p[3];
                let unpremultiply = |v: f32| if a > 0.0 { (v * 255.0 / a).round().clamp(0.0, 255.0) as u8 } else { 0 };
                rgba.extend_from_slice(&[unpremultiply(p[0]), unpremultiply(p[1]), unpremultiply(p[2]), a.round().clamp(0.0, 255.0) as u8]);
            }
        }
        Image { width, height, rgba }
    }
}

/// For each destination pixel, the source pixels it covers and their weights
/// (fractions of the destination pixel's footprint, summing to one).
fn taps(source: usize, dest: usize) -> Vec<Vec<(usize, f32)>> {
    let scale = source as f64 / dest as f64;
    (0..dest).map(|i| {
        let (start, end) = (i as f64 * scale, (i + 1) as f64 * scale);
        let mut taps = Vec::new();
        let mut j = start.floor() as usize;
        while (j as f64) < end && j < source {
            let overlap = end.min(j as f64 + 1.0) - start.max(j as f64);
            if overlap > 1e-9 { taps.push((j, (overlap / scale) as f32)); }
            j += 1;
        }
        taps
    }).collect()
}
