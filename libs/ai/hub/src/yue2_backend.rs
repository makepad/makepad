//! The `yue2` backend: music domain — YuE2-3B lyrics + style -> an ABC
//! score -> stereo 48 kHz song, through the in-repo native port
//! (`makepad_ai_music::yue2_pipeline`). No Python. Fails closed when the
//! weights or CUDA are missing.
//!
//! Request: `{model: "yue2-3b", prompt: <style tags>, lyrics, seed,
//! score_mode?: "full"|"melody"|"off", score?: <ABC>, score_only?: bool,
//! guidance?: cfg, seconds?: upper bound}`. Artifacts: `audio/wav` (unless
//! `score_only`) and the score as text (`.abc`) whenever one exists, so a
//! client can show it, let the user edit chords/melody and send it back as
//! `score` to re-render.
//!
//! Progress is fine-grained: weight upload (bytes), score tokens, codec
//! tokens with an ETA from the score's own length, flow-matching steps and
//! VAE tiles.

use crate::backend::{
    ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, ProgressSink,
};
use crate::error::AssetAiError;

pub const SAMPLE_RATE: u32 = 48_000;

#[derive(Clone, Debug, PartialEq)]
pub struct Yue2Job {
    pub style: String,
    pub lyrics: String,
    pub score_mode: String,
    pub score: Option<String>,
    pub score_only: bool,
    pub seed: u64,
    pub guidance: Option<f32>,
    /// Upper bound on the song length (codec tokens = 25 per second).
    pub max_seconds: Option<f64>,
}

/// What one job produced.
pub struct Yue2Output {
    pub score: Option<String>,
    /// Interleaved stereo; empty for `score_only`.
    pub audio: Vec<f32>,
}

pub fn job_from_params(params: &GenerateParams) -> Result<Yue2Job, AssetAiError> {
    let style = params.prompt.trim();
    if style.is_empty() {
        return Err(AssetAiError::Params(
            "yue2 needs style tags in `prompt` (genre, mood, instruments, vocal)".into(),
        ));
    }
    let score_mode = match params.score_mode.trim() {
        "" => "full".to_string(),
        mode @ ("full" | "melody" | "off") => mode.to_string(),
        other => {
            return Err(AssetAiError::Params(format!(
                "unknown score_mode {other:?} (expected \"full\", \"melody\" or \"off\")"
            )))
        }
    };
    let score = params.score.trim();
    let score = if score.is_empty() { None } else { Some(params.score.clone()) };
    if score.is_some() && score_mode == "off" {
        return Err(AssetAiError::Params(
            "a supplied score needs score_mode \"full\" or \"melody\"".into(),
        ));
    }
    if params.score_only && score_mode == "off" {
        return Err(AssetAiError::Params("score_only needs score_mode \"full\" or \"melody\"".into()));
    }
    if let Some(g) = params.guidance {
        if !g.is_finite() || !(0.0..=20.0).contains(&g) {
            return Err(AssetAiError::Params("guidance must be in [0, 20]".into()));
        }
    }
    Ok(Yue2Job {
        style: style.to_string(),
        lyrics: params.lyrics.trim().to_string(),
        score_mode,
        score,
        score_only: params.score_only,
        seed: params.seed,
        guidance: params.guidance,
        max_seconds: params.seconds.filter(|s| *s >= 10.0),
    })
}

pub type GenFn = Box<
    dyn FnMut(&Yue2Job, ProgressSink, &CancelToken) -> Result<Yue2Output, AssetAiError> + Send,
>;

enum Gen {
    Stub(GenFn),
    #[cfg(feature = "audio")]
    Native(native::Yue2Gen),
}

pub struct Yue2Backend {
    model_id: String,
    gen: Gen,
}

impl Yue2Backend {
    pub fn with_stub(model_id: &str, gen: GenFn) -> Self {
        Self { model_id: model_id.to_string(), gen: Gen::Stub(gen) }
    }

    #[cfg(feature = "audio")]
    pub fn new_native(model_id: &str) -> Self {
        Self { model_id: model_id.to_string(), gen: Gen::Native(native::Yue2Gen::new()) }
    }
}

