//! The `depth` / `depth-native` backend: DEPTH domain — image -> metric depthmap.
//!
//! Production (`depth-native`) runs pinned DA3METRIC-LARGE through
//! libs/diffusion on the Makepad CUDA stack. There is no Python, Torch,
//! subprocess, or silent fallback. Metric scale uses the processed network
//! width (`focal_px * net / 300`); the old box script used the pre-resize
//! width and is retained only as an explicit `python-backends` oracle.
//!
//! Oracle (`depth`, feature `python-backends`): drives the box-provisioned
//! Depth-Anything-3 script (C:\ai\depth_da3.py) through the shared
//! `{in}`/`{out}` subprocess runner. Never selected automatically.
//!
//! Output: TWO artifacts —
//! 1. `image/png`: 16-bit grayscale, depth in MILLIMETERS (u16; values clip
//!    at the 65.535 m far plane, 0 = invalid/unknown).
//! 2. `application/json` sidecar:
//!    `{"metric": true, "unit": "mm", "min": <mm>, "max": <mm>,
//!      "focal_px": <px or null>}` — min/max are the pre-clip range so a
//!    consumer can tell when the far clip engaged.
//!
//! Box provisioning knob (default = the .123 layout — the DA3 stack shares
//! the matte/rembg venv; torch 2.7 satisfies it and xformers is optional):
//!   MAKEPAD_DEPTH_CMD  command template, `{in}`/`{out}` are PNG paths; the
//!                      script must also write the sidecar at `{out}.json`
//!                      (C:\ai\venv\Scripts\python.exe C:\ai\depth_da3.py {in} {out})
//!
//! The standalone metric model predicts NO intrinsics (that's the nested
//! model's any-view branch), so the box script derives focal_px from EXIF
//! when present, else an assumed 60-degree horizontal FOV (script knobs
//! DEPTH_FOCAL_PX / DEPTH_FOV_DEG) — metric depth scales linearly with the
//! true focal; the sidecar reports the value used.
//!
//! Only provisioned boxes advertise the domain (`depth_provisioned()`,
//! backend.rs `backend_provisioned` pattern).

use crate::backend::{
    ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, ProgressSink,
};
use crate::error::AssetAiError;
use crate::subproc_img::png_header;
#[cfg(feature = "python-backends")]
use crate::subproc_img::{cmd_provisioned, run_cancellable, SubprocError};
use makepad_micro_serde::*;
#[cfg(feature = "depth-native")]
use makepad_ai_vision::da3::{
    encode_metric_mm, preprocess_rgb8, Da3MetricLarge, DA3_DEFAULT_HFOV_DEG,
};
#[cfg(feature = "depth-native")]
use makepad_ai_common::DiffusionError;
use std::path::PathBuf;
use std::time::Duration;

pub const DEPTH_CMD_ENV: &str = "MAKEPAD_DEPTH_CMD";
const DEPTH_CMD_DEFAULT: &str = r"C:\ai\venv\Scripts\python.exe C:\ai\depth_da3.py {in} {out}";

/// Per-job budget: warm inference is seconds; cold covers the model load
/// from the box HF cache on a slow disk.
#[cfg(feature = "python-backends")]
const DEPTH_TIMEOUT: Duration = Duration::from_secs(10 * 60);

#[cfg(feature = "python-backends")]
fn depth_cmd() -> String {
    std::env::var(DEPTH_CMD_ENV).unwrap_or_else(|_| DEPTH_CMD_DEFAULT.to_string())
}

/// True only where the DA3 Python oracle stack is actually provisioned.
/// Native depth does not use this probe — its weights are registry-pinned.
#[cfg(feature = "python-backends")]
pub fn depth_provisioned() -> bool {
    cmd_provisioned(&depth_cmd())
}

#[cfg(not(feature = "python-backends"))]
pub fn depth_provisioned() -> bool {
    false
}

/// The metadata sidecar contract (`{out}.json`). Depth values in the PNG are
/// millimeters; `min`/`max` are the PRE-clip metric range in millimeters.
#[derive(Clone, Debug, SerJson, DeJson)]
pub struct DepthMetaJson {
    pub metric: bool,
    pub unit: String,
    pub min: f64,
    pub max: f64,
    /// Focal length in pixels at the depthmap's resolution (avg of fx/fy
    /// from the model's intrinsics prediction); null when unavailable.
    pub focal_px: Option<f64>,
}

/// Pluggable run for tests: takes the input PNG bytes, returns the output
/// PNG bytes plus the sidecar JSON text (what the subprocess writes to
/// `{out}` and `{out}.json`).
pub type DepthFn =
    Box<dyn FnMut(&[u8], ProgressSink) -> Result<(Vec<u8>, String), AssetAiError> + Send>;

