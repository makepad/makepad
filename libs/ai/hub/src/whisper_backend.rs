//! The `whisper` backend: speech-to-text (the `stt` domain) through the
//! in-repo Whisper port — the wire side of the `stt.whisper` pipe a machine
//! node or LAN box publishes. This file only wraps the engine.
//!
//! Request: `{model: "whisper-large-v3-turbo", input_b64: <audio>,
//! input_content_type: "audio/wav", language: "en"}` -> one
//! `application/json` artifact, a [`TranscriptJson`]: the segments with
//! millisecond timing and the joined text. Any audio file the in-repo
//! decoders read (WAV, MP3, Ogg Vorbis, FLAC) is accepted; it is downmixed and
//! resampled to 16 kHz here, so a client never has to.

use crate::backend::{ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, ProgressSink};
use crate::error::AssetAiError;
use crate::protocol::{
    ForceAlignRequestJson, ForceWindowJson, TranscriptJson, TranscriptSegmentJson, FORCE_ALIGN_WINDOWS_INPUT, STT_MODE_FORCE_ALIGN,
    STT_MODE_WORDS,
};
use makepad_micro_serde::DeJson;
use makepad_micro_serde::SerJson;

const WHISPER_SAMPLE_RATE: u32 = 16_000;

/// One utterance handed to the engine: 16 kHz mono PCM plus the language.
pub struct TranscribeJob<'a> {
    pub samples_16k: &'a [f32],
    /// ISO 639-1, or `"auto"` to detect it from the audio.
    pub language: String,
    /// Text the model reads as the words just before (steers script and spelling); empty for none.
    pub prompt: String,
}

/// Pluggable recognition: the real path calls the makepad-ai-speech whisper engine; tests plug in
/// a closure.
pub type TranscribeFn =
    Box<dyn FnMut(&TranscribeJob) -> Result<Vec<TranscriptSegmentJson>, AssetAiError> + Send>;

enum Recognizer {
    Stub(TranscribeFn),
    #[cfg(feature = "stt")]
    Whisper(whisper_engine::WhisperEngine),
}

pub struct WhisperBackend {
    model_id: String,
    recognizer: Recognizer,
}

impl WhisperBackend {
    /// Test/CI constructor: recognition is the given closure, no weights.
    pub fn with_stub(model_id: &str, recognize: TranscribeFn) -> Self {
        Self { model_id: model_id.to_string(), recognizer: Recognizer::Stub(recognize) }
    }

    #[cfg(feature = "stt")]
    pub fn new_whisper(model_id: &str) -> Self {
        Self { model_id: model_id.to_string(), recognizer: Recognizer::Whisper(whisper_engine::WhisperEngine::new()) }
    }
}

/// Decode the request's audio to 16 kHz mono: any file the in-repo decoders
/// read (WAV, MP3, Ogg Vorbis, FLAC; sniffed from the bytes).
fn decode_input(params: &GenerateParams) -> Result<Vec<f32>, AssetAiError> {
    if params.input_bytes.is_empty() {
        return Err(AssetAiError::Params("speech recognition needs `input_b64` audio (WAV, MP3, Ogg Vorbis or FLAC)".into()));
    }
    let audio = makepad_audio_decode::decode_any(&params.input_bytes)
        .map_err(|e| AssetAiError::Params(format!("input audio ({}): {e}", params.input_content_type)))?;
    let channels = audio.channels.max(1) as usize;
    if audio.rate == 0 || audio.frames() == 0 {
        return Err(AssetAiError::Params("input audio holds no samples".into()));
    }
    let samples: Vec<f32> = audio
        .pcm_interleaved_f32
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect();
    Ok(if audio.rate == WHISPER_SAMPLE_RATE {
        samples
    } else {
        crate::resample::resample_channel(&samples, audio.rate, WHISPER_SAMPLE_RATE)
    })
}

