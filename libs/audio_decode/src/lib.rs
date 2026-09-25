//! Compressed audio decoding: MPEG-1/2/2.5 Layer III (MP3), Ogg Vorbis, and
//! FLAC, written here rather than pulled in, like the repo's own inflate, PNG,
//! SQLite and zip readers. No dependencies, no `unsafe`.
//!
//! All three decoders read attacker-supplied bytes — a scraped music library, a
//! downloaded asset pack — so they are *total*: malformed input yields an
//! error, never a panic, and never an allocation sized straight from an
//! unchecked header field. Every buffer that grows with the stream is bounded
//! by [`Limits`].
//!
//! Two shapes of use:
//!
//! * whole-file: [`decode_audio`] / [`decode_any`] → [`DecodedAudio`]
//!   (interleaved `f32` in [-1, 1], the shape the VJ decks and the importer's
//!   waveform pass want);
//! * progressive: [`mp3::Mp3Decoder`] / [`vorbis::VorbisDecoder`] /
//!   [`flac::FlacDecoder`], which hand back one granule/block/frame at a time
//!   into a caller-owned buffer so a deck can start playing before the tail
//!   has been read. The frame loop allocates nothing.
//!
//! Duration without decoding is [`probe_duration`] (Xing/VBRI/LAME for MP3,
//! the last Ogg page's granule position for Vorbis, STREAMINFO for FLAC), and
//! container metadata is [`read_tags`] (ID3v2 text frames, Vorbis comments).

pub mod error;
pub mod flac;
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
    /// empty vector rather than an error. Callers wanting stereo take
    /// channels 0 and 1, and channel 0 for both ears of a mono file --
    /// never the last channel, which on a wide file is a rear or the
    /// low-frequency send.
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
    Flac,
}

/// Bounds every decode honours. Defaults are generous for a music library
/// (two hours of 48 kHz stereo) but finite, so a corrupt header cannot make
/// the process reserve the machine.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Longest decode, in frames (sample positions), per stream.
    pub max_frames: usize,
    /// Highest channel count accepted. Vorbis can declare any number and FLAC
    /// up to eight; MP3 is always mono or stereo.
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
    // FLAC's marker is exact; MP3's probe is a loose frame-sync scan and also
    // treats any ID3v2 prefix as MP3, so a tagged FLAC must win here.
    if flac::looks_like_flac(bytes) {
        return Some(AudioFormat::Flac);
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
        AudioFormat::Flac => flac::decode_all_limited(bytes, limits),
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
        AudioFormat::Flac => flac::probe_duration(bytes),
    }
}

/// How many leading bytes of a `file_len`-byte file [`probe_ends`] needs,
/// judged from the `head` read so far; `None` when `head` is enough (or the
/// format is unknown, which the probe then says). A library scan reads a
/// head, asks, and reads more until this says enough, instead of reading
/// every file whole: the ID3v2 tag and first frames of an MP3, every FLAC
/// metadata block, an Ogg file's header packets.
pub fn head_needed(head: &[u8], file_len: u64) -> Option<usize> {
    match sniff(head)? {
        AudioFormat::Mp3 => mp3::head_needed(head, file_len),
        AudioFormat::OggVorbis => vorbis::head_needed(head),
        AudioFormat::Flac => flac::head_needed(head),
    }
    .filter(|_| (head.len() as u64) < file_len)
}

/// Tags and length in seconds of a `file_len`-byte file, from its first
/// bytes (`head`, as long as [`head_needed`] asks) and its last (`tail`,
/// 128 KiB or the whole file): what [`read_tags`] and [`probe_duration`]
/// give from the whole file, without reading it. The length is `None` when
/// the ends do not tell it; an MP3 without a Xing/VBRI header is timed by
/// its first frame's bitrate.
pub fn probe_ends(head: &[u8], tail: &[u8], file_len: u64) -> Result<(Tags, Option<f64>), AudioError> {
    match sniff(head).ok_or(AudioError::UnknownFormat)? {
        AudioFormat::Mp3 => Ok(mp3::probe_ends(head, tail, file_len)),
        AudioFormat::OggVorbis => vorbis::probe_ends(head, tail),
        AudioFormat::Flac => flac::probe_head(head),
    }
}

