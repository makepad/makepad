// Port of libdeflate's deflate_decompress.c + decompress_template.h
// Original: Copyright 2016 Eric Biggers, MIT license
//
// This is a highly optimized DEFLATE decompressor. Key tricks from libdeflate:
// - Word-sized bitbuffer (64-bit) that doesn't need frequent refills
// - Word-at-a-time copy for match output
// - Packed decode table entries with length/offset/extra bits built in
// - Fast loop with bounds checks only at loop boundaries
// - Branchless bitbuffer refill using unaligned word reads

use std::fmt;

use crate::adler32::adler32;

// --- Error types ---

#[derive(Debug, PartialEq)]
pub enum DecompressError {
    BadData,
    InsufficientSpace,
    ShortOutput,
}

impl fmt::Display for DecompressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecompressError::BadData => write!(f, "invalid deflate data"),
            DecompressError::InsufficientSpace => write!(f, "output buffer too small"),
            DecompressError::ShortOutput => write!(f, "output buffer not fully filled"),
        }
    }
}

impl std::error::Error for DecompressError {}

// --- DEFLATE constants ---

const DEFLATE_BLOCKTYPE_UNCOMPRESSED: u32 = 0;
const DEFLATE_BLOCKTYPE_STATIC_HUFFMAN: u32 = 1;
const DEFLATE_BLOCKTYPE_DYNAMIC_HUFFMAN: u32 = 2;

const DEFLATE_MAX_MATCH_LEN: u32 = 258;

const DEFLATE_NUM_PRECODE_SYMS: usize = 19;
const DEFLATE_NUM_LITLEN_SYMS: usize = 288;
const DEFLATE_NUM_OFFSET_SYMS: usize = 32;
const DEFLATE_MAX_NUM_SYMS: usize = 288;

const DEFLATE_MAX_PRE_CODEWORD_LEN: u32 = 7;
const DEFLATE_MAX_LITLEN_CODEWORD_LEN: u32 = 15;
const DEFLATE_MAX_OFFSET_CODEWORD_LEN: u32 = 15;
const DEFLATE_MAX_CODEWORD_LEN: u32 = 15;
const DEFLATE_MAX_LENS_OVERRUN: usize = 137;
const DEFLATE_MAX_EXTRA_LENGTH_BITS: u32 = 5;
const DEFLATE_MAX_EXTRA_OFFSET_BITS: u32 = 13;

// --- Decode table constants ---

const PRECODE_TABLEBITS: u32 = 7;
const PRECODE_ENOUGH: usize = 128;
const LITLEN_TABLEBITS: u32 = 11;
const LITLEN_ENOUGH: usize = 2342;
const OFFSET_TABLEBITS: u32 = 8;
const OFFSET_ENOUGH: usize = 402;

// --- Huffman decode table entry flags ---

const HUFFDEC_LITERAL: u32 = 0x80000000;
const HUFFDEC_EXCEPTIONAL: u32 = 0x00008000;
const HUFFDEC_SUBTABLE_POINTER: u32 = 0x00004000;
const HUFFDEC_END_OF_BLOCK: u32 = 0x00002000;

// --- Bitstream constants ---

// On 64-bit we use u64 as the bitbuffer
type BitBuf = u64;
const BITBUF_NBITS: u32 = 64;
const WORDBYTES: usize = 8;

// Unaligned access is fast on x86_64 and aarch64
const UNALIGNED_ACCESS_IS_FAST: bool = cfg!(any(
    target_arch = "x86_64",
    target_arch = "x86",
    target_arch = "aarch64",
));

const MAX_BITSLEFT: u32 = if UNALIGNED_ACCESS_IS_FAST {
    BITBUF_NBITS - 1
} else {
    BITBUF_NBITS
};

const CONSUMABLE_NBITS: u32 = MAX_BITSLEFT - 7;

const FASTLOOP_PRELOADABLE_NBITS: u32 = if UNALIGNED_ACCESS_IS_FAST {
    BITBUF_NBITS
} else {
    CONSUMABLE_NBITS
};

const PRELOAD_SLACK: u32 = if FASTLOOP_PRELOADABLE_NBITS > MAX_BITSLEFT {
    FASTLOOP_PRELOADABLE_NBITS - MAX_BITSLEFT
} else {
    0
};

const LENGTH_MAXBITS: u32 = DEFLATE_MAX_LITLEN_CODEWORD_LEN + DEFLATE_MAX_EXTRA_LENGTH_BITS;
const OFFSET_MAXBITS: u32 = DEFLATE_MAX_OFFSET_CODEWORD_LEN + DEFLATE_MAX_EXTRA_OFFSET_BITS;
const OFFSET_MAXFASTBITS: u32 = OFFSET_TABLEBITS + DEFLATE_MAX_EXTRA_OFFSET_BITS;

const FASTLOOP_MAX_BYTES_WRITTEN: usize = 2 + DEFLATE_MAX_MATCH_LEN as usize + 5 * WORDBYTES - 1;
const FASTLOOP_MAX_BYTES_READ: usize =
    (MAX_BITSLEFT + 2 * LITLEN_TABLEBITS + LENGTH_MAXBITS + OFFSET_MAXBITS + 7) as usize / 8
        + WORDBYTES;

// --- Helpers ---

#[inline(always)]
fn bitmask(n: u32) -> BitBuf {
    (1u64 << n) - 1
}

#[inline(always)]
fn bsr32(v: u32) -> u32 {
    debug_assert!(v != 0);
    31 - v.leading_zeros()
}

#[inline(always)]
fn load_word_unaligned(p: *const u8) -> u64 {
    unsafe {
        let mut v = 0u64;
        std::ptr::copy_nonoverlapping(p, &mut v as *mut u64 as *mut u8, 8);
        u64::from_le(v)
    }
}

#[inline(always)]
fn store_word_unaligned(v: u64, p: *mut u8) {
    unsafe {
        let bytes = v.to_le_bytes();
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, 8);
    }
}

#[inline(always)]
fn get_unaligned_le16(p: *const u8) -> u16 {
    unsafe {
        let mut v = 0u16;
        std::ptr::copy_nonoverlapping(p, &mut v as *mut u16 as *mut u8, 2);
        u16::from_le(v)
    }
}

#[inline(always)]
fn get_unaligned_be16(p: *const u8) -> u16 {
    unsafe {
        let mut v = 0u16;
        std::ptr::copy_nonoverlapping(p, &mut v as *mut u16 as *mut u8, 2);
        u16::from_be(v)
    }
}

#[inline(always)]
fn get_unaligned_be32(p: *const u8) -> u32 {
    unsafe {
        let mut v = 0u32;
        std::ptr::copy_nonoverlapping(p, &mut v as *mut u32 as *mut u8, 4);
        u32::from_be(v)
    }
}

#[inline(always)]
const fn can_consume(n: u32) -> bool {
    CONSUMABLE_NBITS >= n
}

#[inline(always)]
const fn can_consume_and_then_preload(consume: u32, preload: u32) -> bool {
    CONSUMABLE_NBITS >= consume && FASTLOOP_PRELOADABLE_NBITS >= consume + preload
}

#[inline(always)]
const fn max_const(a: u32, b: u32) -> u32 {
    if a >= b {
        a
    } else {
        b
    }
}

// --- Static decode result tables ---

static PRECODE_DECODE_RESULTS: [u32; DEFLATE_NUM_PRECODE_SYMS] = {
    let mut t = [0u32; DEFLATE_NUM_PRECODE_SYMS];
    let mut i = 0;
    while i < DEFLATE_NUM_PRECODE_SYMS {
        t[i] = (i as u32) << 16;
        i += 1;
    }
    t
};

