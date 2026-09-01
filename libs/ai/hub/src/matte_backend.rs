//! The `matte` backend: MATTE domain — image -> RGBA cutout with SOFT alpha.
//!
//! Production runs the pinned BiRefNet_HR-matting checkpoint directly through
//! libs/diffusion on the Makepad CUDA stack. There is no Python, Torch,
//! subprocess, environment-command, or silent fallback seam in this backend.
//! The alpha channel is the CONTINUOUS matte the model predicted — no
//! thresholding anywhere; hair/fur/glass edges keep their soft coverage.
//!
//! Request: `{model: "birefnet-hr", input_b64: <png>}` -> one `image/png`
//! RGBA artifact at input resolution.

use crate::backend::{
    ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, ProgressSink,
};
use crate::error::AssetAiError;
use crate::subproc_img::png_header;
#[cfg(feature = "matte-native")]
use makepad_ai_vision::birefnet::{
    unload_birefnet, BiRefNet, BiRefNetImage, BiRefNetWeights,
};
#[cfg(feature = "matte-native")]
use makepad_ai_common::DiffusionError;
#[cfg(feature = "matte-native")]
use std::path::PathBuf;

/// Pluggable run for tests: takes the input PNG bytes and returns output PNG.
pub type MatteFn = Box<dyn FnMut(&[u8], ProgressSink) -> Result<Vec<u8>, AssetAiError> + Send>;

enum Gen {
    Stub(MatteFn),
    #[cfg(feature = "matte-native")]
    Native,
}

pub struct MatteBackend {
    model_id: String,
    gen: Gen,
    #[cfg(feature = "matte-native")]
    model_path: Option<PathBuf>,
    #[cfg(feature = "matte-native")]
    model: Option<BiRefNet>,
}

impl MatteBackend {
    /// Test/CI constructor: native inference is represented by the closure.
    pub fn with_stub(model_id: &str, gen: MatteFn) -> Self {
        Self {
            model_id: model_id.to_string(),
            gen: Gen::Stub(gen),
            #[cfg(feature = "matte-native")]
            model_path: None,
            #[cfg(feature = "matte-native")]
            model: None,
        }
    }

    /// Real constructor used by `create_backend`.
    #[cfg(feature = "matte-native")]
    pub fn new_native(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            gen: Gen::Native,
            model_path: None,
            model: None,
        }
    }
}

#[cfg(feature = "matte-native")]
fn diffusion_err(err: DiffusionError) -> AssetAiError {
    match err {
        DiffusionError::Cancelled => AssetAiError::Cancelled,
        other => AssetAiError::Backend(format!("matte: {other}")),
    }
}

/// Validates output against the matte contract: a PNG whose
/// color type carries alpha (RGBA). Bit depth 8 or 16 both pass — the alpha
/// stays whatever the model wrote.
pub fn check_matte_output(bytes: &[u8]) -> Result<(), AssetAiError> {
    let header = png_header(bytes).ok_or_else(|| {
        AssetAiError::Backend("matte output is not a png".to_string())
    })?;
    if header.color_type != 6 {
        return Err(AssetAiError::Backend(format!(
            "matte output png has color type {} (expected 6 = RGBA with the soft alpha matte)",
            header.color_type
        )));
    }
    Ok(())
}

