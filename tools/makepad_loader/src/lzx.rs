//! LZX as used by Microsoft Cabinet folders (window 32KiB..2MiB).

const PRETREE_LEN: usize = 20;
const MAINTREE_LEN: usize = 256 + 8 * 50;
const LENTREE_LEN: usize = 249;
const ALIGNED_LEN: usize = 8;

pub struct Decoder {
    window: Vec<u8>,
    window_size: usize,
    window_posn: usize,
    r0: usize,
    r1: usize,
    r2: usize,
    intel_filesize: u32,
    intel_curpos: i32,
    intel_started: bool,
    header_read: bool,
    block_remaining: u32,
    block_type: u32,
    main_tree: Huffman,
    length_tree: Huffman,
    aligned_tree: Huffman,
    bits: BitReader,
}

impl Decoder {
    pub fn new(window_bits: u32) -> Result<Self, String> {
        if !(15..=21).contains(&window_bits) {
            return Err(format!("lzx window_bits {window_bits}"));
        }
        let window_size = 1usize << window_bits;
        Ok(Self {
            window: vec![0; window_size],
            window_size,
            window_posn: 0,
            r0: 1,
            r1: 1,
            r2: 1,
            intel_filesize: 0,
            intel_curpos: 0,
            intel_started: false,
            header_read: false,
            block_remaining: 0,
            block_type: 0,
            main_tree: Huffman::empty(),
            length_tree: Huffman::empty(),
            aligned_tree: Huffman::empty(),
            bits: BitReader::new(&[]),
        })
    }

    pub fn decompress(&mut self, input: &[u8], out_len: usize) -> Result<Vec<u8>, String> {
        self.bits.push(input);
        if !self.header_read {
            if self.bits.read(1)? != 0 {
                let hi = self.bits.read(16)?;
                let lo = self.bits.read(16)?;
                self.intel_filesize = (hi << 16) | lo;
            }
            self.header_read = true;
        }
        let mut out = vec![0u8; out_len];
        let mut copied = 0usize;
        while copied < out_len {
            if self.block_remaining == 0 {
                self.read_block_header()?;
            }
            let take = (out_len - copied).min(self.block_remaining as usize);
            match self.block_type {
                1 | 2 => self.decode_compressed(&mut out, copied, take)?,
                3 => self.decode_uncompressed(&mut out, copied, take)?,
                t => return Err(format!("lzx block type {t}")),
            }
            copied += take;
            self.block_remaining -= take as u32;
        }
        if self.intel_started && self.intel_filesize != 0 && out_len > 10 {
            apply_e8(&mut out, &mut self.intel_curpos, self.intel_filesize);
        }
        Ok(out)
    }

    fn read_block_header(&mut self) -> Result<(), String> {
        self.block_type = self.bits.read(3)?;
        let hi = self.bits.read(16)?;
        let lo = self.bits.read(8)?;
        self.block_remaining = (hi << 8) | lo;
        match self.block_type {
            1 => {
                read_lengths(&mut self.bits, &mut self.main_tree, 0, 256)?;
                read_lengths(&mut self.bits, &mut self.main_tree, 256, MAINTREE_LEN)?;
                self.main_tree.build(MAINTREE_LEN)?;
                read_lengths(&mut self.bits, &mut self.length_tree, 0, LENTREE_LEN)?;
                self.length_tree.build(LENTREE_LEN)?;
            }
            2 => {
                let mut lens = [0u8; ALIGNED_LEN];
                for i in 0..ALIGNED_LEN {
                    lens[i] = self.bits.read(3)? as u8;
                }
                self.aligned_tree = Huffman::from_lengths(&lens)?;
                read_lengths(&mut self.bits, &mut self.main_tree, 0, 256)?;
                read_lengths(&mut self.bits, &mut self.main_tree, 256, MAINTREE_LEN)?;
                self.main_tree.build(MAINTREE_LEN)?;
                read_lengths(&mut self.bits, &mut self.length_tree, 0, LENTREE_LEN)?;
                self.length_tree.build(LENTREE_LEN)?;
            }
            3 => {
                self.bits.align_to_byte();
                self.r0 = self.bits.read_le_u32()? as usize;
                self.r1 = self.bits.read_le_u32()? as usize;
                self.r2 = self.bits.read_le_u32()? as usize;
            }
            t => return Err(format!("lzx bad block {t}")),
        }
        Ok(())
    }
}