static LITLEN_DECODE_RESULTS: [u32; DEFLATE_NUM_LITLEN_SYMS] = {
    let mut t = [0u32; DEFLATE_NUM_LITLEN_SYMS];
    // Literals 0..255
    let mut i = 0u32;
    while i < 256 {
        t[i as usize] = HUFFDEC_LITERAL | (i << 16);
        i += 1;
    }
    // End of block (symbol 256)
    t[256] = HUFFDEC_EXCEPTIONAL | HUFFDEC_END_OF_BLOCK;
    // Lengths (symbols 257..285)
    let length_bases: [u32; 29] = [
        3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115,
        131, 163, 195, 227, 258,
    ];
    let length_extra: [u32; 29] = [
        0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
    ];
    i = 0;
    while i < 29 {
        t[257 + i as usize] = (length_bases[i as usize] << 16) | length_extra[i as usize];
        i += 1;
    }
    // Symbols 286, 287 are unused but map to 258,0 like libdeflate
    t[286] = (258 << 16) | 0;
    t[287] = (258 << 16) | 0;
    t
};

static OFFSET_DECODE_RESULTS: [u32; DEFLATE_NUM_OFFSET_SYMS] = {
    let offset_bases: [u32; 32] = [
        1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
        2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577, 24577, 24577,
    ];
    let offset_extra: [u32; 32] = [
        0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12,
        13, 13, 13, 13,
    ];
    let mut t = [0u32; 32];
    let mut i = 0;
    while i < 32 {
        t[i] = (offset_bases[i] << 16) | offset_extra[i];
        i += 1;
    }
    t
};

// --- Decompressor state ---

pub struct Decompressor {
    // Union-like layout: precode_lens / (lens + precode_decode_table) / litlen_decode_table
    // We just allocate all of them separately since Rust doesn't have C unions like this
    precode_lens: [u8; DEFLATE_NUM_PRECODE_SYMS],
    lens: [u8; DEFLATE_NUM_LITLEN_SYMS + DEFLATE_NUM_OFFSET_SYMS + DEFLATE_MAX_LENS_OVERRUN],
    precode_decode_table: [u32; PRECODE_ENOUGH],
    litlen_decode_table: [u32; LITLEN_ENOUGH],
    offset_decode_table: [u32; OFFSET_ENOUGH],
    sorted_syms: [u16; DEFLATE_MAX_NUM_SYMS],
    static_codes_loaded: bool,
    litlen_tablebits: u32,
}

impl Default for Decompressor {
    fn default() -> Self {
        Self::new()
    }
}

impl Decompressor {
    pub fn new() -> Self {
        Decompressor {
            precode_lens: [0; DEFLATE_NUM_PRECODE_SYMS],
            lens: [0; DEFLATE_NUM_LITLEN_SYMS + DEFLATE_NUM_OFFSET_SYMS + DEFLATE_MAX_LENS_OVERRUN],
            precode_decode_table: [0; PRECODE_ENOUGH],
            litlen_decode_table: [0; LITLEN_ENOUGH],
            offset_decode_table: [0; OFFSET_ENOUGH],
            sorted_syms: [0; DEFLATE_MAX_NUM_SYMS],
            static_codes_loaded: false,
            litlen_tablebits: 0,
        }
    }
}

// --- Build decode table ---
// Faithful port of libdeflate's build_decode_table()

fn make_decode_table_entry(decode_results: &[u32], sym: u32, len: u32) -> u32 {
    decode_results[sym as usize] + (len << 8) + len
}

// --- Build decode table (unified, matches libdeflate exactly) ---

fn build_decode_table(
    decode_table: &mut [u32],
    lens: &[u8],
    num_syms: usize,
    decode_results: &[u32],
    mut table_bits: u32,
    max_codeword_len: u32,
    sorted_syms: &mut [u16],
    actual_table_bits: Option<&mut u32>,
) -> bool {
    let mut len_counts = [0u32; DEFLATE_MAX_CODEWORD_LEN as usize + 2];
    let mut offsets = [0u32; DEFLATE_MAX_CODEWORD_LEN as usize + 2];

    // Count codeword lengths
    for i in 0..num_syms {
        len_counts[lens[i] as usize] += 1;
    }

    // Sort symbols by length then by symbol index
    offsets[0] = 0;
    for l in 0..=max_codeword_len as usize {
        offsets[l + 1] = offsets[l] + len_counts[l];
    }
    for sym in 0..num_syms {
        let l = lens[sym] as usize;
        if l != 0 {
            sorted_syms[offsets[l] as usize] = sym as u16;
            offsets[l] += 1;
        }
    }

    let num_used = num_syms as u32 - len_counts[0];

    // Special cases: 0 or 1 used symbols
    if num_used <= 1 {
        if num_used == 0 {
            for i in 0..(1u32 << table_bits) as usize {
                decode_table[i] = 0;
            }
        } else {
            let sym_idx = len_counts[0] as usize;
            let sym = sorted_syms[sym_idx] as u32;
            let entry = make_decode_table_entry(decode_results, sym, 1);
            for i in 0..(1u32 << table_bits) as usize {
                decode_table[i] = entry;
            }
        }
        if let Some(bits) = actual_table_bits {
            *bits = table_bits;
        }
        return true;
    }

    // Verify the code is complete (not over/under-subscribed)
    {
        let mut codespace_used: i64 = 0;
        for len in 1..=max_codeword_len {
            codespace_used += (len_counts[len as usize] as i64) << (max_codeword_len - len);
        }
        if codespace_used != (1i64 << max_codeword_len) {
            return false;
        }
    }

    // Optionally cap table_bits
    if actual_table_bits.is_some() {
        let mut tb = table_bits;
        while tb > 1 && len_counts[tb as usize] == 0 {
            tb -= 1;
        }
        table_bits = tb;
    }
    if let Some(bits) = actual_table_bits {
        *bits = table_bits;
    }

    // --- Fill main table entries (codewords <= table_bits) ---

    let sym_base = len_counts[0] as usize;
    let mut sorted_ptr = sym_base;
    let mut codeword: u32 = 0;
    let mut len: u32 = 1;
    let mut count = len_counts[1];
    let mut cur_table_end: u32 = 1 << 1;

    // Skip empty lengths
    while count == 0 {
        len += 1;
        if len > table_bits {
            break;
        }
        count = len_counts[len as usize];
        cur_table_end = 1 << len;
    }

    // Fill main table
    if len <= table_bits {
        loop {
            // Process all symbols with this codeword length
            loop {
                let entry =
                    make_decode_table_entry(decode_results, sorted_syms[sorted_ptr] as u32, len);
                sorted_ptr += 1;

                // Fill stride entries
                let stride = 1u32 << len;
                let mut i = codeword;
                while i < cur_table_end {
                    decode_table[i as usize] = entry;
                    i += stride;
                }

                // Advance codeword (bit-reversed increment)
                if codeword == (1u32 << len) - 1 {
                    // Special: all 1s
                    // Check if we're done with main table
                    count -= 1;
                    // All codewords at this length exhausted? should be.
                    // Actually, if codeword == all 1s and count > 0,
                    // we'd wrap around. But canonical codes don't do that.
                    // Move to next length.
                    break;
                }
                let bit = 1u32 << bsr32(codeword ^ (cur_table_end - 1));
                codeword &= bit - 1;
                codeword |= bit;

                count -= 1;
                if count == 0 {
                    break;
                }
            }

            // Advance to the next codeword length
            loop {
                len += 1;
                if len > table_bits {
                    break;
                }
                // Double the table
                let end = cur_table_end as usize;
                if end * 2 > decode_table.len() {
                    return false;
                }
                for i in 0..end {
                    decode_table[end + i] = decode_table[i];
                }
                cur_table_end <<= 1;
                count = len_counts[len as usize];
                if count != 0 {
                    break;
                }
            }
            if len > table_bits {
                break;
            }
        }
    }

    cur_table_end = 1u32 << table_bits;

    // --- Fill subtable entries (codewords > table_bits) ---
    // Process remaining symbols
    if sorted_ptr >= sym_base + num_used as usize {
        return true; // No subtables needed
    }

    // Advance len to the first length with remaining symbols
    while len <= max_codeword_len && len_counts[len as usize] == 0 {
        len += 1;
    }
    if len > max_codeword_len {
        return true;
    }

    // Recompute codeword for this length
    // Using the canonical code reconstruction:
    {
        let mut c: u32 = 0;
        for l in 1..=len {
            c = (c + len_counts[l as usize - 1]) << 1;
        }
        // Advance past already-processed symbols at this length
        let already = if len <= table_bits {
            len_counts[len as usize] - count
        } else {
            0
        };
        c += already;
        // Bit-reverse the codeword
        codeword = c.reverse_bits() >> (32 - len);
    }

    count = len_counts[len as usize];

    let mut subtable_prefix: u32 = u32::MAX;

    loop {
        // Start a new subtable if needed
        let prefix = codeword & ((1u32 << table_bits) - 1);
        if prefix != subtable_prefix {
            subtable_prefix = prefix;
            let subtable_start = cur_table_end;
            let mut sub_bits = len - table_bits;
            let mut codespace_used = count;
            while codespace_used < (1u32 << sub_bits) {
                sub_bits += 1;
                if table_bits + sub_bits > max_codeword_len {
                    return false;
                }
                codespace_used =
                    (codespace_used << 1) + len_counts[(table_bits + sub_bits) as usize];
            }
            cur_table_end = subtable_start + (1u32 << sub_bits);
            if cur_table_end as usize > decode_table.len() {
                return false;
            }

            // Write pointer entry in main table
            decode_table[subtable_prefix as usize] = (subtable_start << 16)
                | HUFFDEC_EXCEPTIONAL
                | HUFFDEC_SUBTABLE_POINTER
                | (sub_bits << 8)
                | table_bits;

            // Fill subtable entries
            let entry = make_decode_table_entry(
                decode_results,
                sorted_syms[sorted_ptr] as u32,
                len - table_bits,
            );
            sorted_ptr += 1;

            let stride = 1u32 << (len - table_bits);
            let mut i = subtable_start + (codeword >> table_bits);
            while i < cur_table_end {
                decode_table[i as usize] = entry;
                i += stride;
            }
        } else {
            // Same subtable prefix, just fill the entry
            let subtable_start_search = decode_table[subtable_prefix as usize] >> 16;
            let sub_bits_search = (decode_table[subtable_prefix as usize] >> 8) & 0x3F;
            let local_end = subtable_start_search + (1u32 << sub_bits_search);

            let entry = make_decode_table_entry(
                decode_results,
                sorted_syms[sorted_ptr] as u32,
                len - table_bits,
            );
            sorted_ptr += 1;

            let stride = 1u32 << (len - table_bits);
            let mut i = subtable_start_search + (codeword >> table_bits);
            while i < local_end {
                decode_table[i as usize] = entry;
                i += stride;
            }
        }

        // Advance codeword
        if codeword == (1u32 << len) - 1 {
            // Last codeword
            return true;
        }
        let bit = 1u32 << bsr32(codeword ^ ((1u32 << len) - 1));
        codeword &= bit - 1;
        codeword |= bit;

        count -= 1;
        while count == 0 {
            len += 1;
            if len > max_codeword_len {
                return true;
            }
            count = len_counts[len as usize];
        }
    }
}

