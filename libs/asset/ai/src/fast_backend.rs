//! The `fast` backend: video domain — FastVideo's FastH3 (the DMD2-distilled
//! MiniMax H3 transformer, `FastVideo/FastVideo-FastH3-4-step-Preview-v1-
//! VSA-DataFree`) through the SAME in-repo pipeline the `h3` backend runs:
//! tokenizer -> Qwen3-VL TE -> DiT denoise -> video VAE -> audio VAE -> mp4.
//!
//! What is different from `h3`, and only that:
//! - The DiT. The checkpoint is architecturally MiniMax H3 (56 heads x 128,
//!   hidden 5376, 50 blocks + 2 refiner blocks, ffn 14336, 24/32 in-channels,
//!   patch 1x2x2, shifts 12/3) with the same 638 tensor names, plus one
//!   `attn.to_gate_compress.weight` per block for its sparse-attention
//!   compression branch. The registry manifest names it through the
//!   [`crate::h3_backend::ROLE_DIT_BF16`] role; everything else (text
//!   encoder, both VAEs, tokenizer, processor) is byte-identical to the
//!   `MiniMaxAI/MiniMax-H3` tree and shares its cache paths.
//! - The schedule. The student was distilled on the five-point sigma grid
//!   `linspace(1, 0, 5)` under the stock shifts — exactly FOUR DiT forwards
//!   (`fastvideo_inference.json`: `transformer_forwards: 4`,
//!   `num_inference_steps: 5`, `dmd_denoising_steps: [999, 749, 500, 250]`,
//!   `guidance_scale: 1.0`). That is the H3 rectified-flow scheduler at
//!   `num_inference_steps = 5`, so nothing new runs: the wire `steps` here
//!   counts DiT FORWARDS (what "4-step" means) and is mapped to grid points.
//! - Attention. The checkpoint was trained with VSA-H3 (video sparse
//!   attention, tile 64, 90% sparsity, trained compression gates). This
//!   backend runs the pipeline's DENSE attention over the packed sequence and
//!   never reads the gate tensors — the same thing FastVideo's own
//!   `FLASH_ATTN` route does with this checkpoint. The trained VSA policy is
//!   an open item, not a silent default: see the registry note.
//!
//! Request: `{model: "fasth3-4step", prompt, width, height, frames, steps,
//! seed, codec}` -> one `video/mp4` artifact, exactly the `h3` contract.
//! `steps` = DiT forwards, default [`FAST_DEFAULT_FORWARDS`]; an explicit
//! 1..=[`FAST_MAX_FORWARDS`] is honoured, anything larger (a generic client's
//! H3-style 30/50) runs the trained schedule and says so in the service log.

use crate::backend::{
    ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, ProgressSink,
};
use crate::error::AssetAiError;
use crate::h3_backend::{tier_plan_for_spec, GenFn, H3Backend, H3TierKind, MuxFn, ROLE_DIT_BF16};
use crate::registry::ModelSpec;

/// The trained schedule: five sigma-grid points, four DiT forwards.
pub const FAST_DEFAULT_FORWARDS: u32 = 4;
/// Largest forward count honoured as an explicit request. The student was
/// distilled for four; a few more are a legitimate quality experiment, but
/// a 30- or 50-step request is an H3 preset reaching the wrong backend.
pub const FAST_MAX_FORWARDS: u32 = 8;

/// Resolves the wire `steps` (DiT forwards) to the forward count this job
/// runs. The second value is the requested count when it was FOLDED to the
/// trained schedule, so the caller can log it.
pub fn forwards_for_request(steps: Option<u32>) -> (u32, Option<u32>) {
    match steps {
        None | Some(0) => (FAST_DEFAULT_FORWARDS, None),
        Some(forwards) if forwards <= FAST_MAX_FORWARDS => (forwards, None),
        Some(requested) => (FAST_DEFAULT_FORWARDS, Some(requested)),
    }
}

/// Sigma-grid points the H3 pipeline takes for `forwards` DiT forwards
/// (`h3_schedule`: N points run N-1 forwards, the last point is sigma 0).
pub fn grid_points_for_forwards(forwards: u32) -> u32 {
    forwards + 1
}

/// A `fast` model manifest MUST carry a swapped bf16 DiT: without the role
/// the H3 machinery would quietly run the canonical MiniMax DiT under the
/// FastH3 id. Fail closed at load time.
pub fn check_fast_spec(spec: &ModelSpec) -> Result<(), AssetAiError> {
    let plan = tier_plan_for_spec(spec)?;
    if plan.kind != H3TierKind::Bf16Dit {
        return Err(AssetAiError::Registry(format!(
            "model {}: the fast backend needs a {ROLE_DIT_BF16:?} file role naming the FastH3 \
             transformer shard index, got a {:?} tier plan",
            spec.id, plan.kind
        )));
    }
    Ok(())
}

pub struct FastBackend {
    inner: H3Backend,
}

impl FastBackend {
    /// Test/CI constructor: generation and muxing are the given closures.
    pub fn with_stubs(model_id: &str, gen: GenFn, mux: MuxFn) -> Self {
        Self {
            inner: H3Backend::with_stubs(model_id, gen, mux),
        }
    }

    /// Real constructor used by `create_backend`.
    #[cfg(feature = "video")]
    pub fn new_fast(model_id: &str) -> Self {
        Self {
            inner: H3Backend::new_h3(model_id),
        }
    }
}

