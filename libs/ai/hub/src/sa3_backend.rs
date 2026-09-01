//! The `sa3` backend: audio domain — Stable Audio 3 Small SFX text-to-sound
//! through the in-repo port in libs/diffusion (`sa3_pipeline`, CPU f32 today;
//! the CUDA device path rides the same modules when it lands). No python, no
//! external processes.
//!
//! Layering (kokoro pattern): request handling, parameter defaults and the
//! WAV encoding compile and test EVERYWHERE — generation is pluggable, so CI
//! exercises the whole audio job path with a stub. The real generator
//! (feature `audio`) tokenizes with the in-repo Gemma SentencePiece port,
//! runs T5Gemma TE -> 20-block DiT (8-step ping-pong, CFG-free) -> SAME-S
//! transformer AE decode, and reports denoise progress per step.
//!
//! Request: `{model: "sa3-sfx", prompt, seconds, steps, seed}` -> one
//! `audio/wav` artifact (stereo 16-bit PCM, 44.1 kHz).
//!
//! Determinism note: seeds are deterministic per build but NOT
//! torch-compatible — the same seed yields a valid draw from the same
//! distribution as the reference, not the reference's exact sample (the
//! ping-pong sampler redraws gaussian noise every step; see
//! libs/diffusion/src/sa3_pipeline.rs).

use crate::backend::{CancelToken, ArtifactData, BackendCtx, ContentBackend, GenerateParams, ProgressSink};
use crate::error::AssetAiError;
use crate::wav::encode_wav_pcm16_stereo;

pub const SAMPLE_RATE: u32 = 44_100;
pub const DEFAULT_SECONDS: f64 = 4.0;
pub const DEFAULT_STEPS: u32 = 8;
pub const MAX_SECONDS: f64 = 120.0;

/// Job-fraction band for the pipeline's weight-load progress (labels — "load
/// sa3 dit/ae/te", "prepare sa3 device" — pass through as-is; this plays the
/// budget role h3_backend::stage_from_phase plays for video).
pub fn load_fraction(fraction: f64) -> f64 {
    0.01 + 0.03 * fraction.clamp(0.0, 1.0)
}

/// Job-fraction band for the pipeline's generate progress ("text-encode
/// k/12", "denoise k/N", "ae-decode k/6"); the backend's own wav-encode
/// stage follows at 0.95.
pub fn gen_fraction(fraction: f64) -> f64 {
    0.04 + 0.90 * fraction.clamp(0.0, 1.0)
}

/// One generation request handed to the generator.
#[derive(Clone, Debug)]
pub struct AudioJob {
    pub prompt: String,
    pub seconds: f64,
    pub steps: u32,
    pub seed: u64,
}

/// Pluggable generation: planar stereo `(left, right)` f32 at `SAMPLE_RATE`.
/// The real path calls libs/diffusion; tests plug in a closure.
pub type GenFn = Box<
    dyn FnMut(&AudioJob, ProgressSink, &CancelToken) -> Result<(Vec<f32>, Vec<f32>), AssetAiError>
        + Send,
>;

enum Gen {
    Stub(GenFn),
    #[cfg(feature = "audio")]
    Sa3(sa3_gen::Sa3Gen),
}

pub struct Sa3Backend {
    model_id: String,
    gen: Gen,
}

impl Sa3Backend {
    /// Test/CI constructor: generation is the given closure, no weights.
    pub fn with_stub(model_id: &str, gen: GenFn) -> Self {
        Self {
            model_id: model_id.to_string(),
            gen: Gen::Stub(gen),
        }
    }

    /// Real constructor used by `create_backend`.
    #[cfg(feature = "audio")]
    pub fn new_sa3(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            gen: Gen::Sa3(sa3_gen::Sa3Gen::new()),
        }
    }
}

