//! A decode-only bzip2 reader for the CEF archive: [`MultiBzDecoder`] wraps
//! any `Read` and yields the decompressed bytes of one or more concatenated
//! bzip2 streams. Every block's CRC and every stream's combined CRC are
//! checked; anything malformed is an `InvalidData` error naming what was
//! wrong, never silently truncated output.
//!
//! The format, per stream: `BZh` + level `1`-`9` (block size in 100 kB),
//! then blocks, each a 48-bit magic, the CRC of its output, the BWT origin
//! pointer, the symbol map, Huffman tables + selectors, and the
//! Huffman/MTF/RUNA-RUNB coded BWT column; the stream ends with a 48-bit
//! magic and the combined CRC, padded to a byte.
//!
//! Speed: Huffman symbols decode through a 10-bit lookup table per group
//! (longer codes fall back to a per-length canonical search), bits come from
//! a 64-bit accumulator refilled eight bytes at a time, the inverse BWT uses
//! one linked `u32` table (byte in the low 8 bits, next index above), and
//! the final run-length stage writes straight into the caller's buffer. No
//! allocation happens per block or per read once the first stream started.
//!
//! Blocks with the "randomised" flag (written only by bzip2 0.9.0 and
//! older, decades ago) are refused with a clear error.

use std::io::{self, Read};

const BLOCK_MAGIC: u64 = 0x3141_5926_5359;
const END_MAGIC: u64 = 0x1772_4538_5090;
const MAX_GROUPS: usize = 6;
const MAX_ALPHA: usize = 258;
const MAX_CODE_LEN: usize = 20;
const GROUP_SIZE: u32 = 50;
const MAX_SELECTORS: usize = 2 + 900_000 / GROUP_SIZE as usize;
const LUT_BITS: u32 = 10;
const INPUT_BUFFER: usize = 1 << 16;
/// "No previous byte" for the run-length stage (bytes are 0..=255).
const NO_BYTE: u32 = 256;

fn corrupt(what: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("bz2: {what}"))
}

