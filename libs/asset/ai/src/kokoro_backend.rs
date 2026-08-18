//! The `kokoro` backend: text-to-speech (speech domain) through the EXISTING
//! in-repo Kokoro-82M port in libs/tts — this file only wraps it, it does not
//! re-implement any inference.
//!
//! The seam is deliberately tiny because libs/tts already is a clean engine:
//! `KokoroSpeaker::load_with_voice(model, voice)` +
//! `synthesize_with_speed(text, speed) -> SpeechAudio { samples, 24_000 }`.
//! No Cx/app coupling exists — Metal offload state is process-global inside
//! makepad-ggml, so the speaker itself is plain (Send) data and lives
//! directly on the service worker.
//!
//! Weights are the makepad-converted `.mktts` / `.mkvoice` format. The
//! registry lists the upstream `hexgrad/Kokoro-82M` `.pth`/`.pt` files; a
//! fresh box downloads them from HF like any model and converts them
//! in-process (libs/tts `convert.rs` — byte-identical to the offline Python
//! converter), so nothing is ever hand-carried. A converted file already at
//! its cache path (`converts_to`) wins outright: existing boxes never
//! re-download the upstream source. Resolution order per file: converted in
//! the service cache, then the makepad-tts conventions (env override /
//! working dir / next to the exe) as a dev fallback, then download+convert.
//!
//! Request: `{model: "kokoro", text, voice, speed}` -> one `audio/wav`
//! artifact (mono 16-bit PCM, 24 kHz).

use crate::backend::{CancelToken, ArtifactData, BackendCtx, ContentBackend, GenerateParams, ProgressSink};
use crate::error::AssetAiError;
use crate::wav::encode_wav_pcm16_mono;

pub const DEFAULT_VOICE: &str = "bm_daniel";

/// One synthesis request handed to the synth.
#[derive(Clone, Debug)]
pub struct SpeechJob {
    pub text: String,
    /// Voice pack name without extension, e.g. "bm_daniel".
    pub voice: String,
    pub speed: f32,
}

/// Pluggable synthesis: `(samples, sample_rate)` out. The real path calls
/// libs/tts; tests plug in a closure.
pub type SynthFn = Box<dyn FnMut(&SpeechJob) -> Result<(Vec<f32>, u32), AssetAiError> + Send>;

enum Synth {
    Stub(SynthFn),
    #[cfg(feature = "tts")]
    Kokoro(kokoro_synth::KokoroSynth),
}

pub struct KokoroBackend {
    model_id: String,
    synth: Synth,
}

impl KokoroBackend {
    /// Test/CI constructor: synthesis is the given closure, no weights needed.
    pub fn with_stub(model_id: &str, synth: SynthFn) -> Self {
        Self {
            model_id: model_id.to_string(),
            synth: Synth::Stub(synth),
        }
    }

    /// Real constructor used by `create_backend`.
    #[cfg(feature = "tts")]
    pub fn new_kokoro(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            synth: Synth::Kokoro(kokoro_synth::KokoroSynth::new()),
        }
    }
}

/// Normalizes the requested voice to a bare pack name ("bm_daniel"), with
/// the extension tolerated and the default applied.
pub fn normalize_voice(voice: &str) -> Result<String, AssetAiError> {
    let voice = voice.trim();
    let voice = voice.strip_suffix(".mkvoice").unwrap_or(voice);
    let voice = if voice.is_empty() { DEFAULT_VOICE } else { voice };
    if !voice
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(AssetAiError::Backend(format!(
            "bad voice name {voice:?} (expected e.g. \"bm_daniel\")"
        )));
    }
    Ok(voice.to_string())
}

