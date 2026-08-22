//! Frame headers, the containers wrapped around them (ID3v1/ID3v2), and the
//! VBR headers (Xing/Info, VBRI, LAME) that carry a duration without a decode.

use crate::error::AudioError;

use super::tables::{SFB_LONG_MPEG1, SFB_SHORT_MPEG1};
use super::tables_lsf::{SFB_LONG_LSF, SFB_SHORT_LSF};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Version {
    Mpeg1,
    /// MPEG-2, "low sampling frequency": half the rates, one granule.
    Mpeg2,
    /// MPEG-2.5, Fraunhofer's unofficial-but-universal quarter-rate extension.
    Mpeg25,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelMode {
    Stereo,
    JointStereo,
    DualChannel,
    Mono,
}

/// Layer III bitrates in kbit/s by `bitrate_index`. Index 0 is "free format"
/// (the frame length is not derivable from the header) and index 15 is
/// reserved; both are rejected as `None`.
const BITRATES_MPEG1: [u32; 16] =
    [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0];
const BITRATES_LSF: [u32; 16] =
    [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0];

const RATES_MPEG1: [u32; 3] = [44_100, 48_000, 32_000];
const RATES_MPEG2: [u32; 3] = [22_050, 24_000, 16_000];
const RATES_MPEG25: [u32; 3] = [11_025, 12_000, 8_000];

/// A parsed 32-bit frame header. Constructing one guarantees the frame is a
/// Layer III frame with a derivable length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameHeader {
    pub version: Version,
    pub protected: bool,
    pub bitrate_kbps: u32,
    pub sample_rate: u32,
    pub rate_index: usize,
    pub padding: bool,
    pub mode: ChannelMode,
    pub mode_ext: u8,
    /// Whole frame including the header and any CRC, in bytes.
    pub frame_bytes: usize,
}

impl FrameHeader {
    /// Parse a big-endian header word. `None` when it is not a valid Layer III
    /// header, including the free-format and reserved encodings this decoder
    /// refuses.
    pub fn parse(word: u32) -> Option<Self> {
        if word >> 21 != 0x7ff {
            return None;
        }
        let version = match (word >> 19) & 3 {
            0 => Version::Mpeg25,
            2 => Version::Mpeg2,
            3 => Version::Mpeg1,
            _ => return None, // reserved
        };
        if (word >> 17) & 3 != 1 {
            return None; // not Layer III
        }
        let protected = (word >> 16) & 1 == 0;
        let bitrate_index = ((word >> 12) & 0xf) as usize;
        let rate_index = ((word >> 10) & 3) as usize;
        if rate_index > 2 {
            return None; // reserved sampling frequency
        }
        let bitrate_kbps = match version {
            Version::Mpeg1 => BITRATES_MPEG1[bitrate_index],
            _ => BITRATES_LSF[bitrate_index],
        };
        if bitrate_kbps == 0 {
            return None; // free format or reserved: length is not derivable
        }
        let sample_rate = match version {
            Version::Mpeg1 => RATES_MPEG1[rate_index],
            Version::Mpeg2 => RATES_MPEG2[rate_index],
            Version::Mpeg25 => RATES_MPEG25[rate_index],
        };
        let padding = (word >> 9) & 1 == 1;
        let mode = match (word >> 6) & 3 {
            0 => ChannelMode::Stereo,
            1 => ChannelMode::JointStereo,
            2 => ChannelMode::DualChannel,
            _ => ChannelMode::Mono,
        };
        let mode_ext = ((word >> 4) & 3) as u8;
        let samples = if matches!(version, Version::Mpeg1) { 1152 } else { 576 };
        let frame_bytes = (samples / 8) * bitrate_kbps as usize * 1000 / sample_rate as usize
            + usize::from(padding);
        Some(Self {
            version,
            protected,
            bitrate_kbps,
            sample_rate,
            rate_index,
            padding,
            mode,
            mode_ext,
            frame_bytes,
        })
    }

    pub fn is_lsf(&self) -> bool {
        !matches!(self.version, Version::Mpeg1)
    }

