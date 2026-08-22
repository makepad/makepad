//! Minimal deterministic PNG encoder (stored-deflate) for artifact dumps and
//! tests. Output is valid PNG: zlib stream with uncompressed blocks, filter 0
//! per scanline. Not size-optimized on purpose — production GLB texture
//! embedding happens at the integration seam with the real writers.

use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PngColor {
    Gray,
    Rgb,
    Rgba,
}

impl PngColor {
    fn color_type(self) -> u8 {
        match self {
            PngColor::Gray => 0,
            PngColor::Rgb => 2,
            PngColor::Rgba => 6,
        }
    }

    pub fn bytes_per_pixel(self) -> usize {
        match self {
            PngColor::Gray => 1,
            PngColor::Rgb => 3,
            PngColor::Rgba => 4,
        }
    }
}

static CRC_TABLE: OnceLock<[u32; 256]> = OnceLock::new();

fn crc_table() -> &'static [u32; 256] {
    CRC_TABLE.get_or_init(|| {
        let mut table = [0u32; 256];
        for (n, entry) in table.iter_mut().enumerate() {
            let mut c = n as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xedb88320 ^ (c >> 1) } else { c >> 1 };
            }
            *entry = c;
        }
        table
    })
}

pub fn crc32(data: &[u8]) -> u32 {
    let table = crc_table();
    let mut c = 0xffff_ffffu32;
    for &b in data {
        c = table[((c ^ b as u32) & 0xff) as usize] ^ (c >> 8);
    }
    c ^ 0xffff_ffff
}

pub fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for chunk in data.chunks(5552) {
        for &byte in chunk {
            a += byte as u32;
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

fn push_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let crc_start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[crc_start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// Encode 8-bit image data (row-major, tightly packed) as a PNG file.
pub fn encode_png(width: u32, height: u32, color: PngColor, data: &[u8]) -> Vec<u8> {
    let bpp = color.bytes_per_pixel();
    assert_eq!(
        data.len(),
        width as usize * height as usize * bpp,
        "pixel data length mismatch"
    );

    // Raw scanline stream: filter byte 0 + row bytes.
    let stride = width as usize * bpp;
    let mut raw = Vec::with_capacity((stride + 1) * height as usize);
    for row in 0..height as usize {
        raw.push(0);
        raw.extend_from_slice(&data[row * stride..(row + 1) * stride]);
    }

    // zlib with stored deflate blocks.
    let mut z = Vec::with_capacity(raw.len() + raw.len() / 65535 * 5 + 16);
    z.push(0x78);
    z.push(0x01);
    let mut offset = 0;
    while offset < raw.len() || raw.is_empty() {
        let take = (raw.len() - offset).min(65535);
        let last = offset + take == raw.len();
        z.push(if last { 1 } else { 0 });
        z.extend_from_slice(&(take as u16).to_le_bytes());
        z.extend_from_slice(&(!(take as u16)).to_le_bytes());
        z.extend_from_slice(&raw[offset..offset + take]);
        offset += take;
        if last {
            break;
        }
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut out = Vec::with_capacity(z.len() + 64);
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(color.color_type());
    ihdr.push(0); // compression
    ihdr.push(0); // filter
    ihdr.push(0); // interlace
    push_chunk(&mut out, b"IHDR", &ihdr);
    push_chunk(&mut out, b"IDAT", &z);
    push_chunk(&mut out, b"IEND", &[]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_vectors() {
        assert_eq!(crc32(b"123456789"), 0xcbf43926);
        assert_eq!(adler32(b"Wikipedia"), 0x11e60398);
    }

    #[test]
    fn small_rgb_structure() {
        let png = encode_png(2, 2, PngColor::Rgb, &[255u8; 12]);
        assert_eq!(&png[0..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        // IHDR immediately follows: length 13, type, then width/height BE.
        assert_eq!(&png[8..12], &13u32.to_be_bytes());
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[16..20], &2u32.to_be_bytes());
        assert_eq!(&png[20..24], &2u32.to_be_bytes());
        assert_eq!(png[24], 8);
        assert_eq!(png[25], 2);
        // File ends with the IEND chunk (4 len + 4 type + 4 crc).
        let n = png.len();
        assert_eq!(&png[n - 8..n - 4], b"IEND");
    }

    #[test]
    fn large_gray_spans_multiple_stored_blocks() {
        let w = 300u32;
        let h = 300u32;
        let data = vec![7u8; (w * h) as usize];
        let png = encode_png(w, h, PngColor::Gray, &data);
        // raw = 300 * 301 = 90300 bytes > 65535 -> at least two stored blocks.
        assert!(png.len() > 90300);
        let again = encode_png(w, h, PngColor::Gray, &data);
        assert_eq!(png, again, "encoder must be deterministic");
    }
}