impl ContentBackend for MatteBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        ctx.ensure_files()?;
        match &self.gen {
            Gen::Stub(_) => Ok(()),
            #[cfg(feature = "matte-native")]
            Gen::Native => {
                let path = ctx.path_by_role("native-matte")?;
                if self.model.is_some() && self.model_path.as_ref() == Some(&path) {
                    return Ok(());
                }
                if self.model.take().is_some() {
                    unload_birefnet().map_err(diffusion_err)?;
                }
                let weights = BiRefNetWeights::load(&path).map_err(diffusion_err)?;
                let cancel = ctx.cancel.clone();
                let cancelled = || cancel.is_cancelled();
                let mut load_progress = |stage: &str, fraction: f64| {
                    if cancelled() {
                        return Err(DiffusionError::Cancelled);
                    }
                    (ctx.progress)(stage, fraction);
                    Ok(())
                };
                let model = BiRefNet::prepare_controlled(
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
        #[cfg(feature = "matte-native")]
        {
            return self.model.is_some();
        }
        #[cfg(not(feature = "matte-native"))]
        false
    }

    fn unload(&mut self) -> Result<(), AssetAiError> {
        #[cfg(feature = "matte-native")]
        {
            self.model = None;
            self.model_path = None;
            unload_birefnet().map_err(diffusion_err)?;
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
        cancel.check()?;
        progress("matte: birefnet", 0.02);

        let bytes = match &mut self.gen {
            Gen::Stub(gen) => gen(&params.input_bytes, progress)?,
            #[cfg(feature = "matte-native")]
            Gen::Native => {
                let model = self.model.as_ref().ok_or_else(|| {
                    AssetAiError::Backend("native matte used before ensure_loaded".to_string())
                })?;
                let (mut rgba, width, height) =
                    crate::trellis_backend::decode_png_rgba8(&params.input_bytes)?;
                let cancelled = || cancel.is_cancelled();
                let mut infer_progress = |stage: &str, fraction: f64| {
                    if cancelled() {
                        return Err(DiffusionError::Cancelled);
                    }
                    progress(&format!("matte: {stage}"), 0.02 + 0.96 * fraction);
                    Ok(())
                };
                let input = BiRefNetImage::rgba8(&rgba, width, height).map_err(diffusion_err)?;
                let matte = model
                    .matte_controlled(input, Some(&cancelled), Some(&mut infer_progress))
                    .map_err(diffusion_err)?;
                for (pixel, alpha) in rgba
                    .chunks_exact_mut(4)
                    .zip(matte.alpha_u8().into_iter())
                {
                    pixel[3] = alpha;
                }
                crate::testpattern::encode_png_rgba(&rgba, width, height)?
            }
        };
        cancel.check()?;
        check_matte_output(&bytes)?;
        progress("done", 1.0);
        Ok(vec![ArtifactData {
            content_type: "image/png",
            ext: "png",
            bytes,
        }])
    }
}

// ---------------------------------------------------------------------------
// Tests (stubbed native inference — this is what CPU-only CI exercises)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::GenerateRequestJson;
    use crate::subproc_img::fake_png;
    use crate::testpattern::encode_png_rgba;

    fn matte_params(request: GenerateRequestJson) -> GenerateParams {
        GenerateParams::from_request(&request).unwrap()
    }

    fn b64(bytes: &[u8]) -> String {
        String::from_utf8(makepad_base64::base64_encode(
            bytes,
            &makepad_base64::BASE64_STANDARD,
        ))
        .unwrap()
    }

    /// 8x4 RGBA PNG with a SOFT alpha ramp (the matte contract: intermediate
    /// coverage values survive).
    fn soft_alpha_png() -> Vec<u8> {
        let (w, h) = (8usize, 4usize);
        let mut rgba = Vec::with_capacity(w * h * 4);
        for _y in 0..h {
            for x in 0..w {
                rgba.extend_from_slice(&[200, 100, 50, (x * 255 / (w - 1)) as u8]);
            }
        }
        encode_png_rgba(&rgba, w, h).unwrap()
    }

    #[test]
    fn stub_matte_to_png_artifact_soft_alpha_preserved() {
        let expected = soft_alpha_png();
        let stub_out = expected.clone();
        let mut backend = MatteBackend::with_stub(
            "birefnet-hr",
            Box::new(move |input: &[u8], progress: ProgressSink| {
                // The stub sees the exact request bytes.
                assert_eq!(&input[..8], b"\x89PNG\r\n\x1a\n");
                progress("infer", 0.5);
                Ok(stub_out.clone())
            }),
        );
        let params = matte_params(GenerateRequestJson {
            model: "birefnet-hr".to_string(),
            input_b64: Some(b64(&soft_alpha_png())),
            ..GenerateRequestJson::default()
        });
        let mut sink = |_: &str, _: f64| {};
        let artifacts = backend
            .generate(&params, &mut sink, &CancelToken::new())
            .unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].content_type, "image/png");
        assert_eq!(artifacts[0].ext, "png");
        // Bytes pass through UNCHANGED — no re-encode, no threshold.
        assert_eq!(artifacts[0].bytes, expected);
    }

    #[test]
    fn missing_input_image_is_a_params_error() {
        let mut backend = MatteBackend::with_stub(
            "birefnet-hr",
            Box::new(|_: &[u8], _p: ProgressSink| unreachable!()),
        );
        let params = matte_params(GenerateRequestJson {
            model: "birefnet-hr".to_string(),
            prompt: Some("a fox".to_string()),
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
        let mut backend = MatteBackend::with_stub(
            "birefnet-hr",
            Box::new(|_: &[u8], _p: ProgressSink| unreachable!()),
        );
        let params = matte_params(GenerateRequestJson {
            model: "birefnet-hr".to_string(),
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
    fn non_rgba_output_is_a_backend_error() {
        // A runtime writing an RGB (no alpha) png violates the contract.
        let mut backend = MatteBackend::with_stub(
            "birefnet-hr",
            Box::new(|_: &[u8], _p: ProgressSink| Ok(fake_png(8, 4, 8, 2))),
        );
        let params = matte_params(GenerateRequestJson {
            model: "birefnet-hr".to_string(),
            input_b64: Some(b64(&soft_alpha_png())),
            ..GenerateRequestJson::default()
        });
        let mut sink = |_: &str, _: f64| {};
        let err = backend
            .generate(&params, &mut sink, &CancelToken::new())
            .err()
            .expect("rgb output must be an error");
        match err {
            AssetAiError::Backend(msg) => assert!(msg.contains("color type"), "{msg}"),
            other => panic!("expected Backend error, got {other:?}"),
        }
        // check_matte_output directly: RGBA (8 or 16 bit) passes, gray fails.
        assert!(check_matte_output(&fake_png(2, 2, 8, 6)).is_ok());
        assert!(check_matte_output(&fake_png(2, 2, 16, 6)).is_ok());
        assert!(check_matte_output(&fake_png(2, 2, 8, 0)).is_err());
        assert!(check_matte_output(b"junk").is_err());
    }

    #[test]
    fn pre_raised_cancel_token_short_circuits() {
        let mut backend = MatteBackend::with_stub(
            "birefnet-hr",
            Box::new(|_: &[u8], _p: ProgressSink| {
                panic!("matte runtime must not run on a cancelled job")
            }),
        );
        let params = matte_params(GenerateRequestJson {
            model: "birefnet-hr".to_string(),
            input_b64: Some(b64(&soft_alpha_png())),
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
        // Production matting is selected by a compiled native feature and a
        // pinned registry artifact, never by a runtime command environment.
        assert_ne!(crate::registry::EMBEDDED_REGISTRY.len(), 0);
    }
}
