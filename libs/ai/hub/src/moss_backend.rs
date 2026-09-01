//! The `moss` backend: audio domain — MOSS-SoundEffect v2.0 text-to-sound
//! through the in-repo port in libs/diffusion (`moss_pipeline`; CUDA device
//! path for the DiT, CPU DAC decode of only the requested prefix). The
//! second audio model beside `sa3-sfx`, picked for output variety: full-band
//! 48 kHz, denser/'produced' event character vs SA3's dry one-shots.
//!
//! Layering matches sa3_backend (kokoro pattern): request handling, defaults
//! and WAV encoding compile and test everywhere; generation is pluggable so
//! CI exercises the job path with a stub. The real generator (feature
//! `audio`) tokenizes with the in-repo Qwen BPE port, runs Qwen3-1.7B TE ->
//! 30-block Wan-audio DiT (100-step CFG-4 flow matching over the fixed 30 s
//! latent) -> continuous-DAC decode, mono 48 kHz duplicated to stereo wav.
//!
//! Request: `{model: "moss-sfx", prompt, seconds, steps, seed}` -> one
//! `audio/wav` artifact (stereo 16-bit PCM, 48 kHz; both channels carry the
//! mono DAC output).
//!
//! Determinism note: seeds are deterministic per build but NOT
//! torch-compatible (same stance as sa3-sfx).

use crate::backend::{
    ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, ProgressSink,
};
use crate::error::AssetAiError;
use crate::wav::encode_wav_pcm16_stereo;

pub const SAMPLE_RATE: u32 = 48_000;
pub const DEFAULT_SECONDS: f64 = 4.0;
pub const DEFAULT_STEPS: u32 = 100;
pub const MAX_SECONDS: f64 = 30.0;

/// Job-fraction band for the pipeline's weight-load progress (labels — "load
/// moss te 1.2/3.4GB", "load moss dit 1.4/2.6GB", "load moss dac" — pass
/// through as-is; this plays the budget role h3_backend::stage_from_phase
/// plays for video).
pub fn load_fraction(fraction: f64) -> f64 {
    0.01 + 0.04 * fraction.clamp(0.0, 1.0)
}

/// Job-fraction band for the pipeline's generate progress ("text-encode
/// k/28", "denoise k/N", "dac-decode k/5"); the backend's own wav-encode
/// stage follows at 0.97.
pub fn gen_fraction(fraction: f64) -> f64 {
    0.05 + 0.90 * fraction.clamp(0.0, 1.0)
}

/// One generation request handed to the generator.
#[derive(Clone, Debug)]
pub struct MossJob {
    pub prompt: String,
    pub seconds: f64,
    pub steps: u32,
    pub seed: u64,
}

/// Pluggable generation: mono f32 samples at `SAMPLE_RATE`.
pub type GenFn = Box<
    dyn FnMut(&MossJob, ProgressSink, &CancelToken) -> Result<Vec<f32>, AssetAiError> + Send,
>;

enum Gen {
    Stub(GenFn),
    #[cfg(feature = "audio")]
    Moss(moss_gen::MossGen),
}

pub struct MossBackend {
    model_id: String,
    gen: Gen,
}

impl MossBackend {
    /// Test/CI constructor: generation is the given closure, no weights.
    pub fn with_stub(model_id: &str, gen: GenFn) -> Self {
        Self {
            model_id: model_id.to_string(),
            gen: Gen::Stub(gen),
        }
    }

    /// Real constructor used by `create_backend`.
    #[cfg(feature = "audio")]
    pub fn new_moss(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            gen: Gen::Moss(moss_gen::MossGen::new()),
        }
    }
}

