// Port of the core block codec from upstream LZ4 (`local/lz4/lib/lz4.c` and
// `local/lz4/doc/lz4_Block_format.md`).
//
// This first Rust version intentionally focuses on the block API surface:
// `compress_bound`, fast/default compression, and safe decompression.
// It does not yet expose the frame, HC, or streaming APIs.

use std::{fmt, ptr};

const MINMATCH: usize = 4;
const LASTLITERALS: usize = 5;
const MFLIMIT: usize = 12;
const MAX_DISTANCE: usize = 65_535;
const HASH_LOG: usize = 12;
const HASH_SIZE: usize = 1 << HASH_LOG;
const RUN_MASK: usize = 15;
const ML_MASK: usize = 15;
const SKIP_TRIGGER: usize = 6;
const INVALID_INDEX: u32 = u32::MAX;
const ACCELERATION_DEFAULT: usize = 1;
const ACCELERATION_MAX: usize = 65_537;
const FAST_INPUT_MARGIN: usize = 14 + 2;
const FAST_OUTPUT_MARGIN: usize = 14 + 18;
const INC32_TABLE: [usize; 8] = [0, 1, 2, 1, 0, 4, 4, 4];
const DEC64_TABLE: [isize; 8] = [0, 0, 0, -1, -4, 1, 2, 3];
pub const MAX_INPUT_SIZE: usize = 0x7E00_0000;

#[cfg(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64"))]
const FAST_DEC_LOOP: bool = true;
#[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
const FAST_DEC_LOOP: bool = false;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressError {
    InputTooLarge,
    OutputTooSmall,
}

impl fmt::Display for CompressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompressError::InputTooLarge => write!(f, "input is larger than LZ4 allows"),
            CompressError::OutputTooSmall => write!(f, "output buffer is too small"),
        }
    }
}

impl std::error::Error for CompressError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecompressError {
    MalformedInput,
    OutputTooSmall,
}

impl fmt::Display for DecompressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecompressError::MalformedInput => write!(f, "malformed lz4 block"),
            DecompressError::OutputTooSmall => write!(f, "output buffer is too small"),
        }
    }
}

impl std::error::Error for DecompressError {}

#[inline]
pub const fn compress_bound(input_size: usize) -> usize {
    if input_size > MAX_INPUT_SIZE {
        0
    } else {
        input_size + input_size / 255 + 16
    }
}

pub fn compress_default_into(input: &[u8], output: &mut [u8]) -> Result<usize, CompressError> {
    compress_fast_into(input, output, ACCELERATION_DEFAULT)
}

pub fn compress_default(input: &[u8]) -> Result<Vec<u8>, CompressError> {
    compress_fast(input, ACCELERATION_DEFAULT)
}

pub fn compress_fast(input: &[u8], acceleration: usize) -> Result<Vec<u8>, CompressError> {
    let bound = compress_bound(input.len());
    if bound == 0 {
        return Err(CompressError::InputTooLarge);
    }
    let mut output = vec![0u8; bound];
    let written = compress_fast_into(input, &mut output, acceleration)?;
    output.truncate(written);
    Ok(output)
}

