//! Speech output: a hub TTS session plus the playback buffer it fills.
//!
//! The hub hands back PCM rather than owning a device, so playback goes
//! through `cx.audio_output` like any other audio in Makepad. Muting is then
//! just "stop feeding the buffer", which also makes it instant.
//!
//! Streamed reply text goes in through [`SpeechOutput::feed`]; each finished
//! sentence is queued on the session and spoken while the rest of the reply
//! is still being generated. Which voice speaks — Kokoro in this process, on
//! the machine node, on a LAN box, or the OS voice — is the hub's decision
//! ([`makepad_ai_hub::speech`]); this file only plays what comes back.

#[cfg(feature = "tts")]
use makepad_ai_hub::speech::{TtsConfig, TtsEvent, TtsHandle, TtsSession};
use makepad_widgets::makepad_draw::audio::AudioBuffer;
#[cfg(feature = "tts")]
use makepad_widgets::log;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// The buffer the audio callback plays from. Written by the pump thread,
/// read by the audio thread.
#[derive(Default)]
pub struct Playback {
    pub samples: Vec<f32>,
    pub cursor: f64,
    pub source_rate: f64,
}

impl Playback {
    /// Mix the queued speech additively into a device buffer, resampling on
    /// the fly. Call from the audio callback under the playback lock.
    pub fn mix_into(&mut self, output: &mut AudioBuffer, device_rate: f64) {
        if self.samples.is_empty() || self.source_rate <= 0.0 || device_rate <= 0.0 {
            return;
        }
        let step = self.source_rate / device_rate;
        let channels = output.channel_count();
        for frame in 0..output.frame_count() {
            let index = self.cursor as usize;
            if index + 1 >= self.samples.len() {
                self.samples.clear();
                self.cursor = 0.0;
                break;
            }
            let fraction = (self.cursor - index as f64) as f32;
            let a = self.samples[index];
            let b = self.samples[index + 1];
            let sample = a + (b - a) * fraction;
            for channel in 0..channels {
                output.channel_mut(channel)[frame] += sample;
            }
            self.cursor += step;
        }
        // Sentences append while earlier ones play, so drop the consumed
        // prefix periodically or the buffer grows for the whole reply.
        if self.cursor > 2.0 * self.source_rate {
            let consumed = self.cursor as usize;
            self.samples.drain(..consumed);
            self.cursor -= consumed as f64;
        }
    }

    fn clear(&mut self) {
        self.samples.clear();
        self.cursor = 0.0;
    }
}

/// Don't speak a fragment shorter than this — one-word clips sound like hiccups.
const MIN_SPOKEN_CHARS: usize = 16;

/// Speech output: a hub TTS session plus the buffer it fills.
pub struct SpeechOutput {
    /// Kokoro voice pack name (or any [`makepad_ai_hub::speech::Voice`] id).
    voice: String,
    /// Started on the FIRST utterance, not at construction: Kokoro is ~327 MB
    /// resident and an app that never speaks (text tier, muted, a session
    /// where nobody triggers a reply) must not pay for it.
    #[cfg(feature = "tts")]
    session: OnceLock<TtsHandle>,
    playback: Arc<Mutex<Playback>>,
    muted: Arc<AtomicBool>,
    /// Streamed reply text not yet spoken.
    pending: String,
    /// "Shh" latch: swallow the rest of the current reply (see `hush`).
    hushed: bool,
}

impl SpeechOutput {
    /// Create the output with a named voice (e.g. `"bm_fable"`; a trailing
    /// `.mkvoice` is tolerated). Nothing loads until something is said.
    pub fn new(voice: &str) -> Self {
        Self {
            voice: voice.strip_suffix(".mkvoice").unwrap_or(voice).to_string(),
            #[cfg(feature = "tts")]
            session: OnceLock::new(),
            playback: Arc::new(Mutex::new(Playback::default())),
            muted: Arc::new(AtomicBool::new(false)),
            pending: String::new(),
            hushed: false,
        }
    }

    /// The shared playback buffer, for apps that mix speech into their own
    /// audio callback via [`Playback::mix_into`].
    pub fn playback(&self) -> Arc<Mutex<Playback>> {
        self.playback.clone()
    }