impl ContentBackend for MossBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn ensure_loaded(&mut self, _ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        match &mut self.gen {
            Gen::Stub(_) => Ok(()),
            #[cfg(feature = "audio")]
            Gen::Moss(gen) => gen.ensure_loaded(_ctx),
        }
    }

    fn is_resident(&self) -> bool {
        match &self.gen {
            Gen::Stub(_) => false,
            #[cfg(feature = "audio")]
            Gen::Moss(gen) => gen.is_resident(),
        }
    }

    fn unload(&mut self) -> Result<(), AssetAiError> {
        match &mut self.gen {
            Gen::Stub(_) => Ok(()),
            #[cfg(feature = "audio")]
            Gen::Moss(gen) => gen.unload(),
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
        let job = MossJob {
            prompt: prompt.to_string(),
            seconds: params
                .seconds
                .unwrap_or(DEFAULT_SECONDS)
                .clamp(0.5, MAX_SECONDS),
            steps: params.steps.unwrap_or(DEFAULT_STEPS).clamp(2, 200),
            seed: params.seed,
        };
        cancel.check()?;
        let mono = match &mut self.gen {
            Gen::Stub(gen) => gen(&job, &mut *progress, cancel)?,
            #[cfg(feature = "audio")]
            Gen::Moss(gen) => gen.generate(&job, &mut *progress, cancel)?,
        };
        cancel.check()?;
        if mono.is_empty() {
            return Err(AssetAiError::Backend(
                "sound generation produced no audio".to_string(),
            ));
        }
        progress("wav-encode", 0.97);
        let wav = encode_wav_pcm16_stereo(&mono, &mono, SAMPLE_RATE);
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
mod moss_gen {
    use super::MossJob;
    use crate::backend::{BackendCtx, CancelToken, ProgressSink};
    use crate::error::AssetAiError;
    use makepad_ai_sfx::moss_pipeline::{MossDeviceState, MossPipeline};
    use makepad_ai_common::DiffusionError;
    use std::path::PathBuf;

    fn gen_err(context: &str, err: DiffusionError) -> AssetAiError {
        match err {
            DiffusionError::Cancelled => AssetAiError::Cancelled,
            err => AssetAiError::Backend(format!("{context}: {err:?}")),
        }
    }

    pub struct MossGen {
        /// (te dir, transformer dir, vae pth, tokenizer dir).
        paths: Option<(PathBuf, PathBuf, PathBuf, PathBuf)>,
        loaded: Option<(MossPipeline, Option<MossDeviceState>)>,
    }

    impl MossGen {
        pub fn new() -> Self {
            Self {
                paths: None,
                loaded: None,
            }
        }

        pub fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
            ctx.ensure_files()?;
            let dir_of = |cache_as: &str| -> Result<PathBuf, AssetAiError> {
                let file = ctx
                    .spec
                    .files
                    .iter()
                    .find(|file| file.cache_as == cache_as)
                    .ok_or_else(|| {
                        AssetAiError::Backend(format!(
                            "model {}: registry lists no {cache_as}",
                            ctx.spec.id
                        ))
                    })?;
                let path = file.dest_path(ctx.cache_dir);
                Ok(path.parent().unwrap_or(&path).to_path_buf())
            };
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
                dir_of("audio/moss/text_encoder/model-00001-of-00002.safetensors")?,
                dir_of("audio/moss/transformer/diffusion_pytorch_model.safetensors")?,
                file_at("audio/moss/vae/vae_128d_48k.pth")?,
                dir_of("audio/moss/tokenizer/tokenizer.json")?,
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
                .is_some_and(|(_, device)| device.is_some());
            if has_device {
                makepad_ai_common::backend::release_gpu_runtime_namespaces(&[
                    "mosste::",
                    "mossdit::",
                    "mossdac::",
                ])
                .map_err(|error| AssetAiError::Backend(format!("moss unload: {error}")))?;
            }
            // `MossPipeline` retains the complete CPU f32 checkpoint and the
            // optional prepared f16 copy. Keep it on teardown failure so
            // residency remains truthful and unload can be retried.
            self.loaded = None;
            Ok(())
        }

        pub fn generate(
            &mut self,
            job: &MossJob,
            progress: ProgressSink,
            cancel: &CancelToken,
        ) -> Result<Vec<f32>, AssetAiError> {
            if self.loaded.is_none() {
                progress("load", 0.01);
                let (te, dit, vae, tok) = self
                    .paths
                    .clone()
                    .ok_or_else(|| AssetAiError::Backend("moss: not resolved".to_string()))?;
                // The cold disk read moves: "load moss te 1.2/3.4GB" and
                // "load moss dit 1.4/2.6GB" tick every ~256MB; every
                // emission is a cancel boundary.
                let mut load_hook = |label: &str, fraction: f64| -> Result<(), DiffusionError> {
                    if cancel.is_cancelled() {
                        return Err(DiffusionError::Cancelled);
                    }
                    progress(label, super::load_fraction(fraction));
                    Ok(())
                };
                let pipe = MossPipeline::load(&te, &dit, &vae, &tok, Some(&mut load_hook))
                    .map_err(|e| gen_err("moss load", e))?;
                let device = if makepad_ai_sfx::moss::moss_device_enabled() {
                    // Host f32 -> f16 conversion pass over all weights.
                    progress("prepare moss device", super::load_fraction(1.0));
                    Some(pipe.prepare_device().map_err(|e| gen_err("moss device", e))?)
                } else {
                    None
                };
                self.loaded = Some((pipe, device));
            }
            let (pipe, device) = self.loaded.as_ref().unwrap();
            let steps = job.steps as usize;
            // The pipeline ticks "text-encode k/28" (per TE layer),
            // "denoise k/N" (per step) and "dac-decode k/5" (per DAC
            // block); labels forward as-is, fractions remap onto the job.
            let mut gen_hook = |label: &str, fraction: f64| -> Result<(), DiffusionError> {
                if cancel.is_cancelled() {
                    return Err(DiffusionError::Cancelled);
                }
                progress(label, super::gen_fraction(fraction));
                Ok(())
            };
            let cancel_flag = || cancel.check().is_err();
            let audio = pipe
                .generate_auto(
                    device.as_ref(),
                    &job.prompt,
                    job.seconds,
                    steps,
                    makepad_ai_sfx::moss::MOSS_DEFAULT_CFG,
                    job.seed,
                    Some(&mut gen_hook),
                    Some(&cancel_flag),
                )
                .map_err(|e| gen_err("moss generate", e))?;
            Ok(audio)
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
            model: "moss-sfx".to_string(),
            prompt: Some(prompt.to_string()),
            seconds,
            seed,
            ..GenerateRequestJson::default()
        };
        GenerateParams::from_request(&request).unwrap()
    }

    #[test]
    fn job_fraction_bands_stay_ordered() {
        // load 0.01..0.05 < generate 0.05..0.95 < wav-encode 0.97, with
        // out-of-range pipeline fractions clamped into their band.
        assert!((load_fraction(0.0) - 0.01).abs() < 1e-9);
        assert!((load_fraction(1.0) - 0.05).abs() < 1e-9);
        assert!((gen_fraction(0.0) - 0.05).abs() < 1e-9);
        assert!((gen_fraction(1.0) - 0.95).abs() < 1e-9);
        assert!((gen_fraction(7.0) - 0.95).abs() < 1e-9);
        assert!((load_fraction(-1.0) - 0.01).abs() < 1e-9);
        assert!(gen_fraction(1.0) < 0.97);
    }

    #[test]
    fn stub_backend_is_never_reported_resident() {
        let mut backend = MossBackend::with_stub(
            "moss-test",
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
        assert_send::<MossBackend>();
        let mut backend = MossBackend::new_moss("moss-test");
        assert!(!backend.is_resident());
        backend.unload().unwrap();
    }

    #[test]
    fn stub_generation_to_stereo_wav() {
        let mut backend = MossBackend::with_stub(
            "moss-sfx",
            Box::new(|job: &MossJob, progress: ProgressSink, _c: &CancelToken| {
                assert_eq!(job.prompt, "coin pickup");
                assert!((job.seconds - 2.0).abs() < 1e-9);
                assert_eq!(job.steps, 100);
                assert_eq!(job.seed, 7);
                progress("denoise", 0.5);
                let n = (job.seconds * SAMPLE_RATE as f64) as usize;
                Ok(vec![0.25f32; n])
            }),
        );
        let mut sink = |_: &str, _: f64| {};
        let artifacts = backend
            .generate(&params("coin pickup", Some(2.0), Some(7)), &mut sink, &CancelToken::new())
            .unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].content_type, "audio/wav");
        let wav = &artifacts[0].bytes;
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 2);
        assert_eq!(
            u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]),
            SAMPLE_RATE
        );
        assert_eq!(wav.len(), 44 + 2 * SAMPLE_RATE as usize * 2 * 2);
    }

    #[test]
    fn defaults_and_seconds_clamp() {
        let mut backend = MossBackend::with_stub(
            "moss-sfx",
            Box::new(|job: &MossJob, _p: ProgressSink, _c: &CancelToken| {
                assert!((job.seconds - DEFAULT_SECONDS).abs() < 1e-9);
                assert_eq!(job.steps, DEFAULT_STEPS);
                Ok(vec![0.0f32; 100])
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
        // the backend clamps to the model's 30 s ceiling even though the
        // request-level clamp allows more.
        let mut backend = MossBackend::with_stub(
            "moss-sfx",
            Box::new(|job: &MossJob, _p: ProgressSink, _c: &CancelToken| {
                assert!((job.seconds - MAX_SECONDS).abs() < 1e-9);
                Ok(vec![0.0f32; 100])
            }),
        );
        backend
            .generate(&params("boom", Some(90.0), None), &mut sink, &CancelToken::new())
            .unwrap();
    }

    #[test]
    fn empty_prompt_rejected() {
        let mut backend = MossBackend::with_stub(
            "moss-sfx",
            Box::new(|_j: &MossJob, _p: ProgressSink, _c: &CancelToken| Ok(vec![0.0f32; 10])),
        );
        let mut sink = |_: &str, _: f64| {};
        assert!(backend
            .generate(&params("  ", None, None), &mut sink, &CancelToken::new())
            .is_err());
    }
}
