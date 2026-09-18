//! LZX decompression for Microsoft Cabinet folders (`typeCompress` 3).
//!
//! Clean-room implementation, written only from Microsoft's published Open
//! Specifications and general canonical-Huffman knowledge:
//!
//! - [MS-PATCH] "LZX DELTA Compression and Decompression" (v20160613):
//!   the bitstream of 16-bit little-endian words, the E8 call translation
//!   header and postprocessing, block types (verbatim, aligned offset,
//!   uncompressed), the pretree and delta-coded path lengths, the main,
//!   length and aligned offset trees, repeated offsets R0-R2, position slots
//!   and footer bits, and the 32 KB chunks the bitstream realigns after.
//! - [MS-CAB] "Cabinet File Format": a folder's CFDATA blocks carry one chunk
//!   each (`cbUncomp` bytes of output, 32 KB except the last) and the decoder
//!   state runs on across the blocks of a folder.
//!
//! No other LZX decoder's source was consulted. Licensed like the rest of
//! this crate: MIT OR Apache-2.0.
//!
//! CAB LZX is LZXD without its delta parts: no chunk-size prefix (CFDATA has
//! the size), no reference data, and matches of at most 257 bytes (no Extra
//! Length field). Cabinets use windows of 2^15 to 2^21 bytes.

const MIN_WINDOW_BITS: u32 = 15;
const MAX_WINDOW_BITS: u32 = 21;
/// Uncompressed bytes per chunk; the bitstream realigns after each.
const CHUNK: usize = 32768;
const LITERALS: usize = 256;
const LENGTH_SYMBOLS: usize = 249;
const ALIGNED_SYMBOLS: usize = 8;
const PRETREE_SYMBOLS: usize = 20;
const MAX_CODE_LEN: u32 = 16;
/// E8 translation stops once this many bytes precede a chunk (32768 chunks).
const E8_LIMIT: u64 = 0x4000_0000;
/// Position slots of the largest window (2^21).
const MAX_SLOTS: usize = 50;

/// Footer bits and base formatted offset of every position slot
/// ([MS-PATCH] 2.6.2): slots 0-3 have no footer, then two slots per footer
/// width up to 16 bits, then 17 bits for every further slot.
const fn slot_tables() -> ([u8; MAX_SLOTS], [u32; MAX_SLOTS]) {
    let mut bits = [0u8; MAX_SLOTS];
    let mut base = [0u32; MAX_SLOTS];
    let mut slot = 0;
    while slot < MAX_SLOTS {
        bits[slot] = if slot < 4 {
            0
        } else if slot < 36 {
            (slot / 2 - 1) as u8
        } else {
            17
        };
        if slot > 0 {
            base[slot] = if slot < 4 { slot as u32 } else { base[slot - 1] + (1 << bits[slot - 1]) };
        }
        slot += 1;
    }
    (bits, base)
}
const SLOTS: ([u8; MAX_SLOTS], [u32; MAX_SLOTS]) = slot_tables();
const FOOTER_BITS: [u8; MAX_SLOTS] = SLOTS.0;
const SLOT_BASE: [u32; MAX_SLOTS] = SLOTS.1;

/// Position slots a window needs: the slots whose base lies inside it.
fn position_slots(window_size: usize) -> usize {
    SLOT_BASE.iter().take_while(|&&base| (base as usize) < window_size).count()
}

/// Reads the LZX bitstream: 16-bit little-endian words, each consumed from
/// its most significant bit. Past the end of the input it feeds zero words
/// and counts them, so running out is detected after the fact.
struct Bits<'a> {
    data: &'a [u8],
    /// Next input byte to load.
    pos: usize,
    /// Unconsumed bits, left-aligned.
    buf: u64,
    count: u32,
    /// Zero words fed after the input ended.
    past_end: u32,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Bits { data, pos: 0, buf: 0, count: 0, past_end: 0 }
    }

    #[inline]
    fn refill(&mut self) {
        if let Some(bytes) = self.data.get(self.pos..self.pos + 8) {
            // Four words at once, reordered so the first is most significant.
            let x = u64::from_le_bytes(bytes.try_into().unwrap()).rotate_left(32);
            let x = (x & 0xffff_0000_ffff_0000) >> 16 | (x & 0x0000_ffff_0000_ffff) << 16;
            let words = (64 - self.count) / 16;
            if words > 0 {
                let take = words * 16;
                self.buf |= (x >> (64 - take)) << (64 - self.count - take);
                self.count += take;
                self.pos += 2 * words as usize;
            }
            return;
        }
        while self.count <= 48 {
            let word = match self.data.get(self.pos..self.pos + 2) {
                Some(w) => u16::from_le_bytes([w[0], w[1]]),
                None => match self.data.get(self.pos) {
                    Some(&low) => low as u16,
                    None => {
                        self.past_end += 1;
                        0
                    }
                },
            };
            self.pos += 2;
            self.buf |= (word as u64) << (48 - self.count);
            self.count += 16;
        }
    }

    /// The next 16 bits without consuming them (`count` must be >= 16).
    #[inline]
    fn peek16(&self) -> u32 {
        (self.buf >> 48) as u32
    }

    #[inline]
    fn skip(&mut self, n: u32) {
        self.buf <<= n;
        self.count -= n;
    }

    /// Reads `n` (0..=32) bits, most significant first.
    #[inline]
    fn read(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        if self.count < n {
            self.refill();
        }
        let value = (self.buf >> (64 - n)) as u32;
        self.skip(n);
        value
    }

    /// Drops the 1..=16 padding bits before an uncompressed block's bytes
    /// ([MS-PATCH] 2.3.2.1) and returns the byte offset they end at. The
    /// buffer is emptied: bytes are read directly until it refills.
    fn align_to_bytes(&mut self) -> usize {
        if self.count < 16 {
            self.refill();
        }
        let partial = self.count % 16;
        self.skip(if partial == 0 { 16 } else { partial });
        let at = self.pos - (self.count / 8) as usize;
        self.seek(at);
        at
    }

    fn seek(&mut self, pos: usize) {
        self.pos = pos;
        self.buf = 0;
        self.count = 0;
        self.past_end = 0;
    }

    /// Whether more bits were consumed than the input holds.
    fn overran(&self) -> bool {
        self.past_end * 16 > self.count
    }
}

