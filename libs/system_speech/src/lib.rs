//! The operating system's own speech services, as plain blocking functions.
//!
//! This crate is deliberately the bottom of the stack: no models, no network,
//! no hub, no `Cx`. It answers exactly two questions per platform — "can this
//! OS turn speech into text / text into speech, and how?" — and hands back
//! plain data. `makepad-ai-hub` wraps it as the `stt.system` / `tts.system`
//! pipes, next to the in-repo Whisper and Kokoro engines, so an app never
//! calls this crate directly; it asks the hub.
//!
//! | platform | STT                                | TTS                              |
//! |----------|------------------------------------|----------------------------------|
//! | macOS/iOS| `SpeechAnalyzer` (PCM in)          | `AVSpeechSynthesizer` → PCM      |
//! | Windows  | `Windows.Media.SpeechRecognition`  | `Windows.Media.SpeechSynthesis`  |
//! | Android  | `android.speech.SpeechRecognizer`  | `android.speech.tts.TextToSpeech`|
//! | Linux    | none                               | `espeak-ng` when installed       |
//!
//! **Two STT shapes, honestly modelled.** Apple's recognizer takes PCM the
//! caller recorded ([`stt::transcribe`]). Android (at API 26) and Windows
//! recognizers only listen to the microphone themselves ([`stt::listen`]).
//! [`stt::capabilities`] says which a platform offers; a caller adapts rather
//! than the crate pretending.
//!
//! **Threading.** Every function blocks until done and is meant to be called
//! from a worker thread, never the UI thread: the Android bridge parks the
//! calling thread on a latch while the platform does its work on the main
//! looper, and Apple's synthesizer delivers its audio through the process's
//! MAIN run loop. UI apps always pump one; a headless tool that calls TTS
//! from a worker must keep its main thread in `CFRunLoopRun` (the hub's
//! `speech-roundtrip` bin shows the pattern).

pub mod wav;
mod platform;

use std::sync::mpsc::Sender;

// ------------------------------------------------------------------ common

/// Mono PCM at the producing engine's native rate.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpeechAudio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

impl SpeechAudio {
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn duration_secs(&self) -> f32 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        self.samples.len() as f32 / self.sample_rate as f32
    }

    /// Linear resample to `target` Hz. Good enough for STT input; playback
    /// paths should use their own higher-quality resampler.
    pub fn resampled(&self, target: u32) -> SpeechAudio {
        if self.sample_rate == 0 || target == 0 || self.samples.is_empty() {
            return SpeechAudio { samples: Vec::new(), sample_rate: target };
        }
        if self.sample_rate == target {
            return self.clone();
        }
        let ratio = self.sample_rate as f64 / target as f64;
        let out_len = ((self.samples.len() as f64) / ratio).floor() as usize;
        let mut out = Vec::with_capacity(out_len);
        for i in 0..out_len {
            let pos = i as f64 * ratio;
            let left = pos.floor() as usize;
            let frac = (pos - left as f64) as f32;
            let a = self.samples[left];
            let b = *self.samples.get(left + 1).unwrap_or(&a);
            out.push(a + (b - a) * frac);
        }
        SpeechAudio { samples: out, sample_rate: target }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SpeechError {
    /// Not on this platform, the bridge was not built, or the OS service is
    /// missing (no recognition service installed, no `espeak-ng` binary, ...).
    Unavailable(String),
    /// The OS refused: microphone / speech-recognition permission not granted.
    PermissionDenied,
    /// The engine exists but lacks this capability (e.g. PCM input on Android).
    Unsupported(&'static str),
    /// The engine ran and produced nothing.
    Empty,
    Cancelled,
    Timeout,
    Backend(String),
}

impl std::fmt::Display for SpeechError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpeechError::Unavailable(why) => write!(f, "unavailable: {why}"),
            SpeechError::PermissionDenied => f.write_str("permission denied"),
            SpeechError::Unsupported(what) => write!(f, "unsupported: {what}"),
            SpeechError::Empty => f.write_str("engine produced nothing"),
            SpeechError::Cancelled => f.write_str("cancelled"),
            SpeechError::Timeout => f.write_str("timed out"),
            SpeechError::Backend(why) => write!(f, "backend: {why}"),
        }
    }
}

impl std::error::Error for SpeechError {}

// --------------------------------------------------------------------- STT

/// The rate [`stt::transcribe`] expects its PCM at.
pub const STT_SAMPLE_RATE: u32 = 16_000;

