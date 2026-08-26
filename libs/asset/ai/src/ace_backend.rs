//! The `ace` backend: music domain — ACE-Step 1.5 XL turbo lyrics + prompt
//! -> stereo wav through the in-repo native port (`ace_pipeline`). No
//! Python, no Music3 fallback. Fail closed if weights or CUDA are missing.
//!
//! Request contract matches MiniMax-Music3 so the UI can switch model id:
//! `{model: "ace-step-1.5-xl", prompt, lyrics, seconds, seed, steps}` -> one
//! `audio/wav` artifact (stereo 16-bit PCM, 48 kHz).

use crate::backend::{
    ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, ProgressSink,
};
use crate::error::AssetAiError;
use crate::wav::encode_wav_pcm16_stereo;

pub const SAMPLE_RATE: u32 = 48_000;
pub const MIN_SECONDS: f64 = 10.0;
pub const MAX_SECONDS: f64 = 600.0;
pub const DEFAULT_SECONDS: f64 = 60.0;
pub const DEFAULT_STEPS: u32 = 50;

#[cfg(feature = "audio")]
const MODEL_CACHE_SUBDIR: &str = "music/ACE-Step-1.5-XL";

pub fn load_fraction(fraction: f64) -> f64 {
    0.01 + 0.18 * fraction.clamp(0.0, 1.0)
}

pub fn gen_fraction(fraction: f64) -> f64 {
    0.20 + 0.75 * fraction.clamp(0.0, 1.0)
}

#[derive(Clone, Debug)]
pub struct MusicJob {
    pub prompt: String,
    pub lyrics: String,
    pub seconds: f64,
    pub steps: u32,
    pub seed: u64,
}

pub type GenFn = Box<
    dyn FnMut(&MusicJob, ProgressSink, &CancelToken) -> Result<(Vec<f32>, Vec<f32>), AssetAiError>
        + Send,
>;

enum Gen {
    Stub(GenFn),
    #[cfg(feature = "audio")]
    Ace(ace_gen::AceGen),
}

pub struct AceBackend {
    model_id: String,
    gen: Gen,
}

impl AceBackend {
    pub fn with_stub(model_id: &str, gen: GenFn) -> Self {
        Self {
            model_id: model_id.to_string(),
            gen: Gen::Stub(gen),
        }
    }

    #[cfg(feature = "audio")]
    pub fn new_ace(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            gen: Gen::Ace(ace_gen::AceGen::new()),
        }
    }
}

fn official_inputs(params: &GenerateParams) -> Result<(String, String), AssetAiError> {
    let prompt = params.prompt.trim();
    if prompt.is_empty() {
        return Err(AssetAiError::Params(
            "music generation needs a non-empty music description in `prompt`".into(),
        ));
    }
    let lyrics = params.lyrics.trim();
    Ok((
        prompt.to_string(),
        if lyrics.is_empty() {
            String::new()
        } else {
            lyrics.to_string()
        },
    ))
}

impl ContentBackend for AceBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn ensure_loaded(&mut self, _ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        match &mut self.gen {
            Gen::Stub(_) => Ok(()),
            #[cfg(feature = "audio")]
            Gen::Ace(gen) => gen.ensure_loaded(_ctx),
        }
    }

    fn is_resident(&self) -> bool {
        match &self.gen {
            Gen::Stub(_) => false,
            #[cfg(feature = "audio")]
            Gen::Ace(gen) => gen.is_resident(),
        }
    }

    fn unload(&mut self) -> Result<(), AssetAiError> {
        match &mut self.gen {
            Gen::Stub(_) => Ok(()),
            #[cfg(feature = "audio")]
            Gen::Ace(gen) => gen.unload(),
        }
    }

    fn generate(
        &mut self,
        params: &GenerateParams,
        progress: ProgressSink,
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError> {
        let (prompt, lyrics) = official_inputs(params)?;
        let job = MusicJob {
            prompt,
            lyrics,
            seconds: params
                .seconds
                .unwrap_or(DEFAULT_SECONDS)
                .clamp(MIN_SECONDS, MAX_SECONDS),
            steps: params.steps.unwrap_or(DEFAULT_STEPS).clamp(1, 64),
            seed: params.seed,
        };
        cancel.check()?;
        let (left, right) = match &mut self.gen {
            Gen::Stub(gen) => gen(&job, &mut *progress, cancel)?,
            #[cfg(feature = "audio")]
            Gen::Ace(gen) => gen.generate(&job, &mut *progress, cancel)?,
        };
        cancel.check()?;
        if left.is_empty() || left.len() != right.len() {
            return Err(AssetAiError::Backend(
                "ace generation produced no (or mismatched) audio".to_string(),
            ));
        }
        progress("wav-encode", 0.96);
        let wav = encode_wav_pcm16_stereo(&left, &right, SAMPLE_RATE);
        Ok(vec![ArtifactData {
            content_type: "audio/wav",
            ext: "wav",
            bytes: wav,
        }])
    }
}

#[cfg(feature = "audio")]
mod ace_gen {
    use super::MusicJob;
    use crate::backend::{BackendCtx, CancelToken, ProgressSink};
    use crate::error::AssetAiError;
    use makepad_ai_music::ace_pipeline::{AceGenerate, AcePaths, AcePipeline};
    use makepad_ai_common::DiffusionError;
    use std::path::PathBuf;