impl ContentBackend for Yue2Backend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn ensure_loaded(&mut self, _ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        match &mut self.gen {
            Gen::Stub(_) => Ok(()),
            #[cfg(feature = "audio")]
            Gen::Native(gen) => gen.ensure_loaded(_ctx),
        }
    }

    fn is_resident(&self) -> bool {
        match &self.gen {
            Gen::Stub(_) => false,
            #[cfg(feature = "audio")]
            Gen::Native(gen) => gen.is_resident(),
        }
    }

    fn unload(&mut self) -> Result<(), AssetAiError> {
        match &mut self.gen {
            Gen::Stub(_) => Ok(()),
            #[cfg(feature = "audio")]
            Gen::Native(gen) => gen.unload(),
        }
    }

    fn generate(
        &mut self,
        params: &GenerateParams,
        progress: ProgressSink,
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError> {
        let job = job_from_params(params)?;
        cancel.check()?;
        let out = match &mut self.gen {
            Gen::Stub(gen) => gen(&job, &mut *progress, cancel)?,
            #[cfg(feature = "audio")]
            Gen::Native(gen) => gen.generate(&job, &mut *progress, cancel)?,
        };
        cancel.check()?;
        let mut artifacts = Vec::new();
        if !job.score_only {
            if out.audio.len() < 2 || out.audio.len() % 2 != 0 {
                return Err(AssetAiError::Backend("yue2 produced no audio".into()));
            }
            progress("wav-encode", 0.995);
            let left: Vec<f32> = out.audio.iter().step_by(2).copied().collect();
            let right: Vec<f32> = out.audio.iter().skip(1).step_by(2).copied().collect();
            artifacts.push(ArtifactData {
                content_type: "audio/wav",
                ext: "wav",
                bytes: crate::wav::encode_wav_pcm16_stereo(&left, &right, SAMPLE_RATE),
            });
        }
        if let Some(score) = out.score {
            artifacts.push(ArtifactData {
                content_type: "text/vnd.abc",
                ext: "abc",
                bytes: score.into_bytes(),
            });
        }
        if artifacts.is_empty() {
            return Err(AssetAiError::Backend("yue2 produced nothing".into()));
        }
        Ok(artifacts)
    }
}

/// Progress bands of the whole job (0..1). Load is reported by
/// `ensure_loaded` on the load channel; these cover generation.
pub mod bands {
    pub const SCORE: (f64, f64) = (0.0, 0.25);
    pub const CODEC: (f64, f64) = (0.25, 0.80);
    pub const FLOW: (f64, f64) = (0.80, 0.95);
    pub const DECODE: (f64, f64) = (0.95, 0.99);

    pub fn at(band: (f64, f64), fraction: f64) -> f64 {
        band.0 + (band.1 - band.0) * fraction.clamp(0.0, 1.0)
    }
}

#[cfg(feature = "audio")]
mod native {
    use super::{bands, Yue2Job, Yue2Output};
    use crate::backend::{BackendCtx, CancelToken, ProgressSink};
    use crate::error::AssetAiError;
    use makepad_ai_common::DiffusionError;
    use makepad_ai_music::yue2::{yue2_abc_seconds, Yue2Cot, Yue2Request, Yue2Sampling};
    use makepad_ai_music::yue2_pipeline::{Yue2Pipeline, Yue2Progress, Yue2Vae};
    use std::path::PathBuf;
    use std::time::Instant;

    const MODEL_SUBDIR: &str = "music/YuE2-3B";
    const VAE_SUBDIR: &str = "music/YuE2-Vae";

    fn gen_err(context: &str, err: DiffusionError) -> AssetAiError {
        match err {
            DiffusionError::Cancelled => AssetAiError::Cancelled,
            err => AssetAiError::Backend(format!("{context}: {err:?}")),
        }
    }

    pub struct Yue2Gen {
        dirs: Option<(PathBuf, PathBuf)>,
        loaded: Option<(Yue2Pipeline, Yue2Vae)>,
    }

    impl Yue2Gen {
        pub fn new() -> Self {
            Self { dirs: None, loaded: None }
        }

        pub fn is_resident(&self) -> bool {
            self.loaded.is_some()
        }

        pub fn unload(&mut self) -> Result<(), AssetAiError> {
            if self.loaded.take().is_some() {
                makepad_ai_common::backend::release_gpu_runtime_namespaces(&["yue2::", "yue2vae::"])
                    .map_err(|error| AssetAiError::Backend(format!("yue2 unload: {error}")))?;
            }
            Ok(())
        }

