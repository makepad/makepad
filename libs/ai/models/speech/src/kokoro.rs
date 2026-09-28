//! Kokoro-82M, in plain Rust.
//!
//! Kokoro is Apache-2.0, 82M parameters, 24kHz output, 28 English voices. It is a
//! StyleTTS2 derivative: a phoneme encoder, a prosody predictor that emits
//! durations / F0 / energy, and an ISTFTNet decoder. A 1×256 `style` vector picks
//! the voice and conditions both the predictor and the decoder via AdaIN.
//!
//! Its graph does **not** take text: the inputs are phoneme token ids (max 510),
//! `style`, and `speed`, so [`crate::g2p`] is a prerequisite. Upstream phonemizes
//! with `misaki`, which falls back to espeak-ng; we deliberately avoid that C
//! dependency.
//!
//! Every stage here is validated against the upstream ONNX export by
//! `src/bin/parity.rs` — elementwise, on dumped intermediate tensors, not by ear.
//! The one place bit-parity is impossible in principle: the generator's forward
//! STFT feeds `atan`-based phase channels whose sign is FFT rounding noise
//! wherever a bin is near zero, so isolated phase entries flip against any other
//! implementation. The parity harness pins the transform in complex form instead
//! and holds everything downstream to full tolerance on a spliced reference.
//!
//! This is the correctness build; the hot ops (BERT's matmuls, the generator's
//! convolutions) move to `makepad-ggml` for speed in a later pass.
//!
//! # Weights
//!
//! Upstream ships only `kokoro-v1_0.pth` — no safetensors. A `.pth` is a zip
//! around a pickle, so the converter reads it with the Python standard library
//! alone (no torch, no numpy) and emits the flat format this module loads.

pub mod accel;
pub mod adain;
pub mod bert;
pub mod decoder;
pub mod generator;
pub mod npy;
pub mod ops;
pub mod predictor;
pub mod stft;
pub mod text_encoder;
pub mod timing;
pub mod weights;

use crate::g2p;
use crate::tts::{SpeechAudio, TtsError};

use bert::Bert;
use decoder::Decoder;
use ops::{expand_to_frames, round_half_even};
use predictor::Predictor;
use text_encoder::TextEncoder;
use timing::WordClock;
use weights::Weights;

/// Default weights filename, resolved relative to the working directory.
pub const DEFAULT_MODEL_PATH: &str = "kokoro-v1_0.mktts";

/// The default voice: `daniel`, a British male voice.
pub const DEFAULT_VOICE_PATH: &str = "bm_daniel.mkvoice";

pub const SAMPLE_RATE: u32 = 24_000;

/// Working directory, then next to the executable — the last is what a
/// bundled app sees, where the working directory is anything at all.
fn resolve(default_name: &str) -> Option<String> {
    if std::path::Path::new(default_name).is_file() {
        return Some(default_name.to_string());
    }
    let exe = std::env::current_exe().ok()?;
    let candidate = exe.parent()?.join(default_name);
    candidate
        .is_file()
        .then(|| candidate.to_string_lossy().into_owned())
}

/// The weights path, if the file actually exists.
pub fn model_path_if_present() -> Option<String> {
    resolve(DEFAULT_MODEL_PATH)
}

/// The voice pack path, if the file actually exists.
pub fn voice_path_if_present() -> Option<String> {
    resolve(DEFAULT_VOICE_PATH)
}

/// Like [`voice_path_if_present`], but preferring a specific voice pack file
/// (e.g. `bm_fable.mkvoice`).
pub fn named_voice_path_if_present(name: &str) -> Option<String> {
    resolve(name)
}

pub struct KokoroSpeaker {
    text_encoder: TextEncoder,
    bert: Bert,
    predictor: Predictor,
    decoder: Decoder,
    /// `[510, 256]`: one style row per phoneme count, minus one.
    voice: Vec<f32>,
}

impl KokoroSpeaker {
    pub fn load(model_path: &str) -> Result<Self, TtsError> {
        let voice = voice_path_if_present()
            .ok_or_else(|| TtsError::Backend(format!("voice pack {DEFAULT_VOICE_PATH} not found")))?;
        Self::load_with_voice(model_path, &voice)
    }