    /// The shared mute flag, for audio callbacks that check it directly.
    pub fn muted_flag(&self) -> Arc<AtomicBool> {
        self.muted.clone()
    }

    /// True while synthesized audio is still queued or playing — apps use it
    /// to drop mic transcripts of the assistant's own voice.
    pub fn is_speaking(&self) -> bool {
        self.playback.lock().map(|p| !p.samples.is_empty()).unwrap_or(false)
    }

    /// Convenience for apps with no other audio: install an audio-output
    /// callback that plays speech and nothing else.
    pub fn install_audio_output(&self, cx: &mut makepad_widgets::Cx, index: usize) {
        use makepad_widgets::CxMediaApi;
        let playback = self.playback.clone();
        let muted = self.muted.clone();
        cx.audio_output(index, move |info, output| {
            output.zero();
            if muted.load(Ordering::Relaxed) {
                return;
            }
            if let Ok(mut playback) = playback.lock() {
                playback.mix_into(output, info.sample_rate);
            }
        });
    }

    /// Feed streamed reply text. Each finished sentence is spoken as soon as
    /// it lands, so the voice keeps pace with generation instead of waiting
    /// for it.
    pub fn feed(&mut self, delta: &str) {
        self.pending.push_str(delta);
        // An odd number of fences means we are inside a code block: wait it
        // out rather than reading code aloud a sentence at a time.
        if self.pending.matches("```").count() % 2 == 1 {
            return;
        }
        while let Some(sentence) = self.take_sentence() {
            self.enqueue(&sentence);
        }
    }

    /// Speak whatever is left over at the end of a turn.
    pub fn flush(&mut self) {
        let rest = std::mem::take(&mut self.pending);
        self.enqueue(&rest);
    }

    /// Split off the first complete sentence, if there is one worth speaking.
    fn take_sentence(&mut self) -> Option<String> {
        let mut split_at = None;
        for (index, ch) in self.pending.char_indices() {
            let boundary = matches!(ch, '.' | '!' | '?' | '\n' | ':');
            if boundary && index + ch.len_utf8() >= MIN_SPOKEN_CHARS {
                split_at = Some(index + ch.len_utf8());
                break;
            }
        }
        let at = split_at?;
        let rest = self.pending.split_off(at);
        Some(std::mem::replace(&mut self.pending, rest))
    }

    /// Speak a piece of text right away (subject to mute/hush), e.g. an
    /// acknowledgement while the real reply is still seconds out.
    pub fn enqueue(&self, raw: &str) {
        if self.muted.load(Ordering::Relaxed) || self.hushed {
            return;
        }
        let text = spoken_text(raw);
        if text.is_empty() {
            return;
        }
        #[cfg(feature = "tts")]
        {
            let session = self
                .session
                .get_or_init(|| Self::start_session(&self.voice, self.playback.clone()));
            session.say(text);
        }
        // Built without `tts`: the synthesis stack is ~10 MB of binary, so a
        // build that can never speak does not link it. Every other path (text,
        // muting, cancellation) behaves exactly as it does with speech on.
        #[cfg(not(feature = "tts"))]
        let _ = text;
    }

    /// Start the hub session and the pump thread that moves its audio into
    /// the playback buffer. The pump blocks on the session's events, so it
    /// costs nothing while nobody speaks.
    #[cfg(feature = "tts")]
    fn start_session(voice: &str, playback: Arc<Mutex<Playback>>) -> TtsHandle {
        let (handle, events) = TtsSession::start(TtsConfig {
            voice: Some(voice.to_string()),
            ..TtsConfig::default()
        })
        .split();
        std::thread::Builder::new()
            .name("converse-speech-pump".into())
            .spawn(move || {
                while let Some(event) = events.recv() {
                    match event {
                        TtsEvent::Loading { .. } => {}
                        TtsEvent::Ready(info) => {
                            log!("tts: {} via {}{}", info.engine, info.pipe, match &info.remote {
                                Some(node) => format!(" on {node}"),
                                None => String::new(),
                            });
                        }
                        TtsEvent::Failed(why) => {
                            log!("tts: no voice available: {why}");
                            return;
                        }
                        TtsEvent::Audio { audio, .. } => {
                            let mut playback = playback.lock().unwrap();
                            if playback.source_rate != audio.sample_rate as f64 {
                                playback.clear();
                                playback.source_rate = audio.sample_rate as f64;
                            }
                            // Append, don't replace: sentences queue up behind
                            // each other.
                            playback.samples.extend_from_slice(&audio.samples);
                        }
                        TtsEvent::Error { message, .. } => log!("tts: {message}"),
                    }
                }
            })
            .expect("spawn speech pump");
        handle
    }