fn read_lengths(
    bits: &mut BitReader,
    tree: &mut Huffman,
    start: usize,
    end: usize,
) -> Result<(), String> {
        let mut pre = [0u8; PRETREE_LEN];
        for i in 0..PRETREE_LEN {
            pre[i] = bits.read(4)? as u8;
        }
        let pretree = Huffman::from_lengths(&pre)?;
        let mut i = start;
        while i < end {
            let sym = pretree.decode(bits)?;
            match sym {
                0..=16 => {
                    let len = if i < tree.lengths.len() {
                        tree.lengths[i]
                    } else {
                        0
                    };
                    let v = ((len as i32 + sym as i32 - 8) & 0x1f) as u8;
                    if i >= tree.lengths.len() {
                        tree.lengths.resize(end, 0);
                    }
                    tree.lengths[i] = v;
                    i += 1;
                }
                17 => {
                    let n = 4 + bits.read(4)? as usize;
                    if i + n > end {
                        return Err("lzx pretree 17 overrun".into());
                    }
                    if tree.lengths.len() < end {
                        tree.lengths.resize(end, 0);
                    }
                    for slot in tree.lengths.iter_mut().skip(i).take(n) {
                        *slot = 0;
                    }
                    i += n;
                }
                18 => {
                    let n = 20 + bits.read(5)? as usize;
                    if i + n > end {
                        return Err("lzx pretree 18 overrun".into());
                    }
                    if tree.lengths.len() < end {
                        tree.lengths.resize(end, 0);
                    }
                    for slot in tree.lengths.iter_mut().skip(i).take(n) {
                        *slot = 0;
                    }
                    i += n;
                }
                19 => {
                    let n = 4 + bits.read(1)? as usize;
                    let adj = pretree.decode(bits)? as i32;
                    let base = if i < tree.lengths.len() {
                        tree.lengths[i]
                    } else {
                        0
                    };
                    let v = ((base as i32 + adj - 8) & 0x1f) as u8;
                    if tree.lengths.len() < end {
                        tree.lengths.resize(end, 0);
                    }
                    for slot in tree.lengths.iter_mut().skip(i).take(n) {
                        *slot = v;
                    }
                    i += n;
                }
                _ => return Err("lzx pretree symbol".into()),
            }
        }
        Ok(())
}

impl Decoder {
    fn decode_compressed(&mut self, out: &mut [u8], start: usize, take: usize) -> Result<(), String> {
        let mut i = 0usize;
        while i < take {
            let sym = self.main_tree.decode(&mut self.bits)?;
            if sym < 256 {
                let b = sym as u8;
                self.window[self.window_posn] = b;
                out[start + i] = b;
                self.window_posn = (self.window_posn + 1) % self.window_size;
                i += 1;
                self.intel_started = true;
                continue;
            }
            let footer = (sym - 256) as usize;
            let mut length = (footer & 7) + 2;
            if (footer & 7) == 7 {
                length = 9 + self.length_tree.decode(&mut self.bits)? as usize;
            }
            let pos_slot = footer >> 3;
            let offset = self.match_offset(pos_slot)?;
            if offset == 0 || offset > self.window_size {
                return Err(format!("lzx offset {offset}"));
            }
            for _ in 0..length {
                if i >= take {
                    break;
                }
                let src = (self.window_posn + self.window_size - offset) % self.window_size;
                let b = self.window[src];
                self.window[self.window_posn] = b;
                out[start + i] = b;
                self.window_posn = (self.window_posn + 1) % self.window_size;
                i += 1;
            }
            self.intel_started = true;
        }
        Ok(())
    }

    fn match_offset(&mut self, pos_slot: usize) -> Result<usize, String> {
        if pos_slot < 3 {
            let off = match pos_slot {
                0 => self.r0,
                1 => self.r1,
                _ => self.r2,
            };
            if pos_slot != 0 {
                let t = self.r0;
                self.r0 = off;
                if pos_slot == 1 {
                    self.r1 = t;
                } else {
                    self.r2 = self.r1;
                    self.r1 = t;
                }
            }
            return Ok(off);
        }
        let extra_bits = extra_offset_bits(pos_slot);
        let mut offset = 1usize << extra_bits;
        if extra_bits > 3 && self.block_type == 2 {
            let verbatim = if extra_bits > 3 {
                self.bits.read(extra_bits as u32 - 3)? as usize
            } else {
                0
            };
            let aligned = self.aligned_tree.decode(&mut self.bits)? as usize;
            offset += (verbatim << 3) + aligned;
        } else if extra_bits > 0 {
            offset += self.bits.read(extra_bits as u32)? as usize;
        }
        offset -= 2;
        self.r2 = self.r1;
        self.r1 = self.r0;
        self.r0 = offset;
        Ok(offset)
    }

    fn decode_uncompressed(&mut self, out: &mut [u8], start: usize, take: usize) -> Result<(), String> {
        self.bits.align_to_byte();
        for i in 0..take {
            let b = self.bits.read(8)? as u8;
            self.window[self.window_posn] = b;
            out[start + i] = b;
            self.window_posn = (self.window_posn + 1) % self.window_size;
        }
        if take & 1 != 0 {
            let _ = self.bits.read(8);
        }
        Ok(())
    }
}

fn extra_offset_bits(slot: usize) -> usize {
    if slot < 4 {
        0
    } else {
        (slot >> 1) - 1
    }
}