impl ContentBackend for WhisperBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn ensure_loaded(&mut self, _ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        match &mut self.recognizer {
            Recognizer::Stub(_) => Ok(()),
            #[cfg(feature = "stt")]
            Recognizer::Whisper(engine) => engine.ensure_loaded(_ctx),
        }
    }

    fn is_resident(&self) -> bool {
        match &self.recognizer {
            Recognizer::Stub(_) => false,
            #[cfg(feature = "stt")]
            Recognizer::Whisper(engine) => engine.is_resident(),
        }
    }

    fn unload(&mut self) -> Result<(), AssetAiError> {
        match &mut self.recognizer {
            Recognizer::Stub(_) => {}
            #[cfg(feature = "stt")]
            Recognizer::Whisper(engine) => engine.unload(),
        }
        Ok(())
    }

    fn generate(
        &mut self,
        params: &GenerateParams,
        progress: ProgressSink,
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError> {
        progress("decode", 0.05);
        let samples = decode_input(params)?;
        cancel.check()?;
        let language = if params.language.trim().is_empty() {
            "en".to_string()
        } else {
            // Whisper wants the bare code; a BCP-47 tag loses its region.
            params.language.split(['-', '_']).next().unwrap_or("en").to_ascii_lowercase()
        };
        let mode = SttMode::of(&params.request_domain)?;
        if mode != SttMode::Transcript {
            return self.align(mode, params, &samples, &language, progress, cancel);
        }
        let job = TranscribeJob { samples_16k: &samples, language, prompt: params.prompt.clone() };
        progress("transcribe", 0.1);
        let segments = match &mut self.recognizer {
            Recognizer::Stub(recognize) => recognize(&job)?,
            #[cfg(feature = "stt")]
            Recognizer::Whisper(engine) => engine.transcribe(&job)?,
        };
        cancel.check()?;
        let text = segments
            .iter()
            .map(|s| s.text.trim())
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let language = match &self.recognizer {
            Recognizer::Stub(_) => None,
            #[cfg(feature = "stt")]
            Recognizer::Whisper(engine) => engine.detected_language(),
        };
        let json = TranscriptJson { text, segments, language }.serialize_json();
        progress("done", 1.0);
        Ok(vec![ArtifactData { content_type: "application/json", ext: "json", bytes: json.into_bytes() }])
    }
}

/// What an stt job answers, from the request's `domain`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SttMode {
    /// The segments and the joined text ([`TranscriptJson`]).
    Transcript,
    /// Segments with timed words and their tokens ([`AlignedTranscriptJson`]).
    Words,
    /// Known tokens aligned over windows ([`ForceAlignJson`]).
    ForceAlign,
}

impl SttMode {
    fn of(domain: &str) -> Result<SttMode, AssetAiError> {
        match domain.trim() {
            "" | "stt" => Ok(SttMode::Transcript),
            STT_MODE_WORDS => Ok(SttMode::Words),
            STT_MODE_FORCE_ALIGN => Ok(SttMode::ForceAlign),
            other => Err(AssetAiError::Params(format!(
                "stt domain {other:?}: the answers are the transcript (no domain), \"{STT_MODE_WORDS}\" or \"{STT_MODE_FORCE_ALIGN}\""
            ))),
        }
    }
}

/// The force-align job's windows, checked against the decoded samples.
fn force_windows(params: &GenerateParams, samples: usize) -> Result<Vec<ForceWindowJson>, AssetAiError> {
    let input = params
        .inputs
        .iter()
        .find(|input| input.name == FORCE_ALIGN_WINDOWS_INPUT)
        .ok_or_else(|| AssetAiError::Params(format!("force_align needs the named input `{FORCE_ALIGN_WINDOWS_INPUT}` (application/json)")))?;
    let text = std::str::from_utf8(&input.bytes).map_err(|_| AssetAiError::Params("force_align windows are not UTF-8".into()))?;
    let request = ForceAlignRequestJson::deserialize_json(text).map_err(|e| AssetAiError::Params(format!("force_align windows: {e:?}")))?;
    for (index, window) in request.windows.iter().enumerate() {
        if window.start > window.end || window.end as usize > samples || window.tokens.is_empty() || window.lead_pad > 30 * WHISPER_SAMPLE_RATE as u64 {
            return Err(AssetAiError::Params(format!(
                "force_align window {index} ({}..{} after {} padding, {} tokens) is outside the {samples} input samples",
                window.start,
                window.end,
                window.lead_pad,
                window.tokens.len()
            )));
        }
    }
    Ok(request.windows)
}

/// One word as the engine times it.
pub struct WordTiming {
    pub text: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub score: f32,
}