    pub fn channels(&self) -> usize {
        if matches!(self.mode, ChannelMode::Mono) {
            1
        } else {
            2
        }
    }

    /// Granules per frame: two for MPEG-1, one for the half-rate versions.
    pub fn granules(&self) -> usize {
        if self.is_lsf() {
            1
        } else {
            2
        }
    }

    pub fn samples_per_frame(&self) -> usize {
        self.granules() * 576
    }

    pub fn side_info_bytes(&self) -> usize {
        match (self.is_lsf(), self.channels()) {
            (false, 1) => 17,
            (false, _) => 32,
            (true, 1) => 9,
            (true, _) => 17,
        }
    }

    /// Bytes of main data this frame contributes to the reservoir.
    pub fn main_data_bytes(&self) -> usize {
        self.frame_bytes
            .saturating_sub(4 + usize::from(self.protected) * 2 + self.side_info_bytes())
    }

    pub fn mid_side(&self) -> bool {
        matches!(self.mode, ChannelMode::JointStereo) && self.mode_ext & 2 != 0
    }

    pub fn intensity(&self) -> bool {
        matches!(self.mode, ChannelMode::JointStereo) && self.mode_ext & 1 != 0
    }

    /// Long-block scalefactor band boundaries for this frame's rate.
    pub fn sfb_long(&self) -> &'static [u16; 23] {
        match self.version {
            Version::Mpeg1 => &SFB_LONG_MPEG1[self.rate_index],
            Version::Mpeg2 => &SFB_LONG_LSF[self.rate_index],
            Version::Mpeg25 => &SFB_LONG_LSF[self.rate_index + 3],
        }
    }

    /// Short-block boundaries, in lines per window.
    pub fn sfb_short(&self) -> &'static [u16; 14] {
        match self.version {
            Version::Mpeg1 => &SFB_SHORT_MPEG1[self.rate_index],
            Version::Mpeg2 => &SFB_SHORT_LSF[self.rate_index],
            Version::Mpeg25 => &SFB_SHORT_LSF[self.rate_index + 3],
        }
    }

    /// MPEG-2.5's lowest rate, whose scalefactor bands are twice as wide as
    /// every other rate's and which therefore needs its own boundaries.
    pub fn is_8khz(&self) -> bool {
        matches!(self.version, Version::Mpeg25) && self.rate_index == 2
    }

    /// Start of region 1 when the granule uses window switching, where the
    /// region boundaries are fixed rather than coded (2.4.2.7). 8 kHz has its
    /// own value because its bands are twice as wide.
    pub fn switched_region1_start(&self, short_blocks: bool) -> usize {
        match (short_blocks, self.is_8khz()) {
            (true, false) => 36,
            (true, true) => 72,
            (false, true) => 108,
            (false, false) if self.is_lsf() => 54,
            (false, false) => 36,
        }
    }

    /// Two headers belong to the same stream when these agree; a decoder uses
    /// it to reject a false sync inside audio data.
    pub fn compatible_with(&self, other: &FrameHeader) -> bool {
        self.version == other.version
            && self.sample_rate == other.sample_rate
            && self.channels() == other.channels()
    }
}

/// Length of an ID3v2 tag at the start of `bytes`, or 0 when there is none.
pub fn id3v2_len(bytes: &[u8]) -> usize {
    if bytes.len() < 10 || &bytes[0..3] != b"ID3" || bytes[3] == 0xff || bytes[4] == 0xff {
        return 0;
    }
    // Syncsafe: seven bits per byte, so a size field can never contain a false
    // frame sync.
    let mut size = 0usize;
    for &b in &bytes[6..10] {
        if b & 0x80 != 0 {
            return 0;
        }
        size = (size << 7) | (b & 0x7f) as usize;
    }
    let footer = usize::from(bytes[5] & 0x10 != 0) * 10;
    (10 + size + footer).min(bytes.len())
}