impl ContentBackend for KokoroBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn ensure_loaded(&mut self, _ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        match &mut self.synth {
            Synth::Stub(_) => Ok(()),
            #[cfg(feature = "tts")]
            Synth::Kokoro(synth) => synth.ensure_loaded(_ctx),
        }
    }

    fn generate(
        &mut self,
        params: &GenerateParams,
        progress: ProgressSink,
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError> {
        // `text` is the speech field; fall back to `prompt` so simple clients
        // can reuse the one field they already send.
        let text = if params.text.trim().is_empty() {
            params.prompt.trim()
        } else {
            params.text.trim()
        };
        if text.is_empty() {
            return Err(AssetAiError::Backend(
                "speech synthesis needs non-empty `text`".to_string(),
            ));
        }
        let job = SpeechJob {
            text: text.to_string(),
            voice: normalize_voice(&params.voice)?,
            speed: params.speed,
        };
        cancel.check()?;
        progress("synthesize", 0.1);
        let (samples, sample_rate) = match &mut self.synth {
            Synth::Stub(synth) => synth(&job)?,
            #[cfg(feature = "tts")]
            Synth::Kokoro(synth) => synth.synthesize(&job, &mut *progress, cancel)?,
        };
        cancel.check()?;
        if samples.is_empty() {
            return Err(AssetAiError::Backend(
                "synthesis produced no audio".to_string(),
            ));
        }
        progress("encode", 0.9);
        let wav = encode_wav_pcm16_mono(&samples, sample_rate);
        progress("done", 1.0);
        Ok(vec![ArtifactData {
            content_type: "audio/wav",
            ext: "wav",
            bytes: wav,
        }])
    }
}

// ---------------------------------------------------------------------------
// Real synthesis through libs/tts (feature tts)
// ---------------------------------------------------------------------------

#[cfg(feature = "tts")]
mod kokoro_synth {
    use super::SpeechJob;
    use crate::backend::{BackendCtx, CancelToken, ProgressSink};
    use crate::error::AssetAiError;
    use crate::registry::FileSpec;
    use makepad_tts::kokoro::KokoroSpeaker;
    use std::path::{Path, PathBuf};

    // KokoroSpeaker is plain weight buffers (Metal state is process-global in
    // libs/tts accel.rs), so it can live on the service worker directly.
    const _ASSERT_SPEAKER_SEND: fn() = || {
        fn assert_send<T: Send>() {}
        assert_send::<KokoroSpeaker>();
    };

    /// The cache-relative path of the file's usable form.
    fn usable_rel(file: &FileSpec) -> &str {
        file.converts_to.as_deref().unwrap_or(&file.cache_as)
    }

    /// True when the conversion output exists and is not older than an
    /// upstream source sitting in the cache. A missing upstream counts as
    /// fresh: existing boxes carry only the converted files, and those must
    /// keep working without any download.
    fn conversion_is_fresh(converted: &Path, upstream: &Path) -> bool {
        if !converted.is_file() {
            return false;
        }
        let modified = |path: &Path| std::fs::metadata(path).and_then(|m| m.modified());
        match (modified(converted), modified(upstream)) {
            (Ok(converted), Ok(upstream)) => converted >= upstream,
            // No upstream (or no mtimes on this filesystem): the converted
            // file stands on its own.
            _ => true,
        }
    }

    /// A converted copy found through the makepad-tts dev conventions
    /// (env override / working dir / next to the exe).
    fn dev_copy_exists(converted: &Path) -> bool {
        let Some(name) = converted.file_name().and_then(|name| name.to_str()) else {
            return false;
        };
        if name.ends_with(".mktts") {
            makepad_tts::kokoro::model_path_if_present().is_some()
        } else {
            makepad_tts::kokoro::named_voice_path_if_present(name).is_some()
        }
    }

    pub struct KokoroSynth {
        model_path: Option<PathBuf>,
        cache_dir: Option<PathBuf>,
        /// One speaker per loaded voice; reloaded when the voice changes.
        loaded: Option<(String, KokoroSpeaker)>,
    }

    impl KokoroSynth {
        pub fn new() -> Self {
            Self {
                model_path: None,
                cache_dir: None,
                loaded: None,
            }
        }