impl ContentBackend for Sa3Backend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn ensure_loaded(&mut self, _ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        match &mut self.gen {
            Gen::Stub(_) => Ok(()),
            #[cfg(feature = "audio")]
            Gen::Sa3(gen) => gen.ensure_loaded(_ctx),
        }
    }

    fn is_resident(&self) -> bool {
        match &self.gen {
            Gen::Stub(_) => false,
            #[cfg(feature = "audio")]
            Gen::Sa3(gen) => gen.is_resident(),
        }
    }

    fn unload(&mut self) -> Result<(), AssetAiError> {
        match &mut self.gen {
            Gen::Stub(_) => Ok(()),
            #[cfg(feature = "audio")]
            Gen::Sa3(gen) => gen.unload(),
        }
    }

    fn generate(
        &mut self,
        params: &GenerateParams,
        progress: ProgressSink,
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError> {
        let prompt = params.prompt.trim();
        if prompt.is_empty() {
            return Err(AssetAiError::Backend(
                "sound generation needs a non-empty `prompt`".to_string(),
            ));
        }
        let job = AudioJob {
            prompt: prompt.to_string(),
            seconds: params
                .seconds
                .unwrap_or(DEFAULT_SECONDS)
                .clamp(0.5, MAX_SECONDS),
            steps: params.steps.unwrap_or(DEFAULT_STEPS).clamp(1, 64),
            seed: params.seed,
        };
        cancel.check()?;
        let (left, right) = match &mut self.gen {
            Gen::Stub(gen) => gen(&job, &mut *progress, cancel)?,
            #[cfg(feature = "audio")]
            Gen::Sa3(gen) => gen.generate(&job, &mut *progress, cancel)?,
        };
        cancel.check()?;
        if left.is_empty() || left.len() != right.len() {
            return Err(AssetAiError::Backend(
                "sound generation produced no (or mismatched) audio".to_string(),
            ));
        }
        progress("wav-encode", 0.95);
        let wav = encode_wav_pcm16_stereo(&left, &right, SAMPLE_RATE);
        Ok(vec![ArtifactData {
            content_type: "audio/wav",
            ext: "wav",
            bytes: wav,
        }])
    }
}

// ---------------------------------------------------------------------------
// Real generation through libs/diffusion (feature audio)
// ---------------------------------------------------------------------------

#[cfg(feature = "audio")]
mod sa3_gen {
    use super::AudioJob;
    use crate::backend::{BackendCtx, CancelToken, ProgressSink};
    use crate::error::AssetAiError;
    use makepad_ai_sfx::sa3_pipeline::{Sa3Pipeline, Sa3SeededNoise};
    use makepad_ai_sfx::sa3_tokenizer::Sa3Tokenizer;
    use makepad_ai_sfx::sa3_transformer::Sa3PadMode;
    use makepad_ai_common::DiffusionError;
    use std::path::PathBuf;

    /// DiffusionError -> AssetAiError, preserving cancellation.
    fn gen_err(context: &str, err: DiffusionError) -> AssetAiError {
        match err {
            DiffusionError::Cancelled => AssetAiError::Cancelled,
            err => AssetAiError::Backend(format!("{context}: {err:?}")),
        }
    }

    pub struct Sa3Gen {
        /// (ckpt, t5gemma weights, tokenizer.model), resolved by ensure_loaded.
        paths: Option<(PathBuf, PathBuf, PathBuf)>,
        loaded: Option<(Sa3Pipeline, Sa3Tokenizer)>,
    }

    impl Sa3Gen {
        pub fn new() -> Self {
            Self {
                paths: None,
                loaded: None,
            }
        }

        pub fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
            ctx.ensure_files()?;
            // The main checkpoint and the encoder share the file name
            // model.safetensors, so resolve on exact cache paths.
            let file_at = |cache_as: &str| -> Result<PathBuf, AssetAiError> {
                ctx.spec
                    .files
                    .iter()
                    .find(|file| file.cache_as == cache_as)
                    .map(|file| file.dest_path(ctx.cache_dir))
                    .ok_or_else(|| {
                        AssetAiError::Backend(format!(
                            "model {}: registry lists no {cache_as}",
                            ctx.spec.id
                        ))
                    })
            };
            let paths = (
                file_at("audio/sa3/model.safetensors")?,
                file_at("audio/sa3/t5gemma-b-b-ul2/model.safetensors")?,
                file_at("audio/sa3/t5gemma-b-b-ul2/tokenizer.model")?,
            );
            if self.paths.as_ref() != Some(&paths) {
                self.unload()?;
                self.paths = Some(paths);
            }
            Ok(())
        }

        pub fn is_resident(&self) -> bool {
            self.loaded.is_some()
        }