    pub fn load_with_voice(model_path: &str, voice_path: &str) -> Result<Self, TtsError> {
        let weights = Weights::load(model_path)?;
        let voice = Weights::load(voice_path)?;
        let voice = voice
            .get("style")
            .ok_or_else(|| TtsError::Backend(format!("{voice_path}: no style tensor")))?
            .to_vec();

        // Each component copies the tensors it needs, so the weights file's
        // buffer is dropped at the end of this function.
        Ok(Self {
            text_encoder: TextEncoder::load(&weights)?,
            bert: Bert::load(&weights)?,
            predictor: Predictor::load(&weights)?,
            decoder: Decoder::load(&weights)?,
            voice,
        })
    }

    pub fn synthesize(&mut self, text: &str) -> Result<SpeechAudio, TtsError> {
        self.synthesize_with_speed(text, 1.0)
    }

    /// [`Self::synthesize`] with a speaking-rate multiplier. Matches the
    /// upstream pipeline: predicted per-phoneme durations are divided by
    /// `speed` before rounding to frames, so 2.0 speaks twice as fast at the
    /// same pitch. `speed == 1.0` is bit-identical to [`Self::synthesize`].
    pub fn synthesize_with_speed(&mut self, text: &str, speed: f32) -> Result<SpeechAudio, TtsError> {
        self.synthesize_with_speed_observed(text, speed, &mut |_, _| true)
    }

    /// [`Self::synthesize_with_speed`] with a per-chunk observer for
    /// progress/cancellation: `on_chunk(done, total)` fires before each
    /// text chunk is synthesized; returning `false` stops the run early and
    /// returns whatever audio accumulated so far (the caller decides what an
    /// abort means — service backends surface their own Cancelled error).
    pub fn synthesize_with_speed_observed(
        &mut self,
        text: &str,
        speed: f32,
        on_chunk: &mut dyn FnMut(usize, usize) -> bool,
    ) -> Result<SpeechAudio, TtsError> {
        let speed = if speed.is_finite() && speed > 0.0 {
            speed.clamp(0.25, 4.0)
        } else {
            1.0
        };
        let chunks = split_to_fit(text);
        let total = chunks.len();
        let mut samples = Vec::new();
        let mut clock = WordClock::new(text);
        let mut finished = true;
        for (index, (start, end)) in chunks.iter().enumerate() {
            if !on_chunk(index, total) {
                finished = false;
                break;
            }
            let offset = samples.len() as f64 / SAMPLE_RATE as f64;
            let before = samples.len();
            if let Some(spoken) = self.synthesize_chunk(&text[*start..*end], speed, &mut samples) {
                let rendered = samples.len() - before;
                let frames: usize = spoken.frames.iter().sum();
                if frames > 0 && rendered > 0 {
                    // The decoder renders a fixed number of samples per
                    // predicted frame (600 at 24 kHz); read it off the
                    // output rather than assuming it.
                    let frame_secs = rendered as f64 / frames as f64 / SAMPLE_RATE as f64;
                    clock.add_chunk(*start, &spoken.sources, &spoken.frames, offset, frame_secs);
                }
            }
        }
        if samples.is_empty() {
            return Err(TtsError::Empty);
        }
        // Some voice packs run hot — `bm_fable` peaks around 1.4 where
        // `bm_daniel` stays near 0.6 — and anything past full scale clips at
        // the sink. Scale the utterance down only when it actually exceeds it.
        let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        if peak > 1.0 {
            let gain = 0.99 / peak;
            for sample in samples.iter_mut() {
                *sample *= gain;
            }
        }
        // A run stopped early has no timings: most of its words were never
        // spoken.
        let timings = finished.then(|| clock.finish(text));
        Ok(SpeechAudio {
            samples,
            sample_rate: SAMPLE_RATE,
            timings,
        })
    }

