//! The `triposplat` backend: SPLAT domain — one object image -> a 3D
//! gaussian splat PLY.
//!
//! Production runs the pinned VAST-AI/TripoSplat checkpoints (MIT code and
//! weights) through the in-repo port in libs/ai/models/splat on the Makepad
//! CUDA stack: native BiRefNet cutout, DINOv3 ViT-H/16+ conditioning, a
//! FLUX.2 VAE latent condition, the 24-block rectified-flow denoiser over
//! 8192 splat-latent tokens, and the octree + gaussian decoder. There is no
//! Python, ComfyUI, subprocess or silent fallback in this path.
//!
//! Request: `{model: "triposplat", input_b64: <png>, seed?, steps?,
//! guidance?, gaussians?}` -> one `application/x-ply` artifact, the standard
//! 3DGS binary-little-endian layout the world backend also emits.
//!
//! Layering follows the trellis backend: request handling, PNG decode and
//! artifact shaping compile and test EVERYWHERE (the generator is pluggable,
//! so CI exercises the whole job path with a stub); the real generator sits
//! behind the `splat-native` feature.
//!
//! Matting: TripoSplat's own BiRefNet repack is a DIFFERENT checkpoint from
//! the service's `birefnet-hr` entry (same architecture and byte length,
//! different weights), so it is pinned separately and loaded through the same
//! native BiRefNet runtime. It is evicted before the much larger TripoSplat
//! weights stream in, exactly like the TRELLIS path.

use crate::backend::{
    ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, ProgressSink,
};
use crate::error::AssetAiError;

/// One generation request handed to the generator.
#[derive(Clone, Debug)]
pub struct SplatJob {
    /// Tightly packed RGBA8 input image.
    pub rgba: Vec<u8>,
    pub width: usize,
    pub height: usize,
    pub seed: u64,
    pub steps: usize,
    pub guidance: f32,
    /// Resolved gaussian budget (clamped and rounded by the model crate).
    pub gaussians: usize,
    /// True when the input already carries a meaningful alpha matte. False
    /// means the generator must run the native BiRefNet cutout first; it is
    /// never permission to continue unsegmented.
    pub segmented: bool,
}

/// Reference defaults (`TripoSplatPipeline.run`).
pub const SPLAT_STEPS_DEFAULT: u32 = 20;
pub const SPLAT_GUIDANCE_DEFAULT: f32 = 3.0;
pub const SPLAT_GAUSSIANS_DEFAULT: u32 = 262_144;

/// Pluggable generation: the real path runs the TripoSplat pipeline; tests
/// plug in a closure. Returns the finished PLY bytes.
pub type SplatGenFn = Box<dyn FnMut(&SplatJob, ProgressSink) -> Result<Vec<u8>, AssetAiError> + Send>;

enum Gen {
    Stub(SplatGenFn),
    #[cfg(feature = "splat-native")]
    Native(native::SplatGen),
}

pub struct SplatBackend {
    model_id: String,
    gen: Gen,
}

impl SplatBackend {
    /// Test/CI constructor: generation is the given closure.
    pub fn with_stub(model_id: &str, gen: SplatGenFn) -> Self {
        Self {
            model_id: model_id.to_string(),
            gen: Gen::Stub(gen),
        }
    }

    /// Real constructor used by `create_backend`.
    #[cfg(feature = "splat-native")]
    pub fn new_native(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            gen: Gen::Native(native::SplatGen::new()),
        }
    }
}

/// True only where a CUDA device is actually present. The model is ~1.9B
/// parameters of resident f32 with no CPU or Metal production path, so a box
/// without a device must fail closed rather than advertise the domain.
#[cfg(feature = "splat-native")]
pub fn splat_cuda_provisioned() -> bool {
    makepad_ai_splat::splat_device_available()
}

/// An input alpha channel counts as a matte when it is not uniformly opaque
/// — the same rule the reference uses (`alpha.min() < 255`).
pub fn alpha_is_segmented(rgba: &[u8]) -> bool {
    rgba.chunks_exact(4).any(|pixel| pixel[3] < 255)
}