/// Audio payload with ID3v2 (front) and ID3v1 (back) trimmed off.
pub fn strip_containers(bytes: &[u8]) -> &[u8] {
    let start = id3v2_len(bytes);
    let mut end = bytes.len();
    if end >= start + 128 && &bytes[end - 128..end - 125] == b"TAG" {
        end -= 128;
    }
    bytes.get(start..end).unwrap_or(&[])
}

/// Offset of the first valid frame header at or after `from`, together with
/// the header. A header only counts when a second, compatible header sits
/// exactly one frame later (or the stream ends there), which is what keeps a
/// run of audio bytes that happens to contain `0xffe` from starting a decode
/// in the middle of a frame.
pub fn find_frame(bytes: &[u8], from: usize) -> Option<(usize, FrameHeader)> {
    let mut at = from;
    while at + 4 <= bytes.len() {
        // The sync word is byte-aligned and starts with 0xff.
        let Some(rel) = bytes[at..].iter().position(|&b| b == 0xff) else {
            return None;
        };
        at += rel;
        if at + 4 > bytes.len() {
            return None;
        }
        let word = u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
        if let Some(header) = FrameHeader::parse(word) {
            let next = at + header.frame_bytes;
            let confirmed = if next + 4 <= bytes.len() {
                let w2 = u32::from_be_bytes([
                    bytes[next],
                    bytes[next + 1],
                    bytes[next + 2],
                    bytes[next + 3],
                ]);
                FrameHeader::parse(w2).is_some_and(|h2| header.compatible_with(&h2))
            } else {
                // Nothing left to confirm against: accept only a frame that
                // actually fits, so a stray sync in the last few bytes of a
                // truncated file cannot start a decode.
                next <= bytes.len()
            };
            if confirmed {
                return Some((at, header));
            }
        }
        at += 1;
    }
    None
}

/// True when `bytes` plausibly holds an MP3 stream.
pub fn looks_like_mp3(bytes: &[u8]) -> bool {
    if bytes.len() >= 3 && &bytes[0..3] == b"ID3" {
        return true;
    }
    let audio = strip_containers(bytes);
    // Only scan the head: a sync this far in is not the start of a stream.
    let window = audio.len().min(1 << 16);
    find_frame(&audio[..window], 0).is_some()
}

/// What a Xing/Info or VBRI header says about the whole stream.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VbrInfo {
    /// Frames in the stream, excluding the header frame itself.
    pub frames: Option<u32>,
    pub bytes: Option<u32>,
    /// LAME encoder delay and padding, in samples.
    pub delay: Option<(u16, u16)>,
}

/// Parse the VBR header carried in `frame` (the whole first frame, starting at
/// its sync word), if it has one.
pub fn parse_vbr_header(frame: &[u8], header: &FrameHeader) -> Option<VbrInfo> {
    let at = 4 + usize::from(header.protected) * 2 + header.side_info_bytes();
    if let Some(tag) = frame.get(at..at + 8) {
        if &tag[0..4] == b"Xing" || &tag[0..4] == b"Info" {
            return parse_xing(frame, at);
        }
    }
    // VBRI sits at a fixed offset instead, and only ever in Fraunhofer files.
    if let Some(tag) = frame.get(36..36 + 26) {
        if &tag[0..4] == b"VBRI" {
            let frames = u32::from_be_bytes(tag[14..18].try_into().ok()?);
            let bytes = u32::from_be_bytes(tag[10..14].try_into().ok()?);
            return Some(VbrInfo { frames: Some(frames), bytes: Some(bytes), delay: None });
        }
    }
    None
}

