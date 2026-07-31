//! Kokoro voice output (ported from examples/godot): a synthesis worker
//! fills a PCM buffer that the `cx.audio_output` callback drains. Muting is
//! "stop feeding the buffer", which also makes it instant. Streamed reply
//! text is spoken sentence-by-sentence so the voice keeps pace with
//! generation; nav turn instructions go through `say` directly.

use makepad_tts::Speaker;
use makepad_widgets::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};

/// The buffer the audio callback plays from. Written by the synthesis
/// worker, read by the audio thread.
#[derive(Default)]
pub struct Playback {
    samples: Vec<f32>,
    cursor: f64,
    source_rate: f64,
}

pub struct Speech {
    say_tx: mpsc::Sender<(u64, String)>,
    playback: Arc<Mutex<Playback>>,
    muted: Arc<AtomicBool>,
    /// Bumped on stop. Requests from an older generation are dropped, so a
    /// sentence that was already being synthesized never plays after a cancel.
    generation: Arc<AtomicU64>,
    /// Streamed reply text not yet spoken.
    pending: String,
}

/// Don't speak a fragment shorter than this — one-word clips sound like hiccups.
const MIN_SPOKEN_CHARS: usize = 16;

impl Speech {
    pub fn new() -> Self {
        let playback = Arc::new(Mutex::new(Playback::default()));
        let muted = Arc::new(AtomicBool::new(false));
        let generation = Arc::new(AtomicU64::new(0));
        let (say_tx, requests) = mpsc::channel::<(u64, String)>();

        let worker_playback = playback.clone();
        let worker_generation = generation.clone();
        std::thread::spawn(move || {
            // Off the main thread on purpose: synthesis blocks until the
            // whole utterance is rendered.
            let mut speaker = Speaker::from_makepad_env_with_voice("af_heart.mkvoice");
            log!("tts: backend {:?}", speaker.kind());
            // Discarded warm-up: Kokoro's first synthesis initializes the
            // Metal context on this thread; better now than on the first turn.
            let _ = speaker.synthesize("Hi.");
            while let Ok((generation, text)) = requests.recv() {
                if generation != worker_generation.load(Ordering::Relaxed) {
                    continue;
                }
                match speaker.synthesize(&text) {
                    Ok(audio) if !audio.is_empty() => {
                        // Re-check: synthesis is slow enough that a cancel
                        // can land while it runs.
                        if generation != worker_generation.load(Ordering::Relaxed) {
                            continue;
                        }
                        let mut playback = worker_playback.lock().unwrap();
                        if playback.source_rate != audio.sample_rate as f64 {
                            playback.samples.clear();
                            playback.cursor = 0.0;
                            playback.source_rate = audio.sample_rate as f64;
                        }
                        // Append, don't replace: sentences queue up.
                        playback.samples.extend_from_slice(&audio.samples);
                    }
                    Ok(_) => {}
                    Err(err) => log!("tts: {err:?}"),
                }
            }
        });

        Self {
            say_tx,
            playback,
            muted,
            generation,
            pending: String::new(),
        }
    }

    /// Install the playback callback. Output device selection stays with the
    /// app's AudioDevices handler (`cx.use_audio_outputs`).
    pub fn install_audio_output(&self, cx: &mut Cx) {
        let playback = self.playback.clone();
        let muted = self.muted.clone();
        cx.audio_output(0, move |info, output| {
            output.zero();
            if muted.load(Ordering::Relaxed) {
                return;
            }
            let Ok(mut playback) = playback.lock() else {
                return;
            };
            if playback.samples.is_empty() || playback.source_rate <= 0.0 {
                return;
            }
            // Resample on the fly: the backend's rate is not the device's.
            let step = playback.source_rate / info.sample_rate;
            let channels = output.channel_count();
            for frame in 0..output.frame_count() {
                let index = playback.cursor as usize;
                if index + 1 >= playback.samples.len() {
                    playback.samples.clear();
                    playback.cursor = 0.0;
                    break;
                }
                let fraction = (playback.cursor - index as f64) as f32;
                let a = playback.samples[index];
                let b = playback.samples[index + 1];
                let sample = a + (b - a) * fraction;
                for channel in 0..channels {
                    output.channel_mut(channel)[frame] = sample;
                }
                playback.cursor += step;
            }
            // Sentences append while earlier ones play, so drop the consumed
            // prefix periodically or the buffer grows for the whole reply.
            if playback.cursor > 2.0 * playback.source_rate {
                let consumed = playback.cursor as usize;
                playback.samples.drain(..consumed);
                playback.cursor -= consumed as f64;
            }
        });
    }

    /// Feed streamed reply text. Each finished sentence is spoken as soon
    /// as it lands.
    pub fn feed(&mut self, delta: &str) {
        self.pending.push_str(delta);
        while let Some(sentence) = self.take_sentence() {
            self.enqueue(&sentence);
        }
    }

    /// Speak whatever is left over at the end of a turn.
    pub fn flush(&mut self) {
        let rest = std::mem::take(&mut self.pending);
        self.enqueue(&rest);
    }

    /// Speak a standalone phrase now (nav turn instructions).
    pub fn say(&self, text: &str) {
        self.enqueue(text);
    }

    /// True while synthesized audio is still playing — used to drop mic
    /// transcripts of the assistant's own voice (no echo cancellation yet).
    pub fn is_speaking(&self) -> bool {
        self.playback
            .lock()
            .map(|p| !p.samples.is_empty())
            .unwrap_or(false)
    }

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

    fn enqueue(&self, raw: &str) {
        if self.muted.load(Ordering::Relaxed) {
            return;
        }
        let text = spoken_text(raw);
        if text.is_empty() {
            return;
        }
        let _ = self
            .say_tx
            .send((self.generation.load(Ordering::Relaxed), text));
    }

    pub fn stop(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.pending.clear();
        if let Ok(mut playback) = self.playback.lock() {
            playback.samples.clear();
            playback.cursor = 0.0;
        }
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

/// Text is for reading, not speaking: drop markdown/markup symbols and
/// emoji-ish prefixes that would be read aloud as punctuation soup.
fn spoken_text(text: &str) -> String {
    let mut spoken = String::with_capacity(text.len());
    for line in text.lines() {
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