/// A canonical Huffman decoding table: a direct table indexed by the next
/// `TABLE_BITS` bits, and second-level tables for longer codes indexed by
/// the bits after those, up to `MAX_CODE_LEN`.
struct Huffman {
    table_bits: u32,
    /// `symbol << 8 | length`, or `subtable_offset << 8 | SUBTABLE`;
    /// 0 marks a code no symbol has.
    table: Vec<u32>,
    long: Vec<u32>,
}

const SUBTABLE: u32 = 0x80;

impl Huffman {
    fn new(table_bits: u32) -> Self {
        Huffman { table_bits, table: vec![0; 1 << table_bits], long: Vec::new() }
    }

    /// Rebuilds the table from path lengths ([MS-PATCH] 2.4): codes are
    /// assigned shortest first, and in symbol order within a length. A set
    /// of all-zero lengths makes an empty tree, which fails only when used.
    fn build(&mut self, lens: &[u8]) -> Result<(), String> {
        let mut per_len = [0u32; MAX_CODE_LEN as usize + 1];
        for &len in lens {
            per_len[len as usize] += 1;
        }
        per_len[0] = 0;
        let mut left: i64 = 1;
        for &n in &per_len[1..] {
            left = (left << 1) - n as i64;
            if left < 0 {
                return Err("lzx huffman tree over-subscribed".into());
            }
        }
        let mut next = [0u32; MAX_CODE_LEN as usize + 2];
        for len in 1..=MAX_CODE_LEN as usize {
            next[len + 1] = (next[len] + per_len[len]) << 1;
        }
        let tb = self.table_bits;
        self.table.fill(0);
        self.long.clear();
        let sub_bits = MAX_CODE_LEN - tb;
        for (symbol, &len) in lens.iter().enumerate() {
            let len = len as u32;
            if len == 0 {
                continue;
            }
            let code = next[len as usize];
            next[len as usize] += 1;
            let entry = (symbol as u32) << 8 | len;
            if len <= tb {
                let first = (code << (tb - len)) as usize;
                self.table[first..first + (1 << (tb - len))].fill(entry);
            } else {
                let prefix = (code >> (len - tb)) as usize;
                if self.table[prefix] & SUBTABLE == 0 {
                    self.table[prefix] = (self.long.len() as u32) << 8 | SUBTABLE;
                    self.long.resize(self.long.len() + (1 << sub_bits), 0);
                }
                let base = (self.table[prefix] >> 8) as usize;
                let rest = code & ((1 << (len - tb)) - 1);
                let first = base + (rest << (MAX_CODE_LEN - len)) as usize;
                self.long[first..first + (1 << (MAX_CODE_LEN - len))].fill(entry);
            }
        }
        Ok(())
    }