// --- Main decompression ---

/// Decompress raw DEFLATE data.
/// `input`: compressed deflate stream
/// `output`: pre-allocated output buffer
/// Returns (bytes_consumed, bytes_written) or error.
pub fn deflate_decompress(
    input: &[u8],
    output: &mut [u8],
) -> Result<(usize, usize), DecompressError> {
    let mut d = Decompressor::new();
    deflate_decompress_with(&mut d, input, output)
}

/// Decompress raw DEFLATE data using a reusable decompressor.
pub fn deflate_decompress_with(
    d: &mut Decompressor,
    input: &[u8],
    output: &mut [u8],
) -> Result<(usize, usize), DecompressError> {
    let out_ptr = output.as_mut_ptr();
    let out_len = output.len();
    let in_ptr = input.as_ptr();
    let in_len = input.len();

    unsafe { deflate_decompress_impl(d, in_ptr, in_len, out_ptr, out_len) }
}

unsafe fn deflate_decompress_impl(
    d: &mut Decompressor,
    in_base: *const u8,
    in_nbytes: usize,
    out_base: *mut u8,
    out_nbytes_avail: usize,
) -> Result<(usize, usize), DecompressError> {
    let mut out_next = out_base;
    let out_end = out_base.add(out_nbytes_avail);
    let out_fastloop_end = out_end.sub(out_nbytes_avail.min(FASTLOOP_MAX_BYTES_WRITTEN));

    let mut in_next = in_base;
    let in_end = in_base.add(in_nbytes);
    let in_fastloop_end = in_end.sub(in_nbytes.min(FASTLOOP_MAX_BYTES_READ));

    let mut bitbuf: BitBuf = 0;
    let mut bitsleft: u32 = 0;
    let mut overread_count: usize = 0;

    macro_rules! safety_check {
        ($expr:expr) => {
            if !($expr) {
                return Err(DecompressError::BadData);
            }
        };
    }

    macro_rules! refill_bits_branchless {
        () => {
            bitbuf |= load_word_unaligned(in_next) << (bitsleft as u8 as u32);
            in_next = in_next.add(WORDBYTES - 1);
            in_next = in_next.sub(((bitsleft >> 3) & 0x7) as usize);
            bitsleft |= MAX_BITSLEFT & !7;
        };
    }

    macro_rules! refill_bits {
        () => {
            if UNALIGNED_ACCESS_IS_FAST && (in_end as usize - in_next as usize) >= WORDBYTES {
                refill_bits_branchless!();
            } else {
                while (bitsleft as u8) < CONSUMABLE_NBITS as u8 {
                    if in_next != in_end {
                        bitbuf |= (*in_next as BitBuf) << (bitsleft as u8 as u32);
                        in_next = in_next.add(1);
                    } else {
                        overread_count += 1;
                        safety_check!(overread_count <= WORDBYTES);
                    }
                    bitsleft += 8;
                }
            }
        };
    }

    macro_rules! refill_bits_in_fastloop {
        () => {
            if UNALIGNED_ACCESS_IS_FAST {
                refill_bits_branchless!();
            } else {
                while (bitsleft as u8) < CONSUMABLE_NBITS as u8 {
                    bitbuf |= (*in_next as BitBuf) << (bitsleft as u8 as u32);
                    in_next = in_next.add(1);
                    bitsleft += 8;
                }
            }
        };
    }

    let mut is_final_block;

    'next_block: loop {
        refill_bits!();

        is_final_block = (bitbuf & bitmask(1)) != 0;
        let block_type = ((bitbuf >> 1) & bitmask(2)) as u32;

        if block_type == DEFLATE_BLOCKTYPE_DYNAMIC_HUFFMAN {
            // Dynamic Huffman block
            static PRECODE_PERM: [u8; DEFLATE_NUM_PRECODE_SYMS] = [
                16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
            ];

            let num_litlen_syms = 257 + ((bitbuf >> 3) & bitmask(5)) as usize;
            let num_offset_syms = 1 + ((bitbuf >> 8) & bitmask(5)) as usize;
            let num_explicit_precode_lens = 4 + ((bitbuf >> 13) & bitmask(4)) as usize;

            d.static_codes_loaded = false;

            // Read precode lengths
            if can_consume(3 * (DEFLATE_NUM_PRECODE_SYMS as u32 - 1)) {
                d.precode_lens[PRECODE_PERM[0] as usize] = ((bitbuf >> 17) & bitmask(3)) as u8;
                bitbuf >>= 20;
                bitsleft -= 20;
                refill_bits!();
                for i in 1..num_explicit_precode_lens {
                    d.precode_lens[PRECODE_PERM[i] as usize] = (bitbuf & bitmask(3)) as u8;
                    bitbuf >>= 3;
                    bitsleft -= 3;
                }
            } else {
                bitbuf >>= 17;
                bitsleft -= 17;
                for i in 0..num_explicit_precode_lens {
                    if (bitsleft as u8) < 3 {
                        refill_bits!();
                    }
                    d.precode_lens[PRECODE_PERM[i] as usize] = (bitbuf & bitmask(3)) as u8;
                    bitbuf >>= 3;
                    bitsleft -= 3;
                }
            }
            for i in num_explicit_precode_lens..DEFLATE_NUM_PRECODE_SYMS {
                d.precode_lens[PRECODE_PERM[i] as usize] = 0;
            }

            // Build precode decode table
            safety_check!(build_decode_table(
                &mut d.precode_decode_table,
                &d.precode_lens,
                DEFLATE_NUM_PRECODE_SYMS,
                &PRECODE_DECODE_RESULTS,
                PRECODE_TABLEBITS,
                DEFLATE_MAX_PRE_CODEWORD_LEN,
                &mut d.sorted_syms,
                None,
            ));

            // Decode litlen and offset codeword lengths
            let mut i = 0usize;
            let total_lens = num_litlen_syms + num_offset_syms;
            while i < total_lens {
                if (bitsleft as u8) < DEFLATE_MAX_PRE_CODEWORD_LEN as u8 + 7 {
                    refill_bits!();
                }

                let entry = d.precode_decode_table
                    [(bitbuf & bitmask(DEFLATE_MAX_PRE_CODEWORD_LEN)) as usize];
                bitbuf >>= entry as u8 as u32;
                bitsleft -= entry & 0xFF;
                let presym = (entry >> 16) as u32;

                if presym < 16 {
                    d.lens[i] = presym as u8;
                    i += 1;
                    continue;
                }

                if presym == 16 {
                    safety_check!(i != 0);
                    let rep_val = d.lens[i - 1];
                    let rep_count = 3 + (bitbuf & bitmask(2)) as usize;
                    bitbuf >>= 2;
                    bitsleft -= 2;
                    for j in 0..6.min(DEFLATE_MAX_LENS_OVERRUN + total_lens - i) {
                        d.lens[i + j] = rep_val;
                    }
                    i += rep_count;
                } else if presym == 17 {
                    let rep_count = 3 + (bitbuf & bitmask(3)) as usize;
                    bitbuf >>= 3;
                    bitsleft -= 3;
                    for j in 0..10.min(DEFLATE_MAX_LENS_OVERRUN + total_lens - i) {
                        d.lens[i + j] = 0;
                    }
                    i += rep_count;
                } else {
                    // presym == 18
                    let rep_count = 11 + (bitbuf & bitmask(7)) as usize;
                    bitbuf >>= 7;
                    bitsleft -= 7;
                    let fill_len = rep_count.min(DEFLATE_MAX_LENS_OVERRUN + total_lens - i);
                    for j in 0..fill_len {
                        d.lens[i + j] = 0;
                    }
                    i += rep_count;
                }
            }
            safety_check!(i == total_lens);

            // Build offset decode table first (because lens overlaps litlen_decode_table in C)
            safety_check!(build_decode_table(
                &mut d.offset_decode_table,
                &d.lens[num_litlen_syms..],
                num_offset_syms,
                &OFFSET_DECODE_RESULTS,
                OFFSET_TABLEBITS,
                DEFLATE_MAX_OFFSET_CODEWORD_LEN,
                &mut d.sorted_syms,
                None,
            ));
            safety_check!(build_decode_table(
                &mut d.litlen_decode_table,
                &d.lens,
                num_litlen_syms,
                &LITLEN_DECODE_RESULTS,
                LITLEN_TABLEBITS,
                DEFLATE_MAX_LITLEN_CODEWORD_LEN,
                &mut d.sorted_syms,
                Some(&mut d.litlen_tablebits),
            ));
        } else if block_type == DEFLATE_BLOCKTYPE_UNCOMPRESSED {
            // Uncompressed block
            bitsleft -= 3;
            bitsleft = bitsleft as u8 as u32;
            safety_check!(overread_count <= (bitsleft >> 3) as usize);
            in_next = in_next.sub((bitsleft >> 3) as usize - overread_count);
            overread_count = 0;
            bitbuf = 0;
            bitsleft = 0;

            safety_check!((in_end as usize - in_next as usize) >= 4);
            let len = get_unaligned_le16(in_next) as usize;
            let nlen = get_unaligned_le16(in_next.add(2));
            in_next = in_next.add(4);

            safety_check!(len == (!nlen & 0xFFFF) as usize);
            if len > (out_end as usize - out_next as usize) {
                return Err(DecompressError::InsufficientSpace);
            }
            safety_check!(len <= (in_end as usize - in_next as usize));

            std::ptr::copy_nonoverlapping(in_next, out_next, len);
            in_next = in_next.add(len);
            out_next = out_next.add(len);

            if is_final_block {
                break 'next_block;
            }
            continue 'next_block;
        } else {
            // Static Huffman block
            safety_check!(block_type == DEFLATE_BLOCKTYPE_STATIC_HUFFMAN);

            bitbuf >>= 3;
            bitsleft -= 3;

            if d.static_codes_loaded {
                // Reuse cached tables
            } else {
                d.static_codes_loaded = true;

                let mut i = 0;
                while i < 144 {
                    d.lens[i] = 8;
                    i += 1;
                }
                while i < 256 {
                    d.lens[i] = 9;
                    i += 1;
                }
                while i < 280 {
                    d.lens[i] = 7;
                    i += 1;
                }
                while i < 288 {
                    d.lens[i] = 8;
                    i += 1;
                }
                while i < 288 + 32 {
                    d.lens[i] = 5;
                    i += 1;
                }

                safety_check!(build_decode_table(
                    &mut d.offset_decode_table,
                    &d.lens[288..],
                    32,
                    &OFFSET_DECODE_RESULTS,
                    OFFSET_TABLEBITS,
                    DEFLATE_MAX_OFFSET_CODEWORD_LEN,
                    &mut d.sorted_syms,
                    None,
                ));
                safety_check!(build_decode_table(
                    &mut d.litlen_decode_table,
                    &d.lens,
                    288,
                    &LITLEN_DECODE_RESULTS,
                    LITLEN_TABLEBITS,
                    DEFLATE_MAX_LITLEN_CODEWORD_LEN,
                    &mut d.sorted_syms,
                    Some(&mut d.litlen_tablebits),
                ));
            }
        }

        // --- Decode Huffman block (fast loop + generic loop) ---
        let litlen_tablemask = bitmask(d.litlen_tablebits);

        // Fast loop
        if in_next < in_fastloop_end && out_next < out_fastloop_end {
            refill_bits_in_fastloop!();
            let mut entry = d.litlen_decode_table[(bitbuf & litlen_tablemask) as usize];

            while in_next < in_fastloop_end && out_next < out_fastloop_end {
                let mut saved_bitbuf = bitbuf;
                bitbuf >>= entry as u8 as u32;
                bitsleft -= entry & 0xFF;

                if entry & HUFFDEC_LITERAL != 0 {
                    // Fast literal path - try to decode up to 2 extra literals
                    if can_consume_and_then_preload(
                        2 * LITLEN_TABLEBITS + LENGTH_MAXBITS,
                        OFFSET_TABLEBITS,
                    ) && can_consume_and_then_preload(
                        2 * LITLEN_TABLEBITS + DEFLATE_MAX_LITLEN_CODEWORD_LEN,
                        LITLEN_TABLEBITS,
                    ) {
                        // 1st extra fast literal
                        let lit = (entry >> 16) as u8;
                        entry = d.litlen_decode_table[(bitbuf & litlen_tablemask) as usize];
                        saved_bitbuf = bitbuf;
                        bitbuf >>= entry as u8 as u32;
                        bitsleft -= entry & 0xFF;
                        *out_next = lit;
                        out_next = out_next.add(1);

                        if entry & HUFFDEC_LITERAL != 0 {
                            // 2nd extra fast literal
                            let lit = (entry >> 16) as u8;
                            entry = d.litlen_decode_table[(bitbuf & litlen_tablemask) as usize];
                            saved_bitbuf = bitbuf;
                            bitbuf >>= entry as u8 as u32;
                            bitsleft -= entry & 0xFF;
                            *out_next = lit;
                            out_next = out_next.add(1);

                            if entry & HUFFDEC_LITERAL != 0 {
                                let lit = (entry >> 16) as u8;
                                entry = d.litlen_decode_table[(bitbuf & litlen_tablemask) as usize];
                                refill_bits_in_fastloop!();
                                *out_next = lit;
                                out_next = out_next.add(1);
                                continue;
                            }
                        }
                    } else {
                        let lit = (entry >> 16) as u8;
                        entry = d.litlen_decode_table[(bitbuf & litlen_tablemask) as usize];
                        refill_bits_in_fastloop!();
                        *out_next = lit;
                        out_next = out_next.add(1);
                        continue;
                    }
                }

                // Not a literal: length, subtable, or end-of-block
                if entry & HUFFDEC_EXCEPTIONAL != 0 {
                    if entry & HUFFDEC_END_OF_BLOCK != 0 {
                        if is_final_block {
                            break 'next_block;
                        }
                        continue 'next_block;
                    }
                    // Subtable pointer
                    let subtable_idx = (entry >> 16) as usize;
                    let sub_bits = (entry >> 8) & 0x3F;
                    entry =
                        d.litlen_decode_table[subtable_idx + (bitbuf & bitmask(sub_bits)) as usize];
                    saved_bitbuf = bitbuf;
                    bitbuf >>= entry as u8 as u32;
                    bitsleft -= entry & 0xFF;

                    if !can_consume_and_then_preload(
                        DEFLATE_MAX_LITLEN_CODEWORD_LEN,
                        LITLEN_TABLEBITS,
                    ) || !can_consume_and_then_preload(LENGTH_MAXBITS, OFFSET_TABLEBITS)
                    {
                        refill_bits_in_fastloop!();
                    }

                    if entry & HUFFDEC_LITERAL != 0 {
                        let lit = (entry >> 16) as u8;
                        entry = d.litlen_decode_table[(bitbuf & litlen_tablemask) as usize];
                        refill_bits_in_fastloop!();
                        *out_next = lit;
                        out_next = out_next.add(1);
                        continue;
                    }
                    if entry & HUFFDEC_END_OF_BLOCK != 0 {
                        if is_final_block {
                            break 'next_block;
                        }
                        continue 'next_block;
                    }
                }

                // Decode match length
                let mut length = (entry >> 16) as usize;
                let extra_len_bits = entry & 0xFF;
                let codeword_len = (entry >> 8) as u8 as u32;
                length +=
                    ((saved_bitbuf & bitmask(extra_len_bits as u32)) >> codeword_len) as usize;

                // Decode match offset
                entry = d.offset_decode_table[(bitbuf & bitmask(OFFSET_TABLEBITS)) as usize];

                if can_consume_and_then_preload(OFFSET_MAXBITS, LITLEN_TABLEBITS) {
                    if entry & HUFFDEC_EXCEPTIONAL != 0 {
                        if (bitsleft as u8)
                            < (OFFSET_MAXBITS + LITLEN_TABLEBITS - PRELOAD_SLACK) as u8
                        {
                            refill_bits_in_fastloop!();
                        }
                        bitbuf >>= OFFSET_TABLEBITS;
                        bitsleft -= OFFSET_TABLEBITS;
                        let sub_idx = (entry >> 16) as usize;
                        let sub_bits = (entry >> 8) & 0x3F;
                        entry =
                            d.offset_decode_table[sub_idx + (bitbuf & bitmask(sub_bits)) as usize];
                    } else if (bitsleft as u8)
                        < (OFFSET_MAXFASTBITS + LITLEN_TABLEBITS - PRELOAD_SLACK) as u8
                    {
                        refill_bits_in_fastloop!();
                    }
                } else {
                    refill_bits_in_fastloop!();
                    if entry & HUFFDEC_EXCEPTIONAL != 0 {
                        bitbuf >>= OFFSET_TABLEBITS;
                        bitsleft -= OFFSET_TABLEBITS;
                        let sub_idx = (entry >> 16) as usize;
                        let sub_bits = (entry >> 8) & 0x3F;
                        entry =
                            d.offset_decode_table[sub_idx + (bitbuf & bitmask(sub_bits)) as usize];
                        refill_bits_in_fastloop!();
                    }
                }

                let saved_bitbuf2 = bitbuf;
                bitbuf >>= entry as u8 as u32;
                bitsleft -= entry & 0xFF;
                let mut offset = (entry >> 16) as usize;
                let extra_off_bits = entry & 0xFF;
                let off_codelen = (entry >> 8) as u8 as u32;
                offset +=
                    ((saved_bitbuf2 & bitmask(extra_off_bits as u32)) >> off_codelen) as usize;

                // Validate offset
                let out_pos = out_next as usize - out_base as usize;
                safety_check!(offset <= out_pos);

                let src = out_next.sub(offset);
                let dst = out_next;
                out_next = out_next.add(length);

                // Preload next entry + refill before copy
                if !can_consume_and_then_preload(
                    max_const(OFFSET_MAXBITS - OFFSET_TABLEBITS, OFFSET_MAXFASTBITS),
                    LITLEN_TABLEBITS,
                ) && (bitsleft as u8) < (LITLEN_TABLEBITS - PRELOAD_SLACK) as u8
                {
                    refill_bits_in_fastloop!();
                }
                entry = d.litlen_decode_table[(bitbuf & litlen_tablemask) as usize];
                refill_bits_in_fastloop!();

                // Copy match
                if UNALIGNED_ACCESS_IS_FAST && offset >= WORDBYTES {
                    let mut s = src;
                    let mut d = dst;
                    store_word_unaligned(load_word_unaligned(s), d);
                    s = s.add(WORDBYTES);
                    d = d.add(WORDBYTES);
                    store_word_unaligned(load_word_unaligned(s), d);
                    s = s.add(WORDBYTES);
                    d = d.add(WORDBYTES);
                    store_word_unaligned(load_word_unaligned(s), d);
                    s = s.add(WORDBYTES);
                    d = d.add(WORDBYTES);
                    store_word_unaligned(load_word_unaligned(s), d);
                    s = s.add(WORDBYTES);
                    d = d.add(WORDBYTES);
                    store_word_unaligned(load_word_unaligned(s), d);
                    s = s.add(WORDBYTES);
                    d = d.add(WORDBYTES);
                    while d < out_next {
                        store_word_unaligned(load_word_unaligned(s), d);
                        s = s.add(WORDBYTES);
                        d = d.add(WORDBYTES);
                    }
                } else if UNALIGNED_ACCESS_IS_FAST && offset == 1 {
                    let v = 0x0101010101010101u64 * (*src as u64);
                    let mut d = dst;
                    store_word_unaligned(v, d);
                    d = d.add(WORDBYTES);
                    store_word_unaligned(v, d);
                    d = d.add(WORDBYTES);
                    store_word_unaligned(v, d);
                    d = d.add(WORDBYTES);
                    store_word_unaligned(v, d);
                    d = d.add(WORDBYTES);
                    while d < out_next {
                        store_word_unaligned(v, d);
                        d = d.add(WORDBYTES);
                    }
                } else if UNALIGNED_ACCESS_IS_FAST {
                    let mut s = src;
                    let mut d = dst;
                    store_word_unaligned(load_word_unaligned(s), d);
                    s = s.add(offset);
                    d = d.add(offset);
                    store_word_unaligned(load_word_unaligned(s), d);
                    s = s.add(offset);
                    d = d.add(offset);
                    while d < out_next {
                        store_word_unaligned(load_word_unaligned(s), d);
                        s = s.add(offset);
                        d = d.add(offset);
                    }
                } else {
                    let mut s = src;
                    let mut d = dst;
                    *d = *s;
                    d = d.add(1);
                    s = s.add(1);
                    *d = *s;
                    d = d.add(1);
                    s = s.add(1);
                    while d < out_next {
                        *d = *s;
                        d = d.add(1);
                        s = s.add(1);
                    }
                }
            }
        }

        // Generic loop (near end of buffers)
        loop {
            refill_bits!();
            let mut entry = d.litlen_decode_table[(bitbuf & litlen_tablemask) as usize];
            let mut saved_bitbuf = bitbuf;
            bitbuf >>= entry as u8 as u32;
            bitsleft -= entry & 0xFF;

            if entry & HUFFDEC_SUBTABLE_POINTER != 0 {
                let sub_idx = (entry >> 16) as usize;
                let sub_bits = (entry >> 8) & 0x3F;
                entry = d.litlen_decode_table[sub_idx + (bitbuf & bitmask(sub_bits)) as usize];
                saved_bitbuf = bitbuf;
                bitbuf >>= entry as u8 as u32;
                bitsleft -= entry & 0xFF;
            }

            let value = (entry >> 16) as usize;
            if entry & HUFFDEC_LITERAL != 0 {
                if out_next == out_end {
                    return Err(DecompressError::InsufficientSpace);
                }
                *out_next = value as u8;
                out_next = out_next.add(1);
                continue;
            }
            if entry & HUFFDEC_END_OF_BLOCK != 0 {
                break;
            }

            // Length
            let extra_bits = entry & 0xFF;
            let codelen = (entry >> 8) as u8 as u32;
            let length = value + ((saved_bitbuf & bitmask(extra_bits as u32)) >> codelen) as usize;
            if length > (out_end as usize - out_next as usize) {
                return Err(DecompressError::InsufficientSpace);
            }

            if !can_consume(LENGTH_MAXBITS + OFFSET_MAXBITS) {
                refill_bits!();
            }

            // Offset
            entry = d.offset_decode_table[(bitbuf & bitmask(OFFSET_TABLEBITS)) as usize];
            if entry & HUFFDEC_EXCEPTIONAL != 0 {
                bitbuf >>= OFFSET_TABLEBITS;
                bitsleft -= OFFSET_TABLEBITS;
                let sub_idx = (entry >> 16) as usize;
                let sub_bits = (entry >> 8) & 0x3F;
                entry = d.offset_decode_table[sub_idx + (bitbuf & bitmask(sub_bits)) as usize];
                if !can_consume(OFFSET_MAXBITS) {
                    refill_bits!();
                }
            }
            let mut offset = (entry >> 16) as usize;
            let extra_off_bits = entry & 0xFF;
            let off_codelen = (entry >> 8) as u8 as u32;
            offset += ((bitbuf & bitmask(extra_off_bits as u32)) >> off_codelen) as usize;
            bitbuf >>= entry as u8 as u32;
            bitsleft -= entry & 0xFF;

            let out_pos = out_next as usize - out_base as usize;
            safety_check!(offset <= out_pos);

            let src = out_next.sub(offset);
            let dst = out_next;
            out_next = out_next.add(length);

            // Byte-at-a-time copy for generic loop
            let mut s = src;
            let mut d = dst;
            *d = *s;
            d = d.add(1);
            s = s.add(1);
            *d = *s;
            d = d.add(1);
            s = s.add(1);
            while d < out_next {
                *d = *s;
                d = d.add(1);
                s = s.add(1);
            }
        }

        // Block done
        if is_final_block {
            break 'next_block;
        }
    }

    // Final bookkeeping
    bitsleft = bitsleft as u8 as u32;
    safety_check!(overread_count <= (bitsleft >> 3) as usize);

    let actual_in = in_next.sub((bitsleft >> 3) as usize - overread_count);
    let bytes_consumed = actual_in as usize - in_base as usize;
    let bytes_written = out_next as usize - out_base as usize;

    Ok((bytes_consumed, bytes_written))
}

