//! Speech in the hub: speech-to-text and text-to-speech as sessions, the same
//! shape as [`crate::hub_chat::HubChatSession`] — poll-driven, headless, one
//! worker thread, events out, a wake hook for sleeping UIs.
//!
//! An app asks for a session and gets *a* recognizer or *a* voice; which one
//! is the hub's decision, made once at start and reported in `Ready`:
//!
//! ```text
//!   stt.whisper   in-process (weights here, machine election)  ─┐
//!   stt.whisper   machine node over loopback                    ├─ Auto ladder,
//!   stt.whisper   LAN node (a Mac serving a Quest)              │  best first
//!   stt.system    the OS recognizer (makepad-system-speech)    ─┘
//! ```
//!
//! and the mirror image for `tts.kokoro` / `tts.system`. [`SpeechReach`] is
//! the "don't reach out" knob: `Local` never touches a socket, `Machine` uses
//! loopback only, `Lan` uses everything. The API is identical either way.
//!
//! **Two STT input shapes.** Whisper and the Apple recognizer take PCM the
//! app recorded ([`SttSession::transcribe`], fed by the app's own VAD
//! pipeline). Android and Windows recognizers only listen to the microphone
//! themselves ([`SttSession::listen`]). `Ready` carries the capabilities so
//! the app picks the shape its engine supports instead of guessing.
//!
//! Audio always comes back as PCM: the app owns the device.

mod remote;
mod stt_worker;
mod tts_worker;
pub mod weights;

pub use makepad_system_speech::{Segment, SpeechAudio, SttCapabilities, Transcript, Voice, VoiceGender};

use crate::pipe::PipeId;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;

/// Called after every event so a sleeping consumer wakes up (UI apps pass
/// their platform signal). The same type as the chat session's hook.
pub type WakeHook = Arc<dyn Fn() + Send + Sync>;

/// The rate [`SttSession::transcribe`] expects: mono f32 at 16 kHz.
pub const STT_SAMPLE_RATE: u32 = 16_000;

/// How far a session may look for an engine. Ordered: each level includes
/// the ones below it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SpeechReach {
    /// This process only: in-process engines and the OS engine. No sockets.
    Local,
    /// Plus the other nodes on this machine, over loopback.
    Machine,
    /// Plus dedicated nodes on the LAN.
    Lan,
}

