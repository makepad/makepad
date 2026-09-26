//! RGBA images: PNG decode/encode (on makepad-fast-inflate's zlib, which
//! the tools that use this crate carry anyway), and an area-weighted
//! resampler for deriving icon sizes the source lacks.
use makepad_fast_inflate::{crc32, zlib_compress, zlib_decompress_vec_with_hint};

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

    /// Any PNG (every colour type and bit depth, interlaced or not, with a
    /// tRNS transparency) as 8-bit RGBA; 16-bit samples keep their high byte.
    pub fn decode_png(png: &[u8]) -> Result<Image, String> {
        if !png.starts_with(PNG_SIGNATURE) {
            return Err("not a PNG".into());
        }
        let mut at = 8;
        let (mut width, mut height, mut depth, mut colour, mut interlace) = (0usize, 0usize, 0u8, 0u8, 0u8);
        let (mut palette, mut transparency, mut data) = (Vec::new(), None::<Vec<u8>>, Vec::new());
        loop {
            let header = png.get(at..at + 8).ok_or("PNG ends before IEND")?;
            let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
            let kind = &header[4..8];
            let body = png.get(at + 8..at + 8 + length).ok_or("PNG chunk runs past the end")?;
            let stored = png.get(at + 8 + length..at + 12 + length).ok_or("PNG chunk without CRC")?;
            if crc32(&png[at + 4..at + 8 + length]) != u32::from_be_bytes([stored[0], stored[1], stored[2], stored[3]]) {
                return Err(format!("PNG {} chunk has a bad CRC", String::from_utf8_lossy(kind)));
            }
            at += 12 + length;
            match kind {
                b"IHDR" => {
                    if body.len() != 13 {
                        return Err("PNG header has the wrong size".into());
                    }
                    width = u32::from_be_bytes([body[0], body[1], body[2], body[3]]) as usize;
                    height = u32::from_be_bytes([body[4], body[5], body[6], body[7]]) as usize;
                    (depth, colour, interlace) = (body[8], body[9], body[12]);
                    let valid = match colour {
                        0 => matches!(depth, 1 | 2 | 4 | 8 | 16),
                        3 => matches!(depth, 1 | 2 | 4 | 8),
                        2 | 4 | 6 => matches!(depth, 8 | 16),
                        _ => false,
                    };
                    if !valid || body[10] != 0 || body[11] != 0 || interlace > 1 {
                        return Err(format!("PNG colour type {colour} at depth {depth} is not valid"));
                    }
                    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
                        return Err(format!("PNG {width}x{height} is outside 1..{MAX_DIMENSION}"));
                    }
                }
                b"PLTE" => palette = body.to_vec(),
                b"tRNS" => transparency = Some(body.to_vec()),
                b"IDAT" => data.extend_from_slice(body),
                b"IEND" => break,
                _ => {}
            }
        }
        if width == 0 {
            return Err("PNG without a header".into());
        }
        let channels = match colour { 0 | 3 => 1, 2 => 3, 4 => 2, _ => 4 };
        let bits = channels * depth as usize;
        let stride = |w: usize| (w * bits).div_ceil(8);
        // Adam7 passes (x0, y0, dx, dy), or one pass for the whole image.
        let passes: &[(usize, usize, usize, usize)] = if interlace == 1 {
            &[(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)]
        } else {
            &[(0, 0, 1, 1)]
        };
        let expected: usize = passes.iter().map(|&(x0, y0, dx, dy)| {
            let (w, h) = ((width + dx - 1 - x0) / dx, (height + dy - 1 - y0) / dy);
            if w == 0 || h == 0 { 0 } else { h * (1 + stride(w)) }
        }).sum();
        let raw = zlib_decompress_vec_with_hint(&data, expected).map_err(|e| format!("PNG data: {e:?}"))?;
        if raw.len() < expected {
            return Err("PNG data is shorter than its image".into());
        }
        let bytes_per_pixel = bits.div_ceil(8);
        let mut rgba = vec![0u8; width * height * 4];
        let mut offset = 0;
        for &(x0, y0, dx, dy) in passes {
            if x0 >= width || y0 >= height {
                continue;
            }
            let (w, h) = ((width + dx - 1 - x0) / dx, (height + dy - 1 - y0) / dy);
            let line = stride(w);
            let mut previous = vec![0u8; line];
            for row in 0..h {
                let filter = raw[offset];
                let mut current = raw[offset + 1..offset + 1 + line].to_vec();
                offset += 1 + line;
                unfilter(filter, &mut current, &previous, bytes_per_pixel)?;
                for column in 0..w {
                    let pixel = sample_rgba(&current, column, colour, depth, &palette, transparency.as_deref());
                    let at = ((y0 + row * dy) * width + x0 + column * dx) * 4;
                    rgba[at..at + 4].copy_from_slice(&pixel);
                }
                previous = current;
            }
        }
        Image::new(width as u32, height as u32, rgba)
    }

    /// 8-bit RGBA PNG, each row filtered the way that compresses best by the
    /// usual estimate (smallest sum of absolute filtered values).
    pub fn encode_png(&self) -> Result<Vec<u8>, String> {
        let (width, height) = (self.width as usize, self.height as usize);
        let line = width * 4;
        let mut filtered = Vec::with_capacity(height * (line + 1));
        let zero = vec![0u8; line];
        for y in 0..height {
            let current = &self.rgba[y * line..(y + 1) * line];
            let previous = if y == 0 { &zero[..] } else { &self.rgba[(y - 1) * line..y * line] };
            let mut best: Option<(u64, u8, Vec<u8>)> = None;
            for filter in 0..5u8 {
                let row: Vec<u8> = (0..line).map(|i| {
                    let a = if i >= 4 { current[i - 4] } else { 0 };
                    let c = if i >= 4 { previous[i - 4] } else { 0 };
                    current[i].wrapping_sub(predict(filter, a, previous[i], c))
                }).collect();
                let cost = row.iter().map(|&v| (v as i8).unsigned_abs() as u64).sum();
                if best.as_ref().is_none_or(|(b, _, _)| cost < *b) {
                    best = Some((cost, filter, row));
                }
            }
            let (_, filter, row) = best.unwrap_or((0, 0, current.to_vec()));
            filtered.push(filter);
            filtered.extend_from_slice(&row);
        }
        let mut out = PNG_SIGNATURE.to_vec();
        let mut chunk = |kind: &[u8; 4], body: &[u8]| {
            out.extend_from_slice(&(body.len() as u32).to_be_bytes());
            let start = out.len();
            out.extend_from_slice(kind);
            out.extend_from_slice(body);
            let crc = crc32(&out[start..]);
            out.extend_from_slice(&crc.to_be_bytes());
        };
        let mut header = Vec::with_capacity(13);
        header.extend_from_slice(&self.width.to_be_bytes());
        header.extend_from_slice(&self.height.to_be_bytes());
        header.extend_from_slice(&[8, 6, 0, 0, 0]);
        chunk(b"IHDR", &header);
        chunk(b"IDAT", &zlib_compress(&filtered, 9));
        chunk(b"IEND", &[]);
        Ok(out)
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

/// The PNG predictor of `filter` from the left (a), above (b) and upper-left
/// (c) bytes.
fn predict(filter: u8, a: u8, b: u8, c: u8) -> u8 {
    match filter {
        1 => a,
        2 => b,
        3 => ((a as u16 + b as u16) / 2) as u8,
        4 => {
            let p = a as i16 + b as i16 - c as i16;
            let (pa, pb, pc) = ((p - a as i16).abs(), (p - b as i16).abs(), (p - c as i16).abs());
            if pa <= pb && pa <= pc { a } else if pb <= pc { b } else { c }
        }
        _ => 0,
    }
}

fn unfilter(filter: u8, current: &mut [u8], previous: &[u8], bpp: usize) -> Result<(), String> {
    if filter > 4 {
        return Err(format!("PNG row filter {filter} is not valid"));
    }
    for i in 0..current.len() {
        let a = if i >= bpp { current[i - bpp] } else { 0 };
        let c = if i >= bpp { previous[i - bpp] } else { 0 };
        current[i] = current[i].wrapping_add(predict(filter, a, previous[i], c));
    }
    Ok(())
}

/// Pixel `x` of an unfiltered row as 8-bit RGBA.
fn sample_rgba(row: &[u8], x: usize, colour: u8, depth: u8, palette: &[u8], transparency: Option<&[u8]>) -> [u8; 4] {
    // Sample `i` of the row at `depth` bits, and the same scaled to 8 bits.
    let raw = |i: usize| -> u16 {
        match depth {
            16 => u16::from_be_bytes([row[i * 2], row[i * 2 + 1]]),
            8 => row[i] as u16,
            _ => {
                let bit = i * depth as usize;
                let shift = 8 - depth as usize - bit % 8;
                ((row[bit / 8] >> shift) & ((1u8 << depth) - 1)) as u16
            }
        }
    };
    let eight = |v: u16| -> u8 {
        match depth {
            16 => (v >> 8) as u8,
            8 => v as u8,
            1 => (v * 255) as u8,
            2 => (v * 85) as u8,
            _ => (v * 17) as u8,
        }
    };
    let key = |i: usize| transparency.and_then(|t| t.get(i * 2..i * 2 + 2)).map(|b| u16::from_be_bytes([b[0], b[1]]));
    match colour {
        0 => {
            let v = raw(x);
            let g = eight(v);
            [g, g, g, if key(0) == Some(v) { 0 } else { 255 }]
        }
        2 => {
            let (r, g, b) = (raw(x * 3), raw(x * 3 + 1), raw(x * 3 + 2));
            let clear = key(0) == Some(r) && key(1) == Some(g) && key(2) == Some(b);
            [eight(r), eight(g), eight(b), if clear { 0 } else { 255 }]
        }
        3 => {
            let i = raw(x) as usize;
            let rgb = palette.get(i * 3..i * 3 + 3).unwrap_or(&[0, 0, 0]);
            [rgb[0], rgb[1], rgb[2], transparency.and_then(|t| t.get(i).copied()).unwrap_or(255)]
        }
        4 => {
            let g = eight(raw(x * 2));
            [g, g, g, eight(raw(x * 2 + 1))]
        }
        _ => [eight(raw(x * 4)), eight(raw(x * 4 + 1)), eight(raw(x * 4 + 2)), eight(raw(x * 4 + 3))],
    }
}