// --- Zlib wrapper ---

const ZLIB_MIN_OVERHEAD: usize = 6;
const ZLIB_FOOTER_SIZE: usize = 4;
const ZLIB_CM_DEFLATE: u8 = 8;
const ZLIB_CINFO_32K_WINDOW: u8 = 7;

/// Decompress zlib-wrapped data.
/// Returns (bytes_consumed, bytes_written) or error.
pub fn zlib_decompress(input: &[u8], output: &mut [u8]) -> Result<(usize, usize), DecompressError> {
    let mut d = Decompressor::new();
    zlib_decompress_with(&mut d, input, output)
}

/// Decompress zlib-wrapped data using a reusable decompressor.
pub fn zlib_decompress_with(
    d: &mut Decompressor,
    input: &[u8],
    output: &mut [u8],
) -> Result<(usize, usize), DecompressError> {
    if input.len() < ZLIB_MIN_OVERHEAD {
        return Err(DecompressError::BadData);
    }

    // Parse 2-byte header
    let hdr = get_unaligned_be16(input.as_ptr());
    if (hdr % 31) != 0 {
        return Err(DecompressError::BadData);
    }
    if ((hdr >> 8) & 0xF) as u8 != ZLIB_CM_DEFLATE {
        return Err(DecompressError::BadData);
    }
    if (hdr >> 12) as u8 > ZLIB_CINFO_32K_WINDOW {
        return Err(DecompressError::BadData);
    }
    // FDICT
    if ((hdr >> 5) & 1) != 0 {
        return Err(DecompressError::BadData);
    }

    let deflate_data = &input[2..];
    let (consumed, written) = deflate_decompress_with(d, deflate_data, output)?;

    // Verify Adler-32 footer follows immediately after consumed deflate data
    let footer_start = 2 + consumed;
    if footer_start + ZLIB_FOOTER_SIZE > input.len() {
        return Err(DecompressError::BadData);
    }
    let expected_adler = get_unaligned_be32(unsafe { input.as_ptr().add(footer_start) });
    let actual_adler = adler32(1, &output[..written]);
    if actual_adler != expected_adler {
        return Err(DecompressError::BadData);
    }

    Ok((footer_start + 4, written))
}

