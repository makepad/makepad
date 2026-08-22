//! MPEG-1/2/2.5 Layer III decoding.
//!
//! What is supported: every Layer III variant with a header-derivable frame
//! length — MPEG-1 (44.1/48/32 kHz), MPEG-2 (22.05/24/16 kHz) and MPEG-2.5
//! (11.025/12/8 kHz), mono, stereo, dual channel and both joint-stereo modes
//! (mid/side and intensity), all block types including mixed blocks, and the
//! bit reservoir that lets a granule's data start in an earlier frame.
//!
//! What is refused, loudly rather than silently: **free-format** streams
//! (`bitrate_index` 0), whose frame length is not in the header, and Layers I
//! and II, which share the header but nothing else.
//!
//! ID3v2 and ID3v1 tags are stripped before decoding and the ID3v2 text frames
//! are exposed through [`Tags`]. Xing/Info, VBRI and LAME headers give a
//! duration without decoding.
//!
//! **Gapless.** When a stream carries a LAME/Lavc tag, the encoder delay and
//! padding it declares are trimmed by default, along with the 529 samples of
//! delay the Layer III filterbank itself adds. That is what every player does
//! and what makes a decode line up sample-for-sample with the system decoder;
//! [`Mp3Decoder::set_gapless`] turns it off for a caller that wants the raw
//! filterbank output, and [`Mp3Decoder::gapless`] reports the tag's values.

mod bits;
mod header;
mod layer3;
mod synth;
mod tables;
mod tables_lsf;

pub use header::{ChannelMode, FrameHeader, VbrInfo, Version};

use crate::error::AudioError;
use crate::tags::Tags;
use crate::{DecodedAudio, Limits};
use layer3::{ChannelState, GranuleScratch, SideInfo};

/// Bytes of main data the reservoir can be asked to look back over, from the
/// 9-bit `main_data_begin` field.
const MAX_RESERVOIR: usize = 511;

/// Samples of delay the Layer III synthesis filterbank adds on its own, before
/// any encoder padding. Every gapless scheme accounts for it separately from
/// the LAME tag's numbers.
pub const FILTERBANK_DELAY: u64 = 529;

pub use header::looks_like_mp3;

/// One decoded frame: interleaved samples plus the format they are in.
pub struct Frame<'a> {
    pub rate: u32,
    pub channels: u16,
    /// Interleaved, `channels` values per sample position.
    pub pcm: &'a [f32],
}

/// Progressive MP3 decoding. [`Mp3Decoder::next_frame`] hands back one frame
/// at a time out of an internal buffer, so a deck can start playing a track
/// while the rest of it is still being read, and the frame loop never
/// allocates.
pub struct Mp3Decoder<'a> {
    audio: &'a [u8],
    at: usize,
    format: Option<FrameHeader>,
    vbr: Option<VbrInfo>,
    tags: Tags,
    reservoir: Vec<u8>,
    scratch: GranuleScratch,
    channels: [ChannelState; 2],
    granule: Box<[[f32; 576]; 2]>,
    out: Vec<f32>,
    frames_decoded: u64,
    limits: Limits,
    samples_emitted: u64,
    gapless: bool,
    /// What gapless playback trims, front and back: constants for the stream.
    trim: (u64, u64),
    /// Samples of the front trim still to be dropped.
    skip_front: u64,
}

impl<'a> Mp3Decoder<'a> {
    /// Sync to the first frame of `bytes`. Fails only when there is no MP3
    /// stream at all; per-frame damage is handled during decoding.
    pub fn new(bytes: &'a [u8]) -> Result<Self, AudioError> {
        Self::with_limits(bytes, Limits::default())
    }

