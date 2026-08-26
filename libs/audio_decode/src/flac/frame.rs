//! Frame header, CRC-8/CRC-16, channel assignment, and one-frame decode.

use super::bits::{read_utf8_uint, BitReader, Utf8Error};
use super::metadata::StreamInfo;
use super::subframe;
use crate::error::AudioError;

/// CRC-8 (poly 0x07, init 0, not reflected) of the frame header.
pub fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in data {
        crc = CRC8[(crc ^ b) as usize];
    }
    crc
}

/// CRC-16 (poly 0x8005, init 0, not reflected) of the whole frame minus the
/// two-byte footer.
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &b in data {
        let idx = ((crc >> 8) as u8 ^ b) as usize;
        crc = crc << 8 ^ CRC16[idx];
    }
    crc
}

const fn crc8_table() -> [u8; 256] {
    let mut t = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u8;
        let mut b = 0;
        while b < 8 {
            crc = if crc & 0x80 != 0 { (crc << 1) ^ 0x07 } else { crc << 1 };
            b += 1;
        }
        t[i] = crc;
        i += 1;
    }
    t
}

const fn crc16_table() -> [u16; 256] {
    let mut t = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = (i as u16) << 8;
        let mut b = 0;
        while b < 8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x8005 } else { crc << 1 };
            b += 1;
        }
        t[i] = crc;
        i += 1;
    }
    t
}

const CRC8: [u8; 256] = crc8_table();
const CRC16: [u16; 256] = crc16_table();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelAssignment {
    Independent(u8),
    LeftSide,
    RightSide,
    MidSide,
}

impl ChannelAssignment {
    pub fn channels(self) -> u8 {
        match self {
            ChannelAssignment::Independent(n) => n,
            _ => 2,
        }
    }