pub fn compress_fast_into(
    input: &[u8],
    output: &mut [u8],
    acceleration: usize,
) -> Result<usize, CompressError> {
    if input.len() > MAX_INPUT_SIZE {
        return Err(CompressError::InputTooLarge);
    }
    if input.is_empty() {
        if output.is_empty() {
            return Err(CompressError::OutputTooSmall);
        }
        output[0] = 0;
        return Ok(1);
    }

    let acceleration = acceleration.clamp(ACCELERATION_DEFAULT, ACCELERATION_MAX);
    let mut table = [INVALID_INDEX; HASH_SIZE];
    let mut src: usize;
    let mut anchor = 0usize;
    let src_end = input.len();
    let match_limit = src_end.saturating_sub(LASTLITERALS);
    let mf_limit = src_end.saturating_sub(MFLIMIT);
    let mut dst = 0usize;

    if input.len() < 13 {
        return emit_last_literals(input, anchor, output, dst);
    }

    table[hash_at(input, 0)] = 0;
    src = 1;
    let mut forward = src;
    let mut forward_hash = hash_at(input, forward);

    while forward <= mf_limit {
        let mut step = 1usize;
        let mut search_match_nb = acceleration << SKIP_TRIGGER;
        let mut match_pos;

        loop {
            let hash = forward_hash;
            src = forward;
            forward = forward.saturating_add(step);
            step = (search_match_nb >> SKIP_TRIGGER).max(1);
            search_match_nb = search_match_nb.saturating_add(1);

            let candidate = table[hash];
            table[hash] = src as u32;

            if forward > mf_limit {
                return emit_last_literals(input, anchor, output, dst);
            }
            forward_hash = hash_at(input, forward);

            if candidate == INVALID_INDEX {
                continue;
            }
            match_pos = candidate as usize;
            if src <= match_pos || src - match_pos > MAX_DISTANCE {
                continue;
            }
            if load_u32(input, match_pos) != load_u32(input, src) {
                continue;
            }
            break;
        }

        while src > anchor && match_pos > 0 && input[src - 1] == input[match_pos - 1] {
            src -= 1;
            match_pos -= 1;
        }

        let match_len =
            MINMATCH + count_match(input, src + MINMATCH, match_pos + MINMATCH, match_limit);
        dst = emit_sequence(input, anchor, src, match_pos, match_len, output, dst)?;
        src += match_len;
        anchor = src;

        if src > mf_limit {
            break;
        }

        table[hash_at(input, src - 2)] = (src - 2) as u32;
        let hash = hash_at(input, src);
        let candidate = table[hash];
        table[hash] = src as u32;
        if candidate != INVALID_INDEX {
            let candidate = candidate as usize;
            if src > candidate
                && src - candidate <= MAX_DISTANCE
                && load_u32(input, candidate) == load_u32(input, src)
            {
                let match_len =
                    MINMATCH + count_match(input, src + MINMATCH, candidate + MINMATCH, match_limit);
                dst = emit_sequence(input, anchor, src, candidate, match_len, output, dst)?;
                src += match_len;
                anchor = src;
                if src > mf_limit {
                    break;
                }
                table[hash_at(input, src - 2)] = (src - 2) as u32;
                forward = src + 1;
                if forward <= mf_limit {
                    forward_hash = hash_at(input, forward);
                    continue;
                }
                break;
            }
        }

        forward = src + 1;
        if forward > mf_limit {
            break;
        }
        forward_hash = hash_at(input, forward);
    }

    emit_last_literals(input, anchor, output, dst)
}

pub fn decompress_safe(input: &[u8], output: &mut [u8]) -> Result<usize, DecompressError> {
    if output.is_empty() {
        return if input == [0] {
            Ok(0)
        } else {
            Err(DecompressError::MalformedInput)
        };
    }
    if input.is_empty() {
        return Err(DecompressError::MalformedInput);
    }

    let mut src = 0usize;
    let mut dst = 0usize;
    let short_input_end = input.len().saturating_sub(FAST_INPUT_MARGIN);
    let short_output_end = output.len().saturating_sub(FAST_OUTPUT_MARGIN);
    let input_ptr = input.as_ptr();
    let output_ptr = output.as_mut_ptr();

    while src < input.len() {
        let token = input[src];
        src += 1;

        let lit_token = token as usize >> 4;
        let match_token = token as usize & ML_MASK;
        let literal_len = read_length(lit_token, input, &mut src)?;
        let mut offset = None;

        if FAST_DEC_LOOP
            && lit_token != RUN_MASK
            && src < short_input_end
            && dst <= short_output_end
        {
            unsafe {
                ptr::copy_nonoverlapping(input_ptr.add(src), output_ptr.add(dst), 16);
            }
            src += literal_len;
            dst += literal_len;

            let fast_offset = unsafe { load_u16_le_ptr(input_ptr.add(src)) as usize };
            src += 2;
            if fast_offset == 0 || fast_offset > dst {
                return Err(DecompressError::MalformedInput);
            }
            offset = Some(fast_offset);

            if match_token != ML_MASK && fast_offset >= 8 {
                let match_src = dst - fast_offset;
                unsafe {
                    copy_u64(output_ptr.add(dst), output_ptr.add(match_src));
                    copy_u64(output_ptr.add(dst + 8), output_ptr.add(match_src + 8));
                    ptr::copy_nonoverlapping(output_ptr.add(match_src + 16), output_ptr.add(dst + 16), 2);
                }
                dst += match_token + MINMATCH;
                continue;
            }
        } else {
            if src + literal_len > input.len() {
                return Err(DecompressError::MalformedInput);
            }
            if dst + literal_len > output.len() {
                return Err(DecompressError::OutputTooSmall);
            }
            unsafe {
                copy_bytes(output_ptr.add(dst), input_ptr.add(src), literal_len);
            }
            src += literal_len;
            dst += literal_len;
        }

        if src == input.len() {
            return Ok(dst);
        }

        let offset = match offset {
            Some(offset) => offset,
            None => {
                if src + 2 > input.len() {
                    return Err(DecompressError::MalformedInput);
                }
                let offset = unsafe { load_u16_le_ptr(input_ptr.add(src)) as usize };
                src += 2;
                offset
            }
        };
        if offset == 0 || offset > dst {
            return Err(DecompressError::MalformedInput);
        }

        let match_len = MINMATCH + read_length(match_token, input, &mut src)?;
        if dst + match_len > output.len() {
            return Err(DecompressError::OutputTooSmall);
        }

        unsafe {
            copy_match(output_ptr, dst, offset, match_len);
        }
        dst += match_len;
    }

    Err(DecompressError::MalformedInput)
}

