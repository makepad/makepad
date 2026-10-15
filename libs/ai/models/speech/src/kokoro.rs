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
#[cfg(any(target_os = "linux", target_os = "android"))]
#[path = "kokoro/vulkan/backend.rs"]
pub(crate) mod vulkan;

use crate::g2p;
use crate::tts::{SpeechAudio, TtsError};

use bert::Bert;
use decoder::Decoder;
use ops::{expand_to_frames, round_half_even};
use predictor::Predictor;
use text_encoder::TextEncoder;
use timing::{chunk_phones, WordClock};
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
    /// The text front end (and symbol table) this model wants.
    frontend: g2p::Frontend,
    /// The mapped model file the weights point into.
    mapped: std::sync::Arc<makepad_ai_loader::MappedRegion>,
    /// Set once the mapped pages were dropped after a GPU backend took its copy.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    released: bool,
}

impl KokoroSpeaker {
    pub fn load(model_path: &str) -> Result<Self, TtsError> {
        let voice = voice_path_if_present()
            .ok_or_else(|| TtsError::Backend(format!("voice pack {DEFAULT_VOICE_PATH} not found")))?;
        Self::load_with_voice(model_path, &voice)
    }

    /// The style table of a voice pack (`[510, 256]` floats), to hand to
    /// [`Self::set_voice`]. A few hundred KB: callers keep the packs they use.
    pub fn read_voice(voice_path: &str) -> Result<Vec<f32>, TtsError> {
        let voice = Weights::load(voice_path)?;
        let style = voice
            .get("style")
            .ok_or_else(|| TtsError::Backend(format!("{voice_path}: no style tensor")))?
            .to_vec();
        if style.len() < 510 * 256 {
            return Err(TtsError::Backend(format!("{voice_path}: style table is {} floats, want {}", style.len(), 510 * 256)));
        }
        Ok(style)
    }

    /// Which text front end speaks next: English for the English voices,
    /// Mandarin (v1.0 or v1.1-zh symbols, by the model loaded) for the zf_/zm_
    /// voices. The caller pairs it with the voice it sets.
    pub fn set_frontend(&mut self, frontend: g2p::Frontend) {
        self.frontend = frontend;
    }

    /// Speak with another voice from now on. The weights stay loaded; only the
    /// style table changes, so this is free next to a model load.
    pub fn set_voice(&mut self, voice: Vec<f32>) {
        self.voice = voice;
    }

    pub fn load_with_voice(model_path: &str, voice_path: &str) -> Result<Self, TtsError> {
        // The G2P tables are assets of the model, downloaded next to it.
        let asset_dir = std::path::Path::new(model_path).parent().unwrap_or(std::path::Path::new("."));
        g2p::data::load(asset_dir).map_err(TtsError::Backend)?;
        let weights = Weights::load(model_path)?;
        let voice = Self::read_voice(voice_path)?;

        // The components keep views into the mapped weights file instead of
        // copies; `mapped` keeps the mapping alive and lets a GPU backend drop
        // its pages.
        Ok(Self {
            text_encoder: TextEncoder::load(&weights)?,
            bert: Bert::load(&weights)?,
            predictor: Predictor::load(&weights)?,
            decoder: Decoder::load(&weights)?,
            voice,
            frontend: g2p::Frontend::English,
            mapped: weights.mapped().clone(),
            #[cfg(any(target_os = "linux", target_os = "android"))]
            released: false,
        })
    }

    /// Drops the mapped weight pages from this process (a GPU backend holds its
    /// own copy now); the CPU path faults them back in from the file if it runs.
    pub fn release_mapped_weights(&self) {
        self.mapped.release(0, self.mapped.len());
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
        self.synthesize_pieces(text, speed, false, on_chunk, &mut |_| {})
    }

    /// [`Self::synthesize_with_speed_observed`] for streaming playback: the text is
    /// cut at clause punctuation (small pieces merged to at least [`CLAUSE_MIN_TOKENS`]
    /// phonemes) and `on_audio` receives each piece's samples the moment they are
    /// rendered, so the first clause can play while the rest is still being made.
    /// The returned take is the whole utterance with its word timings, as ever.
    pub fn synthesize_streaming(
        &mut self,
        text: &str,
        speed: f32,
        on_chunk: &mut dyn FnMut(usize, usize) -> bool,
        on_audio: &mut dyn FnMut(&[f32]),
    ) -> Result<SpeechAudio, TtsError> {
        self.synthesize_pieces(text, speed, true, on_chunk, on_audio)
    }