enum Gen {
    Stub(DepthFn),
    #[cfg(feature = "depth-native")]
    Native,
    #[cfg(feature = "python-backends")]
    Subprocess,
}

pub struct DepthBackend {
    model_id: String,
    gen: Gen,
    /// Recorded at ensure_loaded — temp files live under `<cache>/tmp`.
    cache_dir: Option<PathBuf>,
    #[cfg(feature = "depth-native")]
    model_path: Option<PathBuf>,
    #[cfg(feature = "depth-native")]
    model: Option<Da3MetricLarge>,
}

impl DepthBackend {
    /// Test/CI constructor: the subprocess is the given closure.
    pub fn with_stub(model_id: &str, gen: DepthFn) -> Self {
        Self {
            model_id: model_id.to_string(),
            gen: Gen::Stub(gen),
            cache_dir: None,
            #[cfg(feature = "depth-native")]
            model_path: None,
            #[cfg(feature = "depth-native")]
            model: None,
        }
    }

    /// Production constructor used by `create_backend` for `depth-native`.
    #[cfg(feature = "depth-native")]
    pub fn new_native(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            gen: Gen::Native,
            cache_dir: None,
            model_path: None,
            model: None,
        }
    }

    /// Explicit Python oracle. Never selected automatically.
    #[cfg(feature = "python-backends")]
    pub fn new_subprocess(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            gen: Gen::Subprocess,
            cache_dir: None,
            #[cfg(feature = "depth-native")]
            model_path: None,
            #[cfg(feature = "depth-native")]
            model: None,
        }
    }
}

#[cfg(feature = "depth-native")]
fn diffusion_err(err: DiffusionError) -> AssetAiError {
    match err {
        DiffusionError::Cancelled => AssetAiError::Cancelled,
        other => AssetAiError::Backend(format!("depth: {other}")),
    }
}

/// Validates the depthmap against the contract: 16-bit grayscale PNG.
pub fn check_depth_output(bytes: &[u8]) -> Result<(), AssetAiError> {
    let header = png_header(bytes).ok_or_else(|| {
        AssetAiError::Backend("depth output is not a png".to_string())
    })?;
    if header.color_type != 0 || header.bit_depth != 16 {
        return Err(AssetAiError::Backend(format!(
            "depth output png is color type {} / {} bit (expected 16-bit grayscale, mm)",
            header.color_type, header.bit_depth
        )));
    }
    Ok(())
}

/// Validates + parses the metadata sidecar.
pub fn check_depth_sidecar(json: &str) -> Result<DepthMetaJson, AssetAiError> {
    let meta = DepthMetaJson::deserialize_json(json).map_err(|e| {
        AssetAiError::Backend(format!("depth sidecar json: {e:?}"))
    })?;
    if !meta.metric || meta.unit != "mm" {
        return Err(AssetAiError::Backend(format!(
            "depth sidecar contract violated: metric={} unit={:?} (expected metric mm)",
            meta.metric, meta.unit
        )));
    }
    Ok(meta)
}