fn emit_sequence(
    input: &[u8],
    anchor: usize,
    match_start: usize,
    match_pos: usize,
    match_len: usize,
    output: &mut [u8],
    mut dst: usize,
) -> Result<usize, CompressError> {
    let literal_len = match_start - anchor;
    let literal_len_bytes = extra_len_bytes(literal_len);
    let match_code = match_len - MINMATCH;
    let match_len_bytes = extra_len_bytes(match_code);
    let needed = 1 + literal_len_bytes + literal_len + 2 + match_len_bytes;
    if dst + needed > output.len() {
        return Err(CompressError::OutputTooSmall);
    }

    let token_pos = dst;
    output[token_pos] = 0;
    dst += 1;

    let lit_nibble = literal_len.min(RUN_MASK);
    output[token_pos] = (lit_nibble << 4) as u8;
    dst = write_length_bytes(literal_len, RUN_MASK, output, dst)?;

    unsafe {
        copy_bytes(output.as_mut_ptr().add(dst), input.as_ptr().add(anchor), literal_len);
    }
    dst += literal_len;

    let offset = match_start - match_pos;
    output[dst..dst + 2].copy_from_slice(&(offset as u16).to_le_bytes());
    dst += 2;

    let match_nibble = match_code.min(ML_MASK);
    output[token_pos] |= match_nibble as u8;
    dst = write_length_bytes(match_code, ML_MASK, output, dst)?;

    Ok(dst)
}

fn emit_last_literals(
    input: &[u8],
    anchor: usize,
    output: &mut [u8],
    mut dst: usize,
) -> Result<usize, CompressError> {
    let literal_len = input.len() - anchor;
    let needed = 1 + extra_len_bytes(literal_len) + literal_len;
    if dst + needed > output.len() {
        return Err(CompressError::OutputTooSmall);
    }

    let token_pos = dst;
    output[token_pos] = 0;
    dst += 1;

    let lit_nibble = literal_len.min(RUN_MASK);
    output[token_pos] = (lit_nibble << 4) as u8;
    dst = write_length_bytes(literal_len, RUN_MASK, output, dst)?;
    unsafe {
        copy_bytes(output.as_mut_ptr().add(dst), input.as_ptr().add(anchor), literal_len);
    }
    dst += literal_len;
    Ok(dst)
}

#[inline]
fn extra_len_bytes(len: usize) -> usize {
    if len < RUN_MASK {
        0
    } else {
        1 + (len - RUN_MASK) / 255
    }
}

fn write_length_bytes(
    len: usize,
    inline_limit: usize,
    output: &mut [u8],
    mut dst: usize,
) -> Result<usize, CompressError> {
    if len < inline_limit {
        return Ok(dst);
    }
    let mut remaining = len - inline_limit;
    while remaining >= 255 {
        if dst >= output.len() {
            return Err(CompressError::OutputTooSmall);
        }
        output[dst] = 255;
        dst += 1;
        remaining -= 255;
    }
    if dst >= output.len() {
        return Err(CompressError::OutputTooSmall);
    }
    output[dst] = remaining as u8;
    dst += 1;
    Ok(dst)
}

fn read_length(nibble: usize, input: &[u8], src: &mut usize) -> Result<usize, DecompressError> {
    let mut len = nibble;
    if nibble != 15 {
        return Ok(len);
    }
    loop {
        let byte = *input.get(*src).ok_or(DecompressError::MalformedInput)? as usize;
        *src += 1;
        len = len.checked_add(byte).ok_or(DecompressError::MalformedInput)?;
        if byte != 255 {
            return Ok(len);
        }
    }
}