/// Container metadata: ID3v2 text frames or Vorbis comments.
pub fn read_tags(bytes: &[u8]) -> Result<Tags, AudioError> {
    match sniff(bytes).ok_or(AudioError::UnknownFormat)? {
        AudioFormat::Mp3 => Ok(mp3::read_tags(bytes)),
        AudioFormat::OggVorbis => vorbis::read_tags(bytes),
        AudioFormat::Flac => flac::read_tags(bytes),
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
    fn sniff_finds_flac_before_mp3() {
        assert_eq!(sniff(b"fLaC\0\0\0\0"), Some(AudioFormat::Flac));
        // ID3v2 with a zero-length body, then the FLAC marker: looks_like_mp3
        // would claim this as MP3 because of the ID3 prefix.
        let mut tagged = vec![b'I', b'D', b'3', 3, 0, 0, 0, 0, 0, 0];
        tagged.extend_from_slice(b"fLaC");
        assert_eq!(sniff(&tagged), Some(AudioFormat::Flac));
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

    /// Grow `head` from a file's start as [`head_needed`] asks, then probe the
    /// ends: what a library scan does.
    fn probe_file(file: &[u8], first: usize) -> (Tags, Option<f64>) {
        let mut head = &file[..first.min(file.len())];
        while let Some(need) = head_needed(head, file.len() as u64) {
            assert!(need > head.len(), "asks for more than it has");
            head = &file[..need.min(file.len())];
        }
        let tail = &file[file.len().saturating_sub(128 * 1024)..];
        probe_ends(head, tail, file.len() as u64).unwrap()
    }

    /// A constant-bitrate MP3 behind a big ID3v2 tag and before an ID3v1
    /// one: the head grows past the tag, the length is the bitrate over the
    /// audio bytes, the title comes from the tag.
    #[test]
    fn an_mp3_is_timed_and_tagged_from_its_ends() {
        let mut title = vec![3u8];
        title.extend_from_slice(b"Ends");
        let mut body = Vec::new();
        body.extend_from_slice(b"TIT2");
        body.extend_from_slice(&(title.len() as u32).to_be_bytes());
        body.extend_from_slice(&[0, 0]);
        body.extend_from_slice(&title);
        body.resize(200_000, 0);
        let size = body.len();
        let mut file = b"ID3\x03\x00\x00".to_vec();
        file.extend_from_slice(&[(size >> 21) as u8 & 0x7f, (size >> 14) as u8 & 0x7f, (size >> 7) as u8 & 0x7f, size as u8 & 0x7f]);
        file.extend_from_slice(&body);
        // 128 kbit/s, 44.1 kHz, no padding: 417-byte frames of silence.
        let frames = 2000;
        for _ in 0..frames {
            let mut frame = vec![0u8; 417];
            frame[..4].copy_from_slice(&0xfffb_9064u32.to_be_bytes());
            file.extend_from_slice(&frame);
        }
        let mut id3v1 = b"TAG".to_vec();
        id3v1.resize(128, b' ');
        file.extend_from_slice(&id3v1);
        let (tags, seconds) = probe_file(&file, 4096);
        assert_eq!(tags.title.as_deref(), Some("Ends"));
        let exact = frames as f64 * 1152.0 / 44_100.0;
        let seconds = seconds.unwrap();
        assert!((seconds - exact).abs() / exact < 0.01, "{seconds} vs {exact}");
    }

    /// A FLAC file whose comments sit behind a large cover picture: the head
    /// grows block by block until the last one is in.
    #[test]
    fn flac_metadata_behind_a_picture_is_reached() {
        let mut file = b"fLaC".to_vec();
        let mut info = vec![0x10, 0x00, 0x10, 0x00, 0, 0, 0, 0, 0, 0];
        // 44.1 kHz, 2 channels, 16 bits, 441 000 samples.
        let word: u64 = (44_100u64 << 44) | (1u64 << 41) | (15u64 << 36) | 441_000;
        info.extend_from_slice(&word.to_be_bytes());
        info.extend_from_slice(&[0; 16]);
        file.extend_from_slice(&[0, 0, 0, 34]);
        file.extend_from_slice(&info);
        let picture = 300_000usize;
        file.extend_from_slice(&[6, (picture >> 16) as u8, (picture >> 8) as u8, picture as u8]);
        file.resize(file.len() + picture, 0);
        let mut comment = Vec::new();
        comment.extend_from_slice(&0u32.to_le_bytes());
        comment.extend_from_slice(&1u32.to_le_bytes());
        let entry = b"TITLE=Far In";
        comment.extend_from_slice(&(entry.len() as u32).to_le_bytes());
        comment.extend_from_slice(entry);
        file.extend_from_slice(&[0x80 | 4, 0, 0, comment.len() as u8]);
        file.extend_from_slice(&comment);
        file.resize(file.len() + 50_000, 0);
        let (tags, seconds) = probe_file(&file, 4096);
        assert_eq!(tags.title.as_deref(), Some("Far In"));
        assert_eq!(seconds, Some(10.0));
    }
}