impl ContentBackend for DepthBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        ctx.ensure_files()?;
        self.cache_dir = Some(ctx.cache_dir.to_path_buf());
        match &self.gen {
            Gen::Stub(_) => Ok(()),
            #[cfg(feature = "depth-native")]
            Gen::Native => {
                let path = ctx.path_by_role("native-depth")?;
                if self.model.is_some() && self.model_path.as_ref() == Some(&path) {
                    return Ok(());
                }
                self.model = None;
                let model = Da3MetricLarge::load(&path).map_err(diffusion_err)?;
                self.model_path = Some(path);
                self.model = Some(model);
                Ok(())
            }
            #[cfg(feature = "python-backends")]
            Gen::Subprocess => {
                if !depth_provisioned() {
                    return Err(AssetAiError::Unavailable(format!(
                        "depth command not provisioned on this machine: {:?} (set {})",
                        depth_cmd(),
                        DEPTH_CMD_ENV
                    )));
                }
                Ok(())
            }
        }
    }

    fn is_resident(&self) -> bool {
        #[cfg(feature = "depth-native")]
        {
            return self.model.is_some();
        }
        #[cfg(not(feature = "depth-native"))]
        false
    }

    fn unload(&mut self) -> Result<(), AssetAiError> {
        #[cfg(feature = "depth-native")]
        {
            self.model = None;
            self.model_path = None;
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
        progress("depth: da3", 0.02);

        let (png, sidecar) = match &mut self.gen {
            Gen::Stub(gen) => gen(&params.input_bytes, progress)?,
            #[cfg(feature = "depth-native")]
            Gen::Native => {
                let model = self.model.as_ref().ok_or_else(|| {
                    AssetAiError::Backend("native depth used before ensure_loaded".to_string())
                })?;
                let (rgba, width, height) =
                    crate::trellis_backend::decode_png_rgba8(&params.input_bytes)?;
                let mut rgb = Vec::with_capacity(width * height * 3);
                for pixel in rgba.chunks_exact(4) {
                    rgb.extend_from_slice(&pixel[..3]);
                }
                let cancelled = || cancel.is_cancelled();
                let mut infer_progress = |stage: &str, fraction: f64| {
                    if cancelled() {
                        return Err(DiffusionError::Cancelled);
                    }
                    progress(&format!("depth: {stage}"), 0.02 + 0.96 * fraction);
                    Ok(())
                };
                let preprocessed = preprocess_rgb8(&rgb, width, height).map_err(diffusion_err)?;
                let prediction = model
                    .forward_normalized(
                        &preprocessed.pixels,
                        preprocessed.width,
                        preprocessed.height,
                        Some(&mut infer_progress),
                    )
                    .map_err(diffusion_err)?;
                let map = encode_metric_mm(
                    &prediction,
                    width,
                    height,
                    DA3_DEFAULT_HFOV_DEG,
                )
                .map_err(diffusion_err)?;
                let png = crate::testpattern::encode_png_gray16(&map.mm, map.width, map.height)?;
                let sidecar = format!(
                    "{{\"metric\":true,\"unit\":\"mm\",\"min\":{:.1},\"max\":{:.1},\"focal_px\":{:.2}}}",
                    map.min_mm, map.max_mm, map.focal_px
                );
                (png, sidecar)
            }
            #[cfg(feature = "python-backends")]
            Gen::Subprocess => {
                let tmp_dir = self
                    .cache_dir
                    .clone()
                    .unwrap_or_else(std::env::temp_dir)
                    .join("tmp");
                // Subprocess `@P` phases land inside the 0.05..0.95 band.
                let mut sub_progress = |stage: &str, frac: f64| {
                    progress(&format!("depth: {stage}"), 0.05 + 0.90 * frac);
                };
                let out = run_cancellable(
                    &depth_cmd(),
                    &tmp_dir,
                    "depth",
                    &params.input_bytes,
                    DEPTH_TIMEOUT,
                    cancel,
                    &mut sub_progress,
                )
                .map_err(|err| match err {
                    SubprocError::Cancelled => AssetAiError::Cancelled,
                    other => AssetAiError::Backend(format!("depth: {other}")),
                })?;
                let sidecar = out.sidecar_json.ok_or_else(|| {
                    AssetAiError::Backend(
                        "depth subprocess wrote no {out}.json metadata sidecar".to_string(),
                    )
                })?;
                (out.out_bytes, sidecar)
            }
        };
        cancel.check()?;
        check_depth_output(&png)?;
        check_depth_sidecar(&sidecar)?;
        progress("done", 1.0);
        Ok(vec![
            ArtifactData {
                content_type: "image/png",
                ext: "png",
                bytes: png,
            },
            ArtifactData {
                content_type: "application/json",
                ext: "json",
                bytes: sidecar.into_bytes(),
            },
        ])
    }
}