    #[inline]
    fn decode(&self, bits: &mut Bits) -> Result<u32, String> {
        if bits.count < MAX_CODE_LEN {
            bits.refill();
        }
        let next = bits.peek16();
        let mut entry = self.table[(next >> (MAX_CODE_LEN - self.table_bits)) as usize];
        if entry & SUBTABLE != 0 {
            let rest = next & ((1 << (MAX_CODE_LEN - self.table_bits)) - 1);
            entry = self.long[(entry >> 8) as usize + rest as usize];
        }
        let len = entry & 0x1f;
        if len == 0 {
            return Err("lzx huffman code not in tree".into());
        }
        bits.skip(len);
        Ok(entry >> 8)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Verbatim,
    Aligned,
    Uncompressed,
}

/// Decodes one cabinet folder's LZX stream, one CFDATA block at a time.
pub struct Decoder {
    /// The sliding window, a ring of `2^window_bits` bytes.
    window: Vec<u8>,
    mask: usize,
    /// Bytes decoded into the window so far (a match may run a few bytes
    /// past the chunk being returned).
    decoded: u64,
    /// Bytes returned so far: the start of the next chunk.
    returned: u64,
    /// Path lengths of the previous trees, the base of the next deltas.
    main_lens: Vec<u8>,
    length_lens: [u8; LENGTH_SYMBOLS],
    main: Huffman,
    length: Huffman,
    aligned: Huffman,
    pretree: Huffman,
    /// Repeated offsets R0, R1, R2.
    recent: [u32; 3],
    kind: BlockKind,
    /// Bytes the current block has yet to produce (0: read a header next).
    block_left: usize,
    /// The current uncompressed block has odd size, so a pad byte ends it.
    block_odd: bool,
    header_read: bool,
    /// E8_file_size when E8 translation is on.
    e8_size: Option<u32>,
}

impl Decoder {
    pub fn new(window_bits: u32) -> Result<Self, String> {
        if !(MIN_WINDOW_BITS..=MAX_WINDOW_BITS).contains(&window_bits) {
            return Err(format!("lzx window 2^{window_bits} unsupported"));
        }
        let size = 1usize << window_bits;
        let main_symbols = LITERALS + 8 * position_slots(size);
        Ok(Decoder {
            window: vec![0; size],
            mask: size - 1,
            decoded: 0,
            returned: 0,
            main_lens: vec![0; main_symbols],
            length_lens: [0; LENGTH_SYMBOLS],
            main: Huffman::new(10),
            length: Huffman::new(8),
            aligned: Huffman::new(7),
            pretree: Huffman::new(6),
            recent: [1, 1, 1],
            kind: BlockKind::Verbatim,
            block_left: 0,
            block_odd: false,
            header_read: false,
            e8_size: None,
        })
    }

    /// Decodes one CFDATA block: `input` is its compressed bytes and
    /// `out_len` its `cbUncomp` (at most 32 KB).
    pub fn decompress(&mut self, input: &[u8], out_len: usize) -> Result<Vec<u8>, String> {
        if out_len > CHUNK {
            return Err(format!("lzx chunk of {out_len} bytes exceeds 32 KB"));
        }
        let start = self.returned;
        let end = start + out_len as u64;
        let mut bits = Bits::new(input);
        if !self.header_read && out_len > 0 {
            self.header_read = true;
            if bits.read(1) == 1 {
                let high = bits.read(16);
                let low = bits.read(16);
                self.e8_size = Some(high << 16 | low);
            }
        }
        while self.decoded < end {
            if self.block_left == 0 {
                self.read_block_header(&mut bits)?;
            }
            let want = self.block_left.min((end - self.decoded) as usize);
            let produced = match self.kind {
                BlockKind::Uncompressed => self.copy_stored(&mut bits, want)?,
                BlockKind::Verbatim => self.decode_tokens::<false>(&mut bits, want)?,
                BlockKind::Aligned => self.decode_tokens::<true>(&mut bits, want)?,
            };
            self.block_left -= produced;
            if self.block_left == 0 && self.kind == BlockKind::Uncompressed && self.block_odd {
                // The pad byte after an odd-sized uncompressed block.
                if bits.pos < input.len() {
                    bits.seek(bits.pos + 1);
                }
            }
        }
        if bits.overran() {
            return Err("lzx input ended early".into());
        }
        let from = start as usize & self.mask;
        let first = out_len.min(self.window.len() - from);
        let mut out = Vec::with_capacity(out_len);
        out.extend_from_slice(&self.window[from..from + first]);
        out.extend_from_slice(&self.window[..out_len - first]);
        self.returned = end;
        if let Some(size) = self.e8_size {
            if start < E8_LIMIT && out_len > 10 {
                undo_e8(&mut out, start as i64, size as i64);
            }
        }
        Ok(out)
    }

    /// Block type, size, and the type's own header ([MS-PATCH] 2.3).
    fn read_block_header(&mut self, bits: &mut Bits) -> Result<(), String> {
        let kind = bits.read(3);
        let size = bits.read(24) as usize;
        if size == 0 {
            return Err("lzx empty block".into());
        }
        self.block_left = size;
        match kind {
            1 | 2 => {
                if kind == 2 {
                    let mut lens = [0u8; ALIGNED_SYMBOLS];
                    for len in &mut lens {
                        *len = bits.read(3) as u8;
                    }
                    self.aligned.build(&lens)?;
                    self.kind = BlockKind::Aligned;
                } else {
                    self.kind = BlockKind::Verbatim;
                }
                let mut main_lens = std::mem::take(&mut self.main_lens);
                let result = self
                    .read_lengths(bits, &mut main_lens[..LITERALS])
                    .and_then(|_| self.read_lengths(bits, &mut main_lens[LITERALS..]))
                    .and_then(|_| self.main.build(&main_lens));
                self.main_lens = main_lens;
                result?;
                let mut length_lens = self.length_lens;
                self.read_lengths(bits, &mut length_lens)?;
                self.length_lens = length_lens;
                self.length.build(&self.length_lens)?;
            }
            3 => {
                self.kind = BlockKind::Uncompressed;
                self.block_odd = size % 2 == 1;
                let at = bits.align_to_bytes();
                let header = bits.data.get(at..at + 12).ok_or("lzx input ended early")?;
                let (words, _) = header.as_chunks::<4>();
                for (slot, bytes) in self.recent.iter_mut().zip(words) {
                    *slot = u32::from_le_bytes(*bytes);
                }
                bits.seek(at + 12);
            }
            other => return Err(format!("lzx invalid block type {other}")),
        }
        Ok(())
    }