    fn synthesize_pieces(
        &mut self,
        text: &str,
        speed: f32,
        by_clause: bool,
        on_chunk: &mut dyn FnMut(usize, usize) -> bool,
        on_audio: &mut dyn FnMut(&[f32]),
    ) -> Result<SpeechAudio, TtsError> {
        let speed = if speed.is_finite() && speed > 0.0 {
            speed.clamp(0.25, 4.0)
        } else {
            1.0
        };
        let mut chunks = split_to_fit(text, self.frontend);
        if by_clause {
            chunks = split_clauses(text, &chunks, self.frontend);
        }
        let total = chunks.len();
        let mut samples = Vec::new();
        let mut clock = WordClock::new(text);
        let mut phones = Vec::new();
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
                    chunk_phones(&spoken.tokens, &spoken.sources, &spoken.frames, offset, frame_secs, &mut phones);
                }
                if by_clause && rendered > 0 {
                    // A piece that would clip is scaled alone; the final take is
                    // normalized as a whole below.
                    let piece = &samples[before..];
                    let peak = piece.iter().fold(0.0f32, |m, s| m.max(s.abs()));
                    if peak > 1.0 {
                        let gain = 0.99 / peak;
                        let scaled: Vec<f32> = piece.iter().map(|s| s * gain).collect();
                        on_audio(&scaled);
                    } else {
                        on_audio(piece);
                    }
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
        // Every weight the GPU path uses is resident after one full chunk:
        // drop the mapped pages and the folded kernels (recomputed from the
        // mapped g/v if the CPU path ever runs again).
        #[cfg(any(target_os = "linux", target_os = "android"))]
        if !self.released && vulkan::weights_resident() {
            self.release_mapped_weights();
            self.text_encoder.drop_host_copies();
            self.predictor.drop_host_copies();
            self.decoder.drop_host_copies();
            weights::trim_heap();
            self.released = true;
        }
        let timings = finished.then(|| clock.finish(text));
        let phones = finished.then_some(phones);
        Ok(SpeechAudio {
            samples,
            sample_rate: SAMPLE_RATE,
            timings,
            phones,
        })
    }

    /// Renders one chunk onto `samples`; returns what the timings need:
    /// per token, its predicted frames and the word it sounds.
    fn synthesize_chunk(&self, text: &str, speed: f32, samples: &mut Vec<f32>) -> Option<SpokenChunk> {
        let (tokens, sources) = g2p::tokens_with_sources_for(text, self.frontend);
        // Two zero pads plus at least one phoneme.
        if tokens.len() < 3 {
            return None;
        }

        // Kokoro v1.1-zh's own sample script slows long Mandarin sentences and
        // speaks short ones a tenth fast; the same shape here.
        let speed = if self.frontend == g2p::Frontend::MandarinV11 {
            speed * zh_v11_speed(tokens.len() - 2)
        } else {
            speed
        };

        // The voice pack is indexed by phoneme count minus one; `tokens` carries
        // a pad at each end.
        let style = &self.voice[(tokens.len() - 3) * 256..][..256];
        let (decoder_style, predictor_style) = style.split_at(128);

        let profile = std::env::var_os("KOKORO_PROFILE").is_some();
        let t0 = std::time::Instant::now();
        let encoded = self.text_encoder.trace(&tokens);
        let t1 = std::time::Instant::now();
        let bert = self.bert.trace(&tokens);
        let t2 = std::time::Instant::now();
        let duration = self.predictor.durations(&bert.output, predictor_style);
        let t3 = std::time::Instant::now();

        let frames: Vec<usize> = duration
            .durations
            .iter()
            .map(|d| round_half_even(*d / speed).max(1.0) as usize)
            .collect();
        let en = expand_to_frames(&duration.encoded, &frames);
        let asr = expand_to_frames(&encoded.output, &frames);

        let t4 = std::time::Instant::now();
        let prosody = self.predictor.prosody(&en, predictor_style);
        let t5 = std::time::Instant::now();
        let waveform = self.decoder.waveform(&asr, &prosody.f0, &prosody.noise, decoder_style);
        if profile {
            let ms = |a: std::time::Instant, b: std::time::Instant| (b - a).as_secs_f64() * 1000.0;
            eprintln!(
                "[kokoro] {} tokens: text_enc {:.0} bert {:.0} durations {:.0} expand {:.0} prosody {:.0} decoder {:.0} ms ({} frames)",
                tokens.len(), ms(t0, t1), ms(t1, t2), ms(t2, t3), ms(t3, t4), ms(t4, t5), t5.elapsed().as_secs_f64() * 1000.0, frames.iter().sum::<usize>()
            );
            ops::print_op_profile();
        }
        samples.extend_from_slice(&waveform);
        Some(SpokenChunk { frames, sources, tokens })
    }
}

