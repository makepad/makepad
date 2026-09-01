//! A picture of a generated map, for a human to look at.
//!
//! The generator's own tests write these: fairness numbers say a map is
//! legal, and only a picture says it is any good. The PNG writer is the
//! smallest correct one (stored deflate) rather than a dependency — this
//! crate stays std-only, and a preview is written once by a test, never in a
//! hot path.

use crate::{PropKind, RtsMap, Style, Terrain};

/// Flat colour per terrain, before height shading. The style only changes
/// the two GROUND colours: a preview of a desert that is green tells a human
/// the wrong thing about the map they are about to look at.
fn terrain_color(terrain: Terrain, style: Style) -> [u8; 3] {
    match terrain {
        Terrain::Clear => match style {
            Style::Desert => [0xc4, 0xa8, 0x6e],
            _ => [0x6a, 0x7c, 0x4a],
        },
        Terrain::Rough => match style {
            Style::Desert => [0xa8, 0x8c, 0x56],
            _ => [0x87, 0x7a, 0x52],
        },
        Terrain::Shore => [0xc2, 0xb6, 0x84],
        Terrain::Road => [0x8d, 0x84, 0x74],
        Terrain::Water => [0x2c, 0x4e, 0x86],
        Terrain::Cliff => [0x4a, 0x40, 0x36],
        Terrain::Plateau => [0x7e, 0x72, 0x5c],
        Terrain::Resource => [0x2f, 0xb0, 0x66],
    }
}

/// RGBA preview at `scale` pixels per cell, with resources tinted by
/// richness, blocking scenery as dark pips and every start as its house
/// colour inside a white ring.
pub fn rgba(map: &RtsMap, scale: u32) -> (Vec<u8>, u32, u32) {
    let scale = scale.max(1);
    let (w, h) = (map.width as u32 * scale, map.height as u32 * scale);
    let mut image = vec![0u8; (w * h * 4) as usize];
    let mut put = |x: u32, y: u32, rgb: [u8; 3]| {
        if x >= w || y >= h {
            return;
        }
        let at = ((y * w + x) * 4) as usize;
        image[at] = rgb[0];
        image[at + 1] = rgb[1];
        image[at + 2] = rgb[2];
        image[at + 3] = 255;
    };
    let stages = map.stage_grid();
    for cy in 0..map.height as u32 {
        for cx in 0..map.width as u32 {
            let at = (cy * map.width as u32 + cx) as usize;
            let terrain = map.terrain[at];
            let mut rgb = terrain_color(terrain, map.spec.style);
            // Raised ground reads lighter, one step per level.
            let lift = map.heights[at] as i32 * 18;
            for channel in rgb.iter_mut() {
                *channel = (*channel as i32 + lift).clamp(0, 255) as u8;
            }
            if terrain == Terrain::Resource {
                let stage = stages[at].unwrap_or(0) as i32;
                let lit = 60 + stage * 16;
                rgb = [(lit / 5) as u8, lit.clamp(0, 255) as u8, (lit / 2) as u8];
            }
            for py in 0..scale {
                for px in 0..scale {
                    put(cx * scale + px, cy * scale + py, rgb);
                }
            }
        }
    }
    for prop in &map.props {
        let rgb = match prop.kind {
            PropKind::Tree => [0x1e, 0x38, 0x1a],
            PropKind::Rock => [0x3a, 0x36, 0x30],
            PropKind::Ruin => [0x5a, 0x4a, 0x4a],
            PropKind::Bloom => [0xd8, 0xe0, 0x40],
        };
        let (bx, by) = (prop.x as u32 * scale, prop.y as u32 * scale);
        for py in 0..scale.max(1) {
            for px in 0..scale.max(1) {
                put(bx + px, by + py, rgb);
            }
        }
    }
    for (index, start) in map.starts.iter().enumerate() {
        let color = map
            .houses
            .get(index)
            .map(|house| house.color)
            .unwrap_or([255, 255, 255]);
        let (bx, by) = (start.x as i32 * scale as i32, start.y as i32 * scale as i32);
        let ring = scale as i32 * 3;
        for dy in -ring..=ring {
            for dx in -ring..=ring {
                let (x, y) = (bx + dx, by + dy);
                if x < 0 || y < 0 {
                    continue;
                }
                let d = ((dx * dx + dy * dy) as f32).sqrt();
                if d > ring as f32 {
                    continue;
                }
                let rgb = if d > ring as f32 - 1.5 { [255, 255, 255] } else { color };
                put(x as u32, y as u32, rgb);
            }
        }
    }
    (image, w, h)
}

/// Encode RGBA8 as a PNG. Stored deflate: bigger on disk, no dependency, and
/// every viewer reads it.
pub fn png(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut raw = Vec::with_capacity((width as usize * 4 + 1) * height as usize);
    for y in 0..height as usize {
        raw.push(0u8); // filter: none
        let row = y * width as usize * 4;
        raw.extend_from_slice(&rgba[row..row + width as usize * 4]);
    }
    let mut out = Vec::new();
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &zlib_stored(&raw));
    chunk(&mut out, b"IEND", &[]);
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = Vec::with_capacity(4 + data.len());
    crc.extend_from_slice(kind);
    crc.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc).to_be_bytes());
}

fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let mut at = 0;
    while at < data.len() {
        let take = (data.len() - at).min(0xffff);
        let last = at + take >= data.len();
        out.push(if last { 1 } else { 0 });
        out.extend_from_slice(&(take as u16).to_le_bytes());
        out.extend_from_slice(&(!(take as u16)).to_le_bytes());
        out.extend_from_slice(&data[at..at + take]);
        at += take;
    }
    if data.is_empty() {
        out.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for byte in data {
        a = (a + *byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in data {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}