    /// Cancel queued and playing speech immediately.
    pub fn stop(&mut self) {
        #[cfg(feature = "tts")]
        if let Some(session) = self.session.get() {
            session.cancel();
        }
        self.pending.clear();
        if let Ok(mut playback) = self.playback.lock() {
            playback.clear();
        }
    }

    /// "Shh": stop speaking AND stay quiet for the rest of the current reply.
    /// Without the latch, the still-streaming reply would resume at its next
    /// sentence boundary. Cleared when a new prompt starts (`unhush`).
    pub fn hush(&mut self) {
        self.hushed = true;
        self.stop();
    }

    pub fn unhush(&mut self) {
        self.hushed = false;
    }

    pub fn is_muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed)
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
        if muted {
            self.stop();
        }
    }
}

/// Markdown is for reading, not for speaking. Drop code blocks and the symbols
/// that would otherwise be read aloud as punctuation soup.
pub fn spoken_text(markdown: &str) -> String {
    let mut spoken = String::with_capacity(markdown.len());
    let mut inside_code = false;
    for line in markdown.lines() {
        if line.trim_start().starts_with("```") {
            inside_code = !inside_code;
            continue;
        }
        if inside_code {
            continue;
        }
        let cleaned: String = line
            .chars()
            .filter(|c| !matches!(c, '*' | '_' | '`' | '#' | '>' | '|' | '→' | '⚙' | '·'))
            .collect();
        let cleaned = cleaned.trim();
        if !cleaned.is_empty() {
            spoken.push_str(cleaned);
            spoken.push(' ');
        }
    }
    spoken.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Constructing the output must NOT start a session or load a model:
    /// Kokoro is ~327 MB resident, and an app that never speaks should not
    /// pay for it. The session starts on the first enqueued utterance.
    #[test]
    fn constructing_speech_output_does_not_start_a_session() {
        let start = std::time::Instant::now();
        let speech = SpeechOutput::new("bm_fable.mkvoice");
        #[cfg(feature = "tts")]
        assert!(speech.session.get().is_none());
        assert!(speech.playback().lock().unwrap().samples.is_empty());
        assert!(!speech.is_speaking());
        assert!(start.elapsed() < std::time::Duration::from_millis(250));
    }

    #[test]
    fn spoken_text_drops_code_and_markup() {
        let text = "Here **you** go:\n```rust\nfn x() {}\n```\nDone → ok.";
        // The arrow is dropped, not replaced, so its two surrounding spaces remain.
        assert_eq!(spoken_text(text), "Here you go: Done  ok.");
    }

    /// The lazy path must still speak. Ignored by default: it loads the real
    /// ~327 MB model (or falls back to the OS voice) and takes seconds.
    ///
    /// ```text
    /// MAKEPAD_TTS_MODEL=$REPO/kokoro-v1_0.mktts \
    /// MAKEPAD_TTS_VOICE=$REPO/bm_fable.mkvoice \
    ///   cargo test -p makepad-converse --release lazily -- --ignored
    /// ```
    #[test]
    #[ignore = "starts a real hub TTS session; needs weights or an OS voice"]
    fn lazily_started_session_still_produces_audio() {
        let mut speech = SpeechOutput::new("bm_fable.mkvoice");
        speech.enqueue("Testing the lazy speech path.");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        while std::time::Instant::now() < deadline {
            if speech.is_speaking() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        panic!("no audio produced within the timeout — the lazy path is broken");
    }
}
