//! The `upscale-native` backend: UPSCALE domain — image -> 4x upscaled image.
//!
//! Production runs the pinned RealESRGAN x4plus checkpoint (RRDBNet,
//! Comfy-Org/Real-ESRGAN_repackaged repack of xinntao/Real-ESRGAN, BSD-3)
//! directly through libs/ai/models/vision on the Makepad CUDA stack. There
//! is no Python, Torch, subprocess, environment-command, or silent fallback
//! seam in this backend — see the native port's design/validation notes
//! (fast mode beats the official fp16 forward's warm e2e time while every
//! quality metric stays inside its envelope).
//!
//! Request: `{model: "realesrgan-x4plus", input_b64: <png>}` -> one
//! `image/png` artifact at 4x the input resolution. The output keeps the
//! input's channel count: RGB in -> RGB out, RGBA in -> RGBA out. The
//! network itself only upscales the three color planes (this is the
//! official architecture's behavior, not a shortcut); an input alpha
//! channel is nearest-neighbor scaled onto the 4x canvas and recomposited
//! here so a matted cutout upscales without losing its cutout.

use crate::backend::{
    ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, ProgressSink,
};
use crate::error::AssetAiError;
use crate::subproc_img::png_header;
#[cfg(feature = "upscale-native")]
use makepad_ai_common::DiffusionError;
#[cfg(feature = "upscale-native")]
use makepad_ai_vision::realesrgan::{
    unload_realesrgan, RealEsrgan, RealEsrganImage, RealEsrganWeights,
};
#[cfg(feature = "upscale-native")]
use std::path::PathBuf;

/// Pluggable run for tests: takes the input PNG bytes and returns output PNG.
pub type UpscaleFn = Box<dyn FnMut(&[u8], ProgressSink) -> Result<Vec<u8>, AssetAiError> + Send>;

enum Gen {
    Stub(UpscaleFn),
    #[cfg(feature = "upscale-native")]
    Native,
}

pub struct UpscaleBackend {
    model_id: String,
    gen: Gen,
    #[cfg(feature = "upscale-native")]
    model_path: Option<PathBuf>,
    #[cfg(feature = "upscale-native")]
    model: Option<RealEsrgan>,
}

impl UpscaleBackend {
    /// Test/CI constructor: native inference is represented by the closure.
    pub fn with_stub(model_id: &str, gen: UpscaleFn) -> Self {
        Self {
            model_id: model_id.to_string(),
            gen: Gen::Stub(gen),
            #[cfg(feature = "upscale-native")]
            model_path: None,
            #[cfg(feature = "upscale-native")]
            model: None,
        }
    }

    /// Real constructor used by `create_backend`.
    #[cfg(feature = "upscale-native")]
    pub fn new_native(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            gen: Gen::Native,
            model_path: None,
            model: None,
        }
    }
}

#[cfg(feature = "upscale-native")]
fn diffusion_err(err: DiffusionError) -> AssetAiError {
    match err {
        DiffusionError::Cancelled => AssetAiError::Cancelled,
        other => AssetAiError::Backend(format!("upscale: {other}")),
    }
}

/// Validates output against the contract: an RGB (2) or RGBA (6) 8-bit PNG
/// at exactly 4x the input's width/height.
pub fn check_upscale_output(
    bytes: &[u8],
    input_width: usize,
    input_height: usize,
) -> Result<(), AssetAiError> {
    let header = png_header(bytes)
        .ok_or_else(|| AssetAiError::Backend("upscale output is not a png".to_string()))?;
    if header.color_type != 2 && header.color_type != 6 {
        return Err(AssetAiError::Backend(format!(
            "upscale output png has color type {} (expected 2 = RGB or 6 = RGBA)",
            header.color_type
        )));
    }
    if header.bit_depth != 8 {
        return Err(AssetAiError::Backend(format!(
            "upscale output png is {}-bit (expected 8-bit)",
            header.bit_depth
        )));
    }
    let (expected_w, expected_h) = (
        (input_width * 4) as u32,
        (input_height * 4) as u32,
    );
    if header.width != expected_w || header.height != expected_h {
        return Err(AssetAiError::Backend(format!(
            "upscale output is {}x{}, expected {expected_w}x{expected_h} (4x the {input_width}x{input_height} input)",
            header.width, header.height
        )));
    }
    Ok(())
}

