//! The `testpattern` backend: renders a deterministic procedural PNG derived
//! from the prompt hash and seed. No GPU, no model files — it exists so the
//! entire service (registry, queue, job states, artifact serving, client)
//! can be exercised end-to-end on any machine, including CI.

use crate::backend::{
    ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, LiveFrameIn,
    LiveFrameOut, ProgressSink, RgbImage,
};
use crate::error::AssetAiError;
use crate::sha256::Sha256;
use makepad_zune_core::bit_depth::BitDepth;
use makepad_zune_core::colorspace::ColorSpace;
use makepad_zune_core::options::{DecoderOptions, EncoderOptions};
use makepad_zune_png::{PngDecoder, PngEncoder};
use std::io::BufReader;

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

    fn live_supported(&self) -> bool {
        true
    }

    /// Blends the (optional, resized-if-needed) init image with an animated
    /// procedural pattern keyed by prompt+seed (palette/rings) and
    /// frame_index (rotation phase), honoring `strength` (0 = pass-through
    /// init, 1 = pure pattern). This is the CPU everywhere-testable stand-in
    /// for a real diffusion live-edit step — no GPU, no model weights.
    ///
    /// Camera motion (`config.camera`) is intentionally NOT applied here:
    /// `crate::realtime::warp_feedback` is the ONE place that transform
    /// lives, applied to the previous output before it reaches `init` here
    /// (feedback loop mode only).
    fn live_step(&mut self, frame: LiveFrameIn<'_>, cancel: &CancelToken) -> Result<LiveFrameOut, AssetAiError> {
        cancel.check()?;
        let start = std::time::Instant::now();
        let config = frame.config;
        let width = config.width.max(1);
        let height = config.height.max(1);
        let pattern = render_live_pattern(&config.prompt, config.seed, frame.frame_index, width, height);
        let strength = config.strength.clamp(0.0, 1.0);
        let out_data = match frame.init {
            Some(init) if !init.data.is_empty() => {
                let resized = if init.width == width && init.height == height {
                    init.data.clone()
                } else {
                    resize_nearest_rgb8(init, width, height)
                };
                blend_rgb8(&resized, &pattern, strength)
            }
            _ => pattern,
        };
        cancel.check()?;
        Ok(LiveFrameOut {
            image: RgbImage { width, height, data: out_data },
            model_ms: start.elapsed().as_secs_f64() * 1000.0,
            text_encode_ms: 0.0,
        })
    }
}

/// Animated concentric-ring pattern: palette and ring frequency come from
/// sha256(prompt, seed) (stable across frames, sensitive to prompt changes —
/// same determinism convention as [`render_pattern`]); the ring phase
/// advances with `frame_index` so consecutive live frames visibly move.
fn render_live_pattern(prompt: &str, seed: u64, frame_index: u64, width: u32, height: u32) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(prompt.as_bytes());
    hasher.update(&seed.to_le_bytes());
    let digest = hasher.finish();
    let unit = |byte: u8| byte as f32 / 255.0;
    let color_a = [unit(digest[0]), unit(digest[1]), unit(digest[2])];
    let color_b = [unit(digest[3]), unit(digest[4]), unit(digest[5])];
    let ring_freq = 6.0 + unit(digest[6]) * 12.0;
    let speed = 0.05 + unit(digest[7]) * 0.20;
    let phase = frame_index as f32 * speed;

    let cx = width as f32 / 2.0;
    let cy = height as f32 / 2.0;
    let max_r = (cx * cx + cy * cy).sqrt().max(1.0);
    let mut out = vec![0u8; width as usize * height as usize * 3];
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let r = (dx * dx + dy * dy).sqrt() / max_r;
            let t = (r * ring_freq - phase).sin() * 0.5 + 0.5;
            let idx = (y as usize * width as usize + x as usize) * 3;
            for c in 0..3 {
                let v = color_a[c] + (color_b[c] - color_a[c]) * t;
                out[idx + c] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
    }
    out
}