impl WhisperBackend {
    fn align(
        &mut self,
        mode: SttMode,
        params: &GenerateParams,
        samples: &[f32],
        language: &str,
        progress: ProgressSink,
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError> {
        let windows = if mode == SttMode::ForceAlign { force_windows(params, samples.len())? } else { Vec::new() };
        #[cfg(not(feature = "stt"))]
        {
            let _ = (windows, language, progress, cancel);
            Err(AssetAiError::Unavailable("word alignment needs the whisper engine (the `stt` feature)".into()))
        }
        #[cfg(feature = "stt")]
        {
            use crate::protocol::{AlignedSegmentJson, AlignedTranscriptJson, ForceAlignJson, ForcedWindowJson, WordSpanJson};
            let word_json = |word: &WordTiming| WordSpanJson { text: word.text.clone(), start_ms: word.start_ms, end_ms: word.end_ms, score: word.score };
            let Recognizer::Whisper(engine) = &mut self.recognizer else {
                return Err(AssetAiError::Unavailable("word alignment needs the whisper engine".into()));
            };
            let json = match mode {
                SttMode::Words => {
                    progress("transcribe with word timing", 0.1);
                    let segments = engine
                        .transcribe_aligned(samples, language)?
                        .into_iter()
                        .map(|segment| AlignedSegmentJson {
                            start_ms: segment.start_ms,
                            end_ms: segment.end_ms,
                            text: segment.text,
                            words: segment.words.iter().map(word_json).collect(),
                            tokens: segment.tokens,
                        })
                        .collect();
                    AlignedTranscriptJson { segments }.serialize_json()
                }
                SttMode::ForceAlign => {
                    let mut out = Vec::with_capacity(windows.len());
                    for (index, window) in windows.iter().enumerate() {
                        cancel.check()?;
                        progress(&format!("force align {}/{}", index + 1, windows.len()), 0.1 + 0.85 * index as f64 / windows.len().max(1) as f64);
                        let mut audio = vec![0.0f32; window.lead_pad as usize];
                        audio.extend_from_slice(&samples[window.start as usize..window.end as usize]);
                        let words = engine.force_align(&audio, &window.tokens, language)?;
                        out.push(ForcedWindowJson { words: words.map(|words| words.iter().map(word_json).collect()) });
                    }
                    ForceAlignJson { windows: out }.serialize_json()
                }
                SttMode::Transcript => unreachable!("the transcript is answered by generate"),
            };
            cancel.check()?;
            progress("done", 1.0);
            Ok(vec![ArtifactData { content_type: "application/json", ext: "json", bytes: json.into_bytes() }])
        }
    }
}

#[cfg(feature = "stt")]
mod whisper_engine {
    use super::{TranscribeJob, TranscriptSegmentJson, WordTiming};
    use crate::backend::BackendCtx;
    use crate::error::AssetAiError;
    use makepad_ai_speech::whisper::{WhisperModel, WhisperParams, WhisperState};
    use std::path::PathBuf;

    /// One segment of a word-timed transcription.
    pub struct AlignedSegment {
        pub start_ms: i64,
        pub end_ms: i64,
        pub text: String,
        pub words: Vec<WordTiming>,
        pub tokens: Vec<i32>,
    }

    fn timing(word: makepad_ai_speech::whisper::WordSpan) -> WordTiming {
        WordTiming { text: word.text, start_ms: word.start_ms, end_ms: word.end_ms, score: word.score }
    }

    pub struct WhisperEngine {
        model_path: Option<PathBuf>,
        loaded: Option<(WhisperModel, WhisperState)>,
    }

    impl WhisperEngine {
        pub fn new() -> Self {
            Self { model_path: None, loaded: None }
        }

        /// The registry file (downloaded on demand into the cache) or, as the
        /// dev fallback, the same chain the in-process session uses.
        pub fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
            let path = match ctx.ensure_files() {
                Ok(files) => files
                    .into_iter()
                    .find(|f| f.extension().is_some_and(|e| e == "bin"))
                    .or_else(crate::speech::weights::whisper_model_path),
                Err(_) => crate::speech::weights::whisper_model_path(),
            }
            .ok_or_else(|| {
                AssetAiError::Backend(format!(
                    "whisper weights not found (expected {} in the cache stt/ dir or MAKEPAD_VOICE_MODEL)",
                    crate::speech::weights::WHISPER_MODEL_FILE
                ))
            })?;
            if self.model_path.as_ref() != Some(&path) {
                self.model_path = Some(path.clone());
                self.loaded = None;
            }
            if self.loaded.is_none() {
                let model = WhisperModel::load_file(&path.to_string_lossy())
                    .map_err(|e| AssetAiError::Backend(format!("whisper load {}: {e:?}", path.display())))?;
                let state = WhisperState::new(&model);
                self.loaded = Some((model, state));
            }
            Ok(())
        }