    /// Bits per sample this subframe is coded at. Side channels get one extra.
    pub fn subframe_bps(self, stream_bps: u8, ch: u8) -> u8 {
        let extra = match self {
            ChannelAssignment::Independent(_) => false,
            ChannelAssignment::LeftSide => ch == 1,
            ChannelAssignment::RightSide => ch == 0,
            ChannelAssignment::MidSide => ch == 1,
        };
        if extra { stream_bps.saturating_add(1) } else { stream_bps }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FrameHeader {
    pub variable_blocksize: bool,
    pub blocksize: u32,
    pub sample_rate: u32,
    pub assignment: ChannelAssignment,
    pub bps: u8,
    pub number: u64,
    /// Bytes of header including the CRC-8.
    pub bytes: usize,
}

/// Next byte-aligned 14-bit sync (`0x3FFE` plus the reserved 0).
#[cfg(test)]
pub fn find_sync(data: &[u8], from: usize) -> Option<usize> {
    let mut at = from;
    while at + 2 <= data.len() {
        if data[at] == 0xFF && data[at + 1] & 0xFE == 0xF8 {
            return Some(at);
        }
        at += 1;
    }
    None
}

pub fn parse_header(data: &[u8], info: &StreamInfo) -> Result<FrameHeader, AudioError> {
    if data.len() < 6 {
        return Err(AudioError::Truncated);
    }
    if data[0] != 0xFF || data[1] & 0xFE != 0xF8 {
        return Err(AudioError::Corrupt("flac frame sync"));
    }
    if data[3] & 1 != 0 {
        return Err(AudioError::Corrupt("flac frame reserved"));
    }

    let variable_blocksize = data[1] & 1 != 0;
    let bs_code = data[2] >> 4;
    let sr_code = data[2] & 0x0F;
    let ch_code = data[3] >> 4;
    let ss_code = (data[3] >> 1) & 0x07;

    let mut at = 4usize;
    let (number, nlen) = read_utf8_uint(data, at).map_err(|e| match e {
        Utf8Error::Truncated => AudioError::Truncated,
        Utf8Error::Invalid => AudioError::Corrupt("flac frame number"),
    })?;
    at = at.saturating_add(nlen);
    let max_number = if variable_blocksize { (1u64 << 36) - 1 } else { (1u64 << 31) - 1 };
    if number > max_number {
        return Err(AudioError::Corrupt("flac frame number"));
    }

    let blocksize = match bs_code {
        0 => return Err(AudioError::Corrupt("flac block size")),
        1 => 192,
        2..=5 => 576u32 << (bs_code - 2),
        6 => {
            let b = *data.get(at).ok_or(AudioError::Truncated)?;
            at += 1;
            b as u32 + 1
        }
        7 => {
            let hi = *data.get(at).ok_or(AudioError::Truncated)?;
            let lo = *data.get(at + 1).ok_or(AudioError::Truncated)?;
            at += 2;
            ((hi as u32) << 8 | lo as u32) + 1
        }
        8..=15 => 256u32 << (bs_code - 8),
        _ => return Err(AudioError::Corrupt("flac block size")),
    };

    let sample_rate = match sr_code {
        0 => info.sample_rate,
        1 => 88_200,
        2 => 176_400,
        3 => 192_000,
        4 => 8_000,
        5 => 16_000,
        6 => 22_050,
        7 => 24_000,
        8 => 32_000,
        9 => 44_100,
        10 => 48_000,
        11 => 96_000,
        12 => {
            let b = *data.get(at).ok_or(AudioError::Truncated)?;
            at += 1;
            (b as u32) * 1000
        }
        13 => {
            let hi = *data.get(at).ok_or(AudioError::Truncated)?;
            let lo = *data.get(at + 1).ok_or(AudioError::Truncated)?;
            at += 2;
            (hi as u32) << 8 | lo as u32
        }
        14 => {
            let hi = *data.get(at).ok_or(AudioError::Truncated)?;
            let lo = *data.get(at + 1).ok_or(AudioError::Truncated)?;
            at += 2;
            ((hi as u32) << 8 | lo as u32) * 10
        }
        _ => return Err(AudioError::Corrupt("flac sample rate")),
    };
    if sample_rate == 0 || sample_rate > 655_350 {
        return Err(AudioError::Corrupt("flac sample rate"));
    }

    let assignment = match ch_code {
        0..=7 => ChannelAssignment::Independent(ch_code + 1),
        8 => ChannelAssignment::LeftSide,
        9 => ChannelAssignment::RightSide,
        10 => ChannelAssignment::MidSide,
        _ => return Err(AudioError::Corrupt("flac channel assignment")),
    };

    let bps = match ss_code {
        0 => info.bits_per_sample,
        1 => 8,
        2 => 12,
        3 => return Err(AudioError::Corrupt("flac sample size")),
        4 => 16,
        5 => 20,
        6 => 24,
        7 => 32,
        _ => return Err(AudioError::Corrupt("flac sample size")),
    };

    let crc_byte = *data.get(at).ok_or(AudioError::Truncated)?;
    let got = crc8(&data[..at]);
    if got != crc_byte {
        return Err(AudioError::Corrupt("flac crc8"));
    }
    at += 1;

    if blocksize == 0 || blocksize > 65535 {
        return Err(AudioError::Corrupt("flac block size"));
    }

    Ok(FrameHeader {
        variable_blocksize,
        blocksize,
        sample_rate,
        assignment,
        bps,
        number,
        bytes: at,
    })
}

/// Decode one frame starting at `data[0]`. On success, `pcm` is interleaved
/// `blocksize * channels` i32 samples and the return is the frame's byte
/// length (header + subframes + CRC-16).
pub fn decode_frame(
    data: &[u8],
    info: &StreamInfo,
    pcm: &mut Vec<i64>,
    scratch: &mut [Vec<i64>],
) -> Result<(FrameHeader, usize), AudioError> {
    let header = parse_header(data, info)?;
    let channels = header.assignment.channels() as usize;
    if channels != info.channels as usize || channels > scratch.len() {
        return Err(AudioError::Corrupt("flac frame channel count"));
    }
    if header.sample_rate != info.sample_rate || header.bps != info.bits_per_sample {
        return Err(AudioError::Corrupt("flac frame format change"));
    }
    let blocksize = header.blocksize as usize;
    // The final frame may be shorter than STREAMINFO's minimum block size.
    // The maximum is still an unconditional allocation and format bound.
    if blocksize > info.max_blocksize as usize {
        return Err(AudioError::Corrupt("flac frame block size outside streaminfo"));
    }
    if scratch[..channels].iter().any(|v| v.capacity() < blocksize) {
        return Err(AudioError::Corrupt("flac frame exceeds preallocated block size"));
    }
    let total = blocksize
        .checked_mul(channels)
        .ok_or(AudioError::Corrupt("flac frame sample count"))?;
    if pcm.capacity() < total {
        return Err(AudioError::Corrupt("flac frame exceeds preallocated pcm"));
    }
    let mut r = BitReader::new(&data[header.bytes..]);
    for ch in 0..channels {
        scratch[ch].clear();
        scratch[ch].resize(blocksize, 0);
        let bps = header.assignment.subframe_bps(header.bps, ch as u8);
        subframe::decode_subframe(&mut r, blocksize, bps, &mut scratch[ch])?;
    }
    if !r.read_zero_padding().ok_or(AudioError::Truncated)? {
        return Err(AudioError::Corrupt("flac non-zero frame padding"));
    }
    let body_bits = r.bit_pos();
    if body_bits % 8 != 0 {
        return Err(AudioError::Corrupt("flac frame alignment"));
    }
    let body_bytes = body_bits / 8;
    let crc_at = header.bytes.saturating_add(body_bytes);
    let crc_end = crc_at.checked_add(2).ok_or(AudioError::Truncated)?;
    if crc_end > data.len() {
        return Err(AudioError::Truncated);
    }
    let want = u16::from_be_bytes([data[crc_at], data[crc_at + 1]]);
    let got = crc16(&data[..crc_at]);
    if got != want {
        return Err(AudioError::Corrupt("flac crc16"));
    }

    undo_channel_coding(header.assignment, header.bps, blocksize, scratch)?;

    pcm.clear();
    pcm.resize(total, 0);
    for i in 0..blocksize {
        for ch in 0..channels {
            pcm[i * channels + ch] = scratch[ch][i];
        }
    }
    if info.min_framesize != 0 && crc_end < info.min_framesize as usize {
        return Err(AudioError::Corrupt("flac frame smaller than streaminfo"));
    }
    if info.max_framesize != 0 && crc_end > info.max_framesize as usize {
        return Err(AudioError::Corrupt("flac frame larger than streaminfo"));
    }
    Ok((header, crc_end))
}

fn undo_channel_coding(
    assignment: ChannelAssignment,
    bps: u8,
    blocksize: usize,
    ch: &mut [Vec<i64>],
) -> Result<(), AudioError> {
    match assignment {
        ChannelAssignment::Independent(_) => {}
        ChannelAssignment::LeftSide => {
            // ch0 = left, ch1 = side = left - right  →  right = left - side
            for i in 0..blocksize {
                ch[1][i] = ch[0][i]
                    .checked_sub(ch[1][i])
                    .ok_or(AudioError::Corrupt("flac channel decorrelation overflow"))?;
            }
        }
        ChannelAssignment::RightSide => {
            // ch0 = side = left - right, ch1 = right  →  left = right + side
            for i in 0..blocksize {
                ch[0][i] = ch[1][i]
                    .checked_add(ch[0][i])
                    .ok_or(AudioError::Corrupt("flac channel decorrelation overflow"))?;
            }
        }
        ChannelAssignment::MidSide => {
            // mid stored in ch0, side in ch1. Reconstruct L+R from mid and
            // the side's LSB, then (mid±side)/2. i64 so a 32-bit mid cannot
            // overflow the left shift.
            for i in 0..blocksize {
                let mid = ch[0][i];
                let side = ch[1][i];
                let mid = (mid << 1) | (side & 1);
                ch[0][i] = (mid + side) >> 1;
                ch[1][i] = (mid - side) >> 1;
            }
        }
    }
    let limit = 1i64 << (bps - 1);
    if ch[..assignment.channels() as usize]
        .iter()
        .any(|samples| samples[..blocksize].iter().any(|&s| !(-limit..limit).contains(&s)))
    {
        return Err(AudioError::Corrupt("flac decorrelated sample range"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc8_and_crc16_known_vectors() {
        // CRC-8/SMBUS and CRC-16/UMTS of the nine-digit string.
        assert_eq!(crc8(b"123456789"), 0xF4);
        assert_eq!(crc16(b"123456789"), 0xFEE8);
        assert_eq!(crc8(b""), 0);
        assert_eq!(crc16(b""), 0);
        assert_eq!(crc8(&[0x00]), 0);
        assert_eq!(crc8(&[0xFF]), 0xF3);
    }

    #[test]
    fn find_sync_skips_garbage() {
        let data = [0x00, 0xFF, 0xF8, 0x00];
        assert_eq!(find_sync(&data, 0), Some(1));
        assert_eq!(find_sync(&data, 2), None);
        assert_eq!(find_sync(&[], 0), None);
    }

    #[test]
    fn mid_side_restores_odd_and_even_side() {
        // L=5 R=2 → mid=3 side=3; L=5 R=3 → mid=4 side=2.
        let mut ch = [vec![3, 4], vec![3, 2]];
        undo_channel_coding(ChannelAssignment::MidSide, 16, 2, &mut ch).unwrap();
        assert_eq!(ch[0], vec![5, 5]);
        assert_eq!(ch[1], vec![2, 3]);
    }

    #[test]
    fn left_side_and_right_side() {
        let mut ch = [vec![10], vec![3]]; // left=10, side=3 → right=7
        undo_channel_coding(ChannelAssignment::LeftSide, 16, 1, &mut ch).unwrap();
        assert_eq!(ch[0][0], 10);
        assert_eq!(ch[1][0], 7);

        let mut ch = [vec![3], vec![7]]; // side=3, right=7 → left=10
        undo_channel_coding(ChannelAssignment::RightSide, 16, 1, &mut ch).unwrap();
        assert_eq!(ch[0][0], 10);
        assert_eq!(ch[1][0], 7);
    }

    #[test]
    fn independent_assignment_is_unchanged() {
        let mut ch = [vec![i32::MIN as i64, i32::MAX as i64], vec![7, -9]];
        let before = ch.clone();
        undo_channel_coding(ChannelAssignment::Independent(2), 32, 2, &mut ch).unwrap();
        assert_eq!(ch, before);
    }

    #[test]
    fn side_channel_uses_33_bits_for_32_bit_stereo() {
        let mut ch = [vec![i32::MAX as i64], vec![u32::MAX as i64]];
        undo_channel_coding(ChannelAssignment::LeftSide, 32, 1, &mut ch).unwrap();
        assert_eq!(ch[0][0], i32::MAX as i64);
        assert_eq!(ch[1][0], i32::MIN as i64);
    }
}
