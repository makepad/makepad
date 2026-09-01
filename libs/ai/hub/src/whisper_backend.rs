//! The `whisper` backend: speech-to-text (the `stt` domain) through the
//! in-repo Whisper port — the wire side of the `stt.whisper` pipe a machine
//! node or LAN box publishes. This file only wraps the engine.
//!
//! Request: `{model: "whisper-large-v3-turbo", input_b64: <wav>,
//! input_content_type: "audio/wav", language: "en"}` -> one
//! `application/json` artifact, a [`TranscriptJson`]: the segments with
//! millisecond timing and the joined text. Any WAV the in-repo decoder reads
//! is accepted; it is downmixed and resampled to 16 kHz here, so a client
//! never has to.

use crate::backend::{ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, ProgressSink};
use crate::error::AssetAiError;
use crate::protocol::{TranscriptJson, TranscriptSegmentJson};
use makepad_micro_serde::SerJson;

const WHISPER_SAMPLE_RATE: u32 = 16_000;

/// One utterance handed to the engine: 16 kHz mono PCM plus the language.
pub struct TranscribeJob<'a> {
    pub samples_16k: &'a [f32],
    pub language: String,
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

/// Decode the request's audio to 16 kHz mono.
fn decode_input(params: &GenerateParams) -> Result<Vec<f32>, AssetAiError> {
    if params.input_bytes.is_empty() {
        return Err(AssetAiError::Params("speech recognition needs `input_b64` audio (audio/wav)".into()));
    }
    let content_type = params.input_content_type.to_ascii_lowercase();
    if !(content_type.contains("wav") || content_type.contains("wave") || content_type == "application/octet-stream") {
        return Err(AssetAiError::Params(format!(
            "input_content_type {:?}: whisper takes audio/wav",
            params.input_content_type
        )));
    }
    let (samples, rate) = crate::wav::decode_wav_to_mono_f32(&params.input_bytes)
        .map_err(|e| AssetAiError::Params(format!("input wav: {e}")))?;
    Ok(if rate == WHISPER_SAMPLE_RATE {
        samples
    } else {
        crate::resample::resample_channel(&samples, rate, WHISPER_SAMPLE_RATE)
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
        let job = TranscribeJob { samples_16k: &samples, language };
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
        let json = TranscriptJson { text, segments }.serialize_json();
        progress("done", 1.0);
        Ok(vec![ArtifactData { content_type: "application/json", ext: "json", bytes: json.into_bytes() }])
    }
}

#[cfg(feature = "stt")]
mod whisper_engine {
    use super::{TranscribeJob, TranscriptSegmentJson};
    use crate::backend::BackendCtx;
    use crate::error::AssetAiError;
    use makepad_ai_speech::whisper::{WhisperModel, WhisperParams, WhisperState};
    use std::path::PathBuf;

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

        pub fn unload(&mut self) {
            self.loaded = None;
        }

        pub fn transcribe(&mut self, job: &TranscribeJob) -> Result<Vec<TranscriptSegmentJson>, AssetAiError> {
            let (model, state) = self
                .loaded
                .as_mut()
                .ok_or_else(|| AssetAiError::Backend("whisper used before ensure_loaded".into()))?;
            let mut params = WhisperParams::default();
            params.language = job.language.clone();
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
    fn refuses_a_request_without_audio() {
        let mut backend = WhisperBackend::with_stub("whisper-large-v3-turbo", Box::new(|_| Ok(Vec::new())));
        let request = GenerateRequestJson { model: "whisper-large-v3-turbo".into(), ..Default::default() };
        let params = GenerateParams::from_request(&request).unwrap();
        assert!(backend.generate(&params, &mut |_, _| {}, &CancelToken::new()).is_err());
    }
}