        pub fn is_resident(&self) -> bool {
            self.loaded.is_some()
        }

        /// The language the last `"auto"` transcription settled on.
        pub fn detected_language(&self) -> Option<String> {
            let (_, state) = self.loaded.as_ref()?;
            state.detected_language().map(|(code, _)| code.to_string())
        }

        pub fn unload(&mut self) {
            self.loaded = None;
            // The CUDA backend keeps Whisper's f32 weights on the device per
            // thread; without this an unloaded Whisper still holds ~4 GB.
            makepad_ai_speech::whisper::release_device_state();
        }

        /// The bake's word-timed transcription: cross-attention DTW over
        /// the alignment heads, greedy, no prompt, the model's own token
        /// limit per window.
        pub fn transcribe_aligned(&mut self, samples_16k: &[f32], language: &str) -> Result<Vec<AlignedSegment>, AssetAiError> {
            let (model, state) = self
                .loaded
                .as_mut()
                .ok_or_else(|| AssetAiError::Backend("whisper used before ensure_loaded".into()))?;
            let mut params = WhisperParams::default();
            params.language = language.to_string();
            params.no_timestamps = false;
            params.single_segment = false;
            params.temperature = 0.0;
            params.suppress_blank = true;
            params.max_tokens = 0;
            Ok(state
                .transcribe_aligned(model, samples_16k, &params)
                .into_iter()
                .map(|segment| AlignedSegment {
                    start_ms: segment.start_ms,
                    end_ms: segment.end_ms,
                    text: segment.text,
                    words: segment.words.into_iter().map(timing).collect(),
                    tokens: segment.tokens,
                })
                .collect())
        }

        /// Teacher-forced alignment of `tokens` over `samples_16k`.
        pub fn force_align(&mut self, samples_16k: &[f32], tokens: &[i32], language: &str) -> Result<Option<Vec<WordTiming>>, AssetAiError> {
            let (model, state) = self
                .loaded
                .as_mut()
                .ok_or_else(|| AssetAiError::Backend("whisper used before ensure_loaded".into()))?;
            Ok(state.force_align(model, samples_16k, tokens, language).map(|words| words.into_iter().map(timing).collect()))
        }

        pub fn transcribe(&mut self, job: &TranscribeJob) -> Result<Vec<TranscriptSegmentJson>, AssetAiError> {
            let (model, state) = self
                .loaded
                .as_mut()
                .ok_or_else(|| AssetAiError::Backend("whisper used before ensure_loaded".into()))?;
            let mut params = WhisperParams::default();
            params.language = job.language.clone();
            params.prompt = job.prompt.clone();
            params.no_timestamps = false;
            params.single_segment = false;
            params.temperature = 0.0;
            params.suppress_blank = true;
            Ok(state
                .transcribe(model, job.samples_16k, &params)
                .into_iter()
                .map(|s| TranscriptSegmentJson { start_ms: s.start_ms, end_ms: s.end_ms, text: s.text })
                .collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::GenerateParams;
    use crate::protocol::GenerateRequestJson;
    use makepad_micro_serde::DeJson;

    fn params_with_wav(samples: &[f32], rate: u32, language: &str) -> GenerateParams {
        let wav = crate::wav::encode_wav_pcm16_mono(samples, rate);
        let request = GenerateRequestJson {
            model: "whisper-large-v3-turbo".into(),
            input_b64: Some(
                String::from_utf8(makepad_base64::base64_encode(&wav, &makepad_base64::BASE64_STANDARD)).unwrap(),
            ),
            input_content_type: Some("audio/wav".into()),
            language: Some(language.into()),
            ..Default::default()
        };
        GenerateParams::from_request(&request).unwrap()
    }

    #[test]
    fn transcribes_wav_input_to_a_json_transcript() {
        let mut seen_len = 0usize;
        let mut seen_lang = String::new();
        let mut backend = WhisperBackend::with_stub(
            "whisper-large-v3-turbo",
            Box::new(|job| {
                Ok(vec![TranscriptSegmentJson { start_ms: 0, end_ms: 500, text: format!("heard {} samples", job.samples_16k.len()) }])
            }),
        );
        let params = params_with_wav(&vec![0.1; 16_000], 16_000, "en-GB");
        let out = backend
            .generate(&params, &mut |_, _| {}, &CancelToken::new())
            .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].content_type, "application/json");
        let json = TranscriptJson::deserialize_json(std::str::from_utf8(&out[0].bytes).unwrap()).unwrap();
        assert_eq!(json.text, "heard 16000 samples");
        assert_eq!(json.segments[0].end_ms, 500);
        let _ = (&mut seen_len, &mut seen_lang);
    }