    /// Renders one chunk onto `samples`; returns what the timings need:
    /// per token, its predicted frames and the word it sounds.
    fn synthesize_chunk(&self, text: &str, speed: f32, samples: &mut Vec<f32>) -> Option<SpokenChunk> {
        let (tokens, sources) = g2p::tokens_with_sources(text);
        // Two zero pads plus at least one phoneme.
        if tokens.len() < 3 {
            return None;
        }

        // The voice pack is indexed by phoneme count minus one; `tokens` carries
        // a pad at each end.
        let style = &self.voice[(tokens.len() - 3) * 256..][..256];
        let (decoder_style, predictor_style) = style.split_at(128);

        let encoded = self.text_encoder.trace(&tokens);
        let bert = self.bert.trace(&tokens);
        let duration = self.predictor.durations(&bert.output, predictor_style);

        let frames: Vec<usize> = duration
            .durations
            .iter()
            .map(|d| round_half_even(*d / speed).max(1.0) as usize)
            .collect();
        let en = expand_to_frames(&duration.encoded, &frames);
        let asr = expand_to_frames(&encoded.output, &frames);

        let prosody = self.predictor.prosody(&en, predictor_style);
        let out = self.decoder.run(&asr, &prosody.f0, &prosody.noise, decoder_style);
        samples.extend_from_slice(&out.generator.waveform);
        Some(SpokenChunk { frames, sources })
    }
}

struct SpokenChunk {
    frames: Vec<usize>,
    sources: Vec<Option<usize>>,
}

/// Break text into pieces that each fit the 510-phoneme window: whole sentences
/// while they fit, single words when one sentence alone does not. `g2p::tokens`
/// would otherwise silently truncate. Pieces are byte ranges of `text`, in
/// order, so the word timings can place every chunk's words in the whole text.
fn split_to_fit(text: &str) -> Vec<(usize, usize)> {
    let fits = |range: (usize, usize)| g2p::tokens(&text[range.0..range.1]).len() <= g2p::MAX_TOKENS + 1;
    if fits((0, text.len())) {
        return vec![(0, text.len())];
    }
    // Every piece below is a subslice of `text`; its range is its offset.
    let range_of = |piece: &str| {
        let start = piece.as_ptr() as usize - text.as_ptr() as usize;
        (start, start + piece.len())
    };

    let mut out = Vec::new();
    // `current` is always a run of consecutive pieces (`None` = nothing yet).
    let mut current: Option<(usize, usize)> = None;
    let mut push = |current: &mut Option<(usize, usize)>| {
        if let Some((start, end)) = *current {
            if !text[start..end].trim().is_empty() {
                out.push((start, end));
                *current = None;
            }
        }
    };
    let extend = |current: Option<(usize, usize)>, piece: (usize, usize)| match current {
        Some((start, _)) => (start, piece.1),
        None => piece,
    };

    for sentence in split_after(text, &['.', '!', '?', '\n']) {
        let sentence = range_of(sentence);
        let candidate = extend(current, sentence);
        if fits(candidate) {
            current = Some(candidate);
            continue;
        }
        push(&mut current);
        if fits(sentence) {
            current = Some(sentence);
            continue;
        }
        // A single run-on sentence past the window: fall back to words.
        for word in text[sentence.0..sentence.1].split_inclusive(char::is_whitespace) {
            let word = range_of(word);
            let candidate = extend(current, word);
            if fits(candidate) {
                current = Some(candidate);
            } else {
                push(&mut current);
                current = Some(word);
            }
        }
        push(&mut current);
    }
    push(&mut current);
    out
}

/// Split into pieces, each ending just after one of `stops` (or at the end).
fn split_after<'a>(text: &'a str, stops: &'a [char]) -> impl Iterator<Item = &'a str> {
    text.split_inclusive(move |c| stops.contains(&c))
}

#[cfg(test)]
mod tests {
    use super::split_to_fit;

    #[test]
    fn long_text_splits_into_ordered_ranges_that_fit() {
        let sentence = "The quick brown fox jumps over the lazy dog again and again. ";
        let text = sentence.repeat(40);
        let chunks = split_to_fit(&text);
        assert!(chunks.len() > 1);
        let mut at = 0;
        for (start, end) in &chunks {
            assert!(*start >= at && end > start);
            assert!(crate::g2p::tokens(&text[*start..*end]).len() <= crate::g2p::MAX_TOKENS + 1);
            at = *end;
        }
        // Nothing but whitespace is left out.
        let kept: String = chunks.iter().map(|(a, b)| &text[*a..*b]).collect();
        assert_eq!(kept.split_whitespace().count(), text.split_whitespace().count());
        assert_eq!(split_to_fit("Short."), vec![(0, 6)]);
    }
}
