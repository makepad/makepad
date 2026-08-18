//! The `segment` / `segment-native` backend: SEGMENT domain —
//! image + multiplex prompt -> mask PNG + optional RGBA cutout.
//!
//! Production (`segment-native`) runs the pinned Comfy-Org SAM 3.1 multiplex
//! checkpoint through libs/diffusion on the Makepad CUDA stack. There is no
//! Python, Torch, subprocess, or silent fallback. Facebook TOS weights are
//! never fetched.
//!
//! Request: `{model: "sam3-1-multiplex", input_b64: <png>, prompt: "cat:1"}`
//! -> TWO artifacts:
//! 1. `image/png` grayscale soft mask at input resolution
//! 2. `image/png` RGBA cutout (input RGB + predicted alpha)

use crate::backend::{
    ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, ProgressSink,
};
use crate::error::AssetAiError;
use crate::subproc_img::png_header;
use makepad_zune_core::bit_depth::BitDepth;
use makepad_zune_core::colorspace::ColorSpace;
use makepad_zune_core::options::EncoderOptions;
use makepad_zune_png::PngEncoder;
#[cfg(feature = "segment-native")]
use makepad_ai_vision::sam3::{unload_sam3, Sam3, Sam3Image, Sam3Weights};
#[cfg(feature = "segment-native")]
use makepad_ai_common::DiffusionError;
#[cfg(feature = "segment-native")]
use std::path::PathBuf;

/// Pluggable run for tests: input PNG + prompt -> (mask png, rgba png).
pub type SegmentFn =
    Box<dyn FnMut(&[u8], &str, ProgressSink) -> Result<(Vec<u8>, Vec<u8>), AssetAiError> + Send>;

enum Gen {
    Stub(SegmentFn),
    #[cfg(feature = "segment-native")]
    Native,
}

pub struct SegmentBackend {
    model_id: String,
    gen: Gen,
    #[cfg(feature = "segment-native")]
    model_path: Option<PathBuf>,
    #[cfg(feature = "segment-native")]
    model: Option<Sam3>,
}

impl SegmentBackend {
    pub fn with_stub(model_id: &str, gen: SegmentFn) -> Self {
        Self {
            model_id: model_id.to_string(),
            gen: Gen::Stub(gen),
            #[cfg(feature = "segment-native")]
            model_path: None,
            #[cfg(feature = "segment-native")]
            model: None,
        }
    }

    #[cfg(feature = "segment-native")]
    pub fn new_native(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            gen: Gen::Native,
            model_path: None,
            model: None,
        }
    }
}

#[cfg(feature = "segment-native")]
fn diffusion_err(err: DiffusionError) -> AssetAiError {
    match err {
        DiffusionError::Cancelled => AssetAiError::Cancelled,
        other => AssetAiError::Backend(format!("segment: {other}")),
    }
}

pub fn encode_png_gray8(pixels: &[u8], width: usize, height: usize) -> Result<Vec<u8>, AssetAiError> {
    if pixels.len() != width * height {
        return Err(AssetAiError::Backend(format!(
            "gray8 png encode expected {} samples, got {}",
            width * height,
            pixels.len()
        )));
    }
    let options = EncoderOptions::default()
        .set_width(width)
        .set_height(height)
        .set_depth(BitDepth::Eight)
        .set_colorspace(ColorSpace::Luma);
    let mut encoder = PngEncoder::new(pixels, options);
    let mut out = Vec::new();
    encoder
        .encode(&mut out)
        .map_err(|err| AssetAiError::Backend(format!("gray8 png encode failed: {err:?}")))?;
    Ok(out)
}

pub fn check_mask_output(bytes: &[u8]) -> Result<(), AssetAiError> {
    let header = png_header(bytes).ok_or_else(|| {
        AssetAiError::Backend("segment mask output is not a png".to_string())
    })?;
    if header.color_type != 0 && header.color_type != 6 {
        return Err(AssetAiError::Backend(format!(
            "segment mask png has color type {} (expected 0 = gray or 6 = RGBA)",
            header.color_type
        )));
    }
    Ok(())
}

