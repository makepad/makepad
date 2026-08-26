// Implemented from RFC 9639 (the FLAC format specification, an open IETF standard); no code from reference implementations.
//! FLAC decoder (RFC 9639).
//!
//! Native streams starting with `fLaC`, plus the ID3v2-prefixed files some
//! taggers emit. STREAMINFO is required and first; VORBIS_COMMENT becomes
//! [`Tags`]; SEEKTABLE is parsed; PICTURE and the rest are skipped by length.
//! Frames check both CRCs; a mismatch is an error, never a silent skip.
//!
//! Like the rest of the crate this is total on malformed input — it reads
//! downloaded files, so a corrupt track must degrade to a clean error, never
//! a panic.

pub mod bits;
pub mod frame;
pub mod lpc;
pub mod metadata;
pub mod rice;
pub mod subframe;

use crate::error::AudioError;
use crate::tags::Tags;
use crate::{DecodedAudio, Limits};
use metadata::{Md5, SeekPoint, StreamInfo};

pub use metadata::looks_like_flac;

/// Highest sample rate the uncommon 16-bit×10 coding can name.
const MAX_RATE: u32 = 655_350;
const MAX_BLOCKSIZE: usize = 65_535;
const MAX_CHANNELS: usize = 8;

/// One decoded frame: interleaved samples plus the format they are in.
pub struct Frame<'a> {
    pub rate: u32,
    pub channels: u16,
    /// Interleaved, `channels` values per sample position.
    pub pcm: &'a [f32],
}

/// Progressive FLAC decoding. [`FlacDecoder::next_frame`] hands back one
/// frame at a time out of an internal buffer, so a deck can start playing a
/// track while the rest of it is still being read, and the frame loop never
/// allocates (the subframe scratch is reserved from STREAMINFO).
pub struct FlacDecoder<'a> {
    data: &'a [u8],
    at: usize,
    info: StreamInfo,
    tags: Tags,
    seektable: Vec<SeekPoint>,
    limits: Limits,
    samples_emitted: u64,
    md5: Md5,
    finished: bool,
    scratch: Vec<Vec<i64>>,
    pcm_i64: Vec<i64>,
    out: Vec<f32>,
    frames_decoded: u64,
    variable_blocksize: Option<bool>,
    fixed_blocksize: Option<u32>,
}

impl<'a> FlacDecoder<'a> {
    /// Parse the marker and metadata. No audio is decoded.
    pub fn new(bytes: &'a [u8]) -> Result<Self, AudioError> {
        Self::with_limits(bytes, Limits::default())
    }

    pub fn with_limits(bytes: &'a [u8], limits: Limits) -> Result<Self, AudioError> {
        let max_ch = limits.max_channels.min(MAX_CHANNELS);
        let meta = metadata::parse_metadata(bytes, max_ch)?;
        if meta.info.sample_rate == 0 || meta.info.sample_rate > MAX_RATE {
            return Err(AudioError::BadHeader("flac sample rate"));
        }
        if meta.info.total_samples > limits.max_frames as u64 {
            return Err(AudioError::TooLarge("flac frame count"));
        }
        let channels = meta.info.channels as usize;
        let max_bs = (meta.info.max_blocksize as usize).clamp(1, MAX_BLOCKSIZE);
        let scratch = (0..channels).map(|_| Vec::with_capacity(max_bs)).collect();
        Ok(Self {
            data: bytes,
            at: meta.first_frame,
            info: meta.info,
            tags: meta.tags,
            seektable: meta.seektable,
            limits,
            samples_emitted: 0,
            md5: Md5::new(),
            finished: false,
            scratch,
            pcm_i64: Vec::with_capacity(max_bs.saturating_mul(channels)),
            out: Vec::with_capacity(max_bs.saturating_mul(channels)),
            frames_decoded: 0,
            variable_blocksize: None,
            fixed_blocksize: None,
        })
    }

    pub fn rate(&self) -> u32 {
        self.info.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.info.channels as u16
    }

    pub fn bits_per_sample(&self) -> u8 {
        self.info.bits_per_sample
    }

    pub fn tags(&self) -> &Tags {
        &self.tags
    }

    pub fn seektable(&self) -> &[SeekPoint] {
        &self.seektable
    }

