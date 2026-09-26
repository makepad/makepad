//! Windows icon images: the per-size `RT_ICON` payloads (32-bit DIB, or PNG
//! for 256 px), the `RT_GROUP_ICON` directory that names them, and the .ico
//! file format for reading existing icons.
use crate::icns::{decode_icns, SourceImage};
use crate::image::{Image, PNG_SIGNATURE};

/// Sizes Explorer, the taskbar and Alt-Tab ask for at 100–200 % scaling.
pub const ICON_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

#[derive(Clone, Debug, PartialEq)]
pub struct IconImage {
    pub width: u32,
    pub height: u32,
    /// A PNG file, or a BITMAPINFOHEADER DIB with doubled height followed by
    /// the colour pixels and the 1-bit AND mask.
    pub data: Vec<u8>,
}

impl IconImage {
    pub fn is_png(&self) -> bool {
        self.data.starts_with(PNG_SIGNATURE)
    }
    pub fn decode(&self) -> Result<Image, String> {
        if self.is_png() { Image::decode_png(&self.data) } else { decode_dib(&self.data) }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GroupEntry {
    pub width: u32,
    pub height: u32,
    pub bit_count: u16,
    pub bytes: u32,
    pub id: u16,
}

/// Decode an icon source by content: `.icns`, a single PNG, or `.ico`.
pub fn load_source(bytes: &[u8]) -> Result<Vec<SourceImage>, String> {
    let images = if bytes.starts_with(b"icns") {
        decode_icns(bytes)?
    } else if bytes.starts_with(PNG_SIGNATURE) {
        vec![SourceImage { image: Image::decode_png(bytes)?, png: Some(bytes.to_vec()) }]
    } else if bytes.starts_with(&[0, 0, 1, 0]) {
        let mut images = Vec::new();
        for icon in parse_ico(bytes)? {
            let png = icon.is_png().then(|| icon.data.clone());
            images.push(SourceImage { image: icon.decode()?, png });
        }
        images
    } else {
        return Err("unsupported icon source: expected .icns, .png or .ico".into());
    };
    if images.is_empty() {
        return Err("icon source holds no pictures we can read".into());
    }
    Ok(images)
}

/// The icon's pictures for each of `ICON_SIZES` up to the largest source.
/// Exact source sizes are used as they are; others are reduced from the
/// smallest larger source. 256 px is stored as PNG, the rest as DIBs, which
/// every Windows version and icon consumer reads.
pub fn icon_images(sources: &[SourceImage]) -> Result<Vec<IconImage>, String> {
    let side = |s: &SourceImage| s.image.width.min(s.image.height);
    let largest = sources.iter().map(side).max().ok_or("icon source holds no pictures")?;
    let mut sizes: Vec<u32> = ICON_SIZES.iter().copied().filter(|&s| s <= largest).collect();
    if sizes.is_empty() {
        sizes.push(largest);
    }
    let mut images = Vec::new();
    for size in sizes {
        let exact = sources.iter().find(|s| s.image.width == size && s.image.height == size);
        if size >= 256 {
            if let Some(png) = exact.and_then(|s| s.png.clone()) {
                images.push(IconImage { width: size, height: size, data: png });
                continue;
            }
        }
        let image = match exact {
            Some(source) => source.image.clone(),
            None => {
                let from = sources.iter().filter(|s| side(s) >= size).min_by_key(|s| side(s)).unwrap();
                from.image.resample(size, size)
            }
        };
        let data = if size >= 256 { image.encode_png()? } else { encode_dib(&image) };
        images.push(IconImage { width: size, height: size, data });
    }
    Ok(images)
}

pub fn encode_dib(image: &Image) -> Vec<u8> {
    let (w, h) = (image.width as usize, image.height as usize);
    let mask_stride = (w + 31) / 32 * 4;
    let mut out = Vec::with_capacity(40 + w * h * 4 + mask_stride * h);
    for v in [40u32, w as u32, (h * 2) as u32] { out.extend_from_slice(&v.to_le_bytes()); }
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&((w * h * 4 + mask_stride * h) as u32).to_le_bytes());
    out.extend_from_slice(&[0; 16]);
    for y in (0..h).rev() {
        for p in image.rgba[y * w * 4..(y + 1) * w * 4].chunks_exact(4) {
            out.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
    }
    // The AND mask marks fully transparent pixels for consumers that ignore
    // the alpha channel.
    for y in (0..h).rev() {
        let mut row = vec![0u8; mask_stride];
        for x in 0..w {
            if image.rgba[(y * w + x) * 4 + 3] == 0 { row[x / 8] |= 0x80 >> (x % 8); }
        }
        out.extend_from_slice(&row);
    }
    out
}

/// Read a 32- or 24-bit icon DIB (the formats `encode_dib` and common icon
/// editors write).
pub fn decode_dib(data: &[u8]) -> Result<Image, String> {
    let u32_at = |at: usize| data.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let header = u32_at(0).ok_or("DIB truncated")? as usize;
    let w = u32_at(4).ok_or("DIB truncated")? as i32;
    let h2 = u32_at(8).ok_or("DIB truncated")? as i32;
    let bits = data.get(14..16).map(|b| u16::from_le_bytes(b.try_into().unwrap())).ok_or("DIB truncated")?;
    let compression = u32_at(16).ok_or("DIB truncated")?;
    if header < 40 || w <= 0 || h2 <= 0 || w > 1024 || h2 > 2048 || compression != 0 || !(bits == 32 || bits == 24) {
        return Err(format!("unsupported icon DIB: {w}x{h2}, {bits} bit, compression {compression}"));
    }
    let (w, h) = (w as usize, h2 as usize / 2);
    let stride = (w * bits as usize / 8 + 3) / 4 * 4;
    let mask_stride = (w + 31) / 32 * 4;
    let pixels = data.get(header..header + stride * h).ok_or("DIB pixels truncated")?;
    let mask = data.get(header + stride * h..header + stride * h + mask_stride * h);
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        let row = &pixels[(h - 1 - y) * stride..];
        for x in 0..w {
            let p = &row[x * bits as usize / 8..];
            let alpha = if bits == 32 {
                p[3]
            } else {
                let masked = mask.map_or(false, |m| m[(h - 1 - y) * mask_stride + x / 8] & (0x80 >> (x % 8)) != 0);
                if masked { 0 } else { 255 }
            };
            rgba[(y * w + x) * 4..(y * w + x + 1) * 4].copy_from_slice(&[p[2], p[1], p[0], alpha]);
        }
    }
    Image::new(w as u32, h as u32, rgba)
}

fn dir_dimension(v: u32) -> u8 {
    if v >= 256 { 0 } else { v as u8 }
}

/// `GRPICONDIR`: the `RT_GROUP_ICON` payload naming `RT_ICON` resources
/// `first_id..`.
pub fn group_icon(images: &[IconImage], first_id: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(6 + 14 * images.len());
    for v in [0u16, 1, images.len() as u16] { out.extend_from_slice(&v.to_le_bytes()); }
    for (i, image) in images.iter().enumerate() {
        out.extend_from_slice(&[dir_dimension(image.width), dir_dimension(image.height), 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(image.data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(first_id + i as u16).to_le_bytes());
    }
    out
}

pub fn parse_group_icon(data: &[u8]) -> Result<Vec<GroupEntry>, String> {
    let u16_at = |at: usize| data.get(at..at + 2).map(|b| u16::from_le_bytes(b.try_into().unwrap())).ok_or("group icon truncated");
    if u16_at(0)? != 0 || u16_at(2)? != 1 {
        return Err("not a group icon directory".into());
    }
    let count = u16_at(4)? as usize;
    (0..count).map(|i| {
        let e = data.get(6 + i * 14..6 + (i + 1) * 14).ok_or("group icon entry truncated")?;
        let dim = |b: u8| if b == 0 { 256 } else { b as u32 };
        Ok(GroupEntry {
            width: dim(e[0]),
            height: dim(e[1]),
            bit_count: u16::from_le_bytes([e[6], e[7]]),
            bytes: u32::from_le_bytes(e[8..12].try_into().unwrap()),
            id: u16::from_le_bytes([e[12], e[13]]),
        })
    }).collect()
}

pub fn write_ico(images: &[IconImage]) -> Vec<u8> {
    let mut out = Vec::new();
    for v in [0u16, 1, images.len() as u16] { out.extend_from_slice(&v.to_le_bytes()); }
    let mut offset = 6 + 16 * images.len();
    for image in images {
        out.extend_from_slice(&[dir_dimension(image.width), dir_dimension(image.height), 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(image.data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += image.data.len();
    }
    for image in images { out.extend_from_slice(&image.data); }
    out
}

pub fn parse_ico(bytes: &[u8]) -> Result<Vec<IconImage>, String> {
    let u16_at = |at: usize| bytes.get(at..at + 2).map(|b| u16::from_le_bytes(b.try_into().unwrap())).ok_or(".ico truncated");
    if u16_at(0)? != 0 || u16_at(2)? != 1 {
        return Err("not an .ico file".into());
    }
    let count = u16_at(4)? as usize;
    (0..count).map(|i| {
        let e = bytes.get(6 + i * 16..6 + (i + 1) * 16).ok_or(".ico entry truncated")?;
        let size = u32::from_le_bytes(e[8..12].try_into().unwrap()) as usize;
        let offset = u32::from_le_bytes(e[12..16].try_into().unwrap()) as usize;
        let data = bytes.get(offset..offset.checked_add(size).ok_or(".ico entry overflows")?).ok_or(".ico image outside the file")?.to_vec();
        let dim = |b: u8| if b == 0 { 256 } else { b as u32 };
        Ok(IconImage { width: dim(e[0]), height: dim(e[1]), data })
    }).collect()
}
