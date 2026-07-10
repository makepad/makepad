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
pub mod weights;

use crate::g2p;
use crate::{SpeechAudio, TtsError};

use bert::Bert;
use decoder::Decoder;
use ops::{expand_to_frames, round_half_even};
use predictor::Predictor;
use text_encoder::TextEncoder;
use weights::Weights;

/// Default weights filename, resolved relative to the working directory.
pub const DEFAULT_MODEL_PATH: &str = "kokoro-v1_0.mktts";

/// Path override, mirroring `MAKEPAD_VOICE_MODEL`.
pub const MODEL_PATH_ENV: &str = "MAKEPAD_TTS_MODEL";

/// The default voice: `daniel`, a British male voice.
pub const DEFAULT_VOICE_PATH: &str = "bm_daniel.mkvoice";

/// Voice pack override.
pub const VOICE_PATH_ENV: &str = "MAKEPAD_TTS_VOICE";

pub const SAMPLE_RATE: u32 = 24_000;

/// Env override, working directory, then next to the executable — the last is
/// what a bundled app sees, where the working directory is anything at all.
fn resolve(env_var: &str, default_name: &str) -> Option<String> {
    if let Ok(path) = std::env::var(env_var) {
        return std::path::Path::new(&path).is_file().then_some(path);
    }
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
    resolve(MODEL_PATH_ENV, DEFAULT_MODEL_PATH)
}

/// The voice pack path, if the file actually exists.
pub fn voice_path_if_present() -> Option<String> {
    resolve(VOICE_PATH_ENV, DEFAULT_VOICE_PATH)
}

/// Like [`voice_path_if_present`], but preferring a specific voice pack file
/// (e.g. `bm_fable.mkvoice`). `MAKEPAD_TTS_VOICE` still wins as an override.
pub fn named_voice_path_if_present(name: &str) -> Option<String> {
    resolve(VOICE_PATH_ENV, name)
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
        let mut samples = Vec::new();
        for chunk in split_to_fit(text) {
            self.synthesize_chunk(&chunk, &mut samples);
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
        Ok(SpeechAudio {
            samples,
            sample_rate: SAMPLE_RATE,
        })
    }

    fn synthesize_chunk(&self, text: &str, samples: &mut Vec<f32>) {
        let tokens = g2p::tokens(text);
        // Two zero pads plus at least one phoneme.
        if tokens.len() < 3 {
            return;
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
            .map(|d| round_half_even(*d).max(1.0) as usize)
            .collect();
        let en = expand_to_frames(&duration.encoded, &frames);
        let asr = expand_to_frames(&encoded.output, &frames);

        let prosody = self.predictor.prosody(&en, predictor_style);
        let out = self.decoder.run(&asr, &prosody.f0, &prosody.noise, decoder_style);
        samples.extend_from_slice(&out.generator.waveform);
    }
}

/// Break text into pieces that each fit the 510-phoneme window: whole sentences
/// while they fit, single words when one sentence alone does not. `g2p::tokens`
/// would otherwise silently truncate.
fn split_to_fit(text: &str) -> Vec<String> {
    let fits = |s: &str| g2p::tokens(s).len() <= g2p::MAX_TOKENS + 1;
    if fits(text) {
        return vec![text.to_string()];
    }

    let mut out = Vec::new();
    let mut current = String::new();
    let mut push = |current: &mut String| {
        if !current.trim().is_empty() {
            out.push(std::mem::take(current));
        }
    };

    for sentence in split_after(text, &['.', '!', '?', '\n']) {
        let candidate = format!("{current}{sentence}");
        if fits(&candidate) {
            current = candidate;
            continue;
        }
        push(&mut current);
        if fits(sentence) {
            current = sentence.to_string();
            continue;
        }
        // A single run-on sentence past the window: fall back to words.
        for word in sentence.split_inclusive(char::is_whitespace) {
            let candidate = format!("{current}{word}");
            if fits(&candidate) {
                current = candidate;
            } else {
                push(&mut current);
                current = word.to_string();
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