    /// Samples the stream claims to hold, from STREAMINFO. `None` when the
    /// field was left at zero (unknown).
    pub fn total_frames(&self) -> Option<u64> {
        (self.info.total_samples > 0).then_some(self.info.total_samples)
    }

    /// Decode the next frame. `Ok(None)` is a clean end of stream; CRC or MD5
    /// mismatch is `Err`.
    pub fn next_frame(&mut self) -> Result<Option<Frame<'_>>, AudioError> {
        if self.finished {
            return Ok(None);
        }
        if self.at >= self.data.len() {
            self.finish_stream()?;
            return Ok(None);
        }

        if self.total_frames() == Some(self.samples_emitted) {
            return Err(AudioError::Corrupt("flac data after declared total"));
        }

        // Strict mode is the only default: a frame must begin exactly at
        // `self.at`. We never scan forward over damage and call it EOF.
        let (header, consumed) = frame::decode_frame(
            &self.data[self.at..],
            &self.info,
            &mut self.pcm_i64,
            &mut self.scratch,
        )?;

        let channels = header.assignment.channels() as usize;
        if self.variable_blocksize.is_some_and(|v| v != header.variable_blocksize) {
            return Err(AudioError::Corrupt("flac blocking strategy change"));
        }
        self.variable_blocksize.get_or_insert(header.variable_blocksize);

        let expected_number = if header.variable_blocksize {
            self.samples_emitted
        } else {
            self.frames_decoded
        };
        if header.number != expected_number {
            return Err(AudioError::Corrupt("flac frame/sample number sequence"));
        }

        let frame_end = self.at.checked_add(consumed).ok_or(AudioError::Truncated)?;
        if !header.variable_blocksize {
            let fixed = *self.fixed_blocksize.get_or_insert(header.blocksize);
            if header.blocksize != fixed {
                let final_short_frame = header.blocksize < fixed
                    && frame_end == self.data.len()
                    && self.total_frames().is_none_or(|total| {
                        self.samples_emitted.checked_add(header.blocksize as u64) == Some(total)
                    });
                if !final_short_frame {
                    return Err(AudioError::Corrupt("flac fixed block size change"));
                }
            }
        }

        let n = header.blocksize as usize;
        let next_emitted = self
            .samples_emitted
            .checked_add(n as u64)
            .ok_or(AudioError::TooLarge("flac frame count"))?;
        if let Some(total) = self.total_frames() {
            if next_emitted > total {
                return Err(AudioError::Corrupt("flac frame crosses declared total"));
            }
        }
        let inter = n
            .checked_mul(channels)
            .ok_or(AudioError::Corrupt("flac frame samples"))?;
        if inter != self.pcm_i64.len() {
            return Err(AudioError::Corrupt("flac frame samples"));
        }

        if next_emitted > self.limits.max_frames as u64 {
            return Err(AudioError::TooLarge("flac stream exceeds the frame budget"));
        }

        if self.should_hash() {
            self.md5.update_pcm(&self.pcm_i64, self.info.bits_per_sample);
        }

        self.out.clear();
        let scale = scale_for(self.info.bits_per_sample);
        self.out.extend(self.pcm_i64.iter().map(|&s| s as f32 / scale));

        self.samples_emitted = next_emitted;
        self.frames_decoded += 1;
        self.at = frame_end;
        Ok(Some(Frame {
            rate: self.info.sample_rate,
            channels: channels as u16,
            pcm: &self.out,
        }))
    }

    fn should_hash(&self) -> bool {
        self.info.md5_is_present()
    }

    fn finish_stream(&mut self) -> Result<(), AudioError> {
        if self.finished {
            return Ok(());
        }
        if let Some(total) = self.total_frames() {
            if self.samples_emitted != total {
                return Err(AudioError::Truncated);
            }
        }
        if !self.should_hash() {
            self.finished = true;
            return Ok(());
        }
        // `finish` consumes; swap in a dummy so we can still be called again.
        let hasher = std::mem::replace(&mut self.md5, Md5::new());
        let got = hasher.finish();
        if got != self.info.md5 {
            return Err(AudioError::Corrupt("flac md5"));
        }
        self.finished = true;
        Ok(())
    }
}

fn scale_for(bps: u8) -> f32 {
    let shift = bps.saturating_sub(1).min(31);
    (1u32 << shift) as f32
}

