//! The engine output types: mono PCM plus the synthesis error. Text in, PCM
//! out — the caller owns the audio device.

/// One input word and when it is spoken, in seconds from the start of the
/// audio. `word` is the whitespace-separated word as written, punctuation
/// included.
#[derive(Clone, Debug, PartialEq)]
pub struct WordTiming {
    pub word: String,
    pub start: f64,
    pub end: f64,
}

/// One phoneme of the audio and when it sounds, in seconds from the start:
/// IPA (Kokoro's diphthong letters written out, `eɪ` for its `A`), stress
/// and length marks folded into the phoneme before them. Pauses between
/// words have no entry.
#[derive(Clone, Debug, PartialEq)]
pub struct PhoneTiming {
    pub phone: String,
    pub start: f64,
    pub end: f64,
}

/// Mono PCM produced by a backend.
#[derive(Clone, Debug)]
pub struct SpeechAudio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    /// Every whitespace-separated word of the text, in order, when the
    /// backend knows when it spoke them (Kokoro does, from its duration
    /// predictor). `None` when it does not, or when the run stopped early.
    pub timings: Option<Vec<WordTiming>>,
    /// Every phoneme spoken, in order, when the backend knows them (Kokoro
    /// does: its duration predictor times each phoneme token). `None` as
    /// for `timings`. What lip sync reads.
    pub phones: Option<Vec<PhoneTiming>>,
}

impl SpeechAudio {
    pub fn silent() -> Self {
        Self {
            samples: Vec::new(),
            sample_rate: 24_000,
            timings: None,
            phones: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn duration_secs(&self) -> f32 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        self.samples.len() as f32 / self.sample_rate as f32
    }

    /// Linearly resample to `target` Hz. Backends emit their own rates (Apple
    /// around 22.05kHz, Kokoro 24kHz) and the audio device wants something else
    /// again, usually 44.1 or 48kHz.
    pub fn resampled(&self, target: u32) -> Vec<f32> {
        if self.sample_rate == 0 || target == 0 || self.samples.is_empty() {
            return Vec::new();
        }
        if self.sample_rate == target {
            return self.samples.clone();
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
        out
    }
}

#[derive(Debug)]
pub enum TtsError {
    /// The backend produced nothing for this text.
    Empty,
    Backend(String),
}