/// A transcribed span. Engines without timing report `0..0`.
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Transcript {
    pub segments: Vec<Segment>,
}

impl Transcript {
    pub fn from_text(text: impl Into<String>) -> Self {
        let text: String = text.into();
        if text.trim().is_empty() {
            return Self::default();
        }
        Self { segments: vec![Segment { start_ms: 0, end_ms: 0, text }] }
    }

    pub fn is_empty(&self) -> bool {
        self.segments.iter().all(|s| s.text.trim().is_empty())
    }

    /// All segment text joined with single spaces.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for segment in &self.segments {
            let trimmed = segment.text.trim();
            if trimmed.is_empty() {
                continue;
            }
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(trimmed);
        }
        out
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SttOptions {
    /// ISO 639-1 (`"en"`) or a BCP-47 tag (`"en-US"`). Bare codes map to the
    /// platform's default region.
    pub language: String,
    /// `listen` only: emit [`SttEvent::Partial`] while the utterance is still
    /// being spoken, when the engine can.
    pub partial_results: bool,
    /// Prefer on-device recognition over a cloud-backed one when the engine
    /// offers the choice (Android). Never forces: an engine without an
    /// offline model still recognizes.
    pub prefer_offline: bool,
    /// `transcribe` only: ask for per-segment timing when the engine has it.
    pub timestamps: bool,
}

impl Default for SttOptions {
    fn default() -> Self {
        Self {
            language: "en".to_string(),
            partial_results: true,
            prefer_offline: true,
            timestamps: true,
        }
    }
}

/// What this platform's recognizer can do. Both input shapes may be false
/// (no recognizer at all) or true (Apple: PCM today; a mic session is a
/// natural extension).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SttCapabilities {
    /// [`stt::transcribe`] works: caller-recorded 16 kHz mono PCM in.
    pub pcm_input: bool,
    /// [`stt::listen`] works: the engine owns the microphone.
    pub engine_mic: bool,
    /// `listen` can emit partial hypotheses.
    pub partial_results: bool,
    /// Recognition can run without network.
    pub offline: bool,
}

/// Events from a [`stt::listen`] session, in order. `Ended` is always last;
/// after it (or `Error`) nothing else arrives and the handle is spent.
#[derive(Clone, Debug, PartialEq)]
pub enum SttEvent {
    /// Input level, 0..1, when the engine reports one (Android `onRmsChanged`).
    Level(f32),
    /// Running hypothesis for the utterance in progress; replaces the last.
    Partial(String),
    /// A finished utterance.
    Final(Transcript),
    Error(SpeechError),
    Ended,
}

/// A running microphone session. Dropping it stops listening.
pub struct ListenHandle {
    stop: Option<Box<dyn FnOnce() + Send>>,
}

impl ListenHandle {
    pub fn new(stop: impl FnOnce() + Send + 'static) -> Self {
        Self { stop: Some(Box::new(stop)) }
    }

    /// Stop listening. The engine still delivers any final result it has,
    /// then `Ended`.
    pub fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            stop();
        }
    }
}

impl Drop for ListenHandle {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            stop();
        }
    }
}

pub mod stt {
    //! Speech to text through the OS engine.
    use super::*;

    /// A short stable name for logs and pipe adverts, e.g. `"apple-speechanalyzer"`.
    pub fn engine_name() -> &'static str {
        platform::STT_ENGINE
    }

    /// True when at least one of the two input shapes works here.
    pub fn available() -> bool {
        platform::stt_available()
    }

    pub fn capabilities() -> SttCapabilities {
        platform::stt_capabilities()
    }

    /// Get the engine ready for `language`: download an on-device model,
    /// warm the recognizer, surface a permission problem early. Optional;
    /// `transcribe`/`listen` do it lazily.
    pub fn prepare(language: &str) -> Result<(), SpeechError> {
        platform::stt_prepare(language)
    }

    /// Recognize caller-recorded PCM: mono `f32` at [`STT_SAMPLE_RATE`].
    /// Blocks until the whole buffer is processed.
    pub fn transcribe(samples_16k: &[f32], options: &SttOptions) -> Result<Transcript, SpeechError> {
        if samples_16k.is_empty() {
            return Ok(Transcript::default());
        }
        platform::stt_transcribe(samples_16k, options)
    }

    /// Let the engine listen on the microphone itself, streaming events into
    /// `sink` until the utterance ends or the handle is stopped/dropped. The
    /// caller must already hold microphone permission.
    pub fn listen(options: &SttOptions, sink: Sender<SttEvent>) -> Result<ListenHandle, SpeechError> {
        platform::stt_listen(options, sink)
    }
}

