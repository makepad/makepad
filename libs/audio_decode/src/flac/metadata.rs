//! METADATA_BLOCKs: STREAMINFO is required and first; VORBIS_COMMENT feeds
//! [`Tags`]; SEEKTABLE is kept; PICTURE is skipped (this crate has no picture
//! slot); everything else is skipped by length.

use crate::error::AudioError;
use crate::tags::Tags;

/// Longest metadata block we will look inside. STREAMINFO is 34 bytes; a
/// comment block or seektable larger than this is skipped rather than
/// copied. The skip itself is an index add, so a lying length still cannot
/// allocate.
const MAX_PARSE_BLOCK: usize = 1 << 20;

/// Most seekpoints kept across the stream's SEEKTABLE metadata.
const MAX_SEEKPOINTS: usize = 65_536;

pub const STREAMINFO_LEN: usize = 34;

#[derive(Clone, Debug)]
pub struct StreamInfo {
    pub min_blocksize: u16,
    pub max_blocksize: u16,
    pub min_framesize: u32,
    pub max_framesize: u32,
    pub sample_rate: u32,
    pub channels: u8,
    pub bits_per_sample: u8,
    pub total_samples: u64,
    pub md5: [u8; 16],
}

impl StreamInfo {
    pub fn md5_is_present(&self) -> bool {
        self.md5.iter().any(|&b| b != 0)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SeekPoint {
    pub sample: u64,
    pub offset: u64,
    pub n_samples: u16,
}

/// One stream's metadata, ending at the first frame byte.
pub struct Metadata {
    pub info: StreamInfo,
    pub tags: Tags,
    pub seektable: Vec<SeekPoint>,
    /// Byte index of the first frame (immediately after the last metadata block).
    pub first_frame: usize,
}

/// `true` when `bytes` starts with the FLAC marker, optionally after an ID3v2
/// tag — some tagged files carry both, and [`crate::mp3::looks_like_mp3`]
/// treats any ID3 prefix as MP3.
pub fn looks_like_flac(bytes: &[u8]) -> bool {
    let at = skip_id3(bytes);
    bytes.get(at..at + 4) == Some(&b"fLaC"[..])
}

pub fn skip_id3(bytes: &[u8]) -> usize {
    if bytes.len() < 10 || &bytes[0..3] != b"ID3" {
        return 0;
    }
    let size = ((bytes[6] as usize & 0x7f) << 21)
        | ((bytes[7] as usize & 0x7f) << 14)
        | ((bytes[8] as usize & 0x7f) << 7)
        | (bytes[9] as usize & 0x7f);
    10usize.saturating_add(size)
}

pub fn parse_metadata(bytes: &[u8], max_channels: usize) -> Result<Metadata, AudioError> {
    let mut at = skip_id3(bytes);
    if bytes.get(at..at + 4) != Some(&b"fLaC"[..]) {
        return Err(AudioError::BadHeader("flac marker"));
    }
    at += 4;

    let mut info: Option<StreamInfo> = None;
    let mut tags = Tags::default();
    let mut seektable = Vec::new();
    let mut saw_streaminfo = false;
    let mut saw_seektable = false;

    loop {
        if at.saturating_add(4) > bytes.len() {
            return Err(AudioError::Truncated);
        }
        let last = bytes[at] & 0x80 != 0;
        let kind = bytes[at] & 0x7f;
        let len = ((bytes[at + 1] as usize) << 16)
            | ((bytes[at + 2] as usize) << 8)
            | bytes[at + 3] as usize;
        at += 4;
        let end = at.checked_add(len).ok_or(AudioError::Truncated)?;
        if end > bytes.len() {
            return Err(AudioError::Truncated);
        }
        let body = &bytes[at..end];

        if !saw_streaminfo {
            if kind != 0 {
                return Err(AudioError::BadHeader("flac streaminfo not first"));
            }
            saw_streaminfo = true;
        }

        match kind {
            0 => {
                if info.is_some() {
                    return Err(AudioError::BadHeader("flac duplicate streaminfo"));
                }
                info = Some(parse_streaminfo(body, max_channels)?);
            }
            3 => {
                if saw_seektable {
                    return Err(AudioError::BadHeader("flac duplicate seektable"));
                }
                saw_seektable = true;
                if body.len() > MAX_PARSE_BLOCK {
                    return Err(AudioError::TooLarge("flac seektable"));
                }
                parse_seektable(body, &mut seektable)?;
            }
            4 => {
                if body.len() > MAX_PARSE_BLOCK {
                    return Err(AudioError::TooLarge("flac vorbis comment"));
                }
                parse_vorbis_comment(body, &mut tags)?;
            }
            6 => {
                // PICTURE. This crate's [`Tags`] has no picture slot, so the
                // block is skipped by length (the header already bounded it).
            }
            127 => return Err(AudioError::BadHeader("flac metadata type 127")),
            _ => {}
        }

        at = end;
        if last {
            break;
        }
    }

    let info = info.ok_or(AudioError::BadHeader("flac streaminfo"))?;
    Ok(Metadata { info, tags, seektable, first_frame: at })
}

fn parse_streaminfo(body: &[u8], max_channels: usize) -> Result<StreamInfo, AudioError> {
    if body.len() != STREAMINFO_LEN {
        return Err(AudioError::BadHeader("flac streaminfo length"));
    }
    let min_blocksize = u16::from_be_bytes([body[0], body[1]]);
    let max_blocksize = u16::from_be_bytes([body[2], body[3]]);
    let min_framesize = ((body[4] as u32) << 16) | ((body[5] as u32) << 8) | body[6] as u32;
    let max_framesize = ((body[7] as u32) << 16) | ((body[8] as u32) << 8) | body[9] as u32;

    // 20 rate | 3 channels-1 | 5 bps-1 | 36 total samples, then 16-byte MD5.
    let w = u64::from_be_bytes([
        body[10], body[11], body[12], body[13], body[14], body[15], body[16], body[17],
    ]);
    let sample_rate = (w >> 44) as u32;
    let channels = ((w >> 41) as u8 & 0x07) + 1;
    let bits_per_sample = ((w >> 36) as u8 & 0x1f) + 1;
    let total_samples = w & ((1u64 << 36) - 1);
    let mut md5 = [0u8; 16];
    md5.copy_from_slice(&body[18..34]);

    if min_blocksize == 0 || min_blocksize > max_blocksize || max_blocksize == 0 {
        return Err(AudioError::BadHeader("flac block size"));
    }
    if min_framesize != 0 && max_framesize != 0 && min_framesize > max_framesize {
        return Err(AudioError::BadHeader("flac frame size range"));
    }
    // A last frame may be short, but STREAMINFO's minimum is 16 except when
    // the whole stream is shorter than that, which we still accept.
    if sample_rate == 0 || sample_rate > 655_350 {
        return Err(AudioError::BadHeader("flac sample rate"));
    }
    if channels as usize > max_channels {
        return Err(AudioError::TooLarge("flac channel count"));
    }
    if !(4..=32).contains(&bits_per_sample) {
        return Err(AudioError::BadHeader("flac sample size"));
    }
    Ok(StreamInfo {
        min_blocksize,
        max_blocksize,
        min_framesize,
        max_framesize,
        sample_rate,
        channels,
        bits_per_sample,
        total_samples,
        md5,
    })
}

fn parse_seektable(body: &[u8], out: &mut Vec<SeekPoint>) -> Result<(), AudioError> {
    if body.len() % 18 != 0 {
        return Err(AudioError::BadHeader("flac seektable"));
    }
    let n = body.len() / 18;
    if n > MAX_SEEKPOINTS.saturating_sub(out.len()) {
        return Err(AudioError::TooLarge("flac seektable"));
    }
    out.reserve(n);
    let mut last_sample = out.last().map(|p| p.sample);
    let mut saw_placeholder = last_sample == Some(u64::MAX);
    for i in 0..n {
        let s = i * 18;
        let sample = u64::from_be_bytes(body[s..s + 8].try_into().unwrap_or([0; 8]));
        let offset = u64::from_be_bytes(body[s + 8..s + 16].try_into().unwrap_or([0; 8]));
        let n_samples = u16::from_be_bytes(body[s + 16..s + 18].try_into().unwrap_or([0; 2]));
        if sample == u64::MAX {
            saw_placeholder = true;
        } else {
            if saw_placeholder || last_sample.is_some_and(|last| sample <= last) {
                return Err(AudioError::BadHeader("flac seektable order"));
            }
            last_sample = Some(sample);
        }
        out.push(SeekPoint { sample, offset, n_samples });
    }
    Ok(())
}

/// Vorbis comments: vendor string, then `KEY=value` pairs. Lengths are
/// little-endian, matching the Vorbis I comment header (no packet type, no
/// framing bit).
fn parse_vorbis_comment(packet: &[u8], tags: &mut Tags) -> Result<(), AudioError> {
    let mut at = 0usize;
    let u32_at = |at: &mut usize| -> Option<usize> {
        let end = at.checked_add(4)?;
        let v = u32::from_le_bytes(packet.get(*at..end)?.try_into().ok()?);
        *at = end;
        Some(v as usize)
    };
    let vendor_len = u32_at(&mut at).ok_or(AudioError::Truncated)?;
    at = at.checked_add(vendor_len).ok_or(AudioError::Truncated)?;
    if at > packet.len() {
        return Err(AudioError::Truncated);
    }
    let count = u32_at(&mut at).ok_or(AudioError::Truncated)?;
    if count > packet.len().saturating_sub(at) / 4 {
        return Err(AudioError::Corrupt("flac vorbis comment count"));
    }
    for _ in 0..count {
        let len = u32_at(&mut at).ok_or(AudioError::Truncated)?;
        let end = at.checked_add(len).filter(|&end| end <= packet.len())
            .ok_or(AudioError::Truncated)?;
        let text = std::str::from_utf8(&packet[at..end])
            .map_err(|_| AudioError::Corrupt("flac vorbis comment utf-8"))?;
        at = end;
        if let Some((key, value)) = text.split_once('=') {
            tags.push(key, value);
        }
    }
    if at != packet.len() {
        return Err(AudioError::Corrupt("flac vorbis comment trailing data"));
    }
    Ok(())
}

/// Incremental MD5 of the unencoded PCM, the signature STREAMINFO carries.
/// Little-endian signed samples, sign-extended to the next whole byte.
pub struct Md5 {
    state: [u32; 4],
    buffer: [u8; 64],
    filled: usize,
    bit_len: u64,
}

impl Md5 {
    pub fn new() -> Self {
        Self {
            state: [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476],
            buffer: [0; 64],
            filled: 0,
            bit_len: 0,
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        let mut data = data;
        self.bit_len = self.bit_len.wrapping_add((data.len() as u64).saturating_mul(8));
        if self.filled > 0 {
            let need = 64 - self.filled;
            if data.len() < need {
                self.buffer[self.filled..self.filled + data.len()].copy_from_slice(data);
                self.filled += data.len();
                return;
            }
            self.buffer[self.filled..].copy_from_slice(&data[..need]);
            let block = self.buffer;
            md5_block(&mut self.state, &block);
            self.filled = 0;
            data = &data[need..];
        }
        while data.len() >= 64 {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[..64]);
            md5_block(&mut self.state, &block);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buffer[..data.len()].copy_from_slice(data);
            self.filled = data.len();
        }
    }

    /// Feed interleaved signed samples packed the way STREAMINFO's MD5 wants.
    pub fn update_pcm(&mut self, samples: &[i64], bps: u8) {
        let width = (bps as usize).div_ceil(8).min(4);
        let mut buf = [0u8; 4];
        for &s in samples {
            let u = s as u32;
            match width {
                1 => self.update(&[s as i8 as u8]),
                2 => self.update(&(s as i16).to_le_bytes()),
                3 => {
                    buf[0] = u as u8;
                    buf[1] = (u >> 8) as u8;
                    buf[2] = (u >> 16) as u8;
                    self.update(&buf[..3]);
                }
                _ => self.update(&(s as i32).to_le_bytes()),
            }
        }
    }

    pub fn finish(mut self) -> [u8; 16] {
        let bit_len = self.bit_len;
        self.update(&[0x80]);
        if self.filled > 56 {
            // Pad this block and an extra one so the length fits.
            let zeros = 64 - self.filled;
            self.update(&[0u8; 64][..zeros]);
        }
        let zeros = 56 - self.filled;
        if zeros > 0 {
            self.update(&[0u8; 64][..zeros]);
        }
        self.update(&bit_len.to_le_bytes());
        let mut out = [0u8; 16];
        for (i, &w) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }
        out
    }
}

fn md5_block(state: &mut [u32; 4], block: &[u8; 64]) {
    let mut w = [0u32; 16];
    for i in 0..16 {
        w[i] = u32::from_le_bytes([
            block[i * 4],
            block[i * 4 + 1],
            block[i * 4 + 2],
            block[i * 4 + 3],
        ]);
    }
    let (mut a, mut b, mut c, mut d) = (state[0], state[1], state[2], state[3]);
    for i in 0..64 {
        let (f, g) = match i {
            0..=15 => ((b & c) | ((!b) & d), i),
            16..=31 => ((d & b) | ((!d) & c), (5 * i + 1) % 16),
            32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
            _ => (c ^ (b | !d), (7 * i) % 16),
        };
        let f = f.wrapping_add(a).wrapping_add(K[i]).wrapping_add(w[g]);
        a = d;
        d = c;
        c = b;
        b = b.wrapping_add(f.rotate_left(S[i]));
    }
    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
}

const S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

const K: [u32; 64] = [
    0xd76a_a478, 0xe8c7_b756, 0x2420_70db, 0xc1bd_ceee, 0xf57c_0faf, 0x4787_c62a, 0xa830_4613,
    0xfd46_9501, 0x6980_98d8, 0x8b44_f7af, 0xffff_5bb1, 0x895c_d7be, 0x6b90_1122, 0xfd98_7193,
    0xa679_438e, 0x49b4_0821, 0xf61e_2562, 0xc040_b340, 0x265e_5a51, 0xe9b6_c7aa, 0xd62f_105d,
    0x0244_1453, 0xd8a1_e681, 0xe7d3_fbc8, 0x21e1_cde6, 0xc337_07d6, 0xf4d5_0d87, 0x455a_14ed,
    0xa9e3_e905, 0xfcef_a3f8, 0x676f_02d9, 0x8d2a_4c8a, 0xfffa_3942, 0x8771_f681, 0x6d9d_6122,
    0xfde5_380c, 0xa4be_ea44, 0x4bde_cfa9, 0xf6bb_4b60, 0xbebf_bc70, 0x289b_7ec6, 0xeaa1_27fa,
    0xd4ef_3085, 0x0488_1d05, 0xd9d4_d039, 0xe6db_99e5, 0x1fa2_7cf8, 0xc4ac_5665, 0xf429_2244,
    0x432a_ff97, 0xab94_23a7, 0xfc93_a039, 0x655b_59c3, 0x8f0c_cc92, 0xffef_f47d, 0x8584_5dd1,
    0x6fa8_7e4f, 0xfe2c_e6e0, 0xa301_4314, 0x4e08_11a1, 0xf753_7e82, 0xbd3a_f235, 0x2ad7_d2bb,
    0xeb86_d391,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_spec_vectors() {
        let hex = |d: [u8; 16]| d.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let m = Md5::new();
        assert_eq!(hex(m.finish()), "d41d8cd98f00b204e9800998ecf8427e");
        let mut m = Md5::new();
        m.update(b"abc");
        assert_eq!(hex(m.finish()), "900150983cd24fb0d6963f7d28e17f72");
        let mut m = Md5::new();
        m.update(b"12345678901234567890123456789012345678901234567890123456789012345678901234567890");
        assert_eq!(hex(m.finish()), "57edf4a22be3c955ac49da2e2107b67a");
    }

    #[test]
    fn looks_like_flac_accepts_marker_and_id3() {
        assert!(looks_like_flac(b"fLaCxxxx"));
        assert!(!looks_like_flac(b"OggS"));
        assert!(!looks_like_flac(b""));
        let mut id3 = vec![b'I', b'D', b'3', 3, 0, 0, 0, 0, 0, 0];
        id3.extend_from_slice(b"fLaC");
        assert!(looks_like_flac(&id3));
        let mut id3_mp3 = vec![b'I', b'D', b'3', 3, 0, 0, 0, 0, 0, 0];
        id3_mp3.extend_from_slice(b"xxxx");
        assert!(!looks_like_flac(&id3_mp3));
    }

    #[test]
    fn streaminfo_fields_round_the_packed_word() {
        let mut body = [0u8; 34];
        body[0..2].copy_from_slice(&16u16.to_be_bytes());
        body[2..4].copy_from_slice(&4096u16.to_be_bytes());
        // rate 44100, 2ch, 16 bps, 1000 samples.
        let mut w = 0u64;
        w |= 44100u64 << 44;
        w |= 1u64 << 41; // channels-1 = 1
        w |= 15u64 << 36; // bps-1 = 15
        w |= 1000;
        body[10..18].copy_from_slice(&w.to_be_bytes());
        let info = parse_streaminfo(&body, 8).unwrap();
        assert_eq!(info.sample_rate, 44100);
        assert_eq!(info.channels, 2);
        assert_eq!(info.bits_per_sample, 16);
        assert_eq!(info.total_samples, 1000);
        assert!(!info.md5_is_present());
    }

    #[test]
    fn vorbis_comment_routes_title() {
        let mut body = Vec::new();
        body.extend_from_slice(&4u32.to_le_bytes());
        body.extend_from_slice(b"xiph");
        body.extend_from_slice(&1u32.to_le_bytes());
        let c = b"TITLE=Hello";
        body.extend_from_slice(&(c.len() as u32).to_le_bytes());
        body.extend_from_slice(c);
        let mut tags = Tags::default();
        parse_vorbis_comment(&body, &mut tags).unwrap();
        assert_eq!(tags.title.as_deref(), Some("Hello"));
    }
}