/// CRC-32 as bzip2 uses it: polynomial 0x04C11DB7, most significant bit
/// first, initial and final value all ones.
static CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = (i as u32) << 24;
        let mut k = 0;
        while k < 8 {
            c = if c & 0x8000_0000 != 0 { (c << 1) ^ 0x04C1_1DB7 } else { c << 1 };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

#[inline(always)]
fn crc_update(crc: u32, byte: u8) -> u32 {
    (crc << 8) ^ CRC_TABLE[((crc >> 24) as u8 ^ byte) as usize]
}

/// Most-significant-bit-first reader. `acc` holds `count` unread bits in its
/// low end. Past the end of the input, zero bytes are fed in and counted in
/// `pad`; consuming into them (`count < pad`) means the data was truncated.
struct Bits<R> {
    inner: R,
    buf: Box<[u8]>,
    pos: usize,
    len: usize,
    eof: bool,
    acc: u64,
    count: u32,
    pad: u32,
}

impl<R: Read> Bits<R> {
    fn new(inner: R) -> Self {
        Bits { inner, buf: vec![0; INPUT_BUFFER].into_boxed_slice(), pos: 0, len: 0, eof: false, acc: 0, count: 0, pad: 0 }
    }

    /// Called with the input buffer drained; leaves it non-empty or sets `eof`.
    fn fill_input(&mut self) -> io::Result<()> {
        while !self.eof {
            match self.inner.read(&mut self.buf) {
                Ok(0) => self.eof = true,
                Ok(n) => {
                    self.pos = 0;
                    self.len = n;
                    break;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Top the accumulator up to at least 56 bits. Only called with
    /// `count <= 32`, so the fast path always loads 4 to 7 bytes.
    #[inline(always)]
    fn refill(&mut self) -> io::Result<()> {
        if self.len - self.pos >= 8 {
            let n = ((64 - self.count) / 8).min(7);
            let word = u64::from_be_bytes(self.buf[self.pos..self.pos + 8].try_into().unwrap());
            self.acc = (self.acc << (n * 8)) | (word >> (64 - n * 8));
            self.pos += n as usize;
            self.count += n * 8;
            return Ok(());
        }
        self.refill_slow()
    }

    #[cold]
    fn refill_slow(&mut self) -> io::Result<()> {
        if self.count < self.pad {
            return Err(corrupt("unexpected end of data (truncated archive?)"));
        }
        while self.count <= 56 {
            if self.pos == self.len {
                self.fill_input()?;
            }
            let byte = if self.pos < self.len {
                self.pos += 1;
                self.buf[self.pos - 1]
            } else {
                self.pad += 8;
                0
            };
            self.acc = (self.acc << 8) | byte as u64;
            self.count += 8;
        }
        Ok(())
    }

    /// The next `n` bits (`n <= 32`) as a number, first bit most significant.
    #[inline(always)]
    fn bits(&mut self, n: u32) -> io::Result<u32> {
        if self.count < n {
            self.refill()?;
        }
        self.count -= n;
        Ok(((self.acc >> self.count) & ((1u64 << n) - 1)) as u32)
    }

    #[inline(always)]
    fn bit(&mut self) -> io::Result<bool> {
        Ok(self.bits(1)? != 0)
    }

    fn check_not_truncated(&self) -> io::Result<()> {
        if self.count < self.pad {
            Err(corrupt("unexpected end of data (truncated archive?)"))
        } else {
            Ok(())
        }
    }

    /// Skip to the next byte boundary (streams are byte aligned).
    fn align(&mut self) {
        self.count -= self.count % 8;
    }

    /// True when no real input is left (only valid at a byte boundary).
    fn at_end(&mut self) -> io::Result<bool> {
        if self.count > self.pad {
            return Ok(false);
        }
        if self.pos == self.len {
            self.fill_input()?;
        }
        Ok(self.pos == self.len)
    }
}

/// One group's canonical Huffman decoder.
struct Table {
    /// Indexed by the next `LUT_BITS` bits: `symbol << 5 | length`, or 0 when
    /// the code is longer than `LUT_BITS` (or invalid).
    lut: [u16; 1 << LUT_BITS],
    max_len: u32,
    /// Per code length: the first canonical code, how many codes, and where
    /// their symbols start in `perm`.
    first: [u32; MAX_CODE_LEN + 1],
    count: [u32; MAX_CODE_LEN + 1],
    offset: [u16; MAX_CODE_LEN + 1],
    perm: [u16; MAX_ALPHA],
}

impl Table {
    fn new() -> Self {
        Table {
            lut: [0; 1 << LUT_BITS],
            max_len: 0,
            first: [0; MAX_CODE_LEN + 1],
            count: [0; MAX_CODE_LEN + 1],
            offset: [0; MAX_CODE_LEN + 1],
            perm: [0; MAX_ALPHA],
        }
    }

    /// Canonical codes as bzip2 assigns them: by length, then by symbol.
    fn build(&mut self, lens: &[u8]) -> io::Result<()> {
        self.count = [0; MAX_CODE_LEN + 1];
        let mut max_len = 0;
        for &l in lens {
            self.count[l as usize] += 1;
            max_len = max_len.max(l as u32);
        }
        self.max_len = max_len;
        let mut code = 0u32;
        let mut index = 0u32;
        for l in 1..=MAX_CODE_LEN {
            self.first[l] = code;
            self.offset[l] = index as u16;
            index += self.count[l];
            code += self.count[l];
            if code > 1 << l {
                return Err(corrupt("Huffman code lengths are oversubscribed"));
            }
            code <<= 1;
        }
        let mut next = self.offset;
        for (symbol, &l) in lens.iter().enumerate() {
            self.perm[next[l as usize] as usize] = symbol as u16;
            next[l as usize] += 1;
        }
        self.lut = [0; 1 << LUT_BITS];
        for l in 1..=max_len.min(LUT_BITS) as usize {
            let fill = 1usize << (LUT_BITS as usize - l);
            for i in 0..self.count[l] {
                let symbol = self.perm[self.offset[l] as usize + i as usize];
                let start = ((self.first[l] + i) as usize) << (LUT_BITS as usize - l);
                self.lut[start..start + fill].fill(symbol << 5 | l as u16);
            }
        }
        Ok(())
    }

    #[inline(always)]
    fn decode<R: Read>(&self, bits: &mut Bits<R>) -> io::Result<usize> {
        if bits.count < MAX_CODE_LEN as u32 {
            bits.refill()?;
        }
        let entry = self.lut[((bits.acc >> (bits.count - LUT_BITS)) & ((1 << LUT_BITS) - 1)) as usize];
        if entry != 0 {
            bits.count -= (entry & 31) as u32;
            return Ok((entry >> 5) as usize);
        }
        self.decode_long(bits)
    }

    #[cold]
    fn decode_long<R: Read>(&self, bits: &mut Bits<R>) -> io::Result<usize> {
        for l in LUT_BITS + 1..=self.max_len {
            let code = ((bits.acc >> (bits.count - l)) & ((1u64 << l) - 1)) as u32;
            let d = code.wrapping_sub(self.first[l as usize]);
            if d < self.count[l as usize] {
                bits.count -= l;
                return Ok(self.perm[(self.offset[l as usize] as u32 + d) as usize] as usize);
            }
        }
        Err(corrupt("invalid Huffman code"))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    /// Expecting `BZh<level>`; `first` is the archive's first stream.
    StreamHeader { first: bool },
    /// Expecting a block magic or the end-of-stream magic.
    NextBlock,
    /// Emitting a decoded block.
    Block,
    Done,
    Failed,
}

/// `Read` over the decompressed contents of concatenated bzip2 streams.
pub struct MultiBzDecoder<R> {
    bits: Bits<R>,
    state: State,
    /// Largest block of the current stream (level * 100 000).
    block_limit: usize,
    /// Inverse-BWT links: byte in the low 8 bits, next position above.
    tt: Vec<u32>,
    tables: Box<[Table]>,
    selectors: Vec<u8>,
    // The block being emitted.
    nblock: usize,
    used: usize,
    tpos: u32,
    last: u32,
    run_count: u32,
    run_left: usize,
    run_byte: u8,
    crc: u32,
    stored_crc: u32,
    combined_crc: u32,
}

impl<R: Read> MultiBzDecoder<R> {
    pub fn new(inner: R) -> Self {
        MultiBzDecoder {
            bits: Bits::new(inner),
            state: State::StreamHeader { first: true },
            block_limit: 0,
            tt: Vec::new(),
            tables: (0..MAX_GROUPS).map(|_| Table::new()).collect(),
            selectors: Vec::with_capacity(MAX_SELECTORS),
            nblock: 0,
            used: 0,
            tpos: 0,
            last: NO_BYTE,
            run_count: 0,
            run_left: 0,
            run_byte: 0,
            crc: 0,
            stored_crc: 0,
            combined_crc: 0,
        }
    }

    fn stream_header(&mut self, first: bool) -> io::Result<bool> {
        if self.bits.at_end()? {
            if first {
                return Err(corrupt("empty input: no bzip2 stream"));
            }
            return Ok(false);
        }
        let b = self.bits.bits(8)?;
        let z = self.bits.bits(8)?;
        let h = self.bits.bits(8)?;
        let level = self.bits.bits(8)?;
        self.bits.check_not_truncated()?;
        if [b, z, h] != [b'B' as u32, b'Z' as u32, b'h' as u32] || !(b'1' as u32..=b'9' as u32).contains(&level) {
            return Err(corrupt(if first {
                "not a bzip2 stream (no BZh1-9 header)"
            } else {
                "data after the end of a bzip2 stream is not another bzip2 stream"
            }));
        }
        self.block_limit = (level - b'0' as u32) as usize * 100_000;
        if self.tt.len() < self.block_limit {
            self.tt.resize(self.block_limit, 0);
        }
        self.combined_crc = 0;
        Ok(true)
    }

    fn next_block(&mut self) -> io::Result<State> {
        let magic = (self.bits.bits(24)? as u64) << 24 | self.bits.bits(24)? as u64;
        self.bits.check_not_truncated()?;
        match magic {
            BLOCK_MAGIC => {
                self.read_block()?;
                Ok(State::Block)
            }
            END_MAGIC => {
                let stored = self.bits.bits(32)?;
                self.bits.check_not_truncated()?;
                if stored != self.combined_crc {
                    return Err(corrupt(format!(
                        "stream CRC mismatch (stored {stored:08x}, computed {:08x})",
                        self.combined_crc
                    )));
                }
                self.bits.align();
                Ok(State::StreamHeader { first: false })
            }
            _ => Err(corrupt(format!("bad block magic {magic:012x}"))),
        }
    }

    /// Huffman + MTF + RUNA/RUNB decode of one block into `tt`, then the
    /// inverse-BWT links; leaves the emit state at the block's start.
    fn read_block(&mut self) -> io::Result<()> {
        let bits = &mut self.bits;
        self.stored_crc = bits.bits(32)?;
        if bits.bit()? {
            return Err(corrupt("randomised blocks (bzip2 0.9.0 and older) are not supported"));
        }
        let orig_ptr = bits.bits(24)? as usize;

        let mut seq_to_unseq = [0u8; 256];
        let mut n_in_use = 0usize;
        let used16 = bits.bits(16)?;
        for i in 0..16 {
            if used16 & (0x8000 >> i) != 0 {
                let m = bits.bits(16)?;
                for j in 0..16 {
                    if m & (0x8000 >> j) != 0 {
                        seq_to_unseq[n_in_use] = (i * 16 + j) as u8;
                        n_in_use += 1;
                    }
                }
            }
        }
        if n_in_use == 0 {
            return Err(corrupt("block uses no byte values"));
        }
        let alpha = n_in_use + 2;

        let n_groups = bits.bits(3)? as usize;
        if !(2..=MAX_GROUPS).contains(&n_groups) {
            return Err(corrupt(format!("{n_groups} Huffman groups (2 to 6 allowed)")));
        }
        let n_selectors = bits.bits(15)? as usize;
        if n_selectors == 0 {
            return Err(corrupt("block has no selectors"));
        }
        let mut group_mtf = [0u8, 1, 2, 3, 4, 5];
        self.selectors.clear();
        for i in 0..n_selectors {
            let mut j = 0;
            while bits.bit()? {
                j += 1;
                if j >= n_groups {
                    return Err(corrupt("selector out of range"));
                }
            }
            // Like bzip2 1.0.8: selectors past the most a block can use are
            // read and dropped.
            if i < MAX_SELECTORS {
                let g = group_mtf[j];
                group_mtf.copy_within(0..j, 1);
                group_mtf[0] = g;
                self.selectors.push(g);
            }
        }

        let mut lens = [0u8; MAX_ALPHA];
        for table in self.tables[..n_groups].iter_mut() {
            let mut len = bits.bits(5)?;
            for l in lens[..alpha].iter_mut() {
                loop {
                    if !(1..=MAX_CODE_LEN as u32).contains(&len) {
                        return Err(corrupt("Huffman code length out of range"));
                    }
                    if !bits.bit()? {
                        break;
                    }
                    if bits.bit()? {
                        len -= 1;
                    } else {
                        len += 1;
                    }
                }
                *l = len as u8;
            }
            table.build(&lens[..alpha])?;
        }

        let limit = self.block_limit;
        let tt = &mut self.tt[..limit];
        let eob = n_in_use + 1;
        let mut mtf = [0u8; 256];
        for (i, m) in mtf.iter_mut().enumerate() {
            *m = i as u8;
        }
        let mut counts = [0u32; 256];
        let mut nblock = 0usize;
        let mut selector = 0usize;
        let mut group_left = 0u32;
        let mut table = &self.tables[0];
        let mut run = 0usize;
        let mut run_weight = 1usize;
        loop {
            if group_left == 0 {
                let Some(&g) = self.selectors.get(selector) else {
                    return Err(corrupt("block runs past its last selector"));
                };
                table = &self.tables[g as usize];
                selector += 1;
                group_left = GROUP_SIZE;
            }
            group_left -= 1;
            let symbol = table.decode(bits)?;
            if symbol <= 1 {
                // RUNA adds the weight, RUNB twice the weight: bijective base 2.
                if run_weight >= 2 * 1024 * 1024 {
                    return Err(corrupt("run length out of range"));
                }
                run += run_weight << symbol;
                run_weight <<= 1;
                continue;
            }
            if run > 0 {
                if run > limit - nblock {
                    return Err(corrupt("block longer than its stream's block size"));
                }
                let byte = seq_to_unseq[mtf[0] as usize];
                counts[byte as usize] += run as u32;
                tt[nblock..nblock + run].fill(byte as u32);
                nblock += run;
                run = 0;
                run_weight = 1;
            }
            if symbol == eob {
                break;
            }
            if nblock >= limit {
                return Err(corrupt("block longer than its stream's block size"));
            }
            let index = symbol - 1;
            let v = mtf[index];
            mtf.copy_within(0..index, 1);
            mtf[0] = v;
            let byte = seq_to_unseq[v as usize];
            counts[byte as usize] += 1;
            tt[nblock] = byte as u32;
            nblock += 1;
        }
        bits.check_not_truncated()?;
        if orig_ptr >= nblock {
            return Err(corrupt("BWT origin pointer outside the block"));
        }

        // Inverse BWT: link each position to the next one in output order.
        let mut cftab = [0u32; 256];
        let mut sum = 0u32;
        for (c, &n) in cftab.iter_mut().zip(counts.iter()) {
            *c = sum;
            sum += n;
        }
        for i in 0..nblock {
            let byte = (tt[i] & 0xff) as usize;
            let slot = cftab[byte] as usize;
            tt[slot] |= (i as u32) << 8;
            cftab[byte] += 1;
        }

        self.nblock = nblock;
        self.used = 0;
        self.tpos = tt[orig_ptr] >> 8;
        self.last = NO_BYTE;
        self.run_count = 0;
        self.run_left = 0;
        self.crc = !0;
        Ok(())
    }

    /// Walk the BWT links and undo the initial run-length stage (4 equal
    /// bytes, then a count of further repeats) straight into `out`. Returns 0
    /// only when the block is exhausted.
    fn emit(&mut self, out: &mut [u8]) -> usize {
        let tt = &self.tt[..self.nblock];
        let nblock = self.nblock;
        let mut used = self.used;
        let mut tpos = self.tpos;
        let mut last = self.last;
        let mut run_count = self.run_count;
        let mut run_left = self.run_left;
        let mut run_byte = self.run_byte;
        let mut crc = self.crc;
        let mut o = 0;
        while o < out.len() {
            if run_left > 0 {
                let n = run_left.min(out.len() - o);
                for b in &mut out[o..o + n] {
                    *b = run_byte;
                    crc = crc_update(crc, run_byte);
                }
                o += n;
                run_left -= n;
                continue;
            }
            if used == nblock {
                break;
            }
            let entry = tt[tpos as usize];
            let byte = entry as u8;
            tpos = entry >> 8;
            used += 1;
            out[o] = byte;
            o += 1;
            crc = crc_update(crc, byte);
            if byte as u32 != last {
                last = byte as u32;
                run_count = 1;
            } else {
                run_count += 1;
                if run_count == 4 {
                    if used < nblock {
                        let entry = tt[tpos as usize];
                        tpos = entry >> 8;
                        used += 1;
                        run_left = (entry & 0xff) as usize;
                        run_byte = byte;
                    }
                    last = NO_BYTE;
                    run_count = 0;
                }
            }
        }
        self.used = used;
        self.tpos = tpos;
        self.last = last;
        self.run_count = run_count;
        self.run_left = run_left;
        self.run_byte = run_byte;
        self.crc = crc;
        o
    }

    fn finish_block(&mut self) -> io::Result<()> {
        let crc = !self.crc;
        if crc != self.stored_crc {
            return Err(corrupt(format!("block CRC mismatch (stored {:08x}, computed {crc:08x})", self.stored_crc)));
        }
        self.combined_crc = self.combined_crc.rotate_left(1) ^ crc;
        Ok(())
    }

    fn step(&mut self, out: &mut [u8]) -> io::Result<usize> {
        loop {
            match self.state {
                State::StreamHeader { first } => {
                    self.state = if self.stream_header(first)? { State::NextBlock } else { State::Done };
                }
                State::NextBlock => self.state = self.next_block()?,
                State::Block => {
                    let n = self.emit(out);
                    if n > 0 {
                        return Ok(n);
                    }
                    self.finish_block()?;
                    self.state = State::NextBlock;
                }
                State::Done => return Ok(0),
                State::Failed => return Err(corrupt("read after an earlier error")),
            }
        }
    }
}

impl<R: Read> Read for MultiBzDecoder<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        let result = self.step(out);
        if result.is_err() {
            self.state = State::Failed;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `printf 'hello, world\n' | bzip2 -9`
    const HELLO: &[u8] = &[
        0x42, 0x5a, 0x68, 0x39, 0x31, 0x41, 0x59, 0x26, 0x53, 0x59, 0x54, 0xa4, 0x97, 0x84, 0x00, 0x00, 0x02, 0xd1,
        0x80, 0x00, 0x10, 0x40, 0x04, 0x06, 0x44, 0x90, 0x80, 0x20, 0x00, 0x31, 0x00, 0x30, 0x20, 0x68, 0x62, 0x00,
        0x49, 0xd4, 0xb2, 0x1f, 0x3f, 0x17, 0x72, 0x45, 0x38, 0x50, 0x90, 0x54, 0xa4, 0x97, 0x84,
    ];
    /// `printf '' | bzip2 -9`
    const EMPTY: &[u8] = &[0x42, 0x5a, 0x68, 0x39, 0x17, 0x72, 0x45, 0x38, 0x50, 0x90, 0x00, 0x00, 0x00, 0x00];
    /// [`runs_text`] through `bzip2 -9`: long runs, runs of exactly 4 and 5,
    /// a 4-run at the very end.
    const RUNS: &[u8] = &[
        0x42, 0x5a, 0x68, 0x39, 0x31, 0x41, 0x59, 0x26, 0x53, 0x59, 0x46, 0x4e, 0x77, 0x87, 0x00, 0x00, 0x0a, 0x51,
        0x81, 0xe0, 0x00, 0x00, 0x20, 0x38, 0x00, 0x00, 0x70, 0x00, 0x80, 0x00, 0x08, 0xa0, 0x00, 0x31, 0x03, 0x40,
        0xd0, 0x23, 0xd4, 0x26, 0x11, 0xea, 0x67, 0xa1, 0x8a, 0xb8, 0xc7, 0x91, 0x51, 0x32, 0xcc, 0x50, 0xac, 0xe6,
        0xa6, 0xe8, 0xe8, 0x83, 0x69, 0xc5, 0xf1, 0x77, 0x24, 0x53, 0x85, 0x09, 0x04, 0x64, 0xe7, 0x78, 0x70,
    ];
    /// `"ab" * 60000` through `bzip2 -1`: two blocks in one stream.
    const AB: &[u8] = &[
        0x42, 0x5a, 0x68, 0x31, 0x31, 0x41, 0x59, 0x26, 0x53, 0x59, 0xf1, 0x4b, 0x8b, 0x27, 0x00, 0x61, 0xa3, 0x01,
        0x00, 0x30, 0x00, 0x20, 0x00, 0x30, 0x80, 0x29, 0x19, 0x42, 0x50, 0x5a, 0xa1, 0x28, 0x2e, 0x62, 0x82, 0xb2,
        0x4c, 0xa6, 0xb3, 0x5b, 0x83, 0x22, 0x54, 0x00, 0x27, 0x19, 0x02, 0x00, 0x60, 0x00, 0x40, 0x00, 0x41, 0x48,
        0x10, 0x64, 0xa1, 0x51, 0xa2, 0x85, 0x47, 0x17, 0x72, 0x45, 0x38, 0x50, 0x90, 0x4f, 0x56, 0x87, 0x65,
    ];

    fn runs_text() -> Vec<u8> {
        let mut v = vec![b'a'; 1000];
        v.extend_from_slice(b"bbbbccccc");
        v.extend_from_slice(&b"xyz".repeat(3));
        v.extend_from_slice(b"yyyy");
        v.extend_from_slice(&[0u8; 300]);
        v.extend_from_slice(b"\xffaaaa");
        v
    }

    fn decode(data: &[u8]) -> io::Result<Vec<u8>> {
        let mut out = Vec::new();
        MultiBzDecoder::new(data).read_to_end(&mut out)?;
        Ok(out)
    }

    /// Reads through a 1-byte output buffer from a reader that hands out
    /// 3 bytes at a time: every state boundary gets crossed mid-call.
    fn decode_trickle(data: &[u8]) -> io::Result<Vec<u8>> {
        struct Trickle<'a>(&'a [u8]);
        impl Read for Trickle<'_> {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                let n = buf.len().min(3).min(self.0.len());
                buf[..n].copy_from_slice(&self.0[..n]);
                self.0 = &self.0[n..];
                Ok(n)
            }
        }
        let mut d = MultiBzDecoder::new(Trickle(data));
        let mut out = Vec::new();
        let mut b = [0u8; 1];
        loop {
            match d.read(&mut b)? {
                0 => return Ok(out),
                _ => out.push(b[0]),
            }
        }
    }

    #[test]
    fn known_vectors() {
        assert_eq!(decode(HELLO).unwrap(), b"hello, world\n");
        assert_eq!(decode(RUNS).unwrap(), runs_text());
        assert_eq!(decode(AB).unwrap(), b"ab".repeat(60000));
        assert_eq!(decode_trickle(RUNS).unwrap(), runs_text());
        assert_eq!(decode_trickle(AB).unwrap(), b"ab".repeat(60000));
    }

    #[test]
    fn empty_stream() {
        assert_eq!(decode(EMPTY).unwrap(), b"");
        assert!(decode(&[]).unwrap_err().to_string().contains("empty input"));
    }

    #[test]
    fn concatenated_streams() {
        let mut data = HELLO.to_vec();
        data.extend_from_slice(EMPTY);
        data.extend_from_slice(RUNS);
        data.extend_from_slice(HELLO);
        let mut want = b"hello, world\n".to_vec();
        want.extend_from_slice(&runs_text());
        want.extend_from_slice(b"hello, world\n");
        assert_eq!(decode(&data).unwrap(), want);
        assert_eq!(decode_trickle(&data).unwrap(), want);
    }

    #[test]
    fn corrupt_data_is_an_error() {
        // Block CRC (bytes 10..14) altered.
        let mut data = HELLO.to_vec();
        data[12] ^= 1;
        let e = decode(&data).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        assert!(e.to_string().contains("block CRC mismatch"), "{e}");
        // Stream CRC (the last 4 bytes, byte aligned here) altered.
        let mut data = HELLO.to_vec();
        *data.last_mut().unwrap() ^= 0x10;
        assert!(decode(&data).unwrap_err().to_string().contains("stream CRC mismatch"));
        // A payload bit flipped: some check catches it.
        let mut data = RUNS.to_vec();
        data[40] ^= 0x04;
        assert_eq!(decode(&data).unwrap_err().kind(), io::ErrorKind::InvalidData);
        // Truncated.
        assert!(decode(&AB[..AB.len() - 6]).unwrap_err().to_string().contains("unexpected end"));
        // Not bzip2, and garbage after a valid stream.
        assert!(decode(b"PK\x03\x04 not bzip2").unwrap_err().to_string().contains("no BZh"));
        let mut data = HELLO.to_vec();
        data.extend_from_slice(b"junk");
        assert!(decode(&data).unwrap_err().to_string().contains("not another bzip2 stream"));
    }
}