#[inline]
fn hash_at(input: &[u8], pos: usize) -> usize {
    let sequence = load_u32(input, pos);
    ((sequence.wrapping_mul(2_654_435_761)) >> ((MINMATCH * 8 - HASH_LOG) as u32)) as usize
        & (HASH_SIZE - 1)
}

#[inline]
fn count_match(input: &[u8], mut a: usize, mut b: usize, limit: usize) -> usize {
    let start = a;
    while a + 8 <= limit && b + 8 <= limit {
        let diff = load_u64(input, a) ^ load_u64(input, b);
        if diff == 0 {
            a += 8;
            b += 8;
        } else {
            return (a - start) + (diff.trailing_zeros() as usize / 8);
        }
    }
    while a < limit && b < limit && input[a] == input[b] {
        a += 1;
        b += 1;
    }
    a - start
}

#[inline]
fn load_u32(input: &[u8], pos: usize) -> u32 {
    unsafe { load_u32_le_ptr(input.as_ptr().add(pos)) }
}

#[inline]
fn load_u64(input: &[u8], pos: usize) -> u64 {
    unsafe { load_u64_le_ptr(input.as_ptr().add(pos)) }
}

#[inline]
unsafe fn load_u16_le_ptr(ptr: *const u8) -> u16 {
    u16::from_le(ptr::read_unaligned(ptr as *const u16))
}

#[inline(always)]
unsafe fn load_u32_le_ptr(ptr: *const u8) -> u32 {
    u32::from_le(ptr::read_unaligned(ptr as *const u32))
}

#[inline(always)]
unsafe fn load_u64_le_ptr(ptr: *const u8) -> u64 {
    u64::from_le(ptr::read_unaligned(ptr as *const u64))
}

#[inline(always)]
unsafe fn copy_bytes(dst: *mut u8, src: *const u8, len: usize) {
    if len != 0 {
        ptr::copy_nonoverlapping(src, dst, len);
    }
}

#[inline(always)]
unsafe fn copy_u64(dst: *mut u8, src: *const u8) {
    let value = ptr::read_unaligned(src as *const u64);
    ptr::write_unaligned(dst as *mut u64, value);
}

#[inline(always)]
unsafe fn copy_match(output_ptr: *mut u8, dst: usize, offset: usize, match_len: usize) {
    let dst_ptr = output_ptr.add(dst);
    let src_ptr = dst_ptr.sub(offset) as *const u8;

    if offset < 8 {
        copy_match_short_offset(dst_ptr, src_ptr, match_len, offset);
        return;
    }

    let mut copied = 0usize;
    while copied + 8 <= match_len {
        copy_u64(dst_ptr.add(copied), src_ptr.add(copied));
        copied += 8;
    }
    while copied < match_len {
        ptr::write(dst_ptr.add(copied), *src_ptr.add(copied));
        copied += 1;
    }
}

#[inline(always)]
unsafe fn copy_match_short_offset(
    dst_ptr: *mut u8,
    src_ptr: *const u8,
    match_len: usize,
    offset: usize,
) {
    let mut copied = 0usize;

    match offset {
        1 | 2 | 4 => {
            let mut pattern = [0u8; 8];
            for index in 0..8 {
                pattern[index] = *src_ptr.add(index % offset);
            }
            let repeated = u64::from_ne_bytes(pattern);
            while copied + 8 <= match_len {
                ptr::write_unaligned(dst_ptr.add(copied) as *mut u64, repeated);
                copied += 8;
            }
            while copied < match_len {
                ptr::write(dst_ptr.add(copied), pattern[copied % offset]);
                copied += 1;
            }
        }
        _ => {
            if match_len < 8 {
                while copied < match_len {
                    ptr::write(dst_ptr.add(copied), *src_ptr.add(copied));
                    copied += 1;
                }
                return;
            }

            ptr::write(dst_ptr, *src_ptr);
            ptr::write(dst_ptr.add(1), *src_ptr.add(1));
            ptr::write(dst_ptr.add(2), *src_ptr.add(2));
            ptr::write(dst_ptr.add(3), *src_ptr.add(3));
            let adjusted = src_ptr.add(INC32_TABLE[offset]);
            ptr::copy_nonoverlapping(adjusted, dst_ptr.add(4), 4);

            copied = 8;
            let mut tail_src = adjust_ptr(adjusted, -DEC64_TABLE[offset]);
            while copied + 8 <= match_len {
                copy_u64(dst_ptr.add(copied), tail_src);
                copied += 8;
                tail_src = tail_src.add(8);
            }
            while copied < match_len {
                ptr::write(dst_ptr.add(copied), *tail_src);
                copied += 1;
                tail_src = tail_src.add(1);
            }
        }
    }
}

