//! Compressed audio decoding: MPEG-1/2/2.5 Layer III (MP3) and Ogg Vorbis,
//! written here rather than pulled in, like the repo's own inflate, PNG,
//! SQLite and zip readers. No dependencies, no `unsafe`.
//!
//! Both decoders read attacker-supplied bytes — a scraped music library, a
//! downloaded asset pack — so both are *total*: malformed input yields an
//! error, never a panic, and never an allocation sized straight from an
//! unchecked header field. Every buffer that grows with the stream is bounded
//! by [`Limits`].
//!
//! Two shapes of use:
//!
//! * whole-file: [`decode_audio`] / [`decode_any`] → [`DecodedAudio`]
//!   (interleaved `f32` in [-1, 1], the shape the VJ decks and the importer's
//!   waveform pass want);
//! * progressive: [`mp3::Mp3Decoder`] / [`vorbis::VorbisDecoder`], which hand
//!   back one granule/block at a time into a caller-owned buffer so a deck can
//!   start playing before the tail has been read. The frame loop allocates
//!   nothing.
//!
//! Duration without decoding is [`probe_duration`] (Xing/VBRI/LAME for MP3,
//! the last Ogg page's granule position for Vorbis), and container metadata is
//! [`read_tags`] (ID3v2 text frames, Vorbis comments).

pub mod error;
pub mod mp3;
pub mod ogg;
pub mod tags;
pub mod vorbis;

pub use error::AudioError;
pub use tags::Tags;

/// Fully decoded audio: interleaved samples in [-1, 1].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DecodedAudio {
    pub rate: u32,
    pub channels: u16,
    /// Interleaved, `channels` values per frame.
    pub pcm_interleaved_f32: Vec<f32>,
}

impl DecodedAudio {
    /// Sample positions, independent of channel count.
    pub fn frames(&self) -> usize {
        if self.channels == 0 {
            0
        } else {
            self.pcm_interleaved_f32.len() / self.channels as usize
        }
    }

    pub fn duration_secs(&self) -> f64 {
        if self.rate == 0 {
            0.0
        } else {
            self.frames() as f64 / self.rate as f64
        }
    }

    /// One channel's samples, deinterleaved. Out-of-range channels give an
    /// empty vector rather than an error: callers ask for `[0]` and
    /// `[channels-1]` to fake stereo out of mono.
    pub fn channel(&self, index: usize) -> Vec<f32> {
        let ch = self.channels as usize;
        if ch == 0 || index >= ch {
            return Vec::new();
        }
        self.pcm_interleaved_f32.iter().skip(index).step_by(ch).copied().collect()
    }
}

/// Which container/codec a byte slice holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioFormat {
    Mp3,
    OggVorbis,
}

/// Bounds every decode honours. Defaults are generous for a music library
/// (two hours of 48 kHz stereo) but finite, so a corrupt header cannot make
/// the process reserve the machine.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Longest decode, in frames (sample positions), per stream.
    pub max_frames: usize,
    /// Highest channel count accepted. Vorbis streams can declare any number;
    /// MP3 is always mono or stereo, so this only ever binds the Ogg path.
    pub max_channels: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self { max_frames: 48_000 * 60 * 120, max_channels: 8 }
    }
}

impl Limits {
    /// A cap in frames, everything else default.
    pub fn with_max_frames(max_frames: usize) -> Self {
        Self { max_frames, ..Self::default() }
    }
}

/// Magic-byte probe. Tolerates a leading ID3v2 tag and leading garbage before
/// the first MP3 frame sync, which real-world files have plenty of.
pub fn sniff(bytes: &[u8]) -> Option<AudioFormat> {
    if bytes.len() >= 4 && &bytes[0..4] == b"OggS" {
        return Some(AudioFormat::OggVorbis);
    }
    if mp3::looks_like_mp3(bytes) {
        return Some(AudioFormat::Mp3);
    }
    None
}

/// Decode a whole file of a known format.
pub fn decode_audio(bytes: &[u8], format: AudioFormat) -> Result<DecodedAudio, AudioError> {
    decode_audio_limited(bytes, format, Limits::default())
}

pub fn decode_audio_limited(
    bytes: &[u8],
    format: AudioFormat,
    limits: Limits,
) -> Result<DecodedAudio, AudioError> {
    match format {
        AudioFormat::Mp3 => mp3::decode_all_limited(bytes, limits),
        AudioFormat::OggVorbis => vorbis::decode_all_limited(bytes, limits),
    }
}

/// Decode a whole file, sniffing the format from its bytes.
pub fn decode_any(bytes: &[u8]) -> Result<DecodedAudio, AudioError> {
    let format = sniff(bytes).ok_or(AudioError::UnknownFormat)?;
    decode_audio(bytes, format)
}

/// Duration in seconds without decoding the audio: an MP3 Xing/VBRI header,
/// else a first-frame CBR estimate; for Ogg, the final page granule.
pub fn probe_duration(bytes: &[u8]) -> Result<f64, AudioError> {
    match sniff(bytes).ok_or(AudioError::UnknownFormat)? {
        AudioFormat::Mp3 => mp3::probe_duration(bytes),
        AudioFormat::OggVorbis => vorbis::probe_duration(bytes),
    }
}

/// Container metadata: ID3v2 text frames or Vorbis comments.
pub fn read_tags(bytes: &[u8]) -> Result<Tags, AudioError> {
    match sniff(bytes).ok_or(AudioError::UnknownFormat)? {
        AudioFormat::Mp3 => Ok(mp3::read_tags(bytes)),
        AudioFormat::OggVorbis => vorbis::read_tags(bytes),
    }
}

#[cfg(test)]
mod lib_tests {
    use super::*;

    #[test]
    fn sniff_rejects_junk_and_empty() {
        assert_eq!(sniff(&[]), None);
        assert_eq!(sniff(b"not audio at all, really quite long garbage"), None);
        assert!(matches!(decode_any(b"nonsense"), Err(AudioError::UnknownFormat)));
    }

    #[test]
    fn sniff_finds_ogg() {
        assert_eq!(sniff(b"OggS\0\x02rest"), Some(AudioFormat::OggVorbis));
    }

    #[test]
    fn decoded_audio_accessors_are_total() {
        let a = DecodedAudio {
            rate: 8_000,
            channels: 2,
            pcm_interleaved_f32: vec![0.0, 1.0, 2.0, 3.0],
        };
        assert_eq!(a.frames(), 2);
        assert_eq!(a.channel(1), vec![1.0, 3.0]);
        assert!(a.channel(9).is_empty());
        assert!(DecodedAudio::default().channel(0).is_empty());
        assert_eq!(DecodedAudio::default().frames(), 0);
        assert_eq!(DecodedAudio::default().duration_secs(), 0.0);
    }
}