    /// Reads a pretree, then the path lengths it codes as deltas against
    /// the previous tree's lengths ([MS-PATCH] 2.5).
    fn read_lengths(&mut self, bits: &mut Bits, lens: &mut [u8]) -> Result<(), String> {
        let mut pre = [0u8; PRETREE_SYMBOLS];
        for len in &mut pre {
            *len = bits.read(4) as u8;
        }
        self.pretree.build(&pre)?;
        let delta = |prev: u8, code: u32| ((prev as u32 + 17 - code) % 17) as u8;
        let mut i = 0;
        while i < lens.len() {
            let code = self.pretree.decode(bits)?;
            let (run, value) = match code {
                0..=16 => (1, delta(lens[i], code)),
                17 => (4 + bits.read(4) as usize, 0),
                18 => (20 + bits.read(5) as usize, 0),
                19 => {
                    let run = 4 + bits.read(1) as usize;
                    let code = self.pretree.decode(bits)?;
                    if code > 16 {
                        return Err("lzx bad pretree run".into());
                    }
                    (run, delta(lens[i], code))
                }
                _ => unreachable!("pretree has 20 symbols"),
            };
            let run_end = i + run;
            if run_end > lens.len() {
                return Err("lzx path lengths run past the tree".into());
            }
            lens[i..run_end].fill(value);
            i = run_end;
        }
        Ok(())
    }

    /// Copies up to `want` bytes of an uncompressed block into the window.
    fn copy_stored(&mut self, bits: &mut Bits, want: usize) -> Result<usize, String> {
        let at = bits.pos;
        let src = bits.data.get(at..at + want).ok_or("lzx input ended early")?;
        let mut pos = self.decoded as usize & self.mask;
        let mut src = src;
        while !src.is_empty() {
            let n = src.len().min(self.window.len() - pos);
            self.window[pos..pos + n].copy_from_slice(&src[..n]);
            src = &src[n..];
            pos = (pos + n) & self.mask;
        }
        bits.seek(at + want);
        self.decoded += want as u64;
        Ok(want)
    }