/// Validates the artifact against the 3DGS contract: a binary-little-endian
/// PLY whose vertex element carries the pre-activation splat columns.
pub fn check_ply_output(bytes: &[u8]) -> Result<usize, AssetAiError> {
    let head = &bytes[..bytes.len().min(4096)];
    let text = String::from_utf8_lossy(head);
    if !text.starts_with("ply\n") {
        return Err(AssetAiError::Backend(
            "splat output is not a PLY file".to_string(),
        ));
    }
    if !text.contains("format binary_little_endian 1.0\n") {
        return Err(AssetAiError::Backend(
            "splat output is not binary little endian".to_string(),
        ));
    }
    for property in ["x", "opacity", "scale_0", "rot_0", "rot_3", "f_dc_2"] {
        if !text.contains(&format!("property float {property}\n")) {
            return Err(AssetAiError::Backend(format!(
                "splat output PLY is missing the '{property}' column"
            )));
        }
    }
    let count = text
        .lines()
        .find_map(|line| line.strip_prefix("element vertex "))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .ok_or_else(|| AssetAiError::Backend("splat output PLY has no vertex count".to_string()))?;
    if count == 0 {
        return Err(AssetAiError::Backend(
            "splat output PLY has zero gaussians".to_string(),
        ));
    }
    Ok(count)
}

impl ContentBackend for SplatBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn ensure_loaded(&mut self, _ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        match &mut self.gen {
            Gen::Stub(_) => Ok(()),
            #[cfg(feature = "splat-native")]
            Gen::Native(gen) => gen.ensure_loaded(_ctx),
        }
    }

    fn is_resident(&self) -> bool {
        match &self.gen {
            Gen::Stub(_) => false,
            #[cfg(feature = "splat-native")]
            Gen::Native(gen) => gen.is_resident(),
        }
    }

    fn unload(&mut self) -> Result<(), AssetAiError> {
        match &mut self.gen {
            Gen::Stub(_) => Ok(()),
            #[cfg(feature = "splat-native")]
            Gen::Native(gen) => gen.unload(),
        }
    }

    fn resident_is_healthy_after_error(&self, error: &AssetAiError) -> bool {
        // Parameter validation and cancellation never touch the weights or
        // the device cache, so `/models` stays ready/loaded; anything else
        // stays conservative.
        matches!(error, AssetAiError::Cancelled | AssetAiError::Params(_))
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
        cancel.check()?;
        progress("preprocess", 0.01);
        let (rgba, width, height) = crate::trellis_backend::decode_png_rgba8(&params.input_bytes)?;
        let segmented = alpha_is_segmented(&rgba);

        let job = SplatJob {
            rgba,
            width,
            height,
            seed: params.seed,
            steps: params.steps.unwrap_or(SPLAT_STEPS_DEFAULT).clamp(1, 200) as usize,
            guidance: params.guidance.unwrap_or(SPLAT_GUIDANCE_DEFAULT),
            gaussians: params.gaussians.unwrap_or(SPLAT_GAUSSIANS_DEFAULT) as usize,
            segmented,
        };
        cancel.check()?;
        let bytes = match &mut self.gen {
            Gen::Stub(gen) => gen(&job, progress)?,
            #[cfg(feature = "splat-native")]
            Gen::Native(gen) => gen.generate(&job, progress, cancel)?,
        };
        cancel.check()?;
        check_ply_output(&bytes)?;
        progress("done", 1.0);
        Ok(vec![ArtifactData {
            content_type: "application/x-ply",
            ext: "ply",
            bytes,
        }])
    }
}

// ---------------------------------------------------------------------------
// Real generation through libs/ai/models/splat (feature splat-native)
// ---------------------------------------------------------------------------

#[cfg(feature = "splat-native")]
mod native {
    use super::SplatJob;
    use crate::backend::{BackendCtx, CancelToken, ProgressSink};
    use crate::error::AssetAiError;
    use makepad_ai_common::DiffusionError;
    use makepad_ai_splat::splat_image::SplatImage;
    use makepad_ai_splat::{unload_splat, Device, SplatParams, SplatPipeline};
    use makepad_ai_vision::birefnet::{unload_birefnet, BiRefNet, BiRefNetImage, BiRefNetWeights};
    use std::path::PathBuf;

    struct Paths {
        dit: PathBuf,
        decoder: PathBuf,
        dino: PathBuf,
        vae: PathBuf,
        matte: PathBuf,
    }

    pub struct SplatGen {
        paths: Option<Paths>,
        pipeline: Option<SplatPipeline>,
        /// Set before the first device upload so the service calls `unload`
        /// on every unwind, even a partial one.
        resident: bool,
    }