fn apply_e8(buf: &mut [u8], curpos: &mut i32, filesize: u32) {
    let mut i = 0usize;
    while i + 5 <= buf.len() {
        if buf[i] != 0xe8 {
            i += 1;
            *curpos += 1;
            continue;
        }
        let abs = i32::from_le_bytes([buf[i + 1], buf[i + 2], buf[i + 3], buf[i + 4]]);
        let pos = *curpos;
        let rel = if abs >= -pos && abs < filesize as i32 {
            abs.wrapping_sub(pos)
        } else {
            abs
        };
        let b = rel.to_le_bytes();
        buf[i + 1..i + 5].copy_from_slice(&b);
        i += 5;
        *curpos += 5;
    }
    *curpos += (buf.len().saturating_sub(i)) as i32;
}

struct Huffman {
    lengths: Vec<u8>,
    table: Vec<u16>,
    bits: u32,
}

impl Huffman {
    fn empty() -> Self {
        Self {
            lengths: Vec::new(),
            table: Vec::new(),
            bits: 0,
        }
    }

    fn from_lengths(lens: &[u8]) -> Result<Self, String> {
        let mut h = Self {
            lengths: lens.to_vec(),
            table: Vec::new(),
            bits: 0,
        };
        h.build(lens.len())?;
        Ok(h)
    }

    fn build(&mut self, nsyms: usize) -> Result<(), String> {
        if self.lengths.len() < nsyms {
            self.lengths.resize(nsyms, 0);
        }
        let max = self.lengths.iter().copied().max().unwrap_or(0) as u32;
        if max == 0 {
            self.table.clear();
            self.bits = 0;
            return Ok(());
        }
        self.bits = max.min(16);
        let size = 1usize << self.bits;
        self.table = vec![0xffff; size];
        let mut bl_count = [0u32; 17];
        for &l in &self.lengths[..nsyms] {
            if l as u32 > 16 {
                return Err("lzx huffman len".into());
            }
            bl_count[l as usize] += 1;
        }
        let mut next_code = [0u32; 17];
        let mut code = 0u32;
        for bits in 1..=16 {
            code = (code + bl_count[bits - 1]) << 1;
            next_code[bits] = code;
        }
        for (sym, &len) in self.lengths[..nsyms].iter().enumerate() {
            if len == 0 {
                continue;
            }
            let mut c = next_code[len as usize];
            next_code[len as usize] += 1;
            let fill = self.bits - len as u32;
            c <<= fill;
            let n = 1usize << fill;
            for i in 0..n {
                let idx = (c as usize) + i;
                if idx < self.table.len() {
                    self.table[idx] = ((len as u16) << 9) | (sym as u16);
                }
            }
        }
        Ok(())
    }

    fn decode(&self, bits: &mut BitReader) -> Result<u32, String> {
        if self.bits == 0 {
            return Err("empty huffman".into());
        }
        let peek = bits.peek(self.bits)?;
        let ent = self.table[peek as usize];
        if ent == 0xffff {
            return Err("lzx huffman decode".into());
        }
        let len = (ent >> 9) as u32;
        let sym = (ent & 0x1ff) as u32;
        bits.consume(len)?;
        Ok(sym)
    }
}

struct BitReader {
    buf: Vec<u8>,
    pos: usize,
    bitbuf: u32,
    bits: u32,
}

impl BitReader {
    fn new(data: &[u8]) -> Self {
        Self {
            buf: data.to_vec(),
            pos: 0,
            bitbuf: 0,
            bits: 0,
        }
    }

    fn push(&mut self, data: &[u8]) {
        if self.pos > 0 {
            self.buf.drain(..self.pos);
            self.pos = 0;
        }
        self.buf.extend_from_slice(data);
    }

    fn ensure(&mut self, n: u32) -> Result<(), String> {
        while self.bits < n {
            if self.pos >= self.buf.len() {
                return Err("lzx bitstream exhausted".into());
            }
            let b = self.buf[self.pos] as u32;
            self.pos += 1;
            self.bitbuf |= b << self.bits;
            self.bits += 8;
        }
        Ok(())
    }

    fn peek(&mut self, n: u32) -> Result<u32, String> {
        self.ensure(n)?;
        Ok(self.bitbuf & ((1u32 << n) - 1))
    }

    fn consume(&mut self, n: u32) -> Result<(), String> {
        self.ensure(n)?;
        self.bitbuf >>= n;
        self.bits -= n;
        Ok(())
    }

    fn read(&mut self, n: u32) -> Result<u32, String> {
        let v = self.peek(n)?;
        self.consume(n)?;
        Ok(v)
    }

    fn align_to_byte(&mut self) {
        let rem = self.bits % 8;
        if rem != 0 {
            self.bitbuf >>= rem;
            self.bits -= rem;
        }
    }

    fn read_le_u32(&mut self) -> Result<u32, String> {
        self.align_to_byte();
        let a = self.read(8)?;
        let b = self.read(8)?;
        let c = self.read(8)?;
        let d = self.read(8)?;
        Ok(a | (b << 8) | (c << 16) | (d << 24))
    }
}
