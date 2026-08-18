//! The `testpattern` backend: renders a deterministic procedural PNG derived
//! from the prompt hash and seed. No GPU, no model files — it exists so the
//! entire service (registry, queue, job states, artifact serving, client)
//! can be exercised end-to-end on any machine, including CI.

use crate::backend::{CancelToken, ArtifactData, BackendCtx, ContentBackend, GenerateParams, ProgressSink};
use crate::error::AssetAiError;
use crate::sha256::Sha256;
use makepad_zune_core::bit_depth::BitDepth;
use makepad_zune_core::colorspace::ColorSpace;
use makepad_zune_core::options::EncoderOptions;
use makepad_zune_png::PngEncoder;

pub struct TestPatternBackend {
    model_id: String,
}

impl TestPatternBackend {
    pub fn new(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
        }
    }
}

impl ContentBackend for TestPatternBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn ensure_loaded(&mut self, _ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        // No files, nothing to load.
        Ok(())
    }

    fn generate(
        &mut self,
        params: &GenerateParams,
        progress: ProgressSink,
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError> {
        progress("render", 0.0);
        // Optional artificial generation time (delay_ms), spent in slices so
        // the job is observably "running" — used by the queue-policy tests.
        if params.delay_ms > 0 {
            let slices = 10u64;
            for slice in 0..slices {
                cancel.check()?;
                std::thread::sleep(std::time::Duration::from_millis(
                    params.delay_ms / slices,
                ));
                progress(
                    &format!("render {}/{}", slice + 1, slices),
                    0.8 * (slice + 1) as f64 / slices as f64,
                );
            }
        }
        cancel.check()?;
        let width = params.width.unwrap_or(512);
        let height = params.height.unwrap_or(512);
        let pixels = render_pattern(&params.prompt, params.seed, width, height);
        progress("encode", 0.9);
        let png = encode_png_rgba(&pixels, width as usize, height as usize)?;
        progress("done", 1.0);
        Ok(vec![ArtifactData {
            content_type: "image/png",
            ext: "png",
            bytes: png,
        }])
    }
}

/// Deterministic plasma-ish pattern; all frequencies/phases/palette come from
/// sha256(prompt, seed), so the same request always renders the same image
/// and different prompts are visually distinct.
fn render_pattern(prompt: &str, seed: u64, width: u32, height: u32) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(prompt.as_bytes());
    hasher.update(&seed.to_le_bytes());
    let digest = hasher.finish();

    let unit = |byte: u8| byte as f32 / 255.0;
    let freq_a = 2.0 + unit(digest[0]) * 14.0;
    let freq_b = 2.0 + unit(digest[1]) * 14.0;
    let freq_c = 1.0 + unit(digest[2]) * 8.0;
    let phase_a = unit(digest[3]) * std::f32::consts::TAU;
    let phase_b = unit(digest[4]) * std::f32::consts::TAU;
    let phase_c = unit(digest[5]) * std::f32::consts::TAU;
    let color_a = [unit(digest[6]), unit(digest[7]), unit(digest[8])];
    let color_b = [unit(digest[9]), unit(digest[10]), unit(digest[11])];

    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            let u = x as f32 / width.max(1) as f32;
            let v = y as f32 / height.max(1) as f32;
            let plasma = ((u * freq_a + phase_a).sin()
                + (v * freq_b + phase_b).sin()
                + ((u + v) * freq_c + phase_c).sin())
                / 3.0;
            let t = plasma * 0.5 + 0.5;
            let mut rgb = [
                color_a[0] + (color_b[0] - color_a[0]) * t,
                color_a[1] + (color_b[1] - color_a[1]) * t,
                color_a[2] + (color_b[2] - color_a[2]) * t,
            ];
            // Corner markers + border so scaling/cropping bugs are visible.
            let border = x < 2 || y < 2 || x >= width - 2 || y >= height - 2;
            let corner = (x < 12 || x >= width.saturating_sub(12))
                && (y < 12 || y >= height.saturating_sub(12));
            if border || corner {
                rgb = [1.0, 1.0, 1.0];
            }
            pixels.push((rgb[0].clamp(0.0, 1.0) * 255.0) as u8);
            pixels.push((rgb[1].clamp(0.0, 1.0) * 255.0) as u8);
            pixels.push((rgb[2].clamp(0.0, 1.0) * 255.0) as u8);
            pixels.push(255);
        }
    }
    pixels
}

pub fn encode_png_rgba(pixels: &[u8], width: usize, height: usize) -> Result<Vec<u8>, AssetAiError> {
    if pixels.len() != width * height * 4 {
        return Err(AssetAiError::Backend(format!(
            "png encode expected {} bytes, got {}",
            width * height * 4,
            pixels.len()
        )));
    }
    let options = EncoderOptions::default()
        .set_width(width)
        .set_height(height)
        .set_depth(BitDepth::Eight)
        .set_colorspace(ColorSpace::RGBA);
    let mut encoder = PngEncoder::new(pixels, options);
    let mut out = Vec::new();
    encoder
        .encode(&mut out)
        .map_err(|err| AssetAiError::Backend(format!("png encode failed: {err:?}")))?;
    Ok(out)
}

/// 16-bit grayscale PNG. Samples are encoded big-endian per the PNG spec.
pub fn encode_png_gray16(samples: &[u16], width: usize, height: usize) -> Result<Vec<u8>, AssetAiError> {
    if samples.len() != width * height {
        return Err(AssetAiError::Backend(format!(
            "gray16 png encode expected {} samples, got {}",
            width * height,
            samples.len()
        )));
    }
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for &sample in samples {
        bytes.extend_from_slice(&sample.to_be_bytes());
    }
    let options = EncoderOptions::default()
        .set_width(width)
        .set_height(height)
        .set_depth(BitDepth::Sixteen)
        .set_colorspace(ColorSpace::Luma);
    let mut encoder = PngEncoder::new(&bytes, options);
    let mut out = Vec::new();
    encoder
        .encode(&mut out)
        .map_err(|err| AssetAiError::Backend(format!("gray16 png encode failed: {err:?}")))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_prompt_sensitive() {
        let a1 = render_pattern("a red fox", 7, 32, 32);
        let a2 = render_pattern("a red fox", 7, 32, 32);
        let b = render_pattern("a blue whale", 7, 32, 32);
        assert_eq!(a1, a2);
        assert_ne!(a1, b);
    }

    #[test]
    fn encodes_valid_png() {
        let pixels = render_pattern("prompt", 1, 48, 24);
        let png = encode_png_rgba(&pixels, 48, 24).unwrap();
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']);
        // IHDR width/height, big-endian at offsets 16 and 20.
        let width = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
        let height = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
        assert_eq!((width, height), (48, 24));
    }

    #[test]
    fn encoder_compresses_repetitive_rgba() {
        let pixels = vec![127u8; 256 * 256 * 4];
        let png = encode_png_rgba(&pixels, 256, 256).unwrap();
        assert!(png.len() < pixels.len() / 20, "png bytes {}", png.len());
    }
}