    #[test]
    fn resamples_non_16k_input_and_bares_the_language() {
        let mut backend = WhisperBackend::with_stub(
            "whisper-large-v3-turbo",
            Box::new(|job| {
                Ok(vec![TranscriptSegmentJson { start_ms: 0, end_ms: 0, text: format!("{} {}", job.samples_16k.len(), job.language) }])
            }),
        );
        // One second at 48 kHz must arrive as one second at 16 kHz.
        let params = params_with_wav(&vec![0.0; 48_000], 48_000, "pt-BR");
        let out = backend.generate(&params, &mut |_, _| {}, &CancelToken::new()).unwrap();
        let json = TranscriptJson::deserialize_json(std::str::from_utf8(&out[0].bytes).unwrap()).unwrap();
        let (len, lang) = json.text.split_once(' ').unwrap();
        let len: usize = len.parse().unwrap();
        assert!((15_900..=16_100).contains(&len), "{len}");
        assert_eq!(lang, "pt");
    }

    #[test]
    fn alignment_modes_are_named_by_the_domain_and_check_their_windows() {
        assert_eq!(SttMode::of("").unwrap(), SttMode::Transcript);
        assert_eq!(SttMode::of("words").unwrap(), SttMode::Words);
        assert_eq!(SttMode::of("force_align").unwrap(), SttMode::ForceAlign);
        assert!(SttMode::of("lyrics").is_err());
        // The stub recognizer has no decoder to align with: a typed refusal.
        let mut backend = WhisperBackend::with_stub("whisper-large-v3-turbo", Box::new(|_| Ok(Vec::new())));
        let mut params = params_with_wav(&vec![0.0; 16_000], 16_000, "en");
        params.request_domain = "words".into();
        assert!(matches!(backend.generate(&params, &mut |_, _| {}, &CancelToken::new()), Err(AssetAiError::Unavailable(_))));
        // Windows must lie inside the decoded samples.
        let windows = |json: &str| {
            let mut params = params_with_wav(&vec![0.0; 16_000], 16_000, "en");
            params.inputs.push(crate::backend::NamedInput { name: "windows".into(), content_type: "application/json".into(), bytes: json.as_bytes().to_vec() });
            force_windows(&params, 16_000)
        };
        assert_eq!(windows(r#"{"windows":[{"start":0,"end":16000,"lead_pad":800,"tokens":[1,2]}]}"#).unwrap().len(), 1);
        assert!(windows(r#"{"windows":[{"start":0,"end":16001,"lead_pad":0,"tokens":[1]}]}"#).is_err());
        assert!(windows(r#"{"windows":[{"start":10,"end":5,"lead_pad":0,"tokens":[1]}]}"#).is_err());
        assert!(windows(r#"{"windows":[{"start":0,"end":5,"lead_pad":0,"tokens":[]}]}"#).is_err());
    }

    #[test]
    fn refuses_a_request_without_audio() {
        let mut backend = WhisperBackend::with_stub("whisper-large-v3-turbo", Box::new(|_| Ok(Vec::new())));
        let request = GenerateRequestJson { model: "whisper-large-v3-turbo".into(), ..Default::default() };
        let params = GenerateParams::from_request(&request).unwrap();
        assert!(backend.generate(&params, &mut |_, _| {}, &CancelToken::new()).is_err());
    }
}