impl ContentBackend for SegmentBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        ctx.ensure_files()?;
        match &self.gen {
            Gen::Stub(_) => Ok(()),
            #[cfg(feature = "segment-native")]
            Gen::Native => {
                let path = ctx.path_by_role("native-segment")?;
                if self.model.is_some() && self.model_path.as_ref() == Some(&path) {
                    return Ok(());
                }
                if self.model.take().is_some() {
                    unload_sam3().map_err(diffusion_err)?;
                }
                let weights = Sam3Weights::load(&path).map_err(diffusion_err)?;
                let cancel = ctx.cancel.clone();
                let cancelled = || cancel.is_cancelled();
                let mut load_progress = |stage: &str, fraction: f64| {
                    if cancelled() {
                        return Err(DiffusionError::Cancelled);
                    }
                    (ctx.progress)(stage, fraction);
                    Ok(())
                };
                let model = Sam3::prepare_controlled(
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
        #[cfg(feature = "segment-native")]
        {
            return self.model.is_some();
        }
        #[cfg(not(feature = "segment-native"))]
        false
    }

    fn unload(&mut self) -> Result<(), AssetAiError> {
        #[cfg(feature = "segment-native")]
        {
            self.model = None;
            self.model_path = None;
            unload_sam3().map_err(diffusion_err)?;
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
        if png_header(&params.input_bytes).is_none() {
            return Err(AssetAiError::Params(
                "input_b64 is not a png".to_string(),
            ));
        }
        if params.prompt.trim().is_empty() {
            return Err(AssetAiError::Params(format!(
                "{} needs a multiplex prompt (e.g. \"cat:1\")",
                self.model_id
            )));
        }
        cancel.check()?;
        progress("segment: sam3", 0.02);

        let (mask_png, rgba_png) = match &mut self.gen {
            Gen::Stub(gen) => gen(&params.input_bytes, &params.prompt, progress)?,
            #[cfg(feature = "segment-native")]
            Gen::Native => {
                let model = self.model.as_ref().ok_or_else(|| {
                    AssetAiError::Backend("native segment used before ensure_loaded".to_string())
                })?;
                let (mut rgba, width, height) =
                    crate::trellis_backend::decode_png_rgba8(&params.input_bytes)?;
                let cancelled = || cancel.is_cancelled();
                let mut infer_progress = |stage: &str, fraction: f64| {
                    if cancelled() {
                        return Err(DiffusionError::Cancelled);
                    }
                    progress(&format!("segment: {stage}"), 0.02 + 0.96 * fraction);
                    Ok(())
                };
                let input = Sam3Image::rgba8(&rgba, width, height).map_err(diffusion_err)?;
                let mask = model
                    .segment_controlled(
                        input,
                        &params.prompt,
                        Some(&cancelled),
                        Some(&mut infer_progress),
                    )
                    .map_err(diffusion_err)?;
                let alpha = mask.alpha_u8();
                for (pixel, a) in rgba.chunks_exact_mut(4).zip(alpha.iter().copied()) {
                    pixel[3] = a;
                }
                let mask_png = encode_png_gray8(&alpha, width, height)?;
                let rgba_png = crate::testpattern::encode_png_rgba(&rgba, width, height)?;
                (mask_png, rgba_png)
            }
        };
        cancel.check()?;
        check_mask_output(&mask_png)?;
        progress("done", 1.0);
        Ok(vec![
            ArtifactData {
                content_type: "image/png",
                ext: "png",
                bytes: mask_png,
            },
            ArtifactData {
                content_type: "image/png",
                ext: "png",
                bytes: rgba_png,
            },
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::GenerateRequestJson;
    use crate::subproc_img::fake_png;
    use crate::testpattern::encode_png_rgba;

    fn segment_params(request: GenerateRequestJson) -> GenerateParams {
        GenerateParams::from_request(&request).unwrap()
    }

    fn b64(bytes: &[u8]) -> String {
        String::from_utf8(makepad_base64::base64_encode(
            bytes,
            &makepad_base64::BASE64_STANDARD,
        ))
        .unwrap()
    }

    fn input_png() -> Vec<u8> {
        encode_png_rgba(&vec![128u8; 8 * 4 * 4], 8, 4).unwrap()
    }

    #[test]
    fn stub_segment_emits_mask_and_rgba() {
        let mask = fake_png(8, 4, 8, 0);
        let rgba = fake_png(8, 4, 8, 6);
        let stub_mask = mask.clone();
        let stub_rgba = rgba.clone();
        let mut backend = SegmentBackend::with_stub(
            "sam3-1-multiplex",
            Box::new(move |input: &[u8], prompt: &str, progress: ProgressSink| {
                assert_eq!(&input[..8], b"\x89PNG\r\n\x1a\n");
                assert_eq!(prompt, "cat:1");
                progress("infer", 0.5);
                Ok((stub_mask.clone(), stub_rgba.clone()))
            }),
        );
        let params = segment_params(GenerateRequestJson {
            model: "sam3-1-multiplex".to_string(),
            prompt: Some("cat:1".to_string()),
            input_b64: Some(b64(&input_png())),
            ..GenerateRequestJson::default()
        });
        let mut sink = |_: &str, _: f64| {};
        let artifacts = backend
            .generate(&params, &mut sink, &CancelToken::new())
            .unwrap();
        assert_eq!(artifacts.len(), 2);
        assert_eq!(artifacts[0].bytes, mask);
        assert_eq!(artifacts[1].bytes, rgba);
    }

    #[test]
    fn missing_prompt_is_a_params_error() {
        let mut backend = SegmentBackend::with_stub(
            "sam3-1-multiplex",
            Box::new(|_: &[u8], _: &str, _: ProgressSink| unreachable!()),
        );
        let params = segment_params(GenerateRequestJson {
            model: "sam3-1-multiplex".to_string(),
            input_b64: Some(b64(&input_png())),
            ..GenerateRequestJson::default()
        });
        let mut sink = |_: &str, _: f64| {};
        let err = backend
            .generate(&params, &mut sink, &CancelToken::new())
            .err()
            .expect("missing prompt must be an error");
        match err {
            AssetAiError::Params(msg) => assert!(msg.contains("prompt"), "{msg}"),
            other => panic!("expected Params error, got {other:?}"),
        }
    }

    #[test]
    fn missing_input_is_a_params_error() {
        let mut backend = SegmentBackend::with_stub(
            "sam3-1-multiplex",
            Box::new(|_: &[u8], _: &str, _: ProgressSink| unreachable!()),
        );
        let params = segment_params(GenerateRequestJson {
            model: "sam3-1-multiplex".to_string(),
            prompt: Some("cat:1".to_string()),
            ..GenerateRequestJson::default()
        });
        let mut sink = |_: &str, _: f64| {};
        assert!(matches!(
            backend.generate(&params, &mut sink, &CancelToken::new()),
            Err(AssetAiError::Params(_))
        ));
    }

    #[test]
    fn pre_raised_cancel_token_short_circuits() {
        let mut backend = SegmentBackend::with_stub(
            "sam3-1-multiplex",
            Box::new(|_: &[u8], _: &str, _: ProgressSink| {
                panic!("segment runtime must not run on a cancelled job")
            }),
        );
        let params = segment_params(GenerateRequestJson {
            model: "sam3-1-multiplex".to_string(),
            prompt: Some("cat:1".to_string()),
            input_b64: Some(b64(&input_png())),
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
    fn gray8_encoder_roundtrip_header() {
        let png = encode_png_gray8(&[0, 128, 255, 64], 2, 2).unwrap();
        assert!(check_mask_output(&png).is_ok());
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }
}