fn parse_xing(frame: &[u8], at: usize) -> Option<VbrInfo> {
    let flags = u32::from_be_bytes(frame.get(at + 4..at + 8)?.try_into().ok()?);
    let mut cursor = at + 8;
    let take4 = |cursor: &mut usize| -> Option<u32> {
        let v = u32::from_be_bytes(frame.get(*cursor..*cursor + 4)?.try_into().ok()?);
        *cursor += 4;
        Some(v)
    };
    let frames = if flags & 1 != 0 { take4(&mut cursor) } else { None };
    let bytes = if flags & 2 != 0 { take4(&mut cursor) } else { None };
    if flags & 4 != 0 {
        cursor += 100; // seek table
    }
    if flags & 8 != 0 {
        cursor += 4; // quality indicator
    }
    // The LAME/Lavc extension, when present, starts right here.
    let delay = frame
        .get(cursor..cursor + 24)
        .filter(|tag| matches!(&tag[0..4], b"LAME" | b"Lavc" | b"Lavf"))
        .map(|tag| {
            let d = ((tag[21] as u16) << 4) | (tag[22] as u16) >> 4;
            let p = (((tag[22] & 0xf) as u16) << 8) | tag[23] as u16;
            (d, p)
        });
    Some(VbrInfo { frames, bytes, delay })
}

/// Frames a duration probe will walk before giving up. Two hours of MPEG-1 is
/// about 300 000, so this only ever stops a crafted file.
const MAX_PROBE_FRAMES: u64 = 4_000_000;

