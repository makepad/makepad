//! PNG out: the captured BGRA8 view target → RGBA PNG bytes / file.

use makepad_zune_png::{
    makepad_zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions},
    PngEncoder,
};

/// Encode a BGRA8 buffer (makepad's native render-target layout) as an
/// opaque RGBA PNG.
pub fn encode_bgra8(width: usize, height: usize, bgra: &[u8]) -> Result<Vec<u8>, String> {
    if bgra.len() < width * height * 4 {
        return Err(format!("png: expected {} bytes, got {}", width * height * 4, bgra.len()));
    }
    let mut rgba = Vec::with_capacity(width * height * 4);
    for px in bgra[..width * height * 4].chunks_exact(4) {
        rgba.extend([px[2], px[1], px[0], 255]);
    }
    let options = EncoderOptions::default()
        .set_width(width)
        .set_height(height)
        .set_depth(BitDepth::Eight)
        .set_colorspace(ColorSpace::RGBA);
    let mut encoder = PngEncoder::new(&rgba, options);
    let mut out = Vec::new();
    encoder.encode(&mut out).map_err(|e| format!("png encode failed: {e:?}"))?;
    Ok(out)
}

pub fn write_bgra8(path: &std::path::Path, width: usize, height: usize, bgra: &[u8]) -> Result<(), String> {
    let bytes = encode_bgra8(width, height, bgra)?;
    std::fs::write(path, bytes).map_err(|e| format!("png write {}: {e}", path.display()))
}