    pub fn with_limits(bytes: &'a [u8], limits: Limits) -> Result<Self, AudioError> {
        let mut tags = Tags::default();
        read_id3v2(bytes, &mut tags);
        let audio = header::strip_containers(bytes);
        let (at, first) = header::find_frame(audio, 0).ok_or(AudioError::Empty)?;
        let vbr = header::parse_vbr_header(&audio[at..], &first);
        // The filterbank's own 529 samples of delay are there whether or not a
        // LAME tag is; only the encoder's own delay and padding need the tag.
        let (skip_front, skip_back) = match vbr.and_then(|v| v.delay) {
            Some((delay, padding)) => (
                delay as u64 + FILTERBANK_DELAY,
                (padding as u64).saturating_sub(FILTERBANK_DELAY),
            ),
            None => (FILTERBANK_DELAY, 0),
        };
        Ok(Self {
            audio,
            at,
            format: Some(first),
            vbr,
            tags,
            reservoir: Vec::with_capacity(MAX_RESERVOIR + 2048),
            scratch: GranuleScratch::new(),
            channels: [ChannelState::new(), ChannelState::new()],
            granule: Box::new([[0.0; 576]; 2]),
            out: Vec::with_capacity(1152 * 2),
            frames_decoded: 0,
            limits,
            samples_emitted: 0,
            gapless: true,
            trim: (skip_front, skip_back),
            skip_front,
        })
    }

    pub fn rate(&self) -> u32 {
        self.format.map_or(0, |h| h.sample_rate)
    }

    pub fn channels(&self) -> u16 {
        self.format.map_or(0, |h| h.channels() as u16)
    }

    pub fn tags(&self) -> &Tags {
        &self.tags
    }

    /// LAME encoder delay and padding in samples, when the stream carries a
    /// LAME/Lavc tag. Trimming these is what makes playback gapless; this
    /// decoder reports them and leaves the decision to the caller.
    pub fn gapless(&self) -> Option<(u16, u16)> {
        self.vbr.and_then(|v| v.delay)
    }

    /// Turn gapless trimming on or off. Samples already emitted cannot be
    /// un-emitted, but everything from here on follows the new setting.
    pub fn set_gapless(&mut self, on: bool) {
        self.gapless = on;
    }

    /// Samples gapless playback trims from the front and the back: the LAME
    /// encoder delay plus the filterbank's own 529, and the encoder padding
    /// less the same 529. Constant for the stream, and zero once gapless is
    /// switched off.
    pub fn trim(&self) -> (u64, u64) {
        if self.gapless {
            self.trim
        } else {
            (0, 0)
        }
    }

    /// Frame count from the Xing/VBRI header, when present.
    pub fn declared_frames(&self) -> Option<u32> {
        self.vbr.and_then(|v| v.frames)
    }