// --------------------------------------------------------------------- TTS

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceGender {
    Unknown,
    Female,
    Male,
}

/// One installed voice. `id` is what [`TtsOptions::voice`] takes and is
/// stable per platform (Apple identifier, WinRT `VoiceInformation.Id`,
/// Android `Voice.getName()`, espeak voice name).
#[derive(Clone, Debug, PartialEq)]
pub struct Voice {
    pub id: String,
    pub name: String,
    /// BCP-47, e.g. `"en-GB"`.
    pub language: String,
    pub gender: VoiceGender,
    /// Renders without network.
    pub offline: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TtsOptions {
    /// A [`Voice::id`]; `None` picks the platform default for `language`.
    pub voice: Option<String>,
    /// BCP-47 or bare code, used when `voice` is `None`.
    pub language: String,
    /// 1.0 = the platform's normal speaking rate; 0.5 half, 2.0 double.
    pub rate: f32,
    /// 1.0 = normal pitch. Engines without pitch control ignore it.
    pub pitch: f32,
}

impl Default for TtsOptions {
    fn default() -> Self {
        Self { voice: None, language: "en".to_string(), rate: 1.0, pitch: 1.0 }
    }
}

pub mod tts {
    //! Text to speech through the OS engine. Always PCM out: the caller owns
    //! the audio device (Makepad plays through `cx.audio_output`), so muting,
    //! mixing and cancellation stay in the app.
    use super::*;

    /// A short stable name for logs and pipe adverts, e.g. `"apple-avspeech"`.
    pub fn engine_name() -> &'static str {
        platform::TTS_ENGINE
    }

    pub fn available() -> bool {
        platform::tts_available()
    }

    /// Installed voices. Empty when unavailable.
    pub fn voices() -> Vec<Voice> {
        platform::tts_voices()
    }

    /// Render `text` to mono PCM at the engine's native rate. Blocks until the
    /// whole utterance is rendered; long texts are the caller's to split.
    pub fn synthesize(text: &str, options: &TtsOptions) -> Result<SpeechAudio, SpeechError> {
        if text.trim().is_empty() {
            return Err(SpeechError::Empty);
        }
        platform::tts_synthesize(text, options)
    }
}

/// Split a bare ISO 639-1 code into the platform default region, or pass a
/// BCP-47 tag through. Shared by the platform modules.
pub(crate) fn bcp47(language: &str) -> String {
    let language = language.trim();
    if language.contains('-') || language.contains('_') {
        return language.replace('_', "-");
    }
    let region = match language.to_ascii_lowercase().as_str() {
        "en" => "en-US",
        "fr" => "fr-FR",
        "de" => "de-DE",
        "es" => "es-ES",
        "it" => "it-IT",
        "pt" => "pt-BR",
        "nl" => "nl-NL",
        "zh" => "zh-CN",
        "ja" => "ja-JP",
        "ko" => "ko-KR",
        "ru" => "ru-RU",
        "yue" => "yue-CN",
        _ => return language.to_string(),
    };
    region.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_text_joins_trimmed_segments() {
        let t = Transcript {
            segments: vec![
                Segment { start_ms: 0, end_ms: 10, text: "  hello ".into() },
                Segment { start_ms: 10, end_ms: 20, text: "".into() },
                Segment { start_ms: 20, end_ms: 30, text: "world".into() },
            ],
        };
        assert_eq!(t.text(), "hello world");
        assert!(!t.is_empty());
        assert!(Transcript::from_text("   ").is_empty());
    }

    #[test]
    fn bare_language_codes_get_a_region() {
        assert_eq!(bcp47("en"), "en-US");
        assert_eq!(bcp47("en-GB"), "en-GB");
        assert_eq!(bcp47("pt_BR"), "pt-BR");
        assert_eq!(bcp47("xx"), "xx");
    }

    #[test]
    fn resample_halves_length_at_half_rate() {
        let audio = SpeechAudio { samples: vec![0.0; 1000], sample_rate: 32_000 };
        assert_eq!(audio.resampled(16_000).samples.len(), 500);
        assert_eq!(audio.resampled(32_000).samples.len(), 1000);
    }

    #[test]
    fn the_platform_answers_without_panicking() {
        // Availability probes must be safe to call anywhere, any platform.
        let _ = stt::available();
        let _ = tts::available();
        let _ = stt::capabilities();
        let _ = stt::engine_name();
        let _ = tts::engine_name();
    }
}