#[inline(always)]
unsafe fn adjust_ptr(ptr: *const u8, delta: isize) -> *const u8 {
    if delta >= 0 {
        ptr.add(delta as usize)
    } else {
        ptr.sub((-delta) as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decompresses_empty_block() {
        let input = [0u8];
        let mut out = [0u8; 1];
        let decoded = decompress_safe(&input, &mut out).expect("empty block should decode");
        assert_eq!(decoded, 0);
    }

    #[test]
    fn decompresses_literal_only_block() {
        let input = [0x50, b'h', b'e', b'l', b'l', b'o'];
        let mut out = [0u8; 5];
        let decoded = decompress_safe(&input, &mut out).expect("literal-only block should decode");
        assert_eq!(decoded, 5);
        assert_eq!(&out, b"hello");
    }

    #[test]
    fn decompresses_overlap_match_block() {
        let input = [0x14, b'a', 0x01, 0x00, 0x50, b'a', b'a', b'a', b'a', b'a'];
        let mut out = [0u8; 14];
        let decoded = decompress_safe(&input, &mut out).expect("overlap match should decode");
        assert_eq!(decoded, 14);
        assert_eq!(&out, b"aaaaaaaaaaaaaa");
    }

    #[test]
    fn decompresses_offset_three_overlap_block() {
        let input = [0x37, b'a', b'b', b'c', 0x03, 0x00, 0x50, b'c', b'a', b'b', b'c', b'a'];
        let mut out = [0u8; 19];
        let decoded = decompress_safe(&input, &mut out).expect("offset-three overlap should decode");
        assert_eq!(decoded, 19);
        assert_eq!(&out, b"abcabcabcabcabcabca");
    }

    #[test]
    fn decompresses_short_match_with_large_offset() {
        let input = [
            0x80, b'a', b'b', b'c', b'd', b'e', b'f', b'g', b'h', 0x08, 0x00, 0x50, b'e', b'f',
            b'g', b'h', b'i',
        ];
        let mut out = [0u8; 17];
        let decoded = decompress_safe(&input, &mut out).expect("short large-offset match should decode");
        assert_eq!(decoded, 17);
        assert_eq!(&out, b"abcdefghabcdefghi");
    }

    #[test]
    fn decompresses_long_literal_length() {
        let payload = *b"abcdefghijklmnopqrst";
        let mut input = vec![0xF0, 5];
        input.extend_from_slice(&payload);
        let mut out = [0u8; 20];
        let decoded = decompress_safe(&input, &mut out).expect("long literal block should decode");
        assert_eq!(decoded, payload.len());
        assert_eq!(&out, &payload);
    }

    #[test]
    fn roundtrips_varied_inputs() {
        let cases: Vec<Vec<u8>> = vec![
            vec![],
            b"a".to_vec(),
            b"hello".to_vec(),
            b"aaaaaaaaaaaaaa".to_vec(),
            b"abcabcabcabcabcabcabcabc".to_vec(),
            (0..256).map(|n| n as u8).collect(),
            (0..4096).map(|n| ((n * 17) & 255) as u8).collect(),
            {
                let mut data = Vec::new();
                for _ in 0..128 {
                    data.extend_from_slice(b"the quick brown fox jumps over the lazy dog ");
                }
                data
            },
        ];

        for case in cases {
            let compressed = compress_default(&case).expect("compression should succeed");
            let mut roundtrip = vec![0u8; case.len()];
            let decoded =
                decompress_safe(&compressed, &mut roundtrip).expect("compressed block should decode");
            assert_eq!(decoded, case.len());
            assert_eq!(roundtrip, case);
        }
    }

    #[test]
    fn rejects_output_that_is_too_small() {
        let compressed = compress_default(b"hello hello hello").expect("compression should work");
        let mut out = [0u8; 4];
        let err = decompress_safe(&compressed, &mut out).expect_err("short output should fail");
        assert_eq!(err, DecompressError::OutputTooSmall);
    }

    #[test]
    fn compress_bound_matches_formula() {
        assert_eq!(compress_bound(0), 16);
        assert_eq!(compress_bound(1), 17);
        assert_eq!(compress_bound(1024), 1024 + 1024 / 255 + 16);
    }
}