    /// Decode the next frame. `Ok(None)` is a clean end of stream.
    ///
    /// A frame whose main data is not yet in the reservoir (the first frames
    /// of a stream, or the first after a resync) decodes to silence rather
    /// than being dropped, so the sample timeline stays exact.
    pub fn next_frame(&mut self) -> Result<Option<Frame<'_>>, AudioError> {
        loop {
            let Some((at, header)) = self.next_header() else {
                return Ok(None);
            };
            self.at = at + header.frame_bytes;
            let format = *self.format.get_or_insert(header);
            if !format.compatible_with(&header) {
                // A mid-stream format change: honour the new one, but the
                // filterbank history no longer applies to it.
                self.format = Some(header);
                self.channels[0].reset();
                self.channels[1].reset();
            }

            let frame_end = (at + header.frame_bytes).min(self.audio.len());
            let frame = &self.audio[at..frame_end];
            // The Xing/Info/VBRI frame is metadata wearing a frame's clothes.
            if self.frames_decoded == 0 && header::parse_vbr_header(frame, &header).is_some() {
                self.frames_decoded += 1;
                continue;
            }
            self.frames_decoded += 1;

            let side_start = 4 + usize::from(header.protected) * 2;
            let main_start = side_start + header.side_info_bytes();
            let side = frame
                .get(side_start..main_start)
                .and_then(|bytes| layer3::read_side_info(bytes, &header).ok());
            let produced = match side {
                Some(side) => {
                    let main = frame.get(main_start..).unwrap_or(&[]);
                    self.decode_frame(&header, &side, main)?
                }
                None => {
                    // Damaged side info. Dropping the frame would shorten the
                    // timeline *and* leave the reservoir describing bytes that
                    // no longer belong to what follows, so emit silence for it
                    // and start the reservoir again.
                    self.reservoir.clear();
                    false
                }
            };
            self.emit(&header, produced)?;
            if self.out.is_empty() {
                // Wholly consumed by the gapless front trim; there is nothing
                // to hand back, so decode the next frame instead of returning
                // an empty one.
                continue;
            }
            return Ok(Some(Frame {
                rate: header.sample_rate,
                channels: header.channels() as u16,
                pcm: &self.out,
            }));
        }
    }

    /// Find the next frame header, resyncing over damage.
    fn next_header(&mut self) -> Option<(usize, FrameHeader)> {
        if self.at + 4 > self.audio.len() {
            return None;
        }
        // The common case: the next frame starts exactly where this one ended.
        let word = u32::from_be_bytes([
            self.audio[self.at],
            self.audio[self.at + 1],
            self.audio[self.at + 2],
            self.audio[self.at + 3],
        ]);
        if let Some(h) = FrameHeader::parse(word) {
            if self.format.is_none_or(|f| f.compatible_with(&h)) {
                return Some((self.at, h));
            }
        }
        // Otherwise scan, which also skips any tag appended mid-file.
        let found = header::find_frame(self.audio, self.at);
        if found.is_some() {
            // Damage means the reservoir's contents no longer belong to the
            // frames that follow.
            self.reservoir.clear();
        }
        found
    }

    /// Run the granules of one frame. The bool says whether `self.out` was
    /// filled; `false` means the reservoir could not supply this frame and the
    /// caller must substitute silence.
    fn decode_frame(
        &mut self,
        header: &FrameHeader,
        side: &SideInfo,
        main: &[u8],
    ) -> Result<bool, AudioError> {
        let available = self.reservoir.len();
        if side.main_data_begin > available {
            // The reservoir does not reach back far enough yet. Keep the data
            // (later frames will need it) and emit silence for this one.
            self.push_reservoir(main);
            return Ok(false);
        }
        let start = available - side.main_data_begin;
        self.push_reservoir(main);

        let mut bit_pos = start * 8;
        for granule in 0..header.granules() {
            match layer3::decode_granule(
                header,
                side,
                granule,
                &self.reservoir,
                bit_pos,
                &mut self.scratch,
                &mut self.channels,
                &mut self.granule,
            ) {
                Ok(next) => {
                    bit_pos = next;
                    self.append_granule(header, granule);
                }
                Err(_) => {
                    // Corrupt granule: silence for the rest of the frame, but
                    // keep whatever granules already decoded.
                    self.granule[0].fill(0.0);
                    self.granule[1].fill(0.0);
                    for g in granule..header.granules() {
                        self.append_granule(header, g);
                    }
                    break;
                }
            }
        }
        self.trim_reservoir();
        Ok(true)
    }

    fn append_granule(&mut self, header: &FrameHeader, granule: usize) {
        if granule == 0 {
            self.out.clear();
        }
        let channels = header.channels();
        for i in 0..576 {
            for ch in 0..channels {
                self.out.push(self.granule[ch][i]);
            }
        }
    }

    fn emit(&mut self, header: &FrameHeader, decoded: bool) -> Result<(), AudioError> {
        if !decoded {
            self.out.clear();
            self.out.resize(header.samples_per_frame() * header.channels(), 0.0);
        }
        let channels = header.channels().max(1);
        if self.gapless && self.skip_front > 0 {
            let have = (self.out.len() / channels) as u64;
            let drop = self.skip_front.min(have);
            self.out.drain(..drop as usize * channels);
            self.skip_front -= drop;
        }
        let frames = (self.out.len() / channels) as u64;
        self.samples_emitted += frames;
        if self.samples_emitted > self.limits.max_frames as u64 {
            return Err(AudioError::TooLarge("mp3 stream exceeds the frame budget"));
        }
        Ok(())
    }

    fn push_reservoir(&mut self, main: &[u8]) {
        // A frame can never contribute more than its own length, so this is
        // bounded by the bitrate, not by anything the file claims.
        self.reservoir.extend_from_slice(main);
    }

    fn trim_reservoir(&mut self) {
        if self.reservoir.len() > MAX_RESERVOIR {
            let drop = self.reservoir.len() - MAX_RESERVOIR;
            self.reservoir.drain(..drop);
        }
    }
}