impl ContentBackend for FastBackend {
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }

    fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        check_fast_spec(ctx.spec)?;
        self.inner.ensure_loaded(ctx)
    }

    fn is_resident(&self) -> bool {
        self.inner.is_resident()
    }

    fn unload(&mut self) -> Result<(), AssetAiError> {
        self.inner.unload()
    }

    fn resident_is_healthy_after_error(&self, error: &AssetAiError) -> bool {
        self.inner.resident_is_healthy_after_error(error)
    }

    fn generate(
        &mut self,
        params: &GenerateParams,
        progress: ProgressSink,
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError> {
        let (forwards, folded) = forwards_for_request(params.steps);
        if let Some(requested) = folded {
            println!(
                "{}: steps={requested} is outside the distilled range 1..={FAST_MAX_FORWARDS}; \
                 running the trained {FAST_DEFAULT_FORWARDS}-forward schedule",
                self.inner.model_id()
            );
        }
        let mut params = params.clone();
        params.steps = Some(grid_points_for_forwards(forwards));
        self.inner.generate(&params, progress, cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::h3_backend::{MuxInput, VideoClip, VideoJob, ROLE_DIT_GGUF};
    use crate::protocol::GenerateRequestJson;
    use crate::registry::{Domain, FileSpec};

    #[test]
    fn steps_are_forwards_and_fold_to_the_trained_schedule() {
        assert_eq!(forwards_for_request(None), (4, None));
        assert_eq!(forwards_for_request(Some(0)), (4, None));
        assert_eq!(forwards_for_request(Some(1)), (1, None));
        assert_eq!(forwards_for_request(Some(4)), (4, None));
        assert_eq!(forwards_for_request(Some(8)), (8, None));
        // The H3 UI presets (30/40/50) reach the preferred backend by
        // default; they mean "the model's schedule", not 30 forwards.
        assert_eq!(forwards_for_request(Some(9)), (4, Some(9)));
        assert_eq!(forwards_for_request(Some(30)), (4, Some(30)));
        assert_eq!(forwards_for_request(Some(50)), (4, Some(50)));
        assert_eq!(grid_points_for_forwards(4), 5);
        assert_eq!(grid_points_for_forwards(1), 2);
    }

    fn stub_clip(job: &VideoJob) -> VideoClip {
        let (w, h) = (job.width as usize, job.height as usize);
        VideoClip {
            width: w,
            height: h,
            num_frames: 1,
            frames_rgb8: vec![0; w * h * 3],
            audio_planar: None,
            audio_rate: 32_000,
        }
    }

    fn backend_expecting_grid_points(expected: u32) -> FastBackend {
        FastBackend::with_stubs(
            "fasth3-4step",
            Box::new(move |job: &VideoJob, _p: ProgressSink, _c: &CancelToken| {
                assert_eq!(job.steps, expected, "grid points handed to the pipeline");
                Ok(stub_clip(job))
            }),
            Box::new(|_input: &MuxInput| Ok(b"MP4STUB".to_vec())),
        )
    }

    fn run(backend: &mut FastBackend, steps: Option<u32>) {
        let params = GenerateParams::from_request(&GenerateRequestJson {
            model: "fasth3-4step".to_string(),
            prompt: Some("a red car".to_string()),
            steps,
            ..GenerateRequestJson::default()
        })
        .unwrap();
        let mut sink = |_: &str, _: f64| {};
        let artifacts = backend.generate(&params, &mut sink, &CancelToken::new()).unwrap();
        assert_eq!(artifacts[0].content_type, "video/mp4");
    }

    #[test]
    fn the_pipeline_sees_forwards_plus_one_grid_points() {
        // Default: the trained four forwards = five grid points.
        run(&mut backend_expecting_grid_points(5), None);
        // Explicit forwards inside the distilled range are honoured.
        run(&mut backend_expecting_grid_points(7), Some(6));
        run(&mut backend_expecting_grid_points(2), Some(1));
        // An H3 preset's step count folds to the trained schedule.
        run(&mut backend_expecting_grid_points(5), Some(50));
    }

    fn spec(id: &str, roles: &[&str]) -> ModelSpec {
        ModelSpec {
            id: id.to_string(),
            domain: Domain::Video,
            backend: "fast".to_string(),
            available: true,
            gated: false,
            vram_gb: None,
            min_vram_gb: None,
            min_compute_cap: None,
            note: None,
            license: None,
            files: roles
                .iter()
                .map(|role| FileSpec {
                    role: Some(role.to_string()),
                    repo: "org/repo".into(),
                    path: format!("{role}.bin"),
                    revision: None,
                    cache_as: format!("video/fast/{role}.bin"),
                    size: None,
                    sha256: None,
                    local: false,
                    converts_to: None,
                    conversion: None,
                })
                .collect(),
        }
    }

    #[test]
    fn a_fast_manifest_must_name_its_swapped_dit() {
        assert!(check_fast_spec(&spec("ok", &[ROLE_DIT_BF16])).is_ok());
        // The bare tree would run the canonical MiniMax DiT under the
        // FastH3 id; a quant DiT is a different tier altogether.
        let err = check_fast_spec(&spec("tree", &[])).unwrap_err();
        assert!(err.to_string().contains(ROLE_DIT_BF16), "{err}");
        assert!(check_fast_spec(&spec("quant", &[ROLE_DIT_GGUF])).is_err());
    }

    #[test]
    fn the_embedded_fast_model_passes_the_manifest_gate() {
        let registry = crate::registry::Registry::embedded().unwrap();
        let fast = registry.find("fasth3-4step").unwrap();
        assert_eq!(fast.backend, "fast");
        check_fast_spec(fast).unwrap();
    }
}