        pub fn unload(&mut self) -> Result<(), AssetAiError> {
            let has_device = self
                .loaded
                .as_ref()
                .is_some_and(|(pipeline, _)| pipeline.device_active());
            if has_device {
                makepad_ai_common::backend::release_gpu_runtime_namespaces(&[
                    "sa3te::",
                    "sa3dit::",
                    "sa3ae::",
                ])
                .map_err(|error| AssetAiError::Backend(format!("sa3 unload: {error}")))?;
            }
            // Drop both the CPU f32 pipeline and the tokenizer only after
            // device teardown succeeded. A failed unload remains truthfully
            // resident and can be retried.
            self.loaded = None;
            Ok(())
        }

        pub fn generate(
            &mut self,
            job: &AudioJob,
            progress: ProgressSink,
            cancel: &CancelToken,
        ) -> Result<(Vec<f32>, Vec<f32>), AssetAiError> {
            let (ckpt, t5gemma, tokenizer_path) = self
                .paths
                .clone()
                .ok_or_else(|| AssetAiError::Backend("sa3 used before ensure_loaded".into()))?;
            if self.loaded.is_none() {
                progress("load weights", 0.01);
                // The pipeline labels each component read ("load sa3
                // dit/ae/te", "prepare sa3 device"); every emission is a
                // cancel boundary.
                let mut load_hook = |label: &str, fraction: f64| -> Result<(), DiffusionError> {
                    if cancel.is_cancelled() {
                        return Err(DiffusionError::Cancelled);
                    }
                    progress(label, super::load_fraction(fraction));
                    Ok(())
                };
                let pipeline = Sa3Pipeline::load(&ckpt, &t5gemma, Some(&mut load_hook))
                    .map_err(|e| gen_err("sa3 load", e))?;
                cancel.check()?;
                let tokenizer = Sa3Tokenizer::load(&tokenizer_path)
                    .map_err(|e| gen_err("sa3 tokenizer", e))?;
                self.loaded = Some((pipeline, tokenizer));
            }
            cancel.check()?;
            let (pipeline, tokenizer) = self.loaded.as_ref().unwrap();
            let (ids, mask) = tokenizer.tokenize_padded(&job.prompt);
            let mut noise = Sa3SeededNoise::new(job.seed);
            let steps = job.steps as usize;
            // The pipeline ticks "text-encode k/12" (per TE layer),
            // "denoise k/N" (per step) and "ae-decode k/6" (per AE block);
            // labels forward as-is, fractions remap onto the job band.
            let mut gen_hook = |label: &str, fraction: f64| -> Result<(), DiffusionError> {
                if cancel.is_cancelled() {
                    return Err(DiffusionError::Cancelled);
                }
                progress(label, super::gen_fraction(fraction));
                Ok(())
            };
            let is_cancelled = || cancel.is_cancelled();
            let mut audio = pipeline
                .generate(
                    &ids,
                    &mask,
                    job.seconds,
                    steps,
                    Sa3PadMode::VZero,
                    &mut noise,
                    Some(&mut gen_hook),
                    Some(&is_cancelled),
                )
                .map_err(|e| gen_err("sa3 generate", e))?;
            let right = audio.pop().unwrap_or_default();
            let left = audio.pop().unwrap_or_default();
            Ok((left, right))
        }
    }
}

