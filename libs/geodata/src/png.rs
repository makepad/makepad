//! Minimal PNG codec for raster overlay tiles: 8-bit grayscale (class-index
//! rasters like noise/flood) and 8-bit RGB (terrarium elevation). Encoder
//! writes filter-0 rows + zlib (flate2); decoder handles exactly what any
//! standard encoder emits for these formats (all five row filters,
//! non-interlaced). CRC32 is the 30-line table version — not worth a dep.

use flate2::write::ZlibEncoder;
use flate2::Compression;
use std::io::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PngFormat {
    Gray8,
    Rgb8,
}

impl PngFormat {
    fn color_type(&self) -> u8 {
        match self {
            PngFormat::Gray8 => 0,
            PngFormat::Rgb8 => 2,
        }
    }
    pub fn bytes_per_pixel(&self) -> usize {
        match self {
            PngFormat::Gray8 => 1,
            PngFormat::Rgb8 => 3,
        }
    }
}

pub fn encode(width: u32, height: u32, format: PngFormat, pixels: &[u8]) -> Vec<u8> {
    let bpp = format.bytes_per_pixel();
    assert_eq!(pixels.len(), width as usize * height as usize * bpp);

    let mut raw = Vec::with_capacity(pixels.len() + height as usize);
    for row in pixels.chunks_exact(width as usize * bpp) {
        raw.push(0); // filter type 0
        raw.extend_from_slice(row);
    }
    let mut z = ZlibEncoder::new(Vec::new(), Compression::new(6));
    z.write_all(&raw).unwrap();
    let idat = z.finish().unwrap();

    let mut out = Vec::with_capacity(idat.len() + 64);
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(format.color_type());
    ihdr.extend_from_slice(&[0, 0, 0]); // deflate, filter 0, no interlace
    write_chunk(&mut out, b"IHDR", &ihdr);
    write_chunk(&mut out, b"IDAT", &idat);
    write_chunk(&mut out, b"IEND", &[]);
    out
}

pub struct DecodedPng {
    pub width: u32,
    pub height: u32,
    pub format: PngFormat,
    pub pixels: Vec<u8>,
}

pub fn decode(data: &[u8]) -> Result<DecodedPng, String> {
    if data.len() < 8 || &data[0..8] != &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a] {
        return Err("not a png".into());
    }
    let mut pos = 8usize;
    let mut width = 0u32;
    let mut height = 0u32;
    let mut format = PngFormat::Gray8;
    let mut idat = Vec::new();
    while pos + 8 <= data.len() {
        let len = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        let kind = &data[pos + 4..pos + 8];
        let body = data
            .get(pos + 8..pos + 8 + len)
            .ok_or("truncated png chunk")?;
        match kind {
            b"IHDR" => {
                width = u32::from_be_bytes(body[0..4].try_into().unwrap());
                height = u32::from_be_bytes(body[4..8].try_into().unwrap());
                if body[8] != 8 || body[12] != 0 {
                    return Err("unsupported png (need 8-bit non-interlaced)".into());
                }
                format = match body[9] {
                    0 => PngFormat::Gray8,
                    2 => PngFormat::Rgb8,
                    other => return Err(format!("unsupported png color type {other}")),
                };
            }
            b"IDAT" => idat.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        pos += 12 + len; // len + type + crc
    }
    if width == 0 || height == 0 {
        return Err("png missing IHDR".into());
    }

    let mut raw = Vec::new();
    flate2::read::ZlibDecoder::new(&idat[..])
        .read_to_end(&mut raw)
        .map_err(|e| format!("png inflate: {e}"))?;

    let bpp = format.bytes_per_pixel();
    let stride = width as usize * bpp;
    if raw.len() < height as usize * (stride + 1) {
        return Err("png data too short".into());
    }
    let mut pixels = vec![0u8; height as usize * stride];
    for y in 0..height as usize {
        let filter = raw[y * (stride + 1)];
        let row_in = &raw[y * (stride + 1) + 1..y * (stride + 1) + 1 + stride];
        for x in 0..stride {
            let a = if x >= bpp {
                pixels[y * stride + x - bpp]
            } else {
                0
            };
            let b = if y > 0 { pixels[(y - 1) * stride + x] } else { 0 };
            let c = if y > 0 && x >= bpp {
                pixels[(y - 1) * stride + x - bpp]
            } else {
                0
            };
            let value = match filter {
                0 => row_in[x],
                1 => row_in[x].wrapping_add(a),
                2 => row_in[x].wrapping_add(b),
                3 => row_in[x].wrapping_add(((u16::from(a) + u16::from(b)) / 2) as u8),
                4 => row_in[x].wrapping_add(paeth(a, b, c)),
                other => return Err(format!("unsupported png filter {other}")),
            };
            pixels[y * stride + x] = value;
        }
    }
    Ok(DecodedPng {
        width,
        height,
        format,
        pixels,
    })
}

use std::io::Read;

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (a, b, c) = (i16::from(a), i16::from(b), i16::from(c));
    let p = a + b - c;
    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
    if pa <= pb && pa <= pc {
        a as u8
    } else if pb <= pc {
        b as u8
    } else {
        c as u8
    }
}

fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    let mut crc = Crc32::new();
    crc.update(kind);
    crc.update(body);
    out.extend_from_slice(&crc.finish().to_be_bytes());
}

struct Crc32 {
    table: [u32; 256],
    value: u32,
}

impl Crc32 {
    fn new() -> Self {
        let mut table = [0u32; 256];
        for (n, entry) in table.iter_mut().enumerate() {
            let mut c = n as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *entry = c;
        }
        Crc32 {
            table,
            value: 0xffff_ffff,
        }
    }
    fn update(&mut self, data: &[u8]) {
        for &byte in data {
            self.value = self.table[((self.value ^ u32::from(byte)) & 0xff) as usize]
                ^ (self.value >> 8);
        }
    }
    fn finish(self) -> u32 {
        self.value ^ 0xffff_ffff
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_round_trip() {
        let width = 32u32;
        let height = 16u32;
        let pixels: Vec<u8> = (0..width * height * 3).map(|i| (i % 251) as u8).collect();
        let encoded = encode(width, height, PngFormat::Rgb8, &pixels);
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded.width, width);
        assert_eq!(decoded.height, height);
        assert_eq!(decoded.format, PngFormat::Rgb8);
        assert_eq!(decoded.pixels, pixels);
    }
}
