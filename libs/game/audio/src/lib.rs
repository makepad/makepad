//! Sampled audio for Makepad Arcade.
//!
//! Three layers, deliberately separate:
//! - **Decode** ([`wav`], [`vorbis`]) turns files into [`Pcm`]. Every decoder
//!   here reads attacker-supplied bytes and is total: malformed input yields
//!   an error, never a panic and never an allocation sized from an unchecked
//!   header field.
//! - **Playback** ([`bank`], [`mixer`]) owns decoded PCM and turns voices into
//!   interleaved stereo. It mixes *alongside* the procedural synth rather than
//!   replacing it — the host sums both.
//! - **Emission** ([`director`], [`materials`]) turns gameplay events into
//!   sounds, so a game is audible without the AI hand-wiring every cue.
//!
//! Everything in the emission path draws from a device-local RNG. Sound is
//! Local tier (game.md): it must never advance the simulation, or two devices
//! in a room would desync over a footstep.

pub mod bank;
pub mod bitread;
pub mod director;
pub mod materials;
pub mod mixer;
pub mod ogg;
pub mod rng;
pub mod vorbis;
pub mod wav;

pub use bank::{SampleBank, SampleId};
pub use director::{AudioDirector, Category, Placement, SoundEvent};
pub use materials::{ImpactCurve, Material, MaterialPair};
pub use mixer::{Mixer, Priority, VoiceHandle, VoiceSpec};
pub use rng::LocalRng;


/// Decoded audio: interleaved samples in [-1, 1].
#[derive(Clone, Debug, PartialEq)]
pub struct Pcm {
    pub channels: usize,
    pub sample_rate: u32,
    pub samples: Vec<f32>,
}

impl Pcm {
    /// Frames (sample positions), independent of channel count.
    pub fn frames(&self) -> usize {
        if self.channels == 0 {
            0
        } else {
            self.samples.len() / self.channels
        }
    }

    pub fn duration_secs(&self) -> f32 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.frames() as f32 / self.sample_rate as f32
        }
    }

    /// Bytes of PCM held, for the bank's memory budget.
    pub fn bytes(&self) -> usize {
        self.samples.len() * std::mem::size_of::<f32>()
    }
}

/// Why a file could not be decoded. Decoders return these instead of panicking
/// so a corrupt download degrades to a missing sound, not a crashed audio
/// thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioError {
    /// Ran off the end of the data.
    Truncated,
    /// Structurally invalid: a length that cannot be honoured, a bad code.
    Malformed,
    NotWav,
    NotOgg,
    /// Recognised container, but a feature this decoder does not implement.
    Unsupported(&'static str),
    UnsupportedFormat {
        format: u16,
        bits: u16,
    },
    UnsupportedChannels(u16),
    UnsupportedRate(u32),
}

impl std::fmt::Display for AudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioError::Truncated => write!(f, "audio data ended early"),
            AudioError::Malformed => write!(f, "audio data is malformed"),
            AudioError::NotWav => write!(f, "not a RIFF/WAVE file"),
            AudioError::NotOgg => write!(f, "not an Ogg bitstream"),
            AudioError::Unsupported(what) => write!(f, "unsupported: {what}"),
            AudioError::UnsupportedFormat { format, bits } => {
                write!(f, "unsupported WAV format {format} at {bits} bits")
            }
            AudioError::UnsupportedChannels(c) => write!(f, "unsupported channel count {c}"),
            AudioError::UnsupportedRate(r) => write!(f, "unsupported sample rate {r}"),
        }
    }
}

impl std::error::Error for AudioError {}

/// Decode by sniffing the container, so callers do not care what a pack ships.
pub fn decode(bytes: &[u8]) -> Result<Pcm, AudioError> {
    if bytes.len() >= 4 && &bytes[0..4] == b"OggS" {
        vorbis::decode(bytes)
    } else if bytes.len() >= 4 && &bytes[0..4] == b"RIFF" {
        wav::decode(bytes)
    } else {
        Err(AudioError::Unsupported("unrecognised audio container"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_reports_frames_and_duration() {
        let p = Pcm {
            channels: 2,
            sample_rate: 100,
            samples: vec![0.0; 400],
        };
        assert_eq!(p.frames(), 200);
        assert!((p.duration_secs() - 2.0).abs() < 1e-6);
        assert_eq!(p.bytes(), 1600);
    }

    #[test]
    fn zero_channels_does_not_divide_by_zero() {
        let p = Pcm {
            channels: 0,
            sample_rate: 0,
            samples: vec![],
        };
        assert_eq!(p.frames(), 0);
        assert_eq!(p.duration_secs(), 0.0);
    }

    #[test]
    fn sniffing_rejects_unknown_containers() {
        assert!(decode(b"MP3\x00whatever").is_err());
        assert!(decode(&[]).is_err());
    }
}
