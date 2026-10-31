//! Basis Universal textures in KTX2 (KTX 2.0 specification, "Basis
//! Universal UASTC Format"): UASTC mip chains are stored with
//! `vkFormat = VK_FORMAT_UNDEFINED` and a Data Format Descriptor whose
//! colour model is `KHR_DF_MODEL_UASTC` (166). One 16-byte block per 4x4
//! texel tile, levels largest first in the index.

use crate::{ktx2, uastc};

/// `KHR_DF_MODEL_UASTC`.
pub const DF_MODEL_UASTC: u8 = 166;
/// `KHR_DF_CHANNEL_UASTC_RGB` / `_RGBA` (KTX2 spec table).
pub const DF_CHANNEL_UASTC_RGB: u8 = 0;
pub const DF_CHANNEL_UASTC_RGBA: u8 = 3;

/// One mip level: its size in texels and its UASTC blocks.
#[derive(Clone, Debug, PartialEq)]
pub struct Level { pub width: u32, pub height: u32, pub blocks: Vec<u8> }

/// A UASTC texture read from, or to be written to, KTX2.
#[derive(Clone, Debug, PartialEq)]
pub struct UastcTexture {
    pub levels: Vec<Level>,
    /// The source had alpha (DFD channel UASTC_RGBA; else UASTC_RGB).
    pub alpha: bool,
    /// Colour data is sRGB-encoded (DFD transfer function sRGB).
    pub srgb: bool,
}

/// The basic Data Format Descriptor block for UASTC (Khronos Data Format
/// 1.3, basic descriptor, one sample of 128 bits).
fn uastc_dfd(alpha: bool, srgb: bool) -> Vec<u8> {
    let mut d = Vec::with_capacity(44);
    let block_size: u32 = 24 + 16;
    d.extend_from_slice(&(4 + block_size).to_le_bytes()); // dfdTotalSize
    d.extend_from_slice(&0u32.to_le_bytes()); // vendorId 0 (Khronos) | descriptorType 0 (basic)
    d.extend_from_slice(&(2u32 | (block_size << 16)).to_le_bytes()); // versionNumber 2 | descriptorBlockSize
    d.extend_from_slice(&[DF_MODEL_UASTC, 1 /* BT709 primaries */, if srgb { 2 } else { 1 } /* sRGB : linear */, 0 /* flags */]);
    d.extend_from_slice(&[3, 3, 0, 0]); // texelBlockDimension: 4x4
    d.extend_from_slice(&[16, 0, 0, 0, 0, 0, 0, 0]); // bytesPlane0 = 16
    // Sample: bitOffset 0, bitLength 127 (128 bits), channel id.
    let channel = if alpha { DF_CHANNEL_UASTC_RGBA } else { DF_CHANNEL_UASTC_RGB } as u32;
    d.extend_from_slice(&(127u32 << 16 | channel << 24).to_le_bytes());
    d.extend_from_slice(&[0, 0, 0, 0]); // samplePosition
    d.extend_from_slice(&0u32.to_le_bytes()); // sampleLower
    d.extend_from_slice(&u32::MAX.to_le_bytes()); // sampleUpper
    d
}

impl UastcTexture {
    /// Serialise to a KTX2 file.
    pub fn to_ktx2(&self) -> Result<Vec<u8>, String> { self.to_ktx2_with(None) }

    /// [`Self::to_ktx2`] carrying a content id (key [`CONTENT_ID_KEY`]): a
    /// builder records the image's identity once so loaders can share GPU
    /// textures by it without hashing the bytes.
    pub fn to_ktx2_with(&self, content_id: Option<u64>) -> Result<Vec<u8>, String> {
        let base = self.levels.first().ok_or("no mip levels")?;
        let mut w = ktx2::Writer::new(ktx2::Header {
            vk_format: ktx2::VK_FORMAT_UNDEFINED, type_size: 1, width: base.width, height: base.height,
            depth: 0, layers: 0, faces: 1, levels: self.levels.len() as u32, supercompression: ktx2::SUPERCOMPRESSION_NONE,
        });
        for level in &self.levels {
            if level.blocks.len() != uastc::encoded_len(level.width, level.height) { return Err(format!("level {}x{} has {} block bytes", level.width, level.height, level.blocks.len())); }
            w.level(level.blocks.clone(), level.blocks.len() as u64);
        }
        w.dfd(uastc_dfd(self.alpha, self.srgb));
        w.key_value("KTXwriter", b"makepad-texcomp\0");
        if let Some(id) = content_id { w.key_value(CONTENT_ID_KEY, &id.to_le_bytes()); }
        w.write()
    }