        pub fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
            ctx.ensure_files()?;
            let model = ctx.cache_dir.join(MODEL_SUBDIR.split('/').collect::<PathBuf>());
            let vae = ctx.cache_dir.join(VAE_SUBDIR.split('/').collect::<PathBuf>());
            if self.dirs.as_ref() != Some(&(model.clone(), vae.clone())) {
                self.unload()?;
                self.dirs = Some((model.clone(), vae.clone()));
            }
            if self.loaded.is_some() {
                return Ok(());
            }
            if !makepad_ai_common::backend::gpu_device_available() {
                return Err(AssetAiError::Unavailable("yue2 needs a CUDA device".into()));
            }
            (ctx.progress)("loading yue2", 0.0);
            let pipe = Yue2Pipeline::load(&model).map_err(|e| gen_err("yue2 load", e))?;
            let cancel = ctx.cancel;
            let progress = &mut *ctx.progress;
            pipe.model
                .upload_all(&mut |done, total| {
                    if cancel.is_cancelled() {
                        return Err(DiffusionError::Cancelled);
                    }
                    progress(
                        &format!(
                            "loading yue2 weights {:.1}/{:.1} GB",
                            done as f64 / 1e9,
                            total as f64 / 1e9
                        ),
                        0.95 * done as f64 / total.max(1) as f64,
                    );
                    Ok(())
                })
                .map_err(|e| gen_err("yue2 weight upload", e))?;
            let vae = Yue2Vae::load(&vae).map_err(|e| gen_err("yue2 vae load", e))?;
            progress("loading yue2", 1.0);
            self.loaded = Some((pipe, vae));
            Ok(())
        }