    fn diffusion_err(err: DiffusionError) -> AssetAiError {
        match err {
            DiffusionError::Cancelled => AssetAiError::Cancelled,
            other => AssetAiError::Backend(format!("triposplat: {other}")),
        }
    }

    impl SplatGen {
        pub fn new() -> Self {
            Self {
                paths: None,
                pipeline: None,
                resident: false,
            }
        }

        pub fn is_resident(&self) -> bool {
            self.resident || self.pipeline.is_some()
        }

        /// Must run on the worker thread that performed the generation: both
        /// the weight cache and the activation pool are thread-local.
        pub fn unload(&mut self) -> Result<(), AssetAiError> {
            self.pipeline = None;
            unload_birefnet().map_err(diffusion_err)?;
            unload_splat().map_err(diffusion_err)?;
            self.resident = false;
            Ok(())
        }

        pub fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
            ctx.ensure_files()?;
            self.paths = Some(Paths {
                dit: ctx.path_by_role("dit")?,
                decoder: ctx.path_by_role("splat-decoder")?,
                dino: ctx.path_by_role("dino-conditioner")?,
                vae: ctx.path_by_role("vae")?,
                matte: ctx.path_by_role("native-matte")?,
            });
            Ok(())
        }

        pub fn generate(
            &mut self,
            job: &SplatJob,
            progress: ProgressSink,
            cancel: &CancelToken,
        ) -> Result<Vec<u8>, AssetAiError> {
            let paths = self.paths.as_ref().ok_or_else(|| {
                AssetAiError::Backend("triposplat used before ensure_loaded".into())
            })?;
            if !makepad_ai_splat::splat_device_available() {
                return Err(AssetAiError::Unavailable(
                    "triposplat requires a CUDA device".to_string(),
                ));
            }
            self.resident = true;
            let cancelled = || cancel.is_cancelled();

            // TripoSplat requires a segmented subject. Opaque inputs are
            // matted here, on the same CUDA worker, and BiRefNet is dropped
            // and evicted before the TripoSplat weights stream in.
            let mut rgba = job.rgba.clone();
            if !job.segmented {
                progress("matte load", 0.005);
                let matte_result = (|| -> Result<_, DiffusionError> {
                    let weights = BiRefNetWeights::load(&paths.matte)?;
                    let mut load_hook = |label: &str, fraction: f64| {
                        if cancelled() {
                            return Err(DiffusionError::Cancelled);
                        }
                        progress(label, 0.005 + 0.005 * fraction.clamp(0.0, 1.0));
                        Ok(())
                    };
                    let model =
                        BiRefNet::prepare_controlled(&weights, Some(&cancelled), Some(&mut load_hook))?;
                    let input = BiRefNetImage::rgba8(&rgba, job.width, job.height)?;
                    let mut matte_hook = |label: &str, fraction: f64| {
                        if cancelled() {
                            return Err(DiffusionError::Cancelled);
                        }
                        progress(label, 0.01 + 0.02 * fraction.clamp(0.0, 1.0));
                        Ok(())
                    };
                    model.matte_controlled(input, Some(&cancelled), Some(&mut matte_hook))
                })();
                // Always release a partially or fully populated BiRefNet
                // cache, including on cancellation or an operator error.
                unload_birefnet().map_err(diffusion_err)?;
                let matte = matte_result.map_err(diffusion_err)?;
                for (pixel, alpha) in rgba.chunks_exact_mut(4).zip(matte.alpha_u8()) {
                    pixel[3] = alpha;
                }
                progress("matte complete", 0.03);
                cancel.check()?;
            }

            if self.pipeline.is_none() {
                let mut load_hook = |label: &str, fraction: f64| {
                    if cancelled() {
                        return Err(DiffusionError::Cancelled);
                    }
                    progress(label, 0.03 + 0.17 * fraction.clamp(0.0, 1.0));
                    Ok(())
                };
                self.pipeline = Some(
                    SplatPipeline::prepare(
                        Device::Cuda,
                        &paths.dino,
                        &paths.dit,
                        &paths.decoder,
                        &paths.vae,
                        Some(&mut load_hook),
                    )
                    .map_err(diffusion_err)?,
                );
            }
            cancel.check()?;

            let pipeline = self
                .pipeline
                .as_ref()
                .ok_or_else(|| AssetAiError::Backend("triposplat pipeline missing".into()))?;
            let image = SplatImage::new(rgba, job.width, job.height, 4).map_err(diffusion_err)?;
            let params = SplatParams {
                seed: job.seed,
                steps: job.steps,
                guidance_scale: job.guidance,
                num_gaussians: job.gaussians,
                ..SplatParams::default()
            };
            let mut run_hook = |label: &str, fraction: f64| {
                if cancelled() {
                    return Err(DiffusionError::Cancelled);
                }
                progress(label, 0.20 + 0.79 * fraction.clamp(0.0, 1.0));
                Ok(())
            };
            pipeline
                .run(&image, &params, Some(&mut run_hook), Some(&cancelled))
                .map_err(diffusion_err)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests (stubbed native inference — this is what CPU-only CI exercises)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::GenerateRequestJson;
    use crate::testpattern::encode_png_rgb8;

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

    /// A minimal but structurally valid 3DGS PLY.
    fn fake_ply(count: usize) -> Vec<u8> {
        let props = [
            "x", "y", "z", "nx", "ny", "nz", "f_dc_0", "f_dc_1", "f_dc_2", "opacity", "scale_0",
            "scale_1", "scale_2", "rot_0", "rot_1", "rot_2", "rot_3",
        ];
        let mut header = String::from("ply\nformat binary_little_endian 1.0\n");
        header.push_str(&format!("element vertex {count}\n"));
        for name in props {
            header.push_str(&format!("property float {name}\n"));
        }
        header.push_str("end_header\n");
        let mut bytes = header.into_bytes();
        bytes.extend(std::iter::repeat(0u8).take(count * props.len() * 4));
        bytes
    }

    #[test]
    fn stub_generation_yields_one_ply_artifact() {
        let mut backend = SplatBackend::with_stub(
            "triposplat",
            Box::new(|job: &SplatJob, progress: ProgressSink| {
                assert_eq!(job.width, 8);
                assert_eq!(job.gaussians, SPLAT_GAUSSIANS_DEFAULT as usize);
                assert_eq!(job.steps, SPLAT_STEPS_DEFAULT as usize);
                assert!(!job.segmented, "an opaque PNG carries no matte");
                progress("denoise", 0.5);
                Ok(fake_ply(4))
            }),
        );
        let params = GenerateParams::from_request(&GenerateRequestJson {
            model: "triposplat".to_string(),
            input_b64: Some(b64(&input_png(8, 4))),
            ..GenerateRequestJson::default()
        })
        .unwrap();
        let mut sink = |_: &str, _: f64| {};
        let artifacts = backend
            .generate(&params, &mut sink, &CancelToken::new())
            .unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].content_type, "application/x-ply");
        assert_eq!(artifacts[0].ext, "ply");
        // Same artifact shape as the world backend's FlashWorld output.
        assert!(artifacts[0].bytes.starts_with(b"ply\n"));
    }

    #[test]
    fn request_knobs_reach_the_job() {
        let mut backend = SplatBackend::with_stub(
            "triposplat",
            Box::new(|job: &SplatJob, _p: ProgressSink| {
                assert_eq!(job.seed, 99);
                assert_eq!(job.steps, 8);
                assert_eq!(job.gaussians, 65_536);
                assert!((job.guidance - 5.5).abs() < 1e-6);
                Ok(fake_ply(1))
            }),
        );
        let params = GenerateParams::from_request(&GenerateRequestJson {
            model: "triposplat".to_string(),
            input_b64: Some(b64(&input_png(4, 4))),
            seed: Some(99),
            steps: Some(8),
            guidance: Some(5.5),
            gaussians: Some(65_536),
            ..GenerateRequestJson::default()
        })
        .unwrap();
        let mut sink = |_: &str, _: f64| {};
        backend
            .generate(&params, &mut sink, &CancelToken::new())
            .unwrap();
    }

    #[test]
    fn gaussian_budget_is_clamped_on_the_wire() {
        for (requested, want) in [(1u32, 32_768u32), (10_000_000, 262_144), (65_536, 65_536)] {
            let params = GenerateParams::from_request(&GenerateRequestJson {
                model: "triposplat".to_string(),
                gaussians: Some(requested),
                ..GenerateRequestJson::default()
            })
            .unwrap();
            assert_eq!(params.gaussians, Some(want), "requested {requested}");
        }
    }

    #[test]
    fn missing_input_image_is_a_params_error() {
        let mut backend = SplatBackend::with_stub(
            "triposplat",
            Box::new(|_j: &SplatJob, _p: ProgressSink| unreachable!()),
        );
        let params = GenerateParams::from_request(&GenerateRequestJson {
            model: "triposplat".to_string(),
            prompt: Some("ignored".to_string()),
            ..GenerateRequestJson::default()
        })
        .unwrap();
        let mut sink = |_: &str, _: f64| {};
        match backend.generate(&params, &mut sink, &CancelToken::new()) {
            Err(AssetAiError::Params(msg)) => assert!(msg.contains("input_b64")),
            Err(other) => panic!("expected a Params error, got {other:?}"),
            Ok(_) => panic!("expected a Params error, got artifacts"),
        }
    }

    #[test]
    fn a_non_ply_generator_result_is_a_backend_error() {
        let mut backend = SplatBackend::with_stub(
            "triposplat",
            Box::new(|_j: &SplatJob, _p: ProgressSink| Ok(b"not a ply".to_vec())),
        );
        let params = GenerateParams::from_request(&GenerateRequestJson {
            model: "triposplat".to_string(),
            input_b64: Some(b64(&input_png(4, 4))),
            ..GenerateRequestJson::default()
        })
        .unwrap();
        let mut sink = |_: &str, _: f64| {};
        assert!(matches!(
            backend.generate(&params, &mut sink, &CancelToken::new()),
            Err(AssetAiError::Backend(_))
        ));
        // The validator's individual rules.
        assert_eq!(check_ply_output(&fake_ply(7)).unwrap(), 7);
        assert!(check_ply_output(&fake_ply(0)).is_err());
        assert!(check_ply_output(b"ply\nformat ascii 1.0\n").is_err());
        let mut missing = String::from("ply\nformat binary_little_endian 1.0\nelement vertex 1\n");
        missing.push_str("property float x\nend_header\n");
        assert!(check_ply_output(missing.as_bytes()).is_err());
    }

    #[test]
    fn pre_raised_cancel_token_short_circuits() {
        let mut backend = SplatBackend::with_stub(
            "triposplat",
            Box::new(|_j: &SplatJob, _p: ProgressSink| {
                panic!("the runtime must not run on a cancelled job")
            }),
        );
        let params = GenerateParams::from_request(&GenerateRequestJson {
            model: "triposplat".to_string(),
            input_b64: Some(b64(&input_png(4, 4))),
            ..GenerateRequestJson::default()
        })
        .unwrap();
        let token = CancelToken::new();
        token.cancel();
        let mut sink = |_: &str, _: f64| {};
        assert!(matches!(
            backend.generate(&params, &mut sink, &token),
            Err(AssetAiError::Cancelled)
        ));
    }

    #[test]
    fn alpha_detection_matches_the_reference_rule() {
        assert!(!alpha_is_segmented(&[1, 2, 3, 255, 4, 5, 6, 255]));
        assert!(alpha_is_segmented(&[1, 2, 3, 255, 4, 5, 6, 254]));
    }

    #[test]
    fn registry_entry_is_pinned_and_in_the_splat_domain() {
        let registry = crate::registry::Registry::parse(crate::registry::EMBEDDED_REGISTRY).unwrap();
        let spec = registry
            .models
            .iter()
            .find(|model| model.id == "triposplat")
            .expect("triposplat must be registered");
        assert_eq!(spec.domain, crate::registry::Domain::Splat);
        assert_eq!(spec.backend, "triposplat");
        assert_eq!(spec.files.len(), 5);
        for file in &spec.files {
            assert_eq!(file.sha256.as_deref().map(str::len), Some(64), "{:?}", file.role);
            assert!(file.size.is_some_and(|size| size > 0), "{:?}", file.role);
            assert_eq!(
                file.revision.as_deref().map(str::len),
                Some(40),
                "{:?}",
                file.role
            );
        }
        // The BiRefNet repack TripoSplat ships is NOT the service's
        // birefnet-hr blob, so it must not reuse that cache_as.
        let matte = spec
            .files
            .iter()
            .find(|file| file.role.as_deref() == Some("native-matte"))
            .unwrap();
        assert_ne!(matte.cache_as, "matte/birefnet-hr/model.safetensors");
    }
}