/// Decode a whole FLAC file to interleaved `f32`.
pub fn decode_all(bytes: &[u8]) -> Result<DecodedAudio, AudioError> {
    decode_all_limited(bytes, Limits::default())
}

pub fn decode_all_limited(bytes: &[u8], limits: Limits) -> Result<DecodedAudio, AudioError> {
    let mut decoder = FlacDecoder::with_limits(bytes, limits)?;
    let channels = decoder.channels();
    let rate = decoder.rate();
    let mut pcm: Vec<f32> = Vec::new();
    if let Some(total) = decoder.total_frames() {
        if total > limits.max_frames as u64 {
            return Err(AudioError::TooLarge("flac frame count"));
        }
        let total = usize::try_from(total).map_err(|_| AudioError::TooLarge("flac frame count"))?;
        let want = total.saturating_mul(channels as usize);
        pcm.reserve(want.min(bytes.len().saturating_mul(64)));
    }
    let cap = limits.max_frames.saturating_mul(channels as usize);
    while let Some(frame) = decoder.next_frame()? {
        if pcm.len() + frame.pcm.len() > cap {
            return Err(AudioError::TooLarge("flac frame count"));
        }
        pcm.extend_from_slice(frame.pcm);
    }
    if pcm.is_empty() || channels == 0 {
        return Err(AudioError::Empty);
    }
    Ok(DecodedAudio { rate, channels, pcm_interleaved_f32: pcm })
}

/// Duration in seconds from STREAMINFO's sample count and rate.
pub fn probe_duration(bytes: &[u8]) -> Result<f64, AudioError> {
    let meta = metadata::parse_metadata(bytes, MAX_CHANNELS)?;
    if meta.info.total_samples == 0 || meta.info.sample_rate == 0 {
        return Err(AudioError::BadHeader("flac total samples"));
    }
    Ok(meta.info.total_samples as f64 / meta.info.sample_rate as f64)
}