// --- Auto-sizing decompression (unknown output size) ---

const INITIAL_VEC_CAPACITY: usize = 4096;
const MAX_VEC_CAPACITY: usize = 256 * 1024 * 1024; // 256 MB safety cap

/// Decompress raw DEFLATE data when the output size is unknown.
/// Automatically sizes the output buffer, retrying with larger buffers as needed.
/// Returns the decompressed data as a Vec<u8>, or an error.
pub fn deflate_decompress_vec(input: &[u8]) -> Result<Vec<u8>, DecompressError> {
    let mut d = Decompressor::new();
    // Start with a reasonable estimate: 3x input or 4KB, whichever is larger
    let mut capacity = (input.len() * 3).max(INITIAL_VEC_CAPACITY);
    loop {
        let mut output = vec![0u8; capacity];
        match deflate_decompress_with(&mut d, input, &mut output) {
            Ok((_consumed, written)) => {
                output.truncate(written);
                return Ok(output);
            }
            Err(DecompressError::InsufficientSpace) => {
                capacity = capacity
                    .checked_mul(2)
                    .ok_or(DecompressError::InsufficientSpace)?;
                if capacity > MAX_VEC_CAPACITY {
                    return Err(DecompressError::InsufficientSpace);
                }
                // Retry with bigger buffer
            }
            Err(e) => return Err(e),
        }
    }
}