/// Speed factor for a Mandarin sentence of `len_ps` phonemes (hexgrad's
/// `speed_callable` in the v1.1-zh samples).
fn zh_v11_speed(len_ps: usize) -> f32 {
    let base = if len_ps <= 83 {
        1.0
    } else if len_ps < 183 {
        1.0 - (len_ps as f32 - 83.0) / 500.0
    } else {
        0.8
    };
    base * 1.1
}

struct SpokenChunk {
    frames: Vec<usize>,
    sources: Vec<Option<usize>>,
    tokens: Vec<u16>,
}

/// Break text into pieces that each fit the 510-phoneme window: whole sentences
/// while they fit, single words when one sentence alone does not. `g2p::tokens`
/// would otherwise silently truncate. Pieces are byte ranges of `text`, in
/// order, so the word timings can place every chunk's words in the whole text.
fn split_to_fit(text: &str, frontend: g2p::Frontend) -> Vec<(usize, usize)> {
    let fits = |range: (usize, usize)| {
        g2p::tokens_with_sources_for(&text[range.0..range.1], frontend).0.len() <= g2p::MAX_TOKENS + 1
    };
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

    for sentence in split_after(text, &['.', '!', '?', '\n', '。', '！', '？', '；']) {
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
/// Phonemes a streamed clause should reach before it is cut: shorter pieces sound
/// choppy (prosody restarts at every piece) and cost more per second of audio.
pub const CLAUSE_MIN_TOKENS: usize = 18;

/// Cut each `ranges` piece after clause punctuation, merging a clause shorter than
/// [`CLAUSE_MIN_TOKENS`] phonemes into the next one. Ranges stay in order and cover
/// the same text.
fn split_clauses(text: &str, ranges: &[(usize, usize)], frontend: g2p::Frontend) -> Vec<(usize, usize)> {
    const STOPS: &[char] = &[',', ';', ':', '.', '!', '?', '\n', '、', '，', '。', '！', '？', '；', '：'];
    let mut out = Vec::new();
    for &(start, end) in ranges {
        let piece = &text[start..end];
        let mut at = start;
        let mut open: Option<usize> = None;
        for clause in split_after(piece, STOPS) {
            let clause_end = at + clause.len();
            let from = *open.get_or_insert(at);
            let tokens = g2p::tokens_with_sources_for(&text[from..clause_end], frontend).0.len().saturating_sub(2);
            if tokens >= CLAUSE_MIN_TOKENS {
                out.push((from, clause_end));
                open = None;
            }
            at = clause_end;
        }
        if let Some(from) = open {
            // A short tail joins the clause before it when there is one in this range.
            match out.last_mut() {
                Some(last) if last.1 == from => last.1 = end,
                _ => out.push((from, end)),
            }
        }
    }
    out
}

fn split_after<'a>(text: &'a str, stops: &'a [char]) -> impl Iterator<Item = &'a str> {
    text.split_inclusive(move |c| stops.contains(&c))
}

#[cfg(test)]
mod tests {
    use super::split_to_fit;

    #[test]
    fn clauses_split_after_punctuation_and_short_ones_merge() {
        use super::{split_clauses, CLAUSE_MIN_TOKENS};
        use crate::g2p::Frontend;
        let text = "Hello there, how are you doing today? I am doing well, thank you for asking. The weather is nice.";
        let whole = vec![(0, text.len())];
        let clauses = split_clauses(text, &whole, Frontend::English);
        assert!(clauses.len() >= 3, "{clauses:?}");
        let mut at = 0;
        for (i, (start, end)) in clauses.iter().enumerate() {
            assert!(*start >= at && end > start);
            at = *end;
            let tokens = crate::g2p::tokens_with_sources_for(&text[*start..*end], Frontend::English).0.len() - 2;
            // Only the last piece may be shorter than the minimum.
            assert!(tokens >= CLAUSE_MIN_TOKENS || i == clauses.len() - 1, "piece {i} has {tokens}");
        }
        assert_eq!(at, text.len());
        // A very short text is one piece.
        let short = "Hi there.";
        assert_eq!(split_clauses(short, &[(0, short.len())], Frontend::English).len(), 1);
    }

    #[test]
    fn long_text_splits_into_ordered_ranges_that_fit() {
        let sentence = "The quick brown fox jumps over the lazy dog again and again. ";
        let text = sentence.repeat(40);
        let chunks = split_to_fit(&text, crate::g2p::Frontend::English);
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
        assert_eq!(split_to_fit("Short.", crate::g2p::Frontend::English), vec![(0, 6)]);
    }
}