/// Vorbis comments from a VORBIS_COMMENT block, if any. STREAMINFO must still
/// parse; a file with no comment block yields empty tags.
pub fn read_tags(bytes: &[u8]) -> Result<Tags, AudioError> {
    Ok(metadata::parse_metadata(bytes, MAX_CHANNELS)?.tags)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flac::bits::write_utf8_uint;
    use crate::flac::frame::{crc16, crc8};
    use crate::flac::metadata::STREAMINFO_LEN;
    use crate::flac::rice::zigzag;

    struct BitWriter {
        bytes: Vec<u8>,
        bit: usize,
    }
    impl BitWriter {
        fn new() -> Self {
            Self { bytes: Vec::new(), bit: 0 }
        }
        fn put(&mut self, value: u64, n: u32) {
            for i in (0..n).rev() {
                if self.bit % 8 == 0 {
                    self.bytes.push(0);
                }
                if (value >> i) & 1 == 1 {
                    let at = self.bytes.len() - 1;
                    self.bytes[at] |= 0x80 >> (self.bit % 8);
                }
                self.bit += 1;
            }
        }
        fn put_signed(&mut self, value: i64, n: u32) {
            let mask = if n >= 64 { u64::MAX } else { (1u64 << n) - 1 };
            self.put((value as u64) & mask, n);
        }
        fn align(&mut self) {
            while self.bit % 8 != 0 {
                self.put(0, 1);
            }
        }
    }

    fn streaminfo_body(
        rate: u32,
        channels: u8,
        bps: u8,
        total: u64,
        blocksize: u16,
        md5: [u8; 16],
    ) -> [u8; 34] {
        let mut body = [0u8; 34];
        body[0..2].copy_from_slice(&blocksize.to_be_bytes());
        body[2..4].copy_from_slice(&blocksize.to_be_bytes());
        let mut w = 0u64;
        w |= (rate as u64) << 44;
        w |= ((channels as u64 - 1) & 7) << 41;
        w |= ((bps as u64 - 1) & 31) << 36;
        w |= total & ((1u64 << 36) - 1);
        body[10..18].copy_from_slice(&w.to_be_bytes());
        body[18..34].copy_from_slice(&md5);
        body
    }

    fn push_block(out: &mut Vec<u8>, last: bool, kind: u8, body: &[u8]) {
        let mut hdr = [0u8; 4];
        hdr[0] = kind | if last { 0x80 } else { 0 };
        let len = body.len();
        hdr[1] = (len >> 16) as u8;
        hdr[2] = (len >> 8) as u8;
        hdr[3] = len as u8;
        out.extend_from_slice(&hdr);
        out.extend_from_slice(body);
    }

    /// One or more CONSTANT frames of `samples` (interleaved), plus STREAMINFO
    /// and an optional VORBIS_COMMENT. MD5 is filled in after the samples.
    fn encode_constant_flac(
        rate: u32,
        channels: u8,
        bps: u8,
        samples: &[i64],
        comments: &[(&str, &str)],
    ) -> Vec<u8> {
        let n_frames_total = samples.len() / channels as usize;
        let mut md5 = Md5::new();
        md5.update_pcm(samples, bps);
        let digest = md5.finish();

        let mut file = b"fLaC".to_vec();
        let info = streaminfo_body(rate, channels, bps, n_frames_total as u64, 1, digest);
        if comments.is_empty() {
            push_block(&mut file, true, 0, &info);
        } else {
            push_block(&mut file, false, 0, &info);
            let mut comment = Vec::new();
            comment.extend_from_slice(&4u32.to_le_bytes());
            comment.extend_from_slice(b"test");
            comment.extend_from_slice(&(comments.len() as u32).to_le_bytes());
            for &(k, v) in comments {
                let pair = format!("{k}={v}");
                comment.extend_from_slice(&(pair.len() as u32).to_le_bytes());
                comment.extend_from_slice(pair.as_bytes());
            }
            push_block(&mut file, true, 4, &comment);
        }

        // One CONSTANT subframe per channel, one sample per frame — enough to
        // exercise the frame loop and the MD5 of the whole stream.
        for i in 0..n_frames_total {
            let mut frame = Vec::new();
            frame.push(0xFF);
            frame.push(0xF8);
            // blocksize code 0001 = 192 is too big; use 0110 + 8-bit (n-1).
            // sample rate 0000 = STREAMINFO; channels independent; bps 000.
            let ch_code = channels - 1;
            frame.push(0x60); // bs 0110, sr 0000
            frame.push(ch_code << 4); // bps from STREAMINFO, reserved 0
            write_utf8_uint(&mut frame, i as u64);
            frame.push(0); // (blocksize-1) = 0 → blocksize 1
            let crc = crc8(&frame);
            frame.push(crc);

            let mut w = BitWriter::new();
            for ch in 0..channels as usize {
                w.put(0, 1);
                w.put(0, 6); // CONSTANT
                w.put(0, 1);
                w.put_signed(samples[i * channels as usize + ch], bps as u32);
            }
            w.align();
            frame.extend_from_slice(&w.bytes);
            let fcrc = crc16(&frame);
            frame.extend_from_slice(&fcrc.to_be_bytes());
            file.extend_from_slice(&frame);
        }
        file
    }

    fn encode_stereo_assignment(bps: u8, code: u8, left: i64, right: i64) -> Vec<u8> {
        let original = [left, right];
        let mut md5 = Md5::new();
        md5.update_pcm(&original, bps);
        let mut file = b"fLaC".to_vec();
        let info = streaminfo_body(48_000, 2, bps, 1, 1, md5.finish());
        push_block(&mut file, true, 0, &info);

        let mut frame = vec![0xFF, 0xF8, 0x60, code << 4, 0, 0];
        frame.push(crc8(&frame));
        let side = left - right;
        let (coded, widths) = match code {
            1 => ([left, right], [bps, bps]),
            8 => ([left, side], [bps, bps + 1]),
            9 => ([side, right], [bps + 1, bps]),
            10 => ([(left + right) >> 1, side], [bps, bps + 1]),
            _ => panic!("test channel assignment"),
        };
        let mut w = BitWriter::new();
        for ch in 0..2 {
            w.put(0, 1);
            w.put(0, 6); // CONSTANT
            w.put(0, 1);
            w.put_signed(coded[ch], widths[ch] as u32);
        }
        w.align();
        frame.extend_from_slice(&w.bytes);
        let frame_crc = crc16(&frame);
        frame.extend_from_slice(&frame_crc.to_be_bytes());
        file.extend_from_slice(&frame);
        file
    }

    fn set_total_samples(file: &mut [u8], total: u64) {
        let mut packed = u64::from_be_bytes(file[18..26].try_into().unwrap());
        packed = (packed & !((1u64 << 36) - 1)) | total;
        file[18..26].copy_from_slice(&packed.to_be_bytes());
    }

    fn rewrite_single_frame_crcs(file: &mut [u8]) {
        let start = 4 + 4 + STREAMINFO_LEN;
        let header_crc = start + 6;
        file[header_crc] = crc8(&file[start..header_crc]);
        let footer = file.len() - 2;
        let value = crc16(&file[start..footer]);
        file[footer..].copy_from_slice(&value.to_be_bytes());
    }

    #[test]
    fn constant_stereo_roundtrip_and_md5() {
        let samples = [1000i64, -1000, 2000, -2000, 0, 1];
        let bytes = encode_constant_flac(44_100, 2, 16, &samples, &[("TITLE", "Ping")]);
        let audio = decode_all(&bytes).unwrap();
        assert_eq!(audio.rate, 44_100);
        assert_eq!(audio.channels, 2);
        assert_eq!(audio.frames(), 3);
        let scale = 32768.0;
        for (i, &s) in samples.iter().enumerate() {
            let got = audio.pcm_interleaved_f32[i];
            assert!((got - s as f32 / scale).abs() < 1e-9, "{got} vs {}", s as f32 / scale);
        }
        let tags = read_tags(&bytes).unwrap();
        assert_eq!(tags.title.as_deref(), Some("Ping"));
        assert_eq!(probe_duration(&bytes).unwrap(), 3.0 / 44_100.0);
        assert!(looks_like_flac(&bytes));
    }

    #[test]
    fn streaming_frames_concatenate_to_the_whole_decode() {
        let samples: Vec<i64> = (0..32).map(|i| i * 100 - 800).collect();
        let bytes = encode_constant_flac(48_000, 1, 16, &samples, &[]);
        let whole = decode_all(&bytes).unwrap();
        let mut decoder = FlacDecoder::new(&bytes).unwrap();
        assert_eq!(decoder.rate(), 48_000);
        assert_eq!(decoder.channels(), 1);
        let mut pieces = Vec::new();
        while let Some(frame) = decoder.next_frame().unwrap() {
            pieces.extend_from_slice(frame.pcm);
        }
        assert_eq!(pieces, whole.pcm_interleaved_f32);
    }

    #[test]
    fn wrong_md5_is_corrupt() {
        let samples = [1i64, 2];
        let mut bytes = encode_constant_flac(8_000, 1, 16, &samples, &[]);
        // Flip a bit of the stored MD5 (STREAMINFO starts at byte 8: 4 marker + 4 header).
        bytes[8 + 18] ^= 1;
        assert!(matches!(decode_all(&bytes), Err(AudioError::Corrupt("flac md5"))));
    }

    #[test]
    fn md5_is_verified_when_total_is_unknown() {
        let mut bytes = encode_constant_flac(8_000, 1, 16, &[1, 2], &[]);
        set_total_samples(&mut bytes, 0);
        assert_eq!(decode_all(&bytes).unwrap().frames(), 2);
        bytes[26] ^= 1;
        assert!(matches!(decode_all(&bytes), Err(AudioError::Corrupt("flac md5"))));
    }

    #[test]
    fn crc8_mismatch_is_corrupt() {
        let samples = [0i64];
        let mut bytes = encode_constant_flac(8_000, 1, 16, &samples, &[]);
        // Last header byte of the first frame is the CRC-8.
        *bytes.last_mut().unwrap() = bytes.last().unwrap().wrapping_add(1);
        // The CRC-16 is the last two bytes; corrupt an earlier frame-header CRC.
        // Walk to the first frame: after STREAMINFO (4+4+34).
        let frame_start = 4 + 4 + 34;
        bytes[frame_start + 6] ^= 0xFF; // the CRC-8 we wrote after utf8 + uncommon blocksize
        assert!(matches!(decode_all(&bytes), Err(AudioError::Corrupt(_))));
    }

    #[test]
    fn crc16_mismatch_is_corrupt() {
        let mut bytes = encode_constant_flac(8_000, 1, 16, &[0], &[]);
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert!(matches!(decode_all(&bytes), Err(AudioError::Corrupt("flac crc16"))));
    }

    #[test]
    fn strict_framing_rejects_truncation_and_missing_sync() {
        let mut truncated = encode_constant_flac(8_000, 1, 16, &[1, 2, 3], &[]);
        truncated[26..42].fill(0); // legal absent MD5 must not hide truncation
        truncated.pop();
        assert!(matches!(decode_all(&truncated), Err(AudioError::Truncated)));

        let mut missing_sync = encode_constant_flac(8_000, 1, 16, &[1, 2, 3], &[]);
        let second_frame = 4 + 4 + STREAMINFO_LEN + 12;
        missing_sync.insert(second_frame, 0);
        assert!(matches!(decode_all(&missing_sync), Err(AudioError::Corrupt("flac frame sync"))));
    }

    #[test]
    fn declared_total_is_exact() {
        let mut premature = encode_constant_flac(8_000, 1, 16, &[1, 2, 3], &[]);
        set_total_samples(&mut premature, 4);
        assert!(matches!(decode_all(&premature), Err(AudioError::Truncated)));

        let mut trailing = encode_constant_flac(8_000, 1, 16, &[1, 2, 3], &[]);
        set_total_samples(&mut trailing, 2);
        assert!(matches!(decode_all(&trailing), Err(AudioError::Corrupt("flac data after declared total"))));

        let mut crossing = encode_fixed0_flac(16_000, 16, &[0, 1, 2, 3, 4, 5, 6, 7]);
        set_total_samples(&mut crossing, 7);
        assert!(matches!(decode_all(&crossing), Err(AudioError::Corrupt("flac frame crosses declared total"))));
    }

    #[test]
    fn frame_format_is_immutable() {
        let mut bytes = encode_constant_flac(8_000, 1, 16, &[0], &[]);
        let start = 4 + 4 + STREAMINFO_LEN;
        bytes[start + 3] |= 1 << 1; // explicit 8-bit frame in a 16-bit stream
        rewrite_single_frame_crcs(&mut bytes);
        assert!(matches!(decode_all(&bytes), Err(AudioError::Corrupt("flac frame format change"))));
    }

    #[test]
    fn non_zero_frame_padding_is_corrupt() {
        let mut bytes = encode_stereo_assignment(16, 8, 1234, -5678);
        let start = 4 + 4 + STREAMINFO_LEN;
        let body = start + 7;
        bytes[body + 6] |= 1; // one of the seven alignment bits
        rewrite_single_frame_crcs(&mut bytes);
        assert!(matches!(decode_all(&bytes), Err(AudioError::Corrupt("flac non-zero frame padding"))));
    }

    #[test]
    fn all_channel_assignments_decode_sample_exact() {
        for code in [1, 8, 9, 10] {
            let bytes = encode_stereo_assignment(16, code, 1234, -5678);
            let audio = decode_all(&bytes).unwrap_or_else(|e| panic!("assignment {code}: {e}"));
            assert_eq!(audio.pcm_interleaved_f32, vec![1234.0 / 32768.0, -5678.0 / 32768.0]);
        }
    }

    #[test]
    fn thirty_two_bit_side_channel_is_supported() {
        let bytes = encode_stereo_assignment(32, 8, i32::MAX as i64, i32::MIN as i64);
        let audio = decode_all(&bytes).unwrap();
        assert_eq!(audio.frames(), 1);
        assert_eq!(audio.channels, 2);
        assert!(audio.pcm_interleaved_f32[0] > 0.999_999);
        assert_eq!(audio.pcm_interleaved_f32[1], -1.0);
    }

    #[test]
    fn progressive_buffers_do_not_grow() {
        let bytes = encode_fixed0_flac(16_000, 16, &[0, 1, -1, 2, -2, 3, -3, 4]);
        let mut decoder = FlacDecoder::new(&bytes).unwrap();
        let before = (
            decoder.pcm_i64.capacity(),
            decoder.out.capacity(),
            decoder.scratch.iter().map(Vec::capacity).collect::<Vec<_>>(),
        );
        assert!(decoder.next_frame().unwrap().is_some());
        let after = (
            decoder.pcm_i64.capacity(),
            decoder.out.capacity(),
            decoder.scratch.iter().map(Vec::capacity).collect::<Vec<_>>(),
        );
        assert_eq!(before, after);
    }

    #[test]
    fn limits_are_enforced() {
        let samples = [0i64; 20];
        let bytes = encode_constant_flac(8_000, 1, 16, &samples, &[]);
        let err = decode_all_limited(&bytes, Limits::with_max_frames(4));
        assert!(matches!(err, Err(AudioError::TooLarge(_))), "{err:?}");
        let tight = Limits { max_channels: 0, ..Limits::default() };
        assert!(matches!(decode_all_limited(&bytes, tight), Err(AudioError::TooLarge(_))));
    }

    #[test]
    fn garbage_is_refused_without_panicking() {
        assert!(decode_all(b"").is_err());
        assert!(decode_all(b"this is not a flac file, not even a little").is_err());
        assert!(FlacDecoder::new(&[0xff; 64]).is_err());
        assert!(looks_like_flac(b"fLaC"));
        assert!(!looks_like_flac(b"ID3"));
    }

    fn write_rice(w: &mut BitWriter, n: i32, param: u32) {
        let u = zigzag(n);
        let q = if param == 0 { u } else { u >> param };
        let rem = if param == 0 { 0 } else { u & ((1 << param) - 1) };
        for _ in 0..q {
            w.put(0, 1);
        }
        w.put(1, 1);
        if param > 0 {
            w.put(rem as u64, param);
        }
    }

    fn encode_fixed0_flac(rate: u32, bps: u8, samples: &[i64]) -> Vec<u8> {
        let n = samples.len();
        let mut md5 = Md5::new();
        md5.update_pcm(samples, bps);
        let digest = md5.finish();
        let mut file = b"fLaC".to_vec();
        let info = streaminfo_body(rate, 1, bps, n as u64, n as u16, digest);
        push_block(&mut file, true, 0, &info);

        let mut frame = Vec::new();
        frame.push(0xFF);
        frame.push(0xF8);
        frame.push(0x60); // uncommon 8-bit blocksize, rate from STREAMINFO
        frame.push(0x00); // mono, bps from STREAMINFO
        write_utf8_uint(&mut frame, 0);
        frame.push((n as u8).wrapping_sub(1));
        let crc = crc8(&frame);
        frame.push(crc);

        let mut w = BitWriter::new();
        w.put(0, 1);
        w.put(0b001000, 6); // FIXED order 0
        w.put(0, 1);
        w.put(0, 2); // rice method 0
        w.put(0, 4); // partition order 0
        w.put(2, 4); // rice param 2
        for &s in samples {
            write_rice(&mut w, s as i32, 2);
        }
        w.align();
        frame.extend_from_slice(&w.bytes);
        let fcrc = crc16(&frame);
        frame.extend_from_slice(&fcrc.to_be_bytes());
        file.extend_from_slice(&frame);
        file
    }

    #[test]
    fn fixed_order0_rice_roundtrip() {
        let samples = [0i64, 1, -1, 2, -2, 7, -8, 15];
        let bytes = encode_fixed0_flac(16_000, 16, &samples);
        let audio = decode_all(&bytes).unwrap();
        assert_eq!(audio.frames(), 8);
        let scale = 32768.0;
        for (i, &s) in samples.iter().enumerate() {
            let got = audio.pcm_interleaved_f32[i];
            assert!((got - s as f32 / scale).abs() < 1e-9, "{i}: {got}");
        }
    }

    #[test]
    fn supported_bit_depths_scale() {
        let bytes8 = encode_constant_flac(8_000, 1, 8, &[64, -128], &[]);
        let a = decode_all(&bytes8).unwrap();
        assert!((a.pcm_interleaved_f32[0] - 0.5).abs() < 1e-6);
        assert!((a.pcm_interleaved_f32[1] + 1.0).abs() < 1e-6);

        let bytes24 = encode_constant_flac(8_000, 1, 24, &[1 << 22], &[]);
        let b = decode_all(&bytes24).unwrap();
        assert!((b.pcm_interleaved_f32[0] - 0.5).abs() < 1e-6);

        let bytes20 = encode_constant_flac(8_000, 1, 20, &[1 << 18], &[]);
        assert!((decode_all(&bytes20).unwrap().pcm_interleaved_f32[0] - 0.5).abs() < 1e-6);

        let bytes32 = encode_constant_flac(8_000, 1, 32, &[1 << 30, i32::MIN as i64], &[]);
        let d = decode_all(&bytes32).unwrap();
        assert_eq!(d.pcm_interleaved_f32, vec![0.5, -1.0]);
    }

}
