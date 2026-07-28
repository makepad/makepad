//! Speech output: a synthesis worker plus the playback buffer it fills.
//!
//! `makepad-tts` returns PCM rather than owning a device, so playback goes
//! through `cx.audio_output` like any other audio in Makepad. Muting is then
//! just "stop feeding the buffer", which also makes it instant.
//!
//! Streamed reply text goes in through [`SpeechOutput::feed`]; each finished
//! sentence is synthesized and spoken while the rest of the reply is still
//! being generated. Lifted out of the gamemaker example so any app can bolt a
//! voice onto an agent.

use makepad_tts::Speaker;
use makepad_widgets::makepad_draw::audio::AudioBuffer;
use makepad_widgets::log;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};

/// The buffer the audio callback plays from. Written by the synthesis worker,
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
}

/// Don't speak a fragment shorter than this — one-word clips sound like hiccups.
const MIN_SPOKEN_CHARS: usize = 16;

/// Speech output: a synthesis worker plus the buffer it fills.
pub struct SpeechOutput {
    say: mpsc::Sender<(u64, String)>,
    playback: Arc<Mutex<Playback>>,
    muted: Arc<AtomicBool>,
    /// Bumped on stop. Requests from an older generation are dropped, so a
    /// sentence that was already being synthesized never plays after a cancel.
    generation: Arc<AtomicU64>,
    /// Streamed reply text not yet spoken.
    pending: String,
    /// "Shh" latch: swallow the rest of the current reply (see `hush`).
    hushed: bool,
}

impl SpeechOutput {
    /// Start the synthesis worker with a named voice pack
    /// (e.g. `"bm_fable.mkvoice"`). Falls back like [`Speaker`] does when the
    /// pack or model is missing.
    pub fn new(voice: &str) -> Self {
        let playback = Arc::new(Mutex::new(Playback::default()));
        let muted = Arc::new(AtomicBool::new(false));
        let generation = Arc::new(AtomicU64::new(0));
        let (say, requests) = mpsc::channel::<(u64, String)>();

        let worker_playback = playback.clone();
        let worker_generation = generation.clone();
        let voice = voice.to_string();
        std::thread::spawn(move || {
            // Off the main thread on purpose: synthesis blocks until the whole
            // utterance is rendered.
            let mut speaker = Speaker::from_makepad_env_with_voice(&voice);
            log!("tts: backend {:?}", speaker.kind());
            // Discarded warm-up: Kokoro's first synthesis initializes the Metal
            // context on this thread; better now than on the first reply.
            let _ = speaker.synthesize("Hi.");
            while let Ok((generation, text)) = requests.recv() {
                if generation != worker_generation.load(Ordering::Relaxed) {
                    continue;
                }
                match speaker.synthesize(&text) {
                    Ok(audio) if !audio.is_empty() => {
                        // Re-check: synthesis is slow enough that a cancel can
                        // land while it runs.
                        if generation != worker_generation.load(Ordering::Relaxed) {
                            continue;
                        }
                        let mut playback = worker_playback.lock().unwrap();
                        if playback.source_rate != audio.sample_rate as f64 {
                            playback.samples.clear();
                            playback.cursor = 0.0;
                            playback.source_rate = audio.sample_rate as f64;
                        }
                        // Append, don't replace: sentences queue up behind
                        // each other.
                        playback.samples.extend_from_slice(&audio.samples);
                    }
                    Ok(_) => {}
                    Err(err) => log!("tts: {err:?}"),
                }
            }
        });

        Self {
            say,
            playback,
            muted,
            generation,
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
        let _ = self
            .say
            .send((self.generation.load(Ordering::Relaxed), text));
    }

    /// Cancel queued and playing speech immediately.
    pub fn stop(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.pending.clear();
        let mut playback = self.playback.lock().unwrap();
        playback.samples.clear();
        playback.cursor = 0.0;
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
            .filter(|c| !matches!(c, '*' | '_' | '`' | '#' | '>' | '|'))
            .collect();
        let cleaned = cleaned.trim();
        if !cleaned.is_empty() {
            spoken.push_str(cleaned);
            spoken.push(' ');
        }
    }
    spoken.trim().to_string()
}