/// Decode a whole MP3 to interleaved `f32`.
pub fn decode_all(bytes: &[u8]) -> Result<DecodedAudio, AudioError> {
    decode_all_limited(bytes, Limits::default())
}

pub fn decode_all_limited(bytes: &[u8], limits: Limits) -> Result<DecodedAudio, AudioError> {
    let mut decoder = Mp3Decoder::with_limits(bytes, limits)?;
    let mut pcm: Vec<f32> = Vec::new();
    let (mut rate, mut channels) = (0u32, 0u16);
    while let Some(frame) = decoder.next_frame()? {
        if channels != 0 && (frame.channels != channels || frame.rate != rate) {
            // One buffer cannot be both; a caller that wants the tail anyway
            // can drive `next_frame` itself and watch the format.
            return Err(AudioError::Unsupported("format changes mid-stream"));
        }
        rate = frame.rate;
        channels = frame.channels;
        pcm.extend_from_slice(frame.pcm);
    }
    // The gapless tail trim needs the whole stream, so it happens here rather
    // than in the streaming path.
    let (_, skip_back) = decoder.trim();
    if skip_back > 0 && channels > 0 {
        let drop = (skip_back as usize).saturating_mul(channels as usize).min(pcm.len());
        pcm.truncate(pcm.len() - drop);
    }
    if pcm.is_empty() || channels == 0 {
        return Err(AudioError::Empty);
    }
    Ok(DecodedAudio { rate, channels, pcm_interleaved_f32: pcm })
}

/// Duration in seconds without decoding.
pub fn probe_duration(bytes: &[u8]) -> Result<f64, AudioError> {
    header::probe_duration(bytes)
}

/// ID3v2 text frames. Never fails: a file with an unreadable tag is still a
/// file we want to play.
pub fn read_tags(bytes: &[u8]) -> Tags {
    let mut tags = Tags::default();
    read_id3v2(bytes, &mut tags);
    tags
}

fn read_id3v2(bytes: &[u8], tags: &mut Tags) {
    let total = header::id3v2_len(bytes);
    if total < 10 {
        return;
    }
    let major = bytes[3];
    let flags = bytes[5];
    let body_end = total.min(bytes.len());
    let mut at = 10usize;
    if flags & 0x40 != 0 {
        // Extended header: its first four bytes are its own length.
        let Some(raw) = bytes.get(at..at + 4) else { return };
        let size = if major >= 4 {
            syncsafe(raw)
        } else {
            (u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]) as usize).saturating_add(4)
        };
        at = at.saturating_add(size.max(4));
    }
    // ID3v2.2 uses three-byte identifiers and sizes; 2.3 and 2.4 use four.
    let (id_len, header_len) = if major <= 2 { (3usize, 6usize) } else { (4, 10) };
    while at.saturating_add(header_len) <= body_end {
        let id = &bytes[at..at + id_len];
        if id[0] == 0 {
            break; // padding
        }
        let size = match id_len {
            3 => ((bytes[at + 3] as usize) << 16)
                | ((bytes[at + 4] as usize) << 8)
                | bytes[at + 5] as usize,
            _ if major >= 4 => syncsafe(&bytes[at + 4..at + 8]),
            _ => u32::from_be_bytes([
                bytes[at + 4],
                bytes[at + 5],
                bytes[at + 6],
                bytes[at + 7],
            ]) as usize,
        };
        let body_start = at + header_len;
        let Some(body) = bytes.get(body_start..body_start.saturating_add(size)) else {
            break;
        };
        if body_end < body_start + size {
            break;
        }
        let Ok(name) = std::str::from_utf8(id) else { break };
        if name.starts_with('T') && !body.is_empty() {
            let text = decode_id3_text(body[0], &body[1..]);
            if name.eq_ignore_ascii_case("TXXX") {
                // "description\0value": key on the description.
                if let Some((key, value)) = text.split_once('\u{0}') {
                    tags.push(key, value);
                } else {
                    tags.push(name, &text);
                }
            } else {
                // Multi-value frames separate with NUL; keep the first.
                tags.push(name, text.split('\u{0}').next().unwrap_or(&text));
            }
        }
        at = body_start.saturating_add(size);
    }
}