        /// Brings the converted `.mktts`/`.mkvoice` weights into the cache
        /// and resolves the model path.
        ///
        /// Per registry file: a converted file already at its `converts_to`
        /// cache path wins outright (existing boxes keep working with zero
        /// downloads — the upstream source never needs to land); a
        /// repo-root/env dev copy is honored next; only then is the upstream
        /// `.pth`/`.pt` downloaded from HF (resumable, with byte progress)
        /// and converted in-process with per-tensor progress.
        pub fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
            self.cache_dir = Some(ctx.cache_dir.to_path_buf());
            let spec = ctx.spec;
            let cache_dir = ctx.cache_dir;
            let total = spec.files.len();
            for (index, file) in spec.files.iter().enumerate() {
                let Some(converted) = file.converted_path(cache_dir) else {
                    // Old-style entry (pre-converted local file, no
                    // converts_to): nothing to ensure — the resolution below
                    // errors helpfully if it is missing.
                    continue;
                };
                let upstream = file.dest_path(cache_dir);
                if conversion_is_fresh(&converted, &upstream) {
                    continue;
                }
                if dev_copy_exists(&converted) {
                    // Synthesis resolves the same dev copy; no download.
                    continue;
                }
                // Download the upstream file (a no-op when it is already
                // cached), then convert it into the loader's format.
                let src = ctx
                    .downloader
                    .ensure_file(file, cache_dir, ctx.download_progress, ctx.cancel)?;
                let stem = converted
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| file.cache_as.clone());
                let progress = &mut *ctx.progress;
                makepad_tts::convert::convert_torch_weights(
                    &src,
                    &converted,
                    &mut |done, tensors| {
                        progress(
                            &format!("convert {stem} {done}/{tensors}"),
                            (index as f64 + done as f64 / tensors.max(1) as f64)
                                / total.max(1) as f64,
                        );
                    },
                )
                .map_err(|e| {
                    AssetAiError::Backend(format!(
                        "convert {} -> {}: {e:?}",
                        src.display(),
                        converted.display()
                    ))
                })?;
            }

            // Resolve the model weights: cache first, then the makepad-tts
            // conventions (env / working dir / exe dir) as the dev fallback.
            let model_file = spec
                .files
                .iter()
                .find(|file| usable_rel(file).ends_with(".mktts"))
                .ok_or_else(|| {
                    AssetAiError::Backend(format!(
                        "model {}: registry lists no .mktts file",
                        spec.id
                    ))
                })?;
            let cached = model_file
                .converted_path(cache_dir)
                .unwrap_or_else(|| model_file.dest_path(cache_dir));
            let resolved = if cached.is_file() {
                Some(cached.clone())
            } else {
                makepad_tts::kokoro::model_path_if_present().map(PathBuf::from)
            };
            match resolved {
                Some(path) => {
                    if self.model_path.as_ref() != Some(&path) {
                        self.model_path = Some(path);
                        self.loaded = None;
                    }
                    Ok(())
                }
                None => Err(AssetAiError::Backend(format!(
                    "kokoro weights not found: expected the converted file at {} \
                     (or set MAKEPAD_TTS_MODEL)",
                    cached.display()
                ))),
            }
        }

        fn voice_path(&self, voice: &str) -> Result<String, AssetAiError> {
            let file_name = format!("{voice}.mkvoice");
            if let Some(cache_dir) = &self.cache_dir {
                let cached = cache_dir.join("tts").join(&file_name);
                if cached.is_file() {
                    return Ok(cached.to_string_lossy().into_owned());
                }
            }
            makepad_tts::kokoro::named_voice_path_if_present(&file_name).ok_or_else(|| {
                AssetAiError::Backend(format!(
                    "voice pack {file_name} not found in the cache tts/ dir or next to the service"
                ))
            })
        }

        pub fn synthesize(
            &mut self,
            job: &SpeechJob,
            progress: ProgressSink,
            cancel: &CancelToken,
        ) -> Result<(Vec<f32>, u32), AssetAiError> {
            let model_path = self
                .model_path
                .clone()
                .ok_or_else(|| AssetAiError::Backend("kokoro used before ensure_loaded".into()))?;
            if self.loaded.as_ref().map(|(voice, _)| voice.as_str()) != Some(job.voice.as_str()) {
                progress("load voice", 0.05);
                let voice_path = self.voice_path(&job.voice)?;
                let speaker = KokoroSpeaker::load_with_voice(
                    &model_path.to_string_lossy(),
                    &voice_path,
                )
                .map_err(|e| AssetAiError::Backend(format!("kokoro load: {e:?}")))?;
                self.loaded = Some((job.voice.clone(), speaker));
            }
            cancel.check()?;
            let (_, speaker) = self.loaded.as_mut().unwrap();
            // Per-text-chunk progress + cancel boundary (a chunk is one
            // sentence-sized 510-phoneme window, sub-second each — the
            // granularity floor for this backend).
            let mut on_chunk = |done: usize, total: usize| {
                progress(
                    &format!("synth {}/{total}", done + 1),
                    0.1 + 0.75 * done as f64 / total.max(1) as f64,
                );
                !cancel.is_cancelled()
            };
            let audio = speaker
                .synthesize_with_speed_observed(&job.text, job.speed, &mut on_chunk)
                .map_err(|e| AssetAiError::Backend(format!("kokoro synthesize: {e:?}")))?;
            cancel.check()?;
            Ok((audio.samples, audio.sample_rate))
        }
    }
}