// ---------------------------------------------------------------------------
// Tests (stubbed subprocess — this is what CI exercises)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::GenerateRequestJson;
    use crate::subproc_img::fake_png;
    use crate::testpattern::encode_png_rgba;

    fn depth_params(request: GenerateRequestJson) -> GenerateParams {
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

    const SIDECAR: &str =
        r#"{"metric":true,"unit":"mm","min":412.0,"max":8931.5,"focal_px":388.7}"#;

    #[test]
    fn stub_depth_to_png_plus_json_artifacts() {
        let mut backend = DepthBackend::with_stub(
            "da3-metric-large",
            Box::new(|input: &[u8], progress: ProgressSink| {
                assert_eq!(&input[..8], b"\x89PNG\r\n\x1a\n");
                progress("infer", 0.5);
                Ok((fake_png(640, 480, 16, 0), SIDECAR.to_string()))
            }),
        );
        let params = depth_params(GenerateRequestJson {
            model: "da3-metric-large".to_string(),
            input_b64: Some(b64(&input_png())),
            ..GenerateRequestJson::default()
        });
        let mut sink = |_: &str, _: f64| {};
        let artifacts = backend
            .generate(&params, &mut sink, &CancelToken::new())
            .unwrap();
        // TWO artifacts: the 16-bit mm depthmap first, then the metadata.
        assert_eq!(artifacts.len(), 2);
        assert_eq!(artifacts[0].content_type, "image/png");
        assert_eq!(artifacts[0].ext, "png");
        assert_eq!(artifacts[0].bytes, fake_png(640, 480, 16, 0));
        assert_eq!(artifacts[1].content_type, "application/json");
        assert_eq!(artifacts[1].ext, "json");
        assert_eq!(artifacts[1].bytes, SIDECAR.as_bytes());
    }

    #[test]
    fn sidecar_contract_validated() {
        let meta = check_depth_sidecar(SIDECAR).unwrap();
        assert!(meta.metric);
        assert_eq!(meta.unit, "mm");
        assert_eq!(meta.min, 412.0);
        assert_eq!(meta.max, 8931.5);
        assert_eq!(meta.focal_px, Some(388.7));
        // focal_px may be null.
        let meta = check_depth_sidecar(
            r#"{"metric":true,"unit":"mm","min":1.0,"max":2.0,"focal_px":null}"#,
        )
        .unwrap();
        assert_eq!(meta.focal_px, None);
        // Non-metric or wrong unit violates the contract.
        assert!(check_depth_sidecar(
            r#"{"metric":false,"unit":"mm","min":1.0,"max":2.0,"focal_px":null}"#
        )
        .is_err());
        assert!(check_depth_sidecar(
            r#"{"metric":true,"unit":"m","min":1.0,"max":2.0,"focal_px":null}"#
        )
        .is_err());
        // Garbage json.
        assert!(check_depth_sidecar("not json").is_err());
    }

    #[test]
    fn wrong_png_format_is_a_backend_error() {
        // 8-bit RGBA is NOT a valid depthmap (must be 16-bit grayscale).
        let mut backend = DepthBackend::with_stub(
            "da3-metric-large",
            Box::new(|_: &[u8], _p: ProgressSink| {
                Ok((fake_png(8, 4, 8, 6), SIDECAR.to_string()))
            }),
        );
        let params = depth_params(GenerateRequestJson {
            model: "da3-metric-large".to_string(),
            input_b64: Some(b64(&input_png())),
            ..GenerateRequestJson::default()
        });
        let mut sink = |_: &str, _: f64| {};
        let err = backend
            .generate(&params, &mut sink, &CancelToken::new())
            .err()
            .expect("rgba8 output must be an error");
        match err {
            AssetAiError::Backend(msg) => {
                assert!(msg.contains("16-bit grayscale"), "{msg}")
            }
            other => panic!("expected Backend error, got {other:?}"),
        }
        // check_depth_output directly.
        assert!(check_depth_output(&fake_png(2, 2, 16, 0)).is_ok());
        assert!(check_depth_output(&fake_png(2, 2, 8, 0)).is_err());
        assert!(check_depth_output(&fake_png(2, 2, 16, 6)).is_err());
        assert!(check_depth_output(b"junk").is_err());
    }

    #[test]
    fn bad_sidecar_fails_the_job() {
        let mut backend = DepthBackend::with_stub(
            "da3-metric-large",
            Box::new(|_: &[u8], _p: ProgressSink| {
                Ok((fake_png(8, 4, 16, 0), "garbage".to_string()))
            }),
        );
        let params = depth_params(GenerateRequestJson {
            model: "da3-metric-large".to_string(),
            input_b64: Some(b64(&input_png())),
            ..GenerateRequestJson::default()
        });
        let mut sink = |_: &str, _: f64| {};
        assert!(matches!(
            backend.generate(&params, &mut sink, &CancelToken::new()),
            Err(AssetAiError::Backend(_))
        ));
    }

    #[test]
    fn missing_input_image_is_a_params_error() {
        let mut backend = DepthBackend::with_stub(
            "da3-metric-large",
            Box::new(|_: &[u8], _p: ProgressSink| unreachable!()),
        );
        let params = depth_params(GenerateRequestJson {
            model: "da3-metric-large".to_string(),
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
    fn pre_raised_cancel_token_short_circuits() {
        let mut backend = DepthBackend::with_stub(
            "da3-metric-large",
            Box::new(|_: &[u8], _p: ProgressSink| {
                panic!("subprocess must not run on a cancelled job")
            }),
        );
        let params = depth_params(GenerateRequestJson {
            model: "da3-metric-large".to_string(),
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
    fn not_provisioned_on_dev_machines() {
        if std::env::var(DEPTH_CMD_ENV).is_err() {
            assert!(!depth_provisioned());
        }
    }
}