    /// Parse a KTX2 file holding uncompressed-scheme UASTC.
    pub fn from_ktx2(bytes: &[u8]) -> Result<Self, String> {
        let view = UastcView::parse(bytes)?;
        Ok(UastcTexture { levels: view.levels.iter().map(|(width, height, data)| Level { width: *width, height: *height, blocks: data.to_vec() }).collect(), alpha: view.alpha, srgb: view.srgb })
    }
}

/// A UASTC KTX2 read in place: each level's blocks borrow the file bytes, so
/// a loader transcodes straight from them into its upload buffer.
pub struct UastcView<'a> { pub levels: Vec<(u32, u32, &'a [u8])>, pub alpha: bool, pub srgb: bool, pub content_id: Option<u64> }

/// KTX2 key/value key of a builder-assigned content id (u64, little-endian).
pub const CONTENT_ID_KEY: &str = "mkContentId";

impl<'a> UastcView<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, String> {
        let r = ktx2::Reader::new(bytes)?;
        let h = r.header().clone();
        if h.vk_format != ktx2::VK_FORMAT_UNDEFINED { return Err("not a Basis texture (vkFormat is set)".into()); }
        if h.supercompression != ktx2::SUPERCOMPRESSION_NONE { return Err(format!("supercompression scheme {} is not supported", h.supercompression)); }
        let dfd = r.dfd().ok_or("UASTC KTX2 without a data format descriptor")?;
        // dfdTotalSize, then the basic block: colorModel at byte 12.
        if dfd.len() < 44 || dfd[12] != DF_MODEL_UASTC { return Err("not a UASTC texture (DFD colour model)".into()); }
        let srgb = dfd[14] == 2;
        let alpha = (dfd[4 + 24 + 3] & 0x0f) == DF_CHANNEL_UASTC_RGBA;
        let mut levels = Vec::new();
        for (i, _) in r.levels().iter().enumerate() {
            let (width, height) = ((h.width >> i).max(1), (h.height.max(1) >> i).max(1));
            let data = r.level_data(i)?;
            if data.len() != uastc::encoded_len(width, height) { return Err(format!("level {i}: {} bytes for {width}x{height}", data.len())); }
            levels.push((width, height, data));
        }
        let content_id = r.key_values().into_iter().find(|(k, _)| k == CONTENT_ID_KEY).and_then(|(_, v)| Some(u64::from_le_bytes(v.get(..8)?.try_into().ok()?)));
        Ok(UastcView { levels, alpha, srgb, content_id })
    }
}

/// Encode an RGBA8 mip chain (largest first, each `w*h*4` bytes) to UASTC.
pub fn encode_mips(mips: &[(u32, u32, &[u8])], quality: uastc::Quality, srgb: bool, threads: usize) -> Result<UastcTexture, String> {
    let alpha = mips.iter().any(|(_, _, rgba)| rgba.chunks_exact(4).any(|p| p[3] != 255));
    let mut levels = Vec::with_capacity(mips.len());
    for &(width, height, rgba) in mips {
        levels.push(Level { width, height, blocks: uastc::encode_image(rgba, width, height, quality, threads)? });
    }
    Ok(UastcTexture { levels, alpha, srgb })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ktx2_round_trip_keeps_levels_and_flags() {
        let base: Vec<u8> = (0..16 * 8).flat_map(|i| [(i * 3) as u8, (i * 7) as u8, 128, if i % 5 == 0 { 100 } else { 255 }]).collect();
        let small: Vec<u8> = (0..8 * 4).flat_map(|i| [(i * 9) as u8, 20, 40, 255]).collect();
        let tex = encode_mips(&[(16, 8, &base), (8, 4, &small)], uastc::Quality::Fast, true, 1).unwrap();
        assert!(tex.alpha);
        let bytes = tex.to_ktx2().unwrap();
        let back = UastcTexture::from_ktx2(&bytes).unwrap();
        assert_eq!(back, tex);
        // Hand-checked DFD fields.
        let r = ktx2::Reader::new(&bytes).unwrap();
        let dfd = r.dfd().unwrap();
        assert_eq!(u32::from_le_bytes(dfd[0..4].try_into().unwrap()) as usize, dfd.len());
        assert_eq!((dfd[12], dfd[16], dfd[17], dfd[20]), (DF_MODEL_UASTC, 3, 3, 16));
    }
}