fn syncsafe(raw: &[u8]) -> usize {
    raw.iter().fold(0usize, |acc, b| (acc << 7) | (b & 0x7f) as usize)
}

/// ID3 text encodings: Latin-1, UTF-16 with BOM, UTF-16BE, UTF-8.
fn decode_id3_text(encoding: u8, body: &[u8]) -> String {
    match encoding {
        0 => body.iter().map(|&b| b as char).collect(),
        1 | 2 => {
            let (units, big_endian) = match (encoding, body) {
                (1, [0xff, 0xfe, rest @ ..]) => (rest, false),
                (1, [0xfe, 0xff, rest @ ..]) => (rest, true),
                (1, rest) => (rest, false),
                (_, rest) => (rest, true),
            };
            let code_units: Vec<u16> = units
                .chunks_exact(2)
                .map(|p| {
                    if big_endian {
                        u16::from_be_bytes([p[0], p[1]])
                    } else {
                        u16::from_le_bytes([p[0], p[1]])
                    }
                })
                .collect();
            String::from_utf16_lossy(&code_units)
        }
        _ => String::from_utf8_lossy(body).into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn garbage_is_refused_without_panicking() {
        assert!(matches!(decode_all(b""), Err(AudioError::Empty)));
        assert!(decode_all(b"this is not an mp3 file, not even a little").is_err());
        assert!(Mp3Decoder::new(&[0xff; 64]).is_err());
    }

    #[test]
    fn id3v2_text_frames_reach_the_tags() {
        // ID3v2.3 with TIT2 (Latin-1) and TPE1 (UTF-16LE with BOM).
        let mut body: Vec<u8> = Vec::new();
        let mut frame = |id: &[u8; 4], payload: &[u8]| {
            body.extend_from_slice(id);
            body.extend_from_slice(&(payload.len() as u32).to_be_bytes());
            body.extend_from_slice(&[0, 0]);
            body.extend_from_slice(payload);
        };
        frame(b"TIT2", &[0, b'S', b'o', b'n', b'g']);
        let utf16: Vec<u8> = [0x01u8, 0xff, 0xfe]
            .iter()
            .copied()
            .chain("Björk".encode_utf16().flat_map(|u| u.to_le_bytes()))
            .collect();
        frame(b"TPE1", &utf16);

        let mut file = vec![0u8; 10];
        file[0..3].copy_from_slice(b"ID3");
        file[3] = 3;
        let size = body.len();
        file[6] = ((size >> 21) & 0x7f) as u8;
        file[7] = ((size >> 14) & 0x7f) as u8;
        file[8] = ((size >> 7) & 0x7f) as u8;
        file[9] = (size & 0x7f) as u8;
        file.extend_from_slice(&body);

        let tags = read_tags(&file);
        assert_eq!(tags.title.as_deref(), Some("Song"));
        assert_eq!(tags.artist.as_deref(), Some("Björk"));
    }

    #[test]
    fn id3v2_parser_survives_a_lying_frame_size() {
        let mut file = vec![0u8; 10];
        file[0..3].copy_from_slice(b"ID3");
        file[3] = 3;
        file[9] = 20;
        file.extend_from_slice(b"TIT2");
        file.extend_from_slice(&[0x7f, 0xff, 0xff, 0xff, 0, 0]); // absurd size
        file.extend_from_slice(&[0, b'x']);
        let tags = read_tags(&file);
        assert!(tags.title.is_none());
    }

    #[test]
    fn text_decoders_handle_every_id3_encoding() {
        assert_eq!(decode_id3_text(0, &[0xe9, b'c']), "éc");
        assert_eq!(decode_id3_text(3, "utf8 ✓".as_bytes()), "utf8 ✓");
        let be: Vec<u8> = "hi".encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
        assert_eq!(decode_id3_text(2, &be), "hi");
        let le: Vec<u8> = [0xffu8, 0xfe]
            .iter()
            .copied()
            .chain("hi".encode_utf16().flat_map(|u| u.to_le_bytes()))
            .collect();
        assert_eq!(decode_id3_text(1, &le), "hi");
        // An odd trailing byte must not panic.
        assert_eq!(decode_id3_text(2, &[0, b'h', 0]), "h");
    }
}