impl Default for SpeechReach {
    fn default() -> Self {
        SpeechReach::Lan
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SttEngine {
    /// Whisper wherever it is (in-process, machine, LAN), else the OS engine.
    #[default]
    Auto,
    /// The OS recognizer only.
    System,
    /// Whisper only; fails when none is in reach.
    Whisper,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TtsEngine {
    /// Kokoro wherever it is, else the OS voice.
    #[default]
    Auto,
    /// The OS synthesizer only.
    System,
    /// Kokoro only; fails when none is in reach.
    Kokoro,
}

#[derive(Clone)]
pub struct SttConfig {
    pub engine: SttEngine,
    pub reach: SpeechReach,
    /// ISO 639-1 (`"en"`) or BCP-47; engines take what they need from it.
    pub language: String,
    /// `listen` mode: emit partial hypotheses when the engine can.
    pub partial_results: bool,
    /// Prefer on-device recognition when the OS engine offers the choice.
    pub prefer_offline: bool,
    /// Ask for per-segment timing.
    pub timestamps: bool,
    /// Whisper: decode the whole buffer as one segment (live dictation).
    pub single_segment: bool,
    /// Whisper: cap tokens per utterance; 0 = the model's default.
    pub max_tokens: usize,
    /// Whisper: no-speech probability above which a chunk is treated as
    /// silence. `None` = the engine default.
    pub silence_threshold: Option<f32>,
    pub wake: Option<WakeHook>,
}

impl Default for SttConfig {
    fn default() -> Self {
        Self {
            engine: SttEngine::Auto,
            reach: SpeechReach::Lan,
            language: "en".to_string(),
            partial_results: true,
            prefer_offline: true,
            timestamps: true,
            single_segment: false,
            max_tokens: 0,
            silence_threshold: None,
            wake: None,
        }
    }
}

impl SttConfig {
    /// The settings the Window voice input has always used for short,
    /// VAD-gated utterances: one segment, few tokens, a stricter silence gate.
    pub fn live_dictation() -> Self {
        Self {
            timestamps: false,
            single_segment: true,
            max_tokens: 48,
            silence_threshold: Some(0.65),
            ..Self::default()
        }
    }

    /// In-process Whisper only: no LAN, no OS recognizer. The window voice
    /// button and search-field mics use this so a machine with hub STT
    /// weights talks to Whisper here, on the default microphone.
    pub fn local_whisper() -> Self {
        Self {
            engine: SttEngine::Whisper,
            reach: SpeechReach::Local,
            ..Self::live_dictation()
        }
    }
}

#[derive(Clone)]
pub struct TtsConfig {
    pub engine: TtsEngine,
    pub reach: SpeechReach,
    /// A [`Voice::id`] from `Ready`'s voice list; `None` = the engine's
    /// default for `language` (Kokoro: `MAKEPAD_TTS_VOICE` or `bm_daniel`).
    pub voice: Option<String>,
    pub language: String,
    /// 1.0 = normal speaking rate.
    pub rate: f32,
    /// 1.0 = normal pitch; engines without pitch control ignore it.
    pub pitch: f32,
    pub wake: Option<WakeHook>,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            engine: TtsEngine::Auto,
            reach: SpeechReach::Lan,
            voice: None,
            language: "en".to_string(),
            rate: 1.0,
            pitch: 1.0,
            wake: None,
        }
    }
}

/// Which recognizer a session ended up with.
#[derive(Clone, Debug, PartialEq)]
pub struct SttEngineInfo {
    /// `stt.whisper` or `stt.system`.
    pub pipe: PipeId,
    /// A human-readable engine name: `"whisper (ggml-large-v3-turbo.bin)"`,
    /// `"apple-speechanalyzer"`, …
    pub engine: String,
    /// The node serving it when it is not this process.
    pub remote: Option<String>,
    pub capabilities: SttCapabilities,
}

/// Which voice a session ended up with.
#[derive(Clone, Debug, PartialEq)]
pub struct TtsEngineInfo {
    /// `tts.kokoro` or `tts.system`.
    pub pipe: PipeId,
    pub engine: String,
    pub remote: Option<String>,
    /// Sample rate the engine renders at.
    pub sample_rate: u32,
    /// The voices this engine offers; ids go into [`TtsConfig::voice`].
    pub voices: Vec<Voice>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SttEvent {
    /// Engine selection / weight load progress.
    Loading { phase: String, fraction: f64 },
    /// The engine is chosen and ready. Exactly once, before any result.
    Ready(SttEngineInfo),
    /// No engine could be had. Nothing else will ever arrive.
    Failed(String),
    /// `listen` mode: input level 0..1, when the engine reports one.
    Level(f32),
    /// `listen` mode: running hypothesis, replacing the previous one.
    Partial(String),
    /// A finished utterance: the id [`SttSession::transcribe`] returned, or
    /// 0 for utterances the engine's own microphone session produced.
    Final { utterance: u64, transcript: Transcript, secs: f64 },
    /// One utterance failed; the session goes on.
    Error { utterance: Option<u64>, message: String },
    /// The engine's microphone session ended (stopped, or the engine decided
    /// the utterance was over). `listen` again to start another.
    ListenEnded,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TtsEvent {
    Loading { phase: String, fraction: f64 },
    Ready(TtsEngineInfo),
    Failed(String),
    /// The rendered speech for one `say`. Mono PCM at the engine's rate.
    Audio { utterance: u64, audio: SpeechAudio, secs: f64 },
    Error { utterance: u64, message: String },
}

pub(crate) enum SttMsg {
    Transcribe { utterance: u64, generation: u64, samples: Vec<f32> },
    Listen,
    StopListening,
}

pub(crate) enum TtsMsg {
    Say { utterance: u64, generation: u64, text: String },
}

/// A running speech-to-text session. Dropping it ends the worker.
///
/// One thread polls it; a consumer that wants to submit from one thread and
/// drain events on another takes it apart with [`SttSession::split`].
pub struct SttSession {
    handle: SttHandle,
    events: SttEvents,
}

/// The submitting half of a session: `Send + Sync`, clone-free by design (one
/// owner decides what the recognizer hears).
pub struct SttHandle {
    to_worker: Sender<SttMsg>,
    next_utterance: AtomicU64,
    generation: Arc<AtomicU64>,
}

/// The receiving half: the worker's events, in order.
pub struct SttEvents {
    from_worker: Receiver<SttEvent>,
}

impl SttSession {
    /// Pick an engine and get it ready, on a worker. Nothing blocks; progress
    /// and the outcome arrive through [`SttSession::poll`].
    pub fn start(config: SttConfig) -> Self {
        let (event_tx, from_worker) = channel();
        let (to_worker, msg_rx) = channel();
        let generation = Arc::new(AtomicU64::new(0));
        let worker_generation = generation.clone();
        thread::Builder::new()
            .name("ai-hub-stt".into())
            .spawn(move || stt_worker::run(config, msg_rx, event_tx, worker_generation))
            .expect("spawn stt worker");
        Self {
            handle: SttHandle { to_worker, next_utterance: AtomicU64::new(1), generation },
            events: SttEvents { from_worker },
        }
    }

    /// Take the session apart: submit from one thread, drain on another.
    pub fn split(self) -> (SttHandle, SttEvents) {
        (self.handle, self.events)
    }

    /// See [`SttHandle::transcribe`].
    pub fn transcribe(&self, samples_16k: Vec<f32>) -> u64 {
        self.handle.transcribe(samples_16k)
    }

    /// See [`SttHandle::listen`].
    pub fn listen(&self) {
        self.handle.listen()
    }

    pub fn stop_listening(&self) {
        self.handle.stop_listening()
    }

    /// See [`SttHandle::cancel`].
    pub fn cancel(&self) {
        self.handle.cancel()
    }

    pub fn poll(&self) -> Vec<SttEvent> {
        self.events.poll()
    }

    pub fn recv(&self) -> Option<SttEvent> {
        self.events.recv()
    }

    pub fn recv_timeout(&self, timeout: std::time::Duration) -> Option<SttEvent> {
        self.events.recv_timeout(timeout)
    }
}

impl SttHandle {
    /// Queue caller-recorded PCM (mono f32 at [`STT_SAMPLE_RATE`]) for
    /// recognition. Returns the utterance id its `Final`/`Error` will carry.
    pub fn transcribe(&self, samples_16k: Vec<f32>) -> u64 {
        let utterance = self.next_utterance.fetch_add(1, Ordering::Relaxed);
        let _ = self.to_worker.send(SttMsg::Transcribe {
            utterance,
            generation: self.generation.load(Ordering::Relaxed),
            samples: samples_16k,
        });
        utterance
    }

    /// Let the engine listen on the microphone itself (engines whose
    /// capabilities say `engine_mic`). Results arrive as `Partial`/`Final`
    /// with utterance id 0, then `ListenEnded`.
    pub fn listen(&self) {
        let _ = self.to_worker.send(SttMsg::Listen);
    }

    pub fn stop_listening(&self) {
        let _ = self.to_worker.send(SttMsg::StopListening);
    }

    /// Drop every queued utterance; one already being recognized is
    /// discarded when it finishes instead of being reported.
    pub fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }
}

impl SttEvents {
    pub fn poll(&self) -> Vec<SttEvent> {
        self.from_worker.try_iter().collect()
    }