    fn gen_err(context: &str, err: DiffusionError) -> AssetAiError {
        match err {
            DiffusionError::Cancelled => AssetAiError::Cancelled,
            err => AssetAiError::Backend(format!("{context}: {err:?}")),
        }
    }

    pub struct AceGen {
        model_dir: Option<PathBuf>,
        loaded: Option<AcePipeline>,
    }

    impl AceGen {
        pub fn new() -> Self {
            Self {
                model_dir: None,
                loaded: None,
            }
        }

        pub fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
            ctx.ensure_files()?;
            let dir = ctx.cache_dir.join(
                super::MODEL_CACHE_SUBDIR
                    .split('/')
                    .collect::<PathBuf>(),
            );
            if self.model_dir.as_ref() != Some(&dir) {
                self.unload()?;
                self.model_dir = Some(dir);
            }
            Ok(())
        }

        pub fn is_resident(&self) -> bool {
            self.loaded.is_some()
        }

        pub fn unload(&mut self) -> Result<(), AssetAiError> {
            if self.loaded.is_some() {
                makepad_ai_common::backend::release_gpu_runtime_namespaces(&["acete::"])
                    .map_err(|error| AssetAiError::Backend(format!("ace unload: {error}")))?;
            }
            self.loaded = None;
            Ok(())
        }

        pub fn generate(
            &mut self,
            job: &MusicJob,
            progress: ProgressSink,
            cancel: &CancelToken,
        ) -> Result<(Vec<f32>, Vec<f32>), AssetAiError> {
            if self.loaded.is_none() {
                let dir = self
                    .model_dir
                    .clone()
                    .ok_or_else(|| AssetAiError::Backend("ace: not resolved".into()))?;
                if !dir.join("transformer").is_dir() && !dir.join("text_encoder").is_dir() {
                    return Err(AssetAiError::Unavailable(format!(
                        "ace-step-1.5-xl weights missing at {} — refusing (no Music3 fallback)",
                        dir.display()
                    )));
                }
                let mut load_hook = |label: &str, fraction: f64| -> Result<(), DiffusionError> {
                    if cancel.is_cancelled() {
                        return Err(DiffusionError::Cancelled);
                    }
                    progress(label, super::load_fraction(fraction));
                    Ok(())
                };
                let pipe = AcePipeline::load(&AcePaths::from_model_dir(&dir), Some(&mut load_hook))
                    .map_err(|e| gen_err("ace load", e))?;
                self.loaded = Some(pipe);
            }
            let pipe = self.loaded.as_ref().unwrap();
            let req = AceGenerate {
                prompt: job.prompt.clone(),
                lyrics: job.lyrics.clone(),
                seconds: job.seconds,
                seed: job.seed,
                steps: job.steps as usize,
                shift: makepad_ai_music::ace::ACE_DEFAULT_SHIFT,
                guidance: makepad_ai_music::ace::ACE_BASE_CFG,
                vocal_language: "en".into(),
            };
            let mut gen_hook = |label: &str, fraction: f64| -> Result<(), DiffusionError> {
                if cancel.is_cancelled() {
                    return Err(DiffusionError::Cancelled);
                }
                progress(label, super::gen_fraction(fraction));
                Ok(())
            };
            pipe.generate(&req, Some(&mut gen_hook))
                .map_err(|e| gen_err("ace generate", e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::GenerateRequestJson;

    fn params(prompt: &str, lyrics: Option<&str>, seconds: Option<f64>) -> GenerateParams {
        let request = GenerateRequestJson {
            model: "ace-step-1.5-xl".to_string(),
            prompt: Some(prompt.to_string()),
            lyrics: lyrics.map(|s| s.to_string()),
            seconds,
            seed: Some(7),
            ..GenerateRequestJson::default()
        };
        GenerateParams::from_request(&request).unwrap()
    }

    #[test]
    fn job_fraction_bands_stay_ordered() {
        assert!((load_fraction(0.0) - 0.01).abs() < 1e-9);
        assert!((load_fraction(1.0) - 0.19).abs() < 1e-9);
        assert!((gen_fraction(0.0) - 0.20).abs() < 1e-9);
        assert!((gen_fraction(1.0) - 0.95).abs() < 1e-9);
        assert!(gen_fraction(1.0) < 0.96);
    }

    #[test]
    fn empty_prompt_is_rejected() {
        let mut backend = AceBackend::with_stub("ace-step-1.5-xl", Box::new(|_, _, _| {
            Ok((vec![0.0], vec![0.0]))
        }));
        let err = match backend.generate(
            &params("", None, None),
            &mut |_, _| {},
            &CancelToken::new(),
        ) {
            Ok(_) => panic!("empty prompt should fail"),
            Err(err) => err,
        };
        assert!(format!("{err}").contains("prompt"));
    }

    #[test]
    fn stub_emits_wav() {
        let mut backend = AceBackend::with_stub(
            "ace-step-1.5-xl",
            Box::new(|job, _, _| {
                assert!(!job.prompt.is_empty());
                Ok((vec![0.1, -0.1], vec![0.1, -0.1]))
            }),
        );
        let arts = backend
            .generate(
                &params("piano ballad", Some("[verse]\nhi"), Some(12.0)),
                &mut |_, _| {},
                &CancelToken::new(),
            )
            .unwrap_or_else(|err| panic!("stub generate failed: {err}"));
        assert_eq!(arts.len(), 1);
        assert_eq!(arts[0].content_type, "audio/wav");
        assert!(arts[0].bytes.len() > 44);
    }
}