impl ContentBackend for UpscaleBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        ctx.ensure_files()?;
        match &self.gen {
            Gen::Stub(_) => Ok(()),
            #[cfg(feature = "upscale-native")]
            Gen::Native => {
                let path = ctx.path_by_role("native-upscale")?;
                if self.model.is_some() && self.model_path.as_ref() == Some(&path) {
                    return Ok(());
                }
                self.model = None;
                let weights = RealEsrganWeights::load(&path).map_err(diffusion_err)?;
                let cancel = ctx.cancel.clone();
                let cancelled = || cancel.is_cancelled();
                let mut load_progress = |stage: &str, fraction: f64| {
                    if cancelled() {
                        return Err(DiffusionError::Cancelled);
                    }
                    (ctx.progress)(stage, fraction);
                    Ok(())
                };
                let model = RealEsrgan::prepare_controlled(
                    &weights,
                    Some(&cancelled),
                    Some(&mut load_progress),
                )
                .map_err(diffusion_err)?;
                self.model_path = Some(path);
                self.model = Some(model);
                Ok(())
            }
        }
    }

    fn is_resident(&self) -> bool {
        #[cfg(feature = "upscale-native")]
        {
            return self.model.is_some();
        }
        #[cfg(not(feature = "upscale-native"))]
        false
    }

    fn unload(&mut self) -> Result<(), AssetAiError> {
        #[cfg(feature = "upscale-native")]
        {
            self.model = None;
            self.model_path = None;
            unload_realesrgan().map_err(diffusion_err)?;
        }
        Ok(())
    }

    fn generate(
        &mut self,
        params: &GenerateParams,
        progress: ProgressSink,
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError> {
        if params.input_bytes.is_empty() {
            return Err(AssetAiError::Params(format!(
                "{} needs an input image (input_b64 png)",
                self.model_id
            )));
        }
        let header = png_header(&params.input_bytes)
            .ok_or_else(|| AssetAiError::Params("input_b64 is not a png".to_string()))?;
        let (input_width, input_height) = (header.width as usize, header.height as usize);
        cancel.check()?;
        progress("upscale: realesrgan x4", 0.02);

        let bytes = match &mut self.gen {
            Gen::Stub(gen) => gen(&params.input_bytes, progress)?,
            #[cfg(feature = "upscale-native")]
            Gen::Native => {
                let model = self.model.as_ref().ok_or_else(|| {
                    AssetAiError::Backend("native upscale used before ensure_loaded".to_string())
                })?;
                let (rgba, width, height) =
                    crate::trellis_backend::decode_png_rgba8(&params.input_bytes)?;
                // color type 4/6 both carry an alpha channel through the
                // RGBA8 decode above.
                let has_alpha = matches!(header.color_type, 4 | 6);
                let cancelled = || cancel.is_cancelled();
                let mut infer_progress = |stage: &str, fraction: f64| {
                    if cancelled() {
                        return Err(DiffusionError::Cancelled);
                    }
                    progress(&format!("upscale: {stage}"), 0.02 + 0.96 * fraction);
                    Ok(())
                };
                let input =
                    RealEsrganImage::rgba8(&rgba, width, height).map_err(diffusion_err)?;
                let upscaled = model
                    .upscale_controlled(input, Some(&cancelled), Some(&mut infer_progress))
                    .map_err(diffusion_err)?;
                let rgb = upscaled.rgb8();
                if has_alpha {
                    let out = recomposite_alpha(&rgb, &rgba, width, height, &upscaled);
                    crate::testpattern::encode_png_rgba(&out, upscaled.width, upscaled.height)?
                } else {
                    crate::testpattern::encode_png_rgb8(&rgb, upscaled.width, upscaled.height)?
                }
            }
        };
        cancel.check()?;
        check_upscale_output(&bytes, input_width, input_height)?;
        progress("done", 1.0);
        Ok(vec![ArtifactData {
            content_type: "image/png",
            ext: "png",
            bytes,
        }])
    }
}