// ---------------------------------------------------------------------------
// Tests (stubbed synthesis — this is what CI exercises)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::GenerateParams;
    use crate::protocol::GenerateRequestJson;

    fn params(text: &str, voice: &str, speed: f64) -> GenerateParams {
        let request = GenerateRequestJson {
            model: "kokoro".to_string(),
            text: Some(text.to_string()),
            voice: Some(voice.to_string()),
            speed: Some(speed),
            ..GenerateRequestJson::default()
        };
        GenerateParams::from_request(&request).unwrap()
    }

    #[test]
    fn voice_normalization() {
        assert_eq!(normalize_voice("").unwrap(), "bm_daniel");
        assert_eq!(normalize_voice("bm_fable").unwrap(), "bm_fable");
        assert_eq!(normalize_voice("af_heart.mkvoice").unwrap(), "af_heart");
        assert!(normalize_voice("../evil").is_err());
        assert!(normalize_voice("a b").is_err());
    }

    #[test]
    fn stub_synthesis_to_wav() {
        let mut backend = KokoroBackend::with_stub(
            "kokoro",
            Box::new(|job: &SpeechJob| {
                assert_eq!(job.text, "hello world");
                assert_eq!(job.voice, "bm_fable");
                assert!((job.speed - 1.5).abs() < 1e-6);
                // 100 samples of silence at 24k.
                Ok((vec![0.0f32; 100], 24_000))
            }),
        );
        let mut sink = |_: &str, _: f64| {};
        let artifacts = backend
            .generate(&params("hello world", "bm_fable", 1.5), &mut sink, &CancelToken::new())
            .unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].content_type, "audio/wav");
        assert_eq!(artifacts[0].ext, "wav");
        let wav = &artifacts[0].bytes;
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(wav.len(), 44 + 100 * 2);
        assert_eq!(
            u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]),
            24_000
        );
    }

    #[test]
    fn prompt_falls_back_for_text() {
        let mut backend = KokoroBackend::with_stub(
            "kokoro",
            Box::new(|job: &SpeechJob| {
                assert_eq!(job.text, "spoken via prompt");
                Ok((vec![0.1f32; 10], 24_000))
            }),
        );
        let request = GenerateRequestJson {
            model: "kokoro".to_string(),
            prompt: Some("spoken via prompt".to_string()),
            ..GenerateRequestJson::default()
        };
        let params = GenerateParams::from_request(&request).unwrap();
        let mut sink = |_: &str, _: f64| {};
        assert_eq!(backend.generate(&params, &mut sink, &CancelToken::new()).unwrap().len(), 1);
    }

    #[test]
    fn pre_raised_cancel_unwinds_before_synthesis() {
        let mut backend = KokoroBackend::with_stub(
            "kokoro",
            Box::new(|_: &SpeechJob| panic!("synthesis must not run for a cancelled job")),
        );
        let cancel = CancelToken::new();
        cancel.cancel();
        let mut sink = |_: &str, _: f64| {};
        assert!(matches!(
            backend.generate(&params("hi", "bm_daniel", 1.0), &mut sink, &cancel),
            Err(AssetAiError::Cancelled)
        ));
    }

    #[test]
    fn empty_text_rejected_and_empty_audio_rejected() {
        let mut backend = KokoroBackend::with_stub(
            "kokoro",
            Box::new(|_: &SpeechJob| Ok((Vec::new(), 24_000))),
        );
        let mut sink = |_: &str, _: f64| {};
        // Empty text errors before synthesis.
        assert!(backend
            .generate(&params("  ", "bm_daniel", 1.0), &mut sink, &CancelToken::new())
            .is_err());
        // Empty audio from the synth errors too.
        assert!(backend
            .generate(&params("hi", "bm_daniel", 1.0), &mut sink, &CancelToken::new())
            .is_err());
    }
}
