//! Speech synthesis for Makepad.
//!
//! This crate is a synthesis *engine*, not an audio device: [`Speaker::synthesize`]
//! returns PCM and the caller feeds it to `Cx::audio_output`. That deliberately
//! mirrors `makepad-voice`, which transcribes buffers rather than owning the
//! microphone — muting, mixing and device choice stay with the application.
//!
//! Backend selection also mirrors `makepad-voice`: prefer the local neural model
//! when its weights are on disk, fall back to the system synthesizer, and degrade
//! to silence on platforms that have neither.

#[cfg(all(any(target_os = "macos", target_os = "ios"), not(no_apple_tts)))]
mod apple;

pub mod g2p;
pub mod kokoro;

/// Mono PCM produced by a backend.
#[derive(Clone, Debug)]
pub struct SpeechAudio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

impl SpeechAudio {
    pub fn silent() -> Self {
        Self {
            samples: Vec::new(),
            sample_rate: 24_000,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TtsBackend {
    /// Kokoro-82M on `makepad-ggml`.
    Kokoro,
    /// The operating system's own synthesizer.
    NativeApple,
    /// No synthesizer on this platform; synthesis yields no samples.
    Silent,
}

#[derive(Debug)]
pub enum TtsError {
    /// The backend produced nothing for this text.
    Empty,
    Backend(String),
}

pub enum Speaker {
    Kokoro(kokoro::KokoroSpeaker),
    NativeApple,
    Silent,
}

impl Speaker {
    /// Prefer Kokoro when its weights are present, else the system voice.
    ///
    /// Set `MAKEPAD_TTS_MODEL` to point at the weights, or drop them next to the
    /// working directory as `kokoro-v1_0.mktts`.
    pub fn from_makepad_env() -> Self {
        if let Some(path) = kokoro::model_path_if_present() {
            match kokoro::KokoroSpeaker::load(&path) {
                Ok(speaker) => return Self::Kokoro(speaker),
                Err(err) => {
                    eprintln!("[tts] kokoro weights at '{path}' unusable ({err:?}); falling back");
                }
            }
        }
        Self::default_for_platform()
    }

    /// [`Speaker::from_makepad_env`], but preferring a specific voice pack
    /// (e.g. `"bm_fable.mkvoice"`). Falls back to the default voice if the
    /// named pack is missing; `MAKEPAD_TTS_VOICE` still wins as an override.
    pub fn from_makepad_env_with_voice(voice: &str) -> Self {
        if let Some(path) = kokoro::model_path_if_present() {
            let loaded = match kokoro::named_voice_path_if_present(voice) {
                Some(voice_path) => kokoro::KokoroSpeaker::load_with_voice(&path, &voice_path),
                None => {
                    eprintln!("[tts] voice pack '{voice}' not found; using default voice");
                    kokoro::KokoroSpeaker::load(&path)
                }
            };
            match loaded {
                Ok(speaker) => return Self::Kokoro(speaker),
                Err(err) => {
                    eprintln!("[tts] kokoro weights at '{path}' unusable ({err:?}); falling back");
                }
            }
        }
        Self::default_for_platform()
    }

    pub fn default_for_platform() -> Self {
        #[cfg(all(any(target_os = "macos", target_os = "ios"), not(no_apple_tts)))]
        {
            Self::NativeApple
        }
        #[cfg(not(all(any(target_os = "macos", target_os = "ios"), not(no_apple_tts))))]
        {
            Self::Silent
        }
    }

    pub fn kind(&self) -> TtsBackend {
        match self {
            Self::Kokoro(_) => TtsBackend::Kokoro,
            Self::NativeApple => TtsBackend::NativeApple,
            Self::Silent => TtsBackend::Silent,
        }
    }

    /// Blocking. Returns mono PCM at the backend's native sample rate.
    pub fn synthesize(&mut self, text: &str) -> Result<SpeechAudio, TtsError> {
        if text.trim().is_empty() {
            return Err(TtsError::Empty);
        }
        match self {
            Self::Kokoro(speaker) => speaker.synthesize(text),

            #[cfg(all(any(target_os = "macos", target_os = "ios"), not(no_apple_tts)))]
            Self::NativeApple => apple::synthesize(text, None, 0.0).ok_or(TtsError::Empty),

            #[cfg(not(all(any(target_os = "macos", target_os = "ios"), not(no_apple_tts))))]
            Self::NativeApple => Ok(SpeechAudio::silent()),

            Self::Silent => Ok(SpeechAudio::silent()),
        }
    }
}

impl Default for Speaker {
    fn default() -> Self {
        Self::from_makepad_env()
    }
}