    /// Decodes literals and matches until at least `want` bytes are out
    /// ([MS-PATCH] 2.7). A match may run past `want`; the bytes it writes
    /// beyond it start the next chunk. Returns the bytes produced.
    fn decode_tokens<const ALIGNED: bool>(&mut self, bits: &mut Bits, want: usize) -> Result<usize, String> {
        let window_len = self.window.len();
        let mask = self.mask;
        let mut pos = self.decoded as usize & mask;
        let mut produced = 0usize;
        // Bytes before this call: a match may reach back no further.
        let history = self.decoded.min(window_len as u64) as usize;
        let block_left = self.block_left;
        while produced < want {
            let symbol = self.main.decode(bits)? as usize;
            if symbol < LITERALS {
                self.window[pos] = symbol as u8;
                pos = (pos + 1) & mask;
                produced += 1;
                continue;
            }
            let header = symbol - LITERALS;
            let mut len = header & 7;
            if len == 7 {
                len += self.length.decode(bits)? as usize;
            }
            len += 2;
            let slot = header >> 3;
            let offset = match slot {
                0 => self.recent[0],
                1 => {
                    self.recent.swap(0, 1);
                    self.recent[0]
                }
                2 => {
                    self.recent.swap(0, 2);
                    self.recent[0]
                }
                _ => {
                    let footer = FOOTER_BITS[slot] as u32;
                    let extra = if ALIGNED && footer >= 3 {
                        (bits.read(footer - 3) << 3) + self.aligned.decode(bits)?
                    } else {
                        bits.read(footer)
                    };
                    let offset = SLOT_BASE[slot] + extra - 2;
                    self.recent = [offset, self.recent[0], self.recent[1]];
                    offset
                }
            } as usize;
            if offset == 0 || offset > history + produced || offset >= window_len {
                return Err(format!("lzx match offset {offset} outside the window"));
            }
            if produced + len > block_left {
                return Err("lzx match runs past its block".into());
            }
            let src = pos.wrapping_sub(offset) & mask;
            if src + len <= window_len && pos + len <= window_len {
                if offset >= len {
                    self.window.copy_within(src..src + len, pos);
                } else if offset == 1 {
                    let byte = self.window[src];
                    self.window[pos..pos + len].fill(byte);
                } else {
                    for i in 0..len {
                        self.window[pos + i] = self.window[src + i];
                    }
                }
            } else {
                for i in 0..len {
                    self.window[(pos + i) & mask] = self.window[(src + i) & mask];
                }
            }
            pos = (pos + len) & mask;
            produced += len;
        }
        self.decoded += produced as u64;
        Ok(produced)
    }
}

/// Reverses E8 call translation on one chunk ([MS-PATCH] 2.2.2): each 0xE8
/// byte before the chunk's last 10 is followed by a 32-bit value that is
/// turned back into the call's relative displacement.
fn undo_e8(chunk: &mut [u8], chunk_offset: i64, file_size: i64) {
    let scan_end = chunk.len() - 10;
    let mut i = 0;
    while let Some(found) = chunk[i..scan_end].iter().position(|&b| b == 0xE8) {
        i += found;
        let current = chunk_offset + i as i64;
        let field = &mut chunk[i + 1..i + 5];
        let value = i32::from_le_bytes([field[0], field[1], field[2], field[3]]) as i64;
        if value >= -current && value < file_size {
            let displacement = if value >= 0 { value - current } else { value + file_size };
            field.copy_from_slice(&(displacement as i32).to_le_bytes());
        }
        i += 5;
        if i >= scan_end {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [MS-PATCH] section 3: "abc" in one uncompressed block (without the
    /// LZXD chunk-size prefix, which CAB replaces with CFDATA.cbData).
    #[test]
    fn spec_example_uncompressed_block() {
        let input = [
            0x00, 0x30, 0x30, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x61, 0x62,
            0x63, 0x00,
        ];
        let mut dec = Decoder::new(17).unwrap();
        assert_eq!(dec.decompress(&input, 3).unwrap(), b"abc");
    }

    #[test]
    fn position_slot_counts_match_the_spec() {
        // [MS-PATCH] 2.1.6 for 2^17..2^21; 2^15 and 2^16 by the same rule.
        let expected = [(15, 30), (16, 32), (17, 34), (18, 36), (19, 38), (20, 42), (21, 50)];
        for (bits, slots) in expected {
            assert_eq!(position_slots(1 << bits), slots, "window 2^{bits}");
        }
        // Spot checks against the table in 2.6.2.
        assert_eq!((SLOT_BASE[4], FOOTER_BITS[4]), (4, 1));
        assert_eq!((SLOT_BASE[24], FOOTER_BITS[24]), (4096, 11));
        assert_eq!((SLOT_BASE[35], FOOTER_BITS[35]), (196608, 16));
        assert_eq!((SLOT_BASE[42], FOOTER_BITS[42]), (1048576, 17));
    }

    #[test]
    fn window_range() {
        assert!(Decoder::new(14).is_err());
        assert!(Decoder::new(22).is_err());
        for bits in 15..=21 {
            assert!(Decoder::new(bits).is_ok());
        }
    }

    // ---- A small spec-following encoder, to round-trip the decoder ----

    struct BitWriter {
        out: Vec<u8>,
        acc: u32,
        n: u32,
    }

    impl BitWriter {
        fn new() -> Self {
            BitWriter { out: Vec::new(), acc: 0, n: 0 }
        }
        fn put(&mut self, value: u32, bits: u32) {
            for i in (0..bits).rev() {
                self.acc = self.acc << 1 | (value >> i) & 1;
                self.n += 1;
                if self.n == 16 {
                    self.out.extend_from_slice(&(self.acc as u16).to_le_bytes());
                    self.acc = 0;
                    self.n = 0;
                }
            }
        }
        /// Pads to a 16-bit boundary (the end of a chunk).
        fn flush(&mut self) {
            if self.n > 0 {
                self.put(0, 16 - self.n);
            }
        }
    }

    /// Canonical codes for path lengths: shortest first, symbol order.
    fn codes(lens: &[u8]) -> Vec<u32> {
        let mut out = vec![0; lens.len()];
        let mut code = 0u32;
        for len in 1..=16u8 {
            for (s, &l) in lens.iter().enumerate() {
                if l == len {
                    out[s] = code;
                    code += 1;
                }
            }
            code <<= 1;
        }
        out
    }

    #[derive(Clone, Copy)]
    enum Kind {
        Verbatim,
        Aligned,
        Stored,
    }

    enum Token {
        Literal(u8),
        Match { len: usize, offset: usize },
    }

    /// E8 preprocessing exactly as [MS-PATCH] 2.2.2 gives it.
    fn apply_e8(data: &mut [u8], file_size: i64) {
        for (index, chunk) in data.chunks_mut(CHUNK).enumerate() {
            let chunk_offset = (index * CHUNK) as i64;
            if chunk_offset >= E8_LIMIT as i64 || chunk.len() <= 10 {
                continue;
            }
            let mut i = 0;
            while i < chunk.len() - 10 {
                if chunk[i] == 0xE8 {
                    let current = chunk_offset + i as i64;
                    let displacement = i32::from_le_bytes(chunk[i + 1..i + 5].try_into().unwrap()) as i64;
                    let mut target = current + displacement;
                    if target >= 0 && target < file_size + current {
                        if target >= file_size {
                            target = displacement - file_size;
                        }
                        chunk[i + 1..i + 5].copy_from_slice(&(target as i32).to_le_bytes());
                    }
                    i += 4;
                }
                i += 1;
            }
        }
    }

    struct Encoder {
        main_lens: Vec<u8>,
        length_lens: Vec<u8>,
        recent: [usize; 3],
        w: BitWriter,
        chunks: Vec<(Vec<u8>, usize)>,
        /// Bytes encoded so far and the start of the open chunk.
        done: usize,
        chunk_start: usize,
        generation: u8,
    }

    impl Encoder {
        fn new(window_bits: u32, e8: Option<u32>) -> Self {
            let slots = position_slots(1 << window_bits);
            let mut w = BitWriter::new();
            match e8 {
                Some(size) => {
                    w.put(1, 1);
                    w.put(size >> 16, 16);
                    w.put(size & 0xffff, 16);
                }
                None => w.put(0, 1),
            }
            Encoder {
                main_lens: vec![0; LITERALS + 8 * slots],
                length_lens: vec![0; LENGTH_SYMBOLS],
                recent: [1, 1, 1],
                w,
                chunks: Vec::new(),
                done: 0,
                chunk_start: 0,
                generation: 0,
            }
        }

        /// Closes the chunk when a 32 KB boundary is reached.
        fn advance(&mut self, n: usize) {
            self.done += n;
            assert!(self.done - self.chunk_start <= CHUNK, "token crossed a chunk");
            if self.done - self.chunk_start == CHUNK {
                self.close_chunk();
            }
        }

        fn close_chunk(&mut self) {
            self.w.flush();
            let bytes = std::mem::take(&mut self.w.out);
            self.chunks.push((bytes, self.done - self.chunk_start));
            self.chunk_start = self.done;
        }

        /// New path lengths coded against the previous ones, using every
        /// pretree code: plain deltas, runs of zeros (17, 18) and runs of a
        /// value (19).
        fn write_lengths(&mut self, prev: &[u8], new: &[u8]) {
            let pre = [5u8; PRETREE_SYMBOLS];
            for &l in &pre {
                self.w.put(l as u32, 4);
            }
            let pcodes = codes(&pre);
            let delta = |p: u8, n: u8| ((p as u32 + 17 - n as u32) % 17) as usize;
            let mut i = 0;
            while i < new.len() {
                let mut run = 1;
                while i + run < new.len() && new[i + run] == new[i] {
                    run += 1;
                }
                if new[i] == 0 && run >= 20 {
                    let n = run.min(51);
                    self.w.put(pcodes[18], 5);
                    self.w.put((n - 20) as u32, 5);
                    i += n;
                } else if new[i] == 0 && run >= 4 {
                    let n = run.min(19);
                    self.w.put(pcodes[17], 5);
                    self.w.put((n - 4) as u32, 4);
                    i += n;
                } else if run >= 4 && prev[i..i + run.min(5)].iter().all(|&p| p == prev[i]) {
                    // 19 codes one delta against prev[i] for the whole run.
                    let n = run.min(5);
                    self.w.put(pcodes[19], 5);
                    self.w.put((n - 4) as u32, 1);
                    self.w.put(pcodes[delta(prev[i], new[i])], 5);
                    i += n;
                } else {
                    self.w.put(pcodes[delta(prev[i], new[i])], 5);
                    i += 1;
                }
            }
        }

        fn block(&mut self, kind: Kind, data: &[u8], tokens: &[Token]) {
            let size: usize = tokens
                .iter()
                .map(|t| match t {
                    Token::Literal(_) => 1,
                    Token::Match { len, .. } => *len,
                })
                .sum();
            let size = if matches!(kind, Kind::Stored) { data.len() } else { size };
            let type_code = match kind {
                Kind::Verbatim => 1,
                Kind::Aligned => 2,
                Kind::Stored => 3,
            };
            self.w.put(type_code, 3);
            self.w.put(size as u32, 24);
            if let Kind::Stored = kind {
                // 1..=16 zero bits to the next 16-bit boundary.
                self.w.put(0, 16 - self.w.n);
                for r in self.recent {
                    self.w.out.extend_from_slice(&(r as u32).to_le_bytes());
                }
                for (i, &byte) in data.iter().enumerate() {
                    self.w.out.push(byte);
                    if i + 1 == data.len() && data.len() % 2 == 1 {
                        // The pad byte stays in the chunk the block ends in.
                        self.w.out.push(0);
                    }
                    self.advance(1);
                }
                return;
            }
            if let Kind::Aligned = kind {
                for _ in 0..ALIGNED_SYMBOLS {
                    self.w.put(3, 3);
                }
            }
            // Fresh lengths each block (alternating so deltas vary): every
            // main symbol 10 bits, but a zero run in the middle of the
            // literals (unused by the tokens), every length symbol 8 bits.
            self.generation += 1;
            let main_len = if self.generation.is_multiple_of(2) { 10 } else { 11 };
            let mut main_lens = vec![main_len; self.main_lens.len()];
            for (s, l) in main_lens.iter_mut().enumerate().take(0x7f).skip(0x60) {
                let used = tokens.iter().any(|t| matches!(t, Token::Literal(b) if *b as usize == s));
                if !used {
                    *l = 0;
                }
            }
            // Match symbols of slots beyond the farthest offset: unused.
            let farthest = tokens
                .iter()
                .map(|t| match t {
                    Token::Match { offset, .. } => offset + 2,
                    Token::Literal(_) => 0,
                })
                .max()
                .unwrap_or(0);
            let last_slot = SLOT_BASE.iter().rposition(|&b| b as usize <= farthest).unwrap();
            for l in &mut main_lens[LITERALS + 8 * (last_slot + 1)..] {
                *l = 0;
            }
            let length_lens = vec![8u8; LENGTH_SYMBOLS];
            let prev_main = self.main_lens.clone();
            self.write_lengths(&prev_main[..LITERALS], &main_lens[..LITERALS]);
            self.write_lengths(&prev_main[LITERALS..], &main_lens[LITERALS..]);
            let prev_len = self.length_lens.clone();
            self.write_lengths(&prev_len, &length_lens);
            self.main_lens = main_lens;
            self.length_lens = length_lens;
            let main_codes = codes(&self.main_lens);
            let length_codes = codes(&self.length_lens);
            for token in tokens {
                match *token {
                    Token::Literal(b) => {
                        self.w.put(main_codes[b as usize], self.main_lens[b as usize] as u32);
                        self.advance(1);
                    }
                    Token::Match { len, offset } => {
                        let formatted = if offset == self.recent[0] {
                            0
                        } else if offset == self.recent[1] {
                            self.recent.swap(0, 1);
                            1
                        } else if offset == self.recent[2] {
                            self.recent.swap(0, 2);
                            2
                        } else {
                            self.recent = [offset, self.recent[0], self.recent[1]];
                            offset + 2
                        };
                        let slot = SLOT_BASE.iter().rposition(|&b| b as usize <= formatted).unwrap();
                        let footer_bits = FOOTER_BITS[slot] as u32;
                        let footer = (formatted - SLOT_BASE[slot] as usize) as u32;
                        let length_header = (len - 2).min(7);
                        let symbol = LITERALS + slot * 8 + length_header;
                        self.w.put(main_codes[symbol], self.main_lens[symbol] as u32);
                        if length_header == 7 {
                            let l = len - 9;
                            self.w.put(length_codes[l], 8);
                        }
                        if matches!(kind, Kind::Aligned) && footer_bits >= 3 {
                            self.w.put(footer >> 3, footer_bits - 3);
                            self.w.put(footer & 7, 3);
                        } else {
                            self.w.put(footer, footer_bits);
                        }
                        self.advance(len);
                    }
                }
            }
        }

        fn finish(mut self) -> Vec<(Vec<u8>, usize)> {
            if self.done > self.chunk_start {
                self.close_chunk();
            }
            self.chunks
        }
    }

    /// Greedy tokens for `data[from..to]`: matches found with a hash of 3
    /// bytes, plus repeats of recent offsets; no token crosses a chunk.
    fn tokenize(data: &[u8], from: usize, to: usize, window: usize, recent: &mut Vec<usize>) -> Vec<Token> {
        let mut heads = std::collections::HashMap::new();
        let mut tokens = Vec::new();
        let start = from.saturating_sub(window - 3);
        for p in start..from {
            if p + 3 <= data.len() {
                heads.insert(&data[p..p + 3], p);
            }
        }
        let mut p = from;
        while p < to {
            let chunk_end = (p / CHUNK + 1) * CHUNK;
            let limit = (to.min(chunk_end) - p).min(257);
            let match_len = |offset: usize| {
                if offset == 0 || offset > p || offset > window - 3 {
                    return 0;
                }
                (0..limit).take_while(|&i| data[p + i] == data[p + i - offset]).count()
            };
            let mut best = (0, 0);
            for &offset in recent.iter() {
                let l = match_len(offset);
                if l > best.0 {
                    best = (l, offset);
                }
            }
            if p + 3 <= data.len() {
                if let Some(&q) = heads.get(&data[p..p + 3]) {
                    let l = match_len(p - q);
                    if l > best.0 + 1 {
                        best = (l, p - q);
                    }
                }
            }
            let step = if best.0 >= 2 {
                tokens.push(Token::Match { len: best.0, offset: best.1 });
                if !recent.contains(&best.1) {
                    recent.insert(0, best.1);
                    recent.truncate(3);
                }
                best.0
            } else {
                tokens.push(Token::Literal(data[p]));
                1
            };
            for q in p..p + step {
                if q + 3 <= data.len() {
                    heads.insert(&data[q..q + 3], q);
                }
            }
            p += step;
        }
        tokens
    }

    fn sample(len: usize, seed: u32) -> Vec<u8> {
        let words: [&[u8]; 8] = [b"lorem ", b"ipsum ", b"\xE8\x10\x20\x00\x00", b"dolor ", b"\x00\x00\x00\x00", b"sit ", b"amet\n", b"\xE8\xF0\xFF\xFF\xFF"];
        let mut state = seed;
        let mut out = Vec::with_capacity(len);
        while out.len() < len {
            state = state.wrapping_mul(1103515245).wrapping_add(12345);
            let pick = (state >> 16) as usize;
            if pick.is_multiple_of(7) {
                out.push((pick >> 3) as u8);
            } else {
                out.extend_from_slice(words[pick % 8]);
            }
        }
        out.truncate(len);
        out
    }

    /// Encodes `data` as the given blocks and decodes it chunk by chunk.
    fn round_trip(window_bits: u32, data: &[u8], blocks: &[(Kind, usize)], e8: Option<u32>) {
        let mut plain = data.to_vec();
        if let Some(size) = e8 {
            apply_e8(&mut plain, size as i64);
        }
        let mut enc = Encoder::new(window_bits, e8);
        let window = 1usize << window_bits;
        let mut at = 0;
        for &(kind, len) in blocks {
            let to = (at + len).min(plain.len());
            if matches!(kind, Kind::Stored) {
                enc.block(kind, &plain[at..to], &[]);
            } else {
                let mut r: Vec<usize> = enc.recent.to_vec();
                let tokens = tokenize(&plain, at, to, window, &mut r);
                enc.block(kind, &plain[at..to], &tokens);
            }
            at = to;
        }
        assert_eq!(at, plain.len(), "blocks cover the data");
        let chunks = enc.finish();
        let mut dec = Decoder::new(window_bits).unwrap();
        let mut out = Vec::new();
        for (bytes, len) in &chunks {
            out.extend(dec.decompress(bytes, *len).unwrap());
        }
        assert_eq!(out.len(), data.len());
        if let Some(i) = out.iter().zip(data).position(|(a, b)| a != b) {
            panic!("window 2^{window_bits}: first difference at byte {i}");
        }
    }

    #[test]
    fn round_trip_every_block_kind_and_window() {
        let data = sample(150_000, 7);
        let blocks = [
            (Kind::Verbatim, 40_001),
            (Kind::Aligned, 30_000),
            (Kind::Stored, 12_345),
            (Kind::Stored, 20_000),
            (Kind::Aligned, 1),
            (Kind::Verbatim, 50_000),
        ];
        for bits in 15..=21 {
            round_trip(bits, &data, &blocks, None);
        }
    }

    #[test]
    fn round_trip_e8_translation() {
        let data = sample(100_000, 11);
        let blocks = [(Kind::Verbatim, 70_000), (Kind::Aligned, 30_000)];
        round_trip(16, &data, &blocks, Some(12_000_000));
        round_trip(21, &data, &blocks, Some(0x7fff));
    }

    #[test]
    fn stored_block_after_aligned_header() {
        // A stored block whose header ends exactly on a word boundary takes
        // a whole 16-bit word of padding.
        let data = sample(40_000, 3);
        round_trip(17, &data, &[(Kind::Verbatim, 10), (Kind::Stored, 39_990)], None);
        round_trip(17, &data, &[(Kind::Stored, 5), (Kind::Stored, 7), (Kind::Stored, 39_988)], None);
    }

    #[test]
    fn errors_instead_of_panics() {
        let data = sample(70_000, 5);
        let mut enc = Encoder::new(16, None);
        let mut r = vec![1, 1, 1];
        let tokens = tokenize(&data, 0, data.len(), 1 << 16, &mut r);
        enc.block(Kind::Verbatim, &data, &tokens);
        let chunks = enc.finish();
        // Truncated chunks.
        for cut in [0, 1, 7, chunks[0].0.len() / 2] {
            let mut dec = Decoder::new(16).unwrap();
            assert!(dec.decompress(&chunks[0].0[..cut], chunks[0].1).is_err(), "cut {cut}");
        }
        // Invalid block types.
        for kind in [0u32, 4, 7] {
            let mut w = BitWriter::new();
            w.put(0, 1);
            w.put(kind, 3);
            w.put(100, 24);
            w.flush();
            w.put(0, 32);
            let mut dec = Decoder::new(16).unwrap();
            assert!(dec.decompress(&w.out, 100).is_err());
        }
        // Garbage never panics.
        let mut state = 1u32;
        for _ in 0..200 {
            let bytes: Vec<u8> = (0..600)
                .map(|_| {
                    state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                    (state >> 24) as u8
                })
                .collect();
            let mut dec = Decoder::new(15).unwrap();
            let _ = dec.decompress(&bytes, 32768);
        }
        assert!(Decoder::new(16).unwrap().decompress(&[], 32769).is_err());
    }
}