// ---------------------------------------------------------------------------
// Tests (stubbed generation — this is what CI exercises)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::GenerateRequestJson;

    fn params(prompt: &str, seconds: Option<f64>, seed: Option<u64>) -> GenerateParams {
        let request = GenerateRequestJson {
            model: "sa3-sfx".to_string(),
            prompt: Some(prompt.to_string()),
            seconds,
            seed,
            ..GenerateRequestJson::default()
        };
        GenerateParams::from_request(&request).unwrap()
    }

    #[test]
    fn job_fraction_bands_stay_ordered() {
        // Pipeline-local fractions (0..=1) land inside their job bands,
        // clamped, and the bands never cross the backend's own wav-encode
        // stage at 0.95: load 0.01..0.04 < generate 0.04..0.94 < 0.95.
        assert!((load_fraction(0.0) - 0.01).abs() < 1e-9);
        assert!((load_fraction(1.0) - 0.04).abs() < 1e-9);
        assert!((gen_fraction(0.0) - 0.04).abs() < 1e-9);
        assert!((gen_fraction(1.0) - 0.94).abs() < 1e-9);
        // Out-of-range pipeline fractions clamp instead of escaping the band.
        assert!((gen_fraction(7.0) - 0.94).abs() < 1e-9);
        assert!((load_fraction(-1.0) - 0.01).abs() < 1e-9);
        assert!(gen_fraction(1.0) < 0.95);
    }

    #[test]
    fn stub_backend_is_never_reported_resident() {
        let mut backend = Sa3Backend::with_stub(
            "sa3-test",
            Box::new(|_, _, _| unreachable!("lifecycle test never generates")),
        );
        assert!(!backend.is_resident());
        backend.unload().unwrap();
        backend.unload().unwrap();
        assert!(!backend.is_resident());
    }

    #[cfg(feature = "audio")]
    #[test]
    fn native_backend_is_send_and_starts_cold() {
        fn assert_send<T: Send>() {}
        assert_send::<Sa3Backend>();
        let mut backend = Sa3Backend::new_sa3("sa3-test");
        assert!(!backend.is_resident());
        backend.unload().unwrap();
    }

    #[test]
    fn stub_generation_to_stereo_wav() {
        let mut backend = Sa3Backend::with_stub(
            "sa3-sfx",
            Box::new(|job: &AudioJob, progress: ProgressSink, _c: &CancelToken| {
                assert_eq!(job.prompt, "sword clash");
                assert!((job.seconds - 3.0).abs() < 1e-9);
                assert_eq!(job.steps, 8);
                assert_eq!(job.seed, 42);
                progress("denoise", 0.5);
                let n = (job.seconds * SAMPLE_RATE as f64) as usize;
                Ok((vec![0.25f32; n], vec![-0.25f32; n]))
            }),
        );
        let mut sink = |_: &str, _: f64| {};
        let artifacts = backend
            .generate(&params("sword clash", Some(3.0), Some(42)), &mut sink, &CancelToken::new())
            .unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].content_type, "audio/wav");
        assert_eq!(artifacts[0].ext, "wav");
        let wav = &artifacts[0].bytes;
        assert_eq!(&wav[0..4], b"RIFF");
        // stereo 16-bit
        assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 2);
        assert_eq!(
            u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]),
            SAMPLE_RATE
        );
        assert_eq!(wav.len(), 44 + 3 * SAMPLE_RATE as usize * 2 * 2);
    }

    #[test]
    fn defaults_and_clamps() {
        let mut backend = Sa3Backend::with_stub(
            "sa3-sfx",
            Box::new(|job: &AudioJob, _p: ProgressSink, _c: &CancelToken| {
                // Default duration + steps applied; huge request clamped.
                assert!((job.seconds - DEFAULT_SECONDS).abs() < 1e-9);
                assert_eq!(job.steps, DEFAULT_STEPS);
                Ok((vec![0.0f32; 100], vec![0.0f32; 100]))
            }),
        );
        let mut sink = |_: &str, _: f64| {};
        assert_eq!(
            backend
                .generate(&params("boom", None, None), &mut sink, &CancelToken::new())
                .unwrap()
                .len(),
            1
        );
        // The wire clamp is the cross-domain sanity range (music songs go to
        // 300 s); the sfx-specific <=120 s cap is applied by the job build.
        let clamped = params("boom", Some(1e9), None);
        assert!((clamped.seconds.unwrap() - 300.0).abs() < 1e-9);
        let low = params("boom", Some(0.01), None);
        assert!((low.seconds.unwrap() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn pre_raised_cancel_unwinds_before_generation() {
        let mut backend = Sa3Backend::with_stub(
            "sa3-sfx",
            Box::new(|_: &AudioJob, _p: ProgressSink, _c: &CancelToken| {
                panic!("generation must not run for a cancelled job");
            }),
        );
        let cancel = CancelToken::new();
        cancel.cancel();
        let mut sink = |_: &str, _: f64| {};
        assert!(matches!(
            backend.generate(&params("boom", None, None), &mut sink, &cancel),
            Err(AssetAiError::Cancelled)
        ));
    }

    #[test]
    fn empty_prompt_and_empty_audio_rejected() {
        let mut backend = Sa3Backend::with_stub(
            "sa3-sfx",
            Box::new(|_: &AudioJob, _p: ProgressSink, _c: &CancelToken| Ok((Vec::new(), Vec::new()))),
        );
        let mut sink = |_: &str, _: f64| {};
        assert!(backend.generate(&params("  ", None, None), &mut sink, &CancelToken::new()).is_err());
        assert!(backend
            .generate(&params("boom", None, None), &mut sink, &CancelToken::new())
            .is_err());
    }
}