    /// Block until the next event, or `None` once the worker is gone. For
    /// headless consumers with a thread to spare; UIs use `poll` + `wake`.
    pub fn recv(&self) -> Option<SttEvent> {
        self.from_worker.recv().ok()
    }

    pub fn recv_timeout(&self, timeout: std::time::Duration) -> Option<SttEvent> {
        self.from_worker.recv_timeout(timeout).ok()
    }
}

/// A running text-to-speech session. Dropping it ends the worker. Take it
/// apart with [`TtsSession::split`] to speak from one thread and play from
/// another.
pub struct TtsSession {
    handle: TtsHandle,
    events: TtsEvents,
}

/// The speaking half of a session (`Send + Sync`).
pub struct TtsHandle {
    to_worker: Sender<TtsMsg>,
    next_utterance: AtomicU64,
    generation: Arc<AtomicU64>,
}

/// The receiving half: rendered audio and everything else, in order.
pub struct TtsEvents {
    from_worker: Receiver<TtsEvent>,
}

impl TtsSession {
    pub fn start(config: TtsConfig) -> Self {
        let (event_tx, from_worker) = channel();
        let (to_worker, msg_rx) = channel();
        let generation = Arc::new(AtomicU64::new(0));
        let worker_generation = generation.clone();
        thread::Builder::new()
            .name("ai-hub-tts".into())
            .spawn(move || tts_worker::run(config, msg_rx, event_tx, worker_generation))
            .expect("spawn tts worker");
        Self {
            handle: TtsHandle { to_worker, next_utterance: AtomicU64::new(1), generation },
            events: TtsEvents { from_worker },
        }
    }