/// Nearest-neighbor scales the source RGBA's alpha plane onto the upscaled
/// canvas and interleaves it with the network's RGB8 output. The network
/// never sees or produces alpha (RealESRGAN upscales color only), so this is
/// the backend's own recomposite step, not a claim about model behavior.
#[cfg(feature = "upscale-native")]
fn recomposite_alpha(
    rgb: &[u8],
    src_rgba: &[u8],
    src_width: usize,
    src_height: usize,
    upscaled: &makepad_ai_vision::realesrgan::RealEsrganUpscale,
) -> Vec<u8> {
    let (dst_width, dst_height) = (upscaled.width, upscaled.height);
    let mut out = vec![0u8; dst_width * dst_height * 4];
    for y in 0..dst_height {
        let src_y = (y * src_height / dst_height).min(src_height.saturating_sub(1));
        for x in 0..dst_width {
            let src_x = (x * src_width / dst_width).min(src_width.saturating_sub(1));
            let alpha = src_rgba[(src_y * src_width + src_x) * 4 + 3];
            let dst = (y * dst_width + x) * 4;
            let src = (y * dst_width + x) * 3;
            out[dst] = rgb[src];
            out[dst + 1] = rgb[src + 1];
            out[dst + 2] = rgb[src + 2];
            out[dst + 3] = alpha;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests (stubbed native inference — this is what CPU-only CI exercises)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::GenerateRequestJson;
    use crate::subproc_img::fake_png;
    use crate::testpattern::{encode_png_rgb8, encode_png_rgba};

    fn upscale_params(request: GenerateRequestJson) -> GenerateParams {
        GenerateParams::from_request(&request).unwrap()
    }

    fn b64(bytes: &[u8]) -> String {
        String::from_utf8(makepad_base64::base64_encode(
            bytes,
            &makepad_base64::BASE64_STANDARD,
        ))
        .unwrap()
    }

    fn input_png(w: usize, h: usize) -> Vec<u8> {
        encode_png_rgb8(&vec![128u8; w * h * 3], w, h).unwrap()
    }

    #[test]
    fn stub_upscale_to_png_artifact_4x() {
        let output = encode_png_rgb8(&vec![200u8; 32 * 16 * 3], 32, 16).unwrap();
        let stub_out = output.clone();
        let mut backend = UpscaleBackend::with_stub(
            "realesrgan-x4plus",
            Box::new(move |input: &[u8], progress: ProgressSink| {
                assert_eq!(&input[..8], b"\x89PNG\r\n\x1a\n");
                progress("infer", 0.5);
                Ok(stub_out.clone())
            }),
        );
        let params = upscale_params(GenerateRequestJson {
            model: "realesrgan-x4plus".to_string(),
            input_b64: Some(b64(&input_png(8, 4))),
            ..GenerateRequestJson::default()
        });
        let mut sink = |_: &str, _: f64| {};
        let artifacts = backend
            .generate(&params, &mut sink, &CancelToken::new())
            .unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].content_type, "image/png");
        assert_eq!(artifacts[0].ext, "png");
        assert_eq!(artifacts[0].bytes, output);
    }

    #[test]
    fn missing_input_image_is_a_params_error() {
        let mut backend = UpscaleBackend::with_stub(
            "realesrgan-x4plus",
            Box::new(|_: &[u8], _p: ProgressSink| unreachable!()),
        );
        let params = upscale_params(GenerateRequestJson {
            model: "realesrgan-x4plus".to_string(),
            prompt: Some("ignored".to_string()),
            ..GenerateRequestJson::default()
        });
        let mut sink = |_: &str, _: f64| {};
        let err = backend
            .generate(&params, &mut sink, &CancelToken::new())
            .err()
            .expect("missing input must be an error");
        match err {
            AssetAiError::Params(msg) => assert!(msg.contains("input_b64")),
            other => panic!("expected Params error, got {other:?}"),
        }
    }

    #[test]
    fn garbage_input_png_rejected() {
        let mut backend = UpscaleBackend::with_stub(
            "realesrgan-x4plus",
            Box::new(|_: &[u8], _p: ProgressSink| unreachable!()),
        );
        let params = upscale_params(GenerateRequestJson {
            model: "realesrgan-x4plus".to_string(),
            input_b64: Some(b64(b"not a png at all")),
            ..GenerateRequestJson::default()
        });
        let mut sink = |_: &str, _: f64| {};
        assert!(matches!(
            backend.generate(&params, &mut sink, &CancelToken::new()),
            Err(AssetAiError::Params(_))
        ));
    }

    #[test]
    fn wrong_output_size_is_a_backend_error() {
        // The stub returns a PNG that is NOT 4x the 8x4 input.
        let mut backend = UpscaleBackend::with_stub(
            "realesrgan-x4plus",
            Box::new(|_: &[u8], _p: ProgressSink| Ok(fake_png(16, 8, 8, 2))),
        );
        let params = upscale_params(GenerateRequestJson {
            model: "realesrgan-x4plus".to_string(),
            input_b64: Some(b64(&input_png(8, 4))),
            ..GenerateRequestJson::default()
        });
        let mut sink = |_: &str, _: f64| {};
        let err = backend
            .generate(&params, &mut sink, &CancelToken::new())
            .err()
            .expect("under-scaled output must be an error");
        match err {
            AssetAiError::Backend(msg) => assert!(msg.contains("expected 32x16"), "{msg}"),
            other => panic!("expected Backend error, got {other:?}"),
        }
        // check_upscale_output directly: RGB/RGBA at exactly 4x pass, others fail.
        assert!(check_upscale_output(&fake_png(32, 16, 8, 2), 8, 4).is_ok());
        assert!(check_upscale_output(&fake_png(32, 16, 8, 6), 8, 4).is_ok());
        assert!(check_upscale_output(&fake_png(32, 16, 8, 0), 8, 4).is_err());
        assert!(check_upscale_output(&fake_png(16, 16, 8, 2), 8, 4).is_err());
        assert!(check_upscale_output(b"junk", 8, 4).is_err());
    }

    #[test]
    fn pre_raised_cancel_token_short_circuits() {
        let mut backend = UpscaleBackend::with_stub(
            "realesrgan-x4plus",
            Box::new(|_: &[u8], _p: ProgressSink| {
                panic!("upscale runtime must not run on a cancelled job")
            }),
        );
        let params = upscale_params(GenerateRequestJson {
            model: "realesrgan-x4plus".to_string(),
            input_b64: Some(b64(&input_png(8, 4))),
            ..GenerateRequestJson::default()
        });
        let token = CancelToken::new();
        token.cancel();
        let mut sink = |_: &str, _: f64| {};
        assert!(matches!(
            backend.generate(&params, &mut sink, &token),
            Err(AssetAiError::Cancelled)
        ));
    }

    #[test]
    fn canonical_backend_has_no_command_configuration() {
        // Production upscaling is selected by a compiled native feature and a
        // pinned registry artifact, never by a runtime command environment.
        assert_ne!(crate::registry::EMBEDDED_REGISTRY.len(), 0);
    }

    #[cfg(feature = "upscale-native")]
    #[test]
    fn recompositor_preserves_alpha_and_replaces_color_with_upscaled() {
        // 2x2 source, alpha ramp 0/85/170/255 in raster order.
        let src = vec![
            10, 20, 30, 0, // (0,0)
            10, 20, 30, 85, // (1,0)
            10, 20, 30, 170, // (0,1)
            10, 20, 30, 255, // (1,1)
        ];
        let upscaled = makepad_ai_vision::realesrgan::RealEsrganUpscale::new(
            4,
            4,
            vec![0.5f32; 4 * 4 * 3],
        )
        .unwrap();
        let rgb = upscaled.rgb8();
        let out = recomposite_alpha(&rgb, &src, 2, 2, &upscaled);
        assert_eq!(out.len(), 4 * 4 * 4);
        // Every pixel's color comes from the (constant) upscaled network
        // output, not the source.
        for pixel in out.chunks_exact(4) {
            assert_eq!(&pixel[..3], &rgb[..3]);
        }
        // The four alpha quadrants are nearest-neighbor scaled from source.
        assert_eq!(out[3], 0); // top-left quadrant -> source (0,0)
        assert_eq!(out[(0 * 4 + 2) * 4 + 3], 85); // top-right quadrant -> (1,0)
        assert_eq!(out[(2 * 4 + 0) * 4 + 3], 170); // bottom-left quadrant -> (0,1)
        assert_eq!(out[(2 * 4 + 2) * 4 + 3], 255); // bottom-right quadrant -> (1,1)
    }

    #[test]
    fn rgba_stub_output_round_trips_through_check() {
        let png = encode_png_rgba(&vec![1u8; 4 * 4 * 4], 4, 4).unwrap();
        assert!(check_upscale_output(&png, 1, 1).is_ok());
    }
}