/// `out = init*(1-strength) + pattern*strength`, elementwise over RGB8.
fn blend_rgb8(init: &[u8], pattern: &[u8], strength: f32) -> Vec<u8> {
    init.iter()
        .zip(pattern.iter())
        .map(|(&i, &p)| {
            let v = i as f32 * (1.0 - strength) + p as f32 * strength;
            v.round().clamp(0.0, 255.0) as u8
        })
        .collect()
}

/// Nearest-neighbor resize (RGB8) — a live session's init image (client
/// input frame, or a feedback-warped previous output) is not guaranteed to
/// already match the current `config.width`/`height` (a control update can
/// change them mid-session).
fn resize_nearest_rgb8(image: &RgbImage, width: u32, height: u32) -> Vec<u8> {
    let src_w = image.width.max(1);
    let src_h = image.height.max(1);
    let mut out = vec![0u8; width as usize * height as usize * 3];
    for y in 0..height {
        let sy = (y as u64 * src_h as u64 / height.max(1) as u64).min(src_h as u64 - 1) as u32;
        for x in 0..width {
            let sx = (x as u64 * src_w as u64 / width.max(1) as u64).min(src_w as u64 - 1) as u32;
            let src_idx = (sy as usize * image.width as usize + sx as usize) * 3;
            let dst_idx = (y as usize * width as usize + x as usize) * 3;
            if src_idx + 3 <= image.data.len() {
                out[dst_idx..dst_idx + 3].copy_from_slice(&image.data[src_idx..src_idx + 3]);
            }
        }
    }
    out
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

/// RGB8 (3 bytes/pixel, no alpha) PNG encoder — the realtime session's
/// `output_encoding = "png"` path (`realtime::encode_output_frame`) and any
/// other RGB-only producer.
pub fn encode_png_rgb8(pixels: &[u8], width: usize, height: usize) -> Result<Vec<u8>, AssetAiError> {
    if pixels.len() != width * height * 3 {
        return Err(AssetAiError::Backend(format!(
            "rgb8 png encode expected {} bytes, got {}",
            width * height * 3,
            pixels.len()
        )));
    }
    let options = EncoderOptions::default()
        .set_width(width)
        .set_height(height)
        .set_depth(BitDepth::Eight)
        .set_colorspace(ColorSpace::RGB);
    let mut encoder = PngEncoder::new(pixels, options);
    let mut out = Vec::new();
    encoder
        .encode(&mut out)
        .map_err(|err| AssetAiError::Backend(format!("rgb8 png encode failed: {err:?}")))?;
    Ok(out)
}

/// Decodes a PNG into tightly packed RGB8 (any source channel count is
/// reduced to RGB — alpha dropped, grayscale replicated across channels).
/// Used for realtime PNG input/output frames and `{"type":"reference"}`
/// images — any RGB-only PNG consumer.
pub fn decode_png_rgb8(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), AssetAiError> {
    let cursor = std::io::Cursor::new(bytes.to_vec());
    let reader = BufReader::new(cursor);
    let options = DecoderOptions::default().png_set_strip_to_8bit(true);
    let mut decoder = PngDecoder::new_with_options(reader, options);
    decoder
        .decode_headers()
        .map_err(|err| AssetAiError::Params(format!("png decode: {err:?}")))?;
    let info = decoder
        .info()
        .cloned()
        .ok_or_else(|| AssetAiError::Params("png decode: no info".into()))?;
    let colorspace = decoder
        .colorspace()
        .ok_or_else(|| AssetAiError::Params("png decode: no colorspace".into()))?;
    let pixels = decoder
        .decode_raw()
        .map_err(|err| AssetAiError::Params(format!("png decode: {err:?}")))?;
    let components = colorspace.num_components();
    if components == 0 {
        return Err(AssetAiError::Params("png decode: zero color channels".into()));
    }
    let (width, height) = (info.width as u32, info.height as u32);
    let mut rgb = vec![0u8; width as usize * height as usize * 3];
    if components >= 3 {
        for (i, chunk) in pixels.chunks_exact(components).enumerate() {
            rgb[i * 3..i * 3 + 3].copy_from_slice(&chunk[..3]);
        }
    } else {
        // Grayscale (1 component) or grayscale+alpha (2): replicate luma.
        for (i, chunk) in pixels.chunks_exact(components).enumerate() {
            let luma = chunk[0];
            rgb[i * 3] = luma;
            rgb[i * 3 + 1] = luma;
            rgb[i * 3 + 2] = luma;
        }
    }
    Ok((rgb, width, height))
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

    #[test]
    fn rgb8_png_round_trips() {
        let mut pixels = vec![0u8; 6 * 4 * 3];
        for (i, px) in pixels.chunks_exact_mut(3).enumerate() {
            px[0] = (i * 7) as u8;
            px[1] = (i * 3) as u8;
            px[2] = (i * 11) as u8;
        }
        let png = encode_png_rgb8(&pixels, 6, 4).unwrap();
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']);
        let (decoded, width, height) = decode_png_rgb8(&png).unwrap();
        assert_eq!((width, height), (6, 4));
        assert_eq!(decoded, pixels);
    }

    #[test]
    fn decode_png_rgb8_handles_rgba_source() {
        let rgba = render_pattern("round trip", 3, 5, 5);
        let png = encode_png_rgba(&rgba, 5, 5).unwrap();
        let (rgb, width, height) = decode_png_rgb8(&png).unwrap();
        assert_eq!((width, height), (5, 5));
        for i in 0..25 {
            assert_eq!(rgb[i * 3], rgba[i * 4]);
            assert_eq!(rgb[i * 3 + 1], rgba[i * 4 + 1]);
            assert_eq!(rgb[i * 3 + 2], rgba[i * 4 + 2]);
        }
    }

    #[test]
    fn live_step_pass_through_at_zero_strength() {
        let mut backend = TestPatternBackend::new("testpattern");
        assert!(backend.live_supported());
        let mut config = crate::backend::LiveConfig::default();
        config.width = 4;
        config.height = 4;
        config.strength = 0.0;
        let init = RgbImage {
            width: 4,
            height: 4,
            data: vec![42u8; 4 * 4 * 3],
        };
        let cancel = CancelToken::new();
        let out = backend
            .live_step(
                LiveFrameIn { init: Some(&init), anchor: None, frame_index: 0, config: &config },
                &cancel,
            )
            .unwrap();
        assert_eq!(out.image.data, init.data, "strength 0.0 is pass-through");
    }

    #[test]
    fn live_step_pure_pattern_at_full_strength_and_no_init() {
        let mut backend = TestPatternBackend::new("testpattern");
        let mut config = crate::backend::LiveConfig::default();
        config.width = 4;
        config.height = 4;
        config.strength = 1.0;
        config.prompt = "anything".to_string();
        let cancel = CancelToken::new();
        // No init image at all (feed mode before any frame arrived): the
        // step still produces a frame of the requested size.
        let out = backend
            .live_step(LiveFrameIn { init: None, anchor: None, frame_index: 0, config: &config }, &cancel)
            .unwrap();
        assert_eq!(out.image.width, 4);
        assert_eq!(out.image.height, 4);
        assert_eq!(out.image.data.len(), 4 * 4 * 3);
    }

    #[test]
    fn live_step_animates_across_frame_index() {
        let mut backend = TestPatternBackend::new("testpattern");
        let mut config = crate::backend::LiveConfig::default();
        config.width = 16;
        config.height = 16;
        config.strength = 1.0;
        let cancel = CancelToken::new();
        let frame0 = backend
            .live_step(LiveFrameIn { init: None, anchor: None, frame_index: 0, config: &config }, &cancel)
            .unwrap();
        let frame5 = backend
            .live_step(LiveFrameIn { init: None, anchor: None, frame_index: 5, config: &config }, &cancel)
            .unwrap();
        assert_ne!(frame0.image.data, frame5.image.data, "pattern must move with frame_index");
    }
}