        pub fn generate(
            &mut self,
            job: &Yue2Job,
            progress: ProgressSink,
            cancel: &CancelToken,
        ) -> Result<Yue2Output, AssetAiError> {
            let (pipe, vae) = self
                .loaded
                .as_ref()
                .ok_or_else(|| AssetAiError::Backend("yue2: not loaded".into()))?;
            let request = Yue2Request {
                style: job.style.clone(),
                lyrics: job.lyrics.clone(),
                cot: Yue2Cot::parse(&job.score_mode).unwrap_or(Yue2Cot::Full),
                seed: job.seed,
                abc: job.score.clone(),
                cfg_scale: job.guidance,
            };
            // Expected score length for an honest bar: ~5 ABC tokens per
            // lyric token is typical of the release; capped by the budget.
            let lyric_tokens = pipe.tokenizer.encode(&job.lyrics).len().max(40);
            let score_guess = (lyric_tokens * 5).clamp(300, Yue2Sampling::ABC.max_tokens) as f64;
            let started = Instant::now();
            let codec_expect = std::cell::Cell::new(0.0f64);
            let mut codec_start = None::<Instant>;
            let mut sink = |p: Yue2Progress| -> Result<(), DiffusionError> {
                if cancel.is_cancelled() {
                    return Err(DiffusionError::Cancelled);
                }
                match p {
                    Yue2Progress::Score { done, .. } => progress(
                        &format!("writing score: {done} tokens"),
                        bands::at(bands::SCORE, (done as f64 / score_guess).min(0.97)),
                    ),
                    Yue2Progress::Codec { done, max } => {
                        let expect = if codec_expect.get() > 0.0 { codec_expect.get() } else { max as f64 };
                        let t0 = *codec_start.get_or_insert_with(Instant::now);
                        let rate = done as f64 / t0.elapsed().as_secs_f64().max(1e-3);
                        let left = ((expect - done as f64).max(0.0) / rate.max(1.0)).round();
                        progress(
                            &format!(
                                "singing: {:.0}/{:.0} s of audio, ~{left:.0} s left",
                                done as f64 / 25.0,
                                expect / 25.0
                            ),
                            bands::at(bands::CODEC, (done as f64 / expect).min(0.99)),
                        )
                    }
                    Yue2Progress::Flow { done, total } => progress(
                        &format!("rendering audio: step {done}/{total}"),
                        bands::at(bands::FLOW, done as f64 / total as f64),
                    ),
                    Yue2Progress::Decode { done, total } => progress(
                        &format!("decoding: {done}/{total}"),
                        bands::at(bands::DECODE, done as f64 / total as f64),
                    ),
                }
                Ok(())
            };
            let plan = pipe
                .plan(&request, &Yue2Sampling::ABC, &mut sink)
                .map_err(|e| gen_err("yue2 score", e))?;
            if job.score_only {
                return Ok(Yue2Output { score: plan.abc, audio: Vec::new() });
            }
            let mut semantic = Yue2Sampling::SEMANTIC;
            if let Some(seconds) = job.max_seconds {
                semantic.max_tokens = ((seconds * 25.0) as usize).clamp(semantic.min_tokens, semantic.max_tokens);
            }
            codec_expect.set(plan
                .abc
                .as_deref()
                .and_then(yue2_abc_seconds)
                .map(|s| (s * 25.0).min(semantic.max_tokens as f64))
                .unwrap_or(0.0));
            let codec = pipe
                .semantic(&request, &plan, &semantic, &mut sink)
                .map_err(|e| gen_err("yue2 codec", e))?;
            let latents = pipe
                .synthesize(&plan.prefix, &codec.tokens, request.seed, Yue2Pipeline::default_steps(), &mut sink)
                .map_err(|e| gen_err("yue2 flow", e))?;
            let audio = vae.decode(&latents, &mut sink).map_err(|e| gen_err("yue2 vae", e))?;
            eprintln!(
                "[yue2] {:.1}s of audio in {:.1}s (score {} tok, codec {} tok)",
                audio.len() as f64 / 2.0 / super::SAMPLE_RATE as f64,
                started.elapsed().as_secs_f64(),
                plan.abc_ids.len(),
                codec.tokens.len()
            );
            Ok(Yue2Output { score: plan.abc, audio })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::GenerateRequestJson;

    fn params(f: impl FnOnce(&mut GenerateRequestJson)) -> GenerateParams {
        let mut request = GenerateRequestJson {
            model: "yue2-3b".to_string(),
            prompt: Some("city pop, female vocal".to_string()),
            lyrics: Some("[Verse]\nla la".to_string()),
            seed: Some(7),
            ..GenerateRequestJson::default()
        };
        f(&mut request);
        GenerateParams::from_request(&request).unwrap()
    }

    #[test]
    fn job_defaults_and_validation() {
        let job = job_from_params(&params(|_| {})).unwrap();
        assert_eq!(job.score_mode, "full");
        assert!(job.score.is_none() && !job.score_only);
        assert!(job_from_params(&params(|r| r.prompt = Some(" ".into()))).is_err());
        assert!(job_from_params(&params(|r| r.score_mode = Some("chords".into()))).is_err());
        assert!(job_from_params(&params(|r| {
            r.score_mode = Some("off".into());
            r.score = Some("X:1".into());
        }))
        .is_err());
        let job = job_from_params(&params(|r| r.score = Some("X:1\nK:C\nC4|".into()))).unwrap();
        assert_eq!(job.score.as_deref(), Some("X:1\nK:C\nC4|"));
    }

    #[test]
    fn stub_emits_wav_and_score() {
        let mut backend = Yue2Backend::with_stub(
            "yue2-3b",
            Box::new(|_, _, _| Ok(Yue2Output { score: Some("X:1".into()), audio: vec![0.1, -0.1, 0.2, -0.2] })),
        );
        let arts = backend
            .generate(&params(|_| {}), &mut |_, _| {}, &CancelToken::new())
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(arts.len(), 2);
        assert_eq!(arts[0].content_type, "audio/wav");
        assert_eq!(arts[1].ext, "abc");
    }

    #[test]
    fn score_only_returns_just_the_score() {
        let mut backend = Yue2Backend::with_stub(
            "yue2-3b",
            Box::new(|job, _, _| {
                assert!(job.score_only);
                Ok(Yue2Output { score: Some("X:1".into()), audio: vec![] })
            }),
        );
        let arts = backend
            .generate(&params(|r| r.score_only = Some(true)), &mut |_, _| {}, &CancelToken::new())
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(arts.len(), 1);
        assert_eq!(arts[0].content_type, "text/vnd.abc");
    }
}