/// Duration in seconds without decoding any audio.
///
/// A Xing/VBRI header answers immediately. Failing that this walks the frame
/// chain, which is header arithmetic only — four bytes read per frame, no
/// Huffman and no filterbank — and is exact for both constant and variable
/// bitrate. Estimating from the file length instead is off by whatever
/// non-audio bytes are appended, which on a real library is a tag the decoder
/// skips but a byte count does not.
pub fn probe_duration(bytes: &[u8]) -> Result<f64, AudioError> {
    let audio = strip_containers(bytes);
    let (at, header) = find_frame(audio, 0).ok_or(AudioError::Empty)?;
    let frame = audio.get(at..).unwrap_or(&[]);
    let rate = header.sample_rate as f64;
    if let Some(vbr) = parse_vbr_header(frame, &header) {
        if let Some(frames) = vbr.frames.filter(|&f| f > 0) {
            return Ok(frames as f64 * header.samples_per_frame() as f64 / rate);
        }
    }
    let mut samples = 0u64;
    let mut frames = 0u64;
    let mut cursor = at;
    while cursor + 4 <= audio.len() && frames < MAX_PROBE_FRAMES {
        let word =
            u32::from_be_bytes([audio[cursor], audio[cursor + 1], audio[cursor + 2], audio[cursor + 3]]);
        match FrameHeader::parse(word) {
            Some(next) if header.compatible_with(&next) && next.frame_bytes > 4 => {
                samples += next.samples_per_frame() as u64;
                frames += 1;
                cursor += next.frame_bytes;
            }
            // Damage, or the tail of the file: resync forward. `find_frame`
            // only ever moves the cursor on, so the whole walk stays linear.
            _ => match find_frame(audio, cursor + 1) {
                Some((next, _)) => cursor = next,
                None => break,
            },
        }
    }
    Ok(samples as f64 / rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 44.1 kHz, 128 kbit/s, joint stereo (mid/side), MPEG-1 Layer III, no CRC.
    const MPEG1_JS: u32 = 0xfffb_9064;

    #[test]
    fn parses_a_typical_mpeg1_header() {
        let h = FrameHeader::parse(MPEG1_JS).expect("valid header");
        assert!(h.mid_side() && !h.intensity());
        assert_eq!(h.version, Version::Mpeg1);
        assert_eq!(h.sample_rate, 44_100);
        assert_eq!(h.bitrate_kbps, 128);
        assert_eq!(h.mode, ChannelMode::JointStereo);
        assert_eq!(h.channels(), 2);
        assert_eq!(h.granules(), 2);
        assert_eq!(h.side_info_bytes(), 32);
        assert_eq!(h.frame_bytes, 417); // 144 * 128000 / 44100
        assert!(!h.protected);
    }

    #[test]
    fn rejects_reserved_and_free_format_encodings() {
        assert!(FrameHeader::parse(0x0000_0000).is_none());
        assert!(FrameHeader::parse(0xffff_ffff).is_none());
        // reserved version (01)
        assert!(FrameHeader::parse(0xffeb_9004).is_none());
        // layer II instead of III
        assert!(FrameHeader::parse(0xfffd_9004).is_none());
        // plain stereo, for contrast with the joint-stereo constant above
        assert_eq!(
            FrameHeader::parse(0xfffb_9004).map(|h| h.mode),
            Some(ChannelMode::Stereo)
        );
        // free-format bitrate index 0
        assert!(FrameHeader::parse(0xfffb_0004).is_none());
        // reserved bitrate index 15
        assert!(FrameHeader::parse(0xfffb_f004).is_none());
        // reserved sampling frequency
        assert!(FrameHeader::parse(0xfffb_9c04).is_none());
    }

    #[test]
    fn lsf_headers_have_half_the_payload() {
        // MPEG-2, Layer III, no CRC, 64 kbit/s, 22.05 kHz, mono.
        let word = (0x7ff << 21) | (2 << 19) | (1 << 17) | (1 << 16) | (8 << 12) | (3 << 6);
        let h = FrameHeader::parse(word).expect("valid lsf header");
        assert_eq!(h.version, Version::Mpeg2);
        assert_eq!(h.sample_rate, 22_050);
        assert_eq!(h.bitrate_kbps, 64);
        assert!(h.is_lsf());
        assert_eq!(h.granules(), 1);
        assert_eq!(h.samples_per_frame(), 576);
        assert_eq!(h.side_info_bytes(), 9);
        assert_eq!(h.frame_bytes, 208); // 72 * 64000 / 22050
        assert_eq!(h.sfb_long()[22], 576);

        // MPEG-2.5 at 8 kHz picks the widened band table.
        let word = (0x7ff << 21) | (1 << 17) | (1 << 16) | (4 << 12) | (2 << 10) | (3 << 6);
        let h = FrameHeader::parse(word).expect("valid mpeg2.5 header");
        assert_eq!(h.version, Version::Mpeg25);
        assert_eq!(h.sample_rate, 8_000);
        assert_eq!(h.sfb_long()[1], 12);
        assert_eq!(h.switched_region1_start(true), 72);
        assert_eq!(h.switched_region1_start(false), 108);
    }

    #[test]
    fn id3v2_length_is_syncsafe_and_bounded() {
        let mut tag = vec![0u8; 10 + 100];
        tag[0..3].copy_from_slice(b"ID3");
        tag[3] = 3;
        tag[9] = 100;
        assert_eq!(id3v2_len(&tag), 110);
        // A non-syncsafe size byte is not an ID3 header we understand.
        tag[9] = 0x80;
        assert_eq!(id3v2_len(&tag), 0);
        assert_eq!(id3v2_len(b"no tag here"), 0);
        assert_eq!(id3v2_len(&[]), 0);
    }

    #[test]
    fn strip_containers_removes_id3v1() {
        let mut data = vec![0x11u8; 300];
        data[172..175].copy_from_slice(b"TAG");
        assert_eq!(strip_containers(&data).len(), 172);
    }

    #[test]
    fn find_frame_needs_a_confirming_second_header() {
        let h = FrameHeader::parse(MPEG1_JS).unwrap();
        let mut data = vec![0u8; 8 + h.frame_bytes * 2];
        // A lone sync word inside noise must not be accepted.
        data[2..6].copy_from_slice(&MPEG1_JS.to_be_bytes());
        assert!(find_frame(&data[..h.frame_bytes], 0).is_none());
        // Two frames in a row is a stream.
        data[8..12].copy_from_slice(&MPEG1_JS.to_be_bytes());
        data[8 + h.frame_bytes..12 + h.frame_bytes].copy_from_slice(&MPEG1_JS.to_be_bytes());
        let (at, found) = find_frame(&data, 8).expect("stream start");
        assert_eq!(at, 8);
        assert_eq!(found, h);
    }

    #[test]
    fn duration_probe_refuses_garbage() {
        assert!(super::probe_duration(b"not an mp3 at all").is_err());
        assert!(super::probe_duration(&[]).is_err());
    }
}