    /// Take the session apart: speak from one thread, play from another.
    pub fn split(self) -> (TtsHandle, TtsEvents) {
        (self.handle, self.events)
    }

    /// See [`TtsHandle::say`].
    pub fn say(&self, text: impl Into<String>) -> u64 {
        self.handle.say(text)
    }

    /// See [`TtsHandle::cancel`].
    pub fn cancel(&self) {
        self.handle.cancel()
    }

    pub fn poll(&self) -> Vec<TtsEvent> {
        self.events.poll()
    }

    pub fn recv(&self) -> Option<TtsEvent> {
        self.events.recv()
    }

    pub fn recv_timeout(&self, timeout: std::time::Duration) -> Option<TtsEvent> {
        self.events.recv_timeout(timeout)
    }
}

impl TtsHandle {
    /// Queue text to render. Returns the utterance id its `Audio`/`Error`
    /// will carry. Utterances render in order.
    pub fn say(&self, text: impl Into<String>) -> u64 {
        let utterance = self.next_utterance.fetch_add(1, Ordering::Relaxed);
        let _ = self.to_worker.send(TtsMsg::Say {
            utterance,
            generation: self.generation.load(Ordering::Relaxed),
            text: text.into(),
        });
        utterance
    }

    /// Drop every queued utterance; one being rendered is discarded when it
    /// finishes. Playback of audio already delivered is the app's to stop.
    pub fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }
}

impl TtsEvents {
    pub fn poll(&self) -> Vec<TtsEvent> {
        self.from_worker.try_iter().collect()
    }

    /// Block until the next event, or `None` once the worker is gone.
    pub fn recv(&self) -> Option<TtsEvent> {
        self.from_worker.recv().ok()
    }

    pub fn recv_timeout(&self, timeout: std::time::Duration) -> Option<TtsEvent> {
        self.from_worker.recv_timeout(timeout).ok()
    }
}

/// True when this build and platform may load a heavy speech model into this
/// process for `Auto`. Phones and headsets keep 1.6 GB Whisper / 327 MB
/// Kokoro off-device (their OS engine or a LAN node serve them) unless the
/// `MAKEPAD` config asks for it by name (`MAKEPAD=whisper`, `MAKEPAD=kokoro`).
#[cfg(any(feature = "stt", feature = "tts"))]
pub(crate) fn in_process_allowed(engine: &str) -> bool {
    if !cfg!(any(target_os = "ios", target_os = "android")) {
        return true;
    }
    std::env::var("MAKEPAD").is_ok_and(|configs| {
        configs.split(['+', ',']).any(|config| config.eq_ignore_ascii_case(engine))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reach_is_ordered_inclusively() {
        assert!(SpeechReach::Local < SpeechReach::Machine);
        assert!(SpeechReach::Machine < SpeechReach::Lan);
        assert_eq!(SpeechReach::default(), SpeechReach::Lan);
    }

    #[test]
    fn live_dictation_keeps_the_window_voice_defaults() {
        let cfg = SttConfig::live_dictation();
        assert!(cfg.single_segment);
        assert_eq!(cfg.max_tokens, 48);
        assert_eq!(cfg.silence_threshold, Some(0.65));
        assert!(!cfg.timestamps);
    }

    #[test]
    fn local_whisper_stays_in_process() {
        let cfg = SttConfig::local_whisper();
        assert_eq!(cfg.engine, SttEngine::Whisper);
        assert_eq!(cfg.reach, SpeechReach::Local);
        assert!(cfg.single_segment);
    }

    #[test]
    fn utterance_ids_count_from_one_and_cancel_bumps_generation() {
        // A session whose worker fails fast (no engines in a Local reach on
        // a machine without weights) still hands out ids and generations.
        let session = TtsSession::start(TtsConfig {
            engine: TtsEngine::Kokoro,
            reach: SpeechReach::Local,
            ..TtsConfig::default()
        });
        assert_eq!(session.say("a"), 1);
        assert_eq!(session.say("b"), 2);
        session.cancel();
        assert_eq!(session.handle.generation.load(Ordering::Relaxed), 1);
    }
}