/// Decompress zlib-wrapped data when the output size is unknown.
/// Automatically sizes the output buffer, retrying with larger buffers as needed.
/// Returns the decompressed data as a Vec<u8>, or an error.
pub fn zlib_decompress_vec(input: &[u8]) -> Result<Vec<u8>, DecompressError> {
    let mut d = Decompressor::new();
    // Start with a reasonable estimate: 3x input or 4KB, whichever is larger
    let mut capacity = (input.len() * 3).max(INITIAL_VEC_CAPACITY);
    loop {
        let mut output = vec![0u8; capacity];
        match zlib_decompress_with(&mut d, input, &mut output) {
            Ok((_consumed, written)) => {
                output.truncate(written);
                return Ok(output);
            }
            Err(DecompressError::InsufficientSpace) => {
                capacity = capacity
                    .checked_mul(2)
                    .ok_or(DecompressError::InsufficientSpace)?;
                if capacity > MAX_VEC_CAPACITY {
                    return Err(DecompressError::InsufficientSpace);
                }
                // Retry with bigger buffer
            }
            Err(e) => return Err(e),
        }
    }
}

/// Decompress zlib-wrapped data with a size hint.
/// If the hint is correct, avoids any retry overhead. If not, falls back to auto-sizing.
/// This is ideal for git objects where the header tells you the size.
pub fn zlib_decompress_vec_with_hint(
    input: &[u8],
    size_hint: usize,
) -> Result<Vec<u8>, DecompressError> {
    let mut d = Decompressor::new();
    // Try with the hint first
    let mut capacity = size_hint;
    loop {
        let mut output = vec![0u8; capacity];
        match zlib_decompress_with(&mut d, input, &mut output) {
            Ok((_consumed, written)) => {
                output.truncate(written);
                return Ok(output);
            }
            Err(DecompressError::InsufficientSpace) => {
                // Hint was wrong, double and retry
                capacity = capacity
                    .checked_mul(2)
                    .ok_or(DecompressError::InsufficientSpace)?;
                if capacity > MAX_VEC_CAPACITY {
                    return Err(DecompressError::InsufficientSpace);
                }
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: compress with C libdeflater, decompress with our Rust port, compare byte-for-byte
    fn roundtrip_vs_c_zlib(data: &[u8], level: i32) {
        // Compress with C libdeflater
        let mut c_comp =
            libdeflater::Compressor::new(libdeflater::CompressionLvl::new(level).unwrap());
        let max_sz = c_comp.zlib_compress_bound(data.len());
        let mut compressed = vec![0u8; max_sz];
        let c_compressed_len = c_comp.zlib_compress(data, &mut compressed).unwrap();
        compressed.truncate(c_compressed_len);

        // Decompress with C libdeflater (reference)
        let mut c_decomp = libdeflater::Decompressor::new();
        let mut c_output = vec![0u8; data.len()];
        let c_written = c_decomp
            .zlib_decompress(&compressed, &mut c_output)
            .unwrap();
        assert_eq!(c_written, data.len());
        assert_eq!(&c_output[..c_written], data);

        // Decompress with our Rust port
        let mut rust_output = vec![0u8; data.len()];
        let (_, rust_written) =
            zlib_decompress(&compressed, &mut rust_output).expect("Rust zlib_decompress failed");
        assert_eq!(rust_written, data.len(), "output length mismatch");
        assert_eq!(&rust_output[..rust_written], data, "output data mismatch");
    }

    fn roundtrip_vs_c_deflate(data: &[u8], level: i32) {
        let mut c_comp =
            libdeflater::Compressor::new(libdeflater::CompressionLvl::new(level).unwrap());
        let max_sz = c_comp.deflate_compress_bound(data.len());
        let mut compressed = vec![0u8; max_sz];
        let c_compressed_len = c_comp.deflate_compress(data, &mut compressed).unwrap();
        compressed.truncate(c_compressed_len);

        // C reference
        let mut c_decomp = libdeflater::Decompressor::new();
        let mut c_output = vec![0u8; data.len()];
        let c_written = c_decomp
            .deflate_decompress(&compressed, &mut c_output)
            .unwrap();
        assert_eq!(&c_output[..c_written], data);

        // Our Rust port
        let mut rust_output = vec![0u8; data.len()];
        let (_, rust_written) = deflate_decompress(&compressed, &mut rust_output)
            .expect("Rust deflate_decompress failed");
        assert_eq!(rust_written, data.len());
        assert_eq!(&rust_output[..rust_written], data);
    }

    #[test]
    fn test_empty() {
        roundtrip_vs_c_zlib(b"", 6);
        roundtrip_vs_c_deflate(b"", 6);
    }

    #[test]
    fn test_one_byte() {
        roundtrip_vs_c_zlib(b"x", 6);
        roundtrip_vs_c_deflate(b"x", 6);
    }

    #[test]
    fn test_short_string() {
        let data = b"Hello, World!";
        for level in [1, 6, 9, 12] {
            roundtrip_vs_c_zlib(data, level);
            roundtrip_vs_c_deflate(data, level);
        }
    }

    #[test]
    fn test_repeated_data() {
        // Highly compressible - exercises long matches
        let data: Vec<u8> = b"ABCDEFGH".iter().copied().cycle().take(65536).collect();
        for level in [1, 6, 9, 12] {
            roundtrip_vs_c_zlib(&data, level);
            roundtrip_vs_c_deflate(&data, level);
        }
    }

    #[test]
    fn test_all_same_byte() {
        // RLE-like compression
        let data = vec![0x42u8; 100_000];
        for level in [1, 6, 12] {
            roundtrip_vs_c_zlib(&data, level);
        }
    }

    #[test]
    fn test_sequential_bytes() {
        let data: Vec<u8> = (0..=255).cycle().take(50_000).collect();
        for level in [1, 6, 12] {
            roundtrip_vs_c_zlib(&data, level);
        }
    }

    #[test]
    fn test_pseudo_random() {
        // Low compressibility - exercises many literals
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let data: Vec<u8> = (0..100_000).map(|_| rng.gen()).collect();
        for level in [1, 6, 12] {
            roundtrip_vs_c_zlib(&data, level);
            roundtrip_vs_c_deflate(&data, level);
        }
    }

    #[test]
    fn test_mixed_compressibility() {
        use rand::{Rng, SeedableRng};
        let mut rng = rand::rngs::StdRng::seed_from_u64(42);
        let mut data = Vec::with_capacity(200_000);
        for _ in 0..20 {
            data.extend(
                b"The quick brown fox jumps over the lazy dog. "
                    .iter()
                    .cycle()
                    .take(5000),
            );
            data.extend((0..5000).map(|_| rng.gen::<u8>()));
        }
        for level in [1, 6, 9, 12] {
            roundtrip_vs_c_deflate(&data, level);
            roundtrip_vs_c_zlib(&data, level);
        }
    }

    #[test]
    fn test_all_compression_levels() {
        let data = b"Pack my box with five dozen liquor jugs. ".repeat(1000);
        for level in 1..=12 {
            roundtrip_vs_c_zlib(&data, level);
            roundtrip_vs_c_deflate(&data, level);
        }
    }

    #[test]
    fn test_uncompressed_block() {
        // Level 0 forces uncompressed blocks
        use std::io::Write;
        let input = b"This is uncompressed data that should be stored as-is";
        let mut encoder =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::none());
        encoder.write_all(input).unwrap();
        let compressed = encoder.finish().unwrap();

        let mut output = vec![0u8; input.len()];
        let (_, written) = deflate_decompress(&compressed, &mut output).unwrap();
        assert_eq!(written, input.len());
        assert_eq!(&output[..written], &input[..]);
    }

    #[test]
    fn test_various_sizes() {
        // Test a range of output sizes including edge cases
        use rand::Rng;
        let mut rng = rand::thread_rng();
        for size in [
            0, 1, 2, 3, 7, 8, 15, 16, 31, 32, 63, 64, 127, 128, 255, 256, 511, 512, 1023, 1024,
            4095, 4096, 8191, 8192, 16383, 16384, 32767, 32768, 65535, 65536,
        ] {
            let data: Vec<u8> = (0..size).map(|_| rng.gen()).collect();
            roundtrip_vs_c_zlib(&data, 6);
        }
    }

    #[test]
    fn test_large_window_offsets() {
        // Create data where matches reference far-back positions (large offsets)
        let mut data = Vec::with_capacity(100_000);
        // Fill 32K with varied data
        for i in 0..32768u32 {
            data.push((i.wrapping_mul(7) ^ i.wrapping_mul(13)) as u8);
        }
        // Now repeat chunks from the beginning (forces large offsets)
        for _ in 0..2 {
            data.extend_from_slice(&data[0..32768].to_vec());
        }
        roundtrip_vs_c_zlib(&data, 6);
        roundtrip_vs_c_zlib(&data, 12);
    }

    #[test]
    fn test_cross_compressed_flate2_to_rust() {
        // Compress with flate2 (miniz_oxide), decompress with our Rust port
        use std::io::Write;
        let data = b"Cross-library compatibility test! ".repeat(5000);
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        encoder.write_all(&data).unwrap();
        let compressed = encoder.finish().unwrap();

        let mut output = vec![0u8; data.len()];
        let (_, written) = zlib_decompress(&compressed, &mut output).unwrap();
        assert_eq!(written, data.len());
        assert_eq!(&output[..written], &data[..]);
    }

    #[test]
    fn test_benchmark_zlib_correctness_1mb() {
        // 1MB test: ensures no off-by-one errors at scale
        use rand::{Rng, SeedableRng};
        let mut rng = rand::rngs::StdRng::seed_from_u64(123);
        let size = 1_000_000;
        // Mix of patterns: 75% compressible, 25% random
        let mut data = Vec::with_capacity(size);
        while data.len() < size {
            if rng.gen::<f32>() < 0.75 {
                data.extend_from_slice(
                    b"Makepad is a creative software development platform built in Rust. ",
                );
            } else {
                let n = (rng.gen::<usize>() % 100) + 1;
                data.extend((0..n).map(|_| rng.gen::<u8>()));
            }
        }
        data.truncate(size);
        roundtrip_vs_c_zlib(&data, 6);
    }

    #[test]
    fn test_garbage_input_no_panic() {
        // Feed random garbage to both deflate and zlib decompressors.
        // Must never panic — only return errors.
        use rand::{Rng, SeedableRng};
        let mut rng = rand::rngs::StdRng::seed_from_u64(0xdead);
        let mut out = vec![0u8; 65536];

        for _ in 0..10_000 {
            let len = rng.gen_range(0..=512);
            let garbage: Vec<u8> = (0..len).map(|_| rng.gen()).collect();
            // These must not panic
            let _ = deflate_decompress(&garbage, &mut out);
            let _ = zlib_decompress(&garbage, &mut out);
        }

        // Also test with tiny output buffers
        let mut tiny_out = vec![0u8; 1];
        for _ in 0..1_000 {
            let len = rng.gen_range(0..=256);
            let garbage: Vec<u8> = (0..len).map(|_| rng.gen()).collect();
            let _ = deflate_decompress(&garbage, &mut tiny_out);
            let _ = zlib_decompress(&garbage, &mut tiny_out);
        }

        // Test with zero-length output buffer
        let mut empty_out = vec![0u8; 0];
        for _ in 0..1_000 {
            let len = rng.gen_range(0..=256);
            let garbage: Vec<u8> = (0..len).map(|_| rng.gen()).collect();
            let _ = deflate_decompress(&garbage, &mut empty_out);
            let _ = zlib_decompress(&garbage, &mut empty_out);
        }
    }

    #[test]
    fn test_zlib_decompress_vec() {
        // Compress with flate2, decompress with our vec API
        use std::io::Write;
        let data = b"Hello, this is a test of the auto-sizing zlib decompressor!".repeat(100);
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&data).unwrap();
        let compressed = encoder.finish().unwrap();

        let result = zlib_decompress_vec(&compressed).unwrap();
        assert_eq!(result, data);
    }

    #[test]
    fn test_deflate_decompress_vec() {
        use std::io::Write;
        let data = b"Auto-sizing deflate decompression test data!".repeat(200);
        let mut encoder =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&data).unwrap();
        let compressed = encoder.finish().unwrap();

        let result = deflate_decompress_vec(&compressed).unwrap();
        assert_eq!(result, data);
    }

    #[test]
    fn test_zlib_decompress_vec_with_hint_correct() {
        use std::io::Write;
        let data = b"Hint test with correct size".repeat(50);
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&data).unwrap();
        let compressed = encoder.finish().unwrap();

        // Correct hint - should decompress in one shot
        let result = zlib_decompress_vec_with_hint(&compressed, data.len()).unwrap();
        assert_eq!(result, data);
    }

    #[test]
    fn test_zlib_decompress_vec_with_hint_too_small() {
        use std::io::Write;
        let data = b"Hint test with undersized hint".repeat(100);
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&data).unwrap();
        let compressed = encoder.finish().unwrap();

        // Hint too small - should retry and succeed
        let result = zlib_decompress_vec_with_hint(&compressed, 10).unwrap();
        assert_eq!(result, data);
    }

    #[test]
    fn test_zlib_decompress_vec_various_sizes() {
        use rand::{Rng, SeedableRng};
        let mut rng = rand::rngs::StdRng::seed_from_u64(99);
        for size in [0, 1, 10, 100, 1000, 10000, 100000] {
            let data: Vec<u8> = (0..size).map(|_| rng.gen()).collect();
            let mut c = libdeflater::Compressor::new(libdeflater::CompressionLvl::new(6).unwrap());
            let max_sz = c.zlib_compress_bound(data.len());
            let mut compressed = vec![0u8; max_sz];
            let clen = c.zlib_compress(&data, &mut compressed).unwrap();
            compressed.truncate(clen);

            let result = zlib_decompress_vec(&compressed).unwrap();
            assert_eq!(result, data, "mismatch at size {}", size);
        }
    }
}
