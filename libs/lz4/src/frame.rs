//! The LZ4 frame format (lz4_Frame_format.md): a magic number, a
//! descriptor, blocks of at most 4 MB, an end mark and an xxHash32 of the
//! content. [`FrameEncoder`] writes one to any `Write` sink and
//! [`FrameDecoder`] reads one as a `Read` source, so a stream of any
//! length passes through two block-sized buffers and never sits in memory
//! whole. The encoder writes independent blocks, which is what the decoder
//! reads: a block-linked frame (the reference tool's default, `lz4 -BD`) is
//! refused rather than misread, and so is a frame that names a dictionary.
//! Skippable frames and concatenated frames are read through.

use crate::{compress_bound, compress_fast_into, decompress_safe};
use std::io::{self, Read, Write};

/// The frame magic, little-endian on the wire: `04 22 4D 18`.
pub const MAGIC: u32 = 0x184D_2204;
const SKIPPABLE_MAGIC: u32 = 0x184D_2A50;
const SKIPPABLE_MASK: u32 = 0xFFFF_FFF0;
const UNCOMPRESSED_BIT: u32 = 0x8000_0000;
/// Block max size ids 4..=7 of the BD byte, in bytes.
const BLOCK_SIZES: [usize; 4] = [64 << 10, 256 << 10, 1 << 20, 4 << 20];
/// What the encoder writes: 4 MB blocks (id 7).
pub const DEFAULT_BLOCK_SIZE: usize = 4 << 20;
/// A pax-sized sanity bound on a skippable frame this reader will skip.
const MAX_SKIPPABLE: u32 = 1 << 30;

const XXH_P1: u32 = 0x9E37_79B1;
const XXH_P2: u32 = 0x85EB_CA77;
const XXH_P3: u32 = 0xC2B2_AE3D;
const XXH_P4: u32 = 0x27D4_EB2F;
const XXH_P5: u32 = 0x1656_67B1;

/// Streaming xxHash32, the checksum the frame format uses for its header,
/// its blocks and its content.
#[derive(Clone)]
pub struct Xxh32 {
    v: [u32; 4],
    seed: u32,
    total: u64,
    buf: [u8; 16],
    buf_len: usize,
}

impl Xxh32 {
    pub fn new(seed: u32) -> Self {
        Self {
            v: [
                seed.wrapping_add(XXH_P1).wrapping_add(XXH_P2),
                seed.wrapping_add(XXH_P2),
                seed,
                seed.wrapping_sub(XXH_P1),
            ],
            seed,
            total: 0,
            buf: [0; 16],
            buf_len: 0,
        }
    }

    #[inline]
    fn round(acc: u32, lane: u32) -> u32 {
        acc.wrapping_add(lane.wrapping_mul(XXH_P2)).rotate_left(13).wrapping_mul(XXH_P1)
    }

    fn stripe(v: &mut [u32; 4], stripe: &[u8]) {
        for (i, lane) in stripe.chunks_exact(4).enumerate() {
            v[i] = Self::round(v[i], u32::from_le_bytes([lane[0], lane[1], lane[2], lane[3]]));
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.total += data.len() as u64;
        if self.buf_len + data.len() < 16 {
            self.buf[self.buf_len..self.buf_len + data.len()].copy_from_slice(data);
            self.buf_len += data.len();
            return;
        }
        if self.buf_len > 0 {
            let take = 16 - self.buf_len;
            self.buf[self.buf_len..].copy_from_slice(&data[..take]);
            let buf = self.buf;
            Self::stripe(&mut self.v, &buf);
            self.buf_len = 0;
            data = &data[take..];
        }
        let mut stripes = data.chunks_exact(16);
        for stripe in &mut stripes {
            Self::stripe(&mut self.v, stripe);
        }
        let rest = stripes.remainder();
        self.buf[..rest.len()].copy_from_slice(rest);
        self.buf_len = rest.len();
    }

    pub fn finish(&self) -> u32 {
        let mut h = if self.total >= 16 {
            self.v[0]
                .rotate_left(1)
                .wrapping_add(self.v[1].rotate_left(7))
                .wrapping_add(self.v[2].rotate_left(12))
                .wrapping_add(self.v[3].rotate_left(18))
        } else {
            self.seed.wrapping_add(XXH_P5)
        };
        h = h.wrapping_add(self.total as u32);
        let rest = &self.buf[..self.buf_len];
        let mut words = rest.chunks_exact(4);
        for word in &mut words {
            let lane = u32::from_le_bytes([word[0], word[1], word[2], word[3]]);
            h = h.wrapping_add(lane.wrapping_mul(XXH_P3)).rotate_left(17).wrapping_mul(XXH_P4);
        }
        for byte in words.remainder() {
            h = h.wrapping_add((*byte as u32).wrapping_mul(XXH_P5)).rotate_left(11).wrapping_mul(XXH_P1);
        }
        h ^= h >> 15;
        h = h.wrapping_mul(XXH_P2);
        h ^= h >> 13;
        h = h.wrapping_mul(XXH_P3);
        h ^= h >> 16;
        h
    }
}

/// xxHash32 of `data` in one call.
pub fn xxh32(data: &[u8], seed: u32) -> u32 {
    let mut h = Xxh32::new(seed);
    h.update(data);
    h.finish()
}

fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

/// Writes an LZ4 frame: independent 4 MB blocks, a content checksum, and
/// the content size when it was given. `finish` writes the end mark; a
/// frame dropped before it is truncated and the decoder says so.
pub struct FrameEncoder<W: Write> {
    sink: W,
    block: Vec<u8>,
    out: Vec<u8>,
    block_size: usize,
    hash: Xxh32,
    content_size: Option<u64>,
    started: bool,
}

impl<W: Write> FrameEncoder<W> {
    pub fn new(sink: W) -> Self {
        Self::with_block_size(sink, DEFAULT_BLOCK_SIZE, None)
    }

    /// The content size goes into the descriptor, where a decoder can read
    /// it before the first block (progress, allocation).
    pub fn with_content_size(sink: W, content_size: u64) -> Self {
        Self::with_block_size(sink, DEFAULT_BLOCK_SIZE, Some(content_size))
    }

    /// `block_size` must be one of 64 KB, 256 KB, 1 MB, 4 MB.
    pub fn with_block_size(sink: W, block_size: usize, content_size: Option<u64>) -> Self {
        assert!(BLOCK_SIZES.contains(&block_size), "lz4 frame block size must be 64K/256K/1M/4M");
        Self {
            sink,
            block: Vec::with_capacity(block_size),
            out: vec![0u8; compress_bound(block_size)],
            block_size,
            hash: Xxh32::new(0),
            content_size,
            started: false,
        }
    }

    fn write_header(&mut self) -> io::Result<()> {
        let mut desc = Vec::with_capacity(11);
        // Version 01, independent blocks, no block checksum, content
        // checksum on, content size when known, no dictionary.
        let mut flg: u8 = 0b0110_0100;
        if self.content_size.is_some() {
            flg |= 0b0000_1000;
        }
        desc.push(flg);
        let bd_id = BLOCK_SIZES.iter().position(|s| *s == self.block_size).unwrap() as u8 + 4;
        desc.push(bd_id << 4);
        if let Some(size) = self.content_size {
            desc.extend_from_slice(&size.to_le_bytes());
        }
        let hc = (xxh32(&desc, 0) >> 8) as u8;
        self.sink.write_all(&MAGIC.to_le_bytes())?;
        self.sink.write_all(&desc)?;
        self.sink.write_all(&[hc])?;
        self.started = true;
        Ok(())
    }

    fn flush_block(&mut self) -> io::Result<()> {
        if !self.started {
            self.write_header()?;
        }
        if self.block.is_empty() {
            return Ok(());
        }
        let written = compress_fast_into(&self.block, &mut self.out, 1).map_err(|e| invalid(e.to_string()))?;
        if written < self.block.len() {
            self.sink.write_all(&(written as u32).to_le_bytes())?;
            self.sink.write_all(&self.out[..written])?;
        } else {
            // Incompressible: stored, the size with its high bit set.
            self.sink.write_all(&(self.block.len() as u32 | UNCOMPRESSED_BIT).to_le_bytes())?;
            self.sink.write_all(&self.block)?;
        }
        self.block.clear();
        Ok(())
    }

    /// The last block, the end mark and the content checksum; the sink is
    /// handed back flushed.
    pub fn finish(mut self) -> io::Result<W> {
        self.flush_block()?;
        if let Some(size) = self.content_size {
            if size != self.hash.total {
                return Err(invalid(format!(
                    "lz4 frame: {} content bytes written, {size} announced",
                    self.hash.total
                )));
            }
        }
        self.sink.write_all(&0u32.to_le_bytes())?;
        self.sink.write_all(&self.hash.finish().to_le_bytes())?;
        self.sink.flush()?;
        Ok(self.sink)
    }
}

impl<W: Write> Write for FrameEncoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if !self.started {
            self.write_header()?;
        }
        let room = self.block_size - self.block.len();
        let n = buf.len().min(room);
        self.block.extend_from_slice(&buf[..n]);
        self.hash.update(&buf[..n]);
        if self.block.len() == self.block_size {
            self.flush_block()?;
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.sink.flush()
    }
}

fn read_u32<R: Read>(source: &mut R) -> io::Result<Option<u32>> {
    let mut b = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        let n = source.read(&mut b[got..])?;
        if n == 0 {
            if got == 0 {
                return Ok(None);
            }
            return Err(invalid("lz4 frame: truncated"));
        }
        got += n;
    }
    Ok(Some(u32::from_le_bytes(b)))
}

fn read_exact<R: Read>(source: &mut R, buf: &mut [u8]) -> io::Result<()> {
    source
        .read_exact(buf)
        .map_err(|e| if e.kind() == io::ErrorKind::UnexpectedEof { invalid("lz4 frame: truncated") } else { e })
}

/// Reads an LZ4 frame as a byte stream. The descriptor is parsed by `new`
/// (so a source that is not a frame fails there), each block is decoded
/// into a buffer of the frame's block size and served from it, the end
/// mark ends the frame, and the content checksum, when present, is
/// verified before `read` reports the end.
pub struct FrameDecoder<R: Read> {
    source: R,
    input: Vec<u8>,
    output: Vec<u8>,
    pos: usize,
    len: usize,
    block_size: usize,
    block_checksum: bool,
    content_checksum: bool,
    content_size: Option<u64>,
    hash: Xxh32,
    /// No frame is open: the next bytes are a magic number or EOF.
    between_frames: bool,
    done: bool,
}

impl<R: Read> FrameDecoder<R> {
    pub fn new(source: R) -> io::Result<Self> {
        let mut d = Self {
            source,
            input: Vec::new(),
            output: Vec::new(),
            pos: 0,
            len: 0,
            block_size: 0,
            block_checksum: false,
            content_checksum: false,
            content_size: None,
            hash: Xxh32::new(0),
            between_frames: true,
            done: false,
        };
        if !d.open_frame()? {
            return Err(invalid("not an lz4 frame: empty input"));
        }
        Ok(d)
    }

    /// The content size the first frame's descriptor announced, if any.
    pub fn content_size(&self) -> Option<u64> {
        self.content_size
    }

    pub fn into_inner(self) -> R {
        self.source
    }

    /// Reads magic numbers until a frame's descriptor is parsed (true) or
    /// the source ends cleanly (false). Skippable frames are skipped.
    fn open_frame(&mut self) -> io::Result<bool> {
        loop {
            let Some(magic) = read_u32(&mut self.source)? else { return Ok(false) };
            if magic & SKIPPABLE_MASK == SKIPPABLE_MAGIC {
                let Some(size) = read_u32(&mut self.source)? else { return Err(invalid("lz4 frame: truncated")) };
                if size > MAX_SKIPPABLE {
                    return Err(invalid("lz4 frame: skippable frame too large"));
                }
                let mut left = size as usize;
                let mut sink = [0u8; 4096];
                while left > 0 {
                    let n = left.min(sink.len());
                    read_exact(&mut self.source, &mut sink[..n])?;
                    left -= n;
                }
                continue;
            }
            if magic != MAGIC {
                return Err(invalid(format!("not an lz4 frame: magic {magic:#010x}")));
            }
            let mut desc = vec![0u8; 2];
            read_exact(&mut self.source, &mut desc)?;
            let flg = desc[0];
            let bd = desc[1];
            if flg >> 6 != 0b01 {
                return Err(invalid(format!("lz4 frame: version {} not supported", flg >> 6)));
            }
            let independent = flg & 0b0010_0000 != 0;
            self.block_checksum = flg & 0b0001_0000 != 0;
            let has_size = flg & 0b0000_1000 != 0;
            self.content_checksum = flg & 0b0000_0100 != 0;
            let has_dict = flg & 0b0000_0001 != 0;
            if !independent {
                return Err(invalid("lz4 frame: block-linked frames are not supported (pack with independent blocks)"));
            }
            if has_dict {
                return Err(invalid("lz4 frame: dictionaries are not supported"));
            }
            let bd_id = (bd >> 4) & 0b111;
            if !(4..=7).contains(&bd_id) {
                return Err(invalid(format!("lz4 frame: block size id {bd_id}")));
            }
            self.block_size = BLOCK_SIZES[bd_id as usize - 4];
            if has_size {
                let mut size = [0u8; 8];
                read_exact(&mut self.source, &mut size)?;
                desc.extend_from_slice(&size);
                let size = u64::from_le_bytes(size);
                if self.content_size.is_none() {
                    self.content_size = Some(size);
                }
            }
            let mut hc = [0u8; 1];
            read_exact(&mut self.source, &mut hc)?;
            if (xxh32(&desc, 0) >> 8) as u8 != hc[0] {
                return Err(invalid("lz4 frame: header checksum mismatch"));
            }
            if self.output.len() < self.block_size {
                self.output = vec![0u8; self.block_size];
            }
            self.hash = Xxh32::new(0);
            self.between_frames = false;
            return Ok(true);
        }
    }

    /// Decodes the next block into `output`. Ok(false) once every frame
    /// has ended and the source is at EOF.
    fn next_block(&mut self) -> io::Result<bool> {
        loop {
            if self.between_frames && !self.open_frame()? {
                return Ok(false);
            }
            let Some(word) = read_u32(&mut self.source)? else { return Err(invalid("lz4 frame: truncated (no end mark)")) };
            if word == 0 {
                if self.content_checksum {
                    let Some(want) = read_u32(&mut self.source)? else { return Err(invalid("lz4 frame: truncated content checksum")) };
                    if want != self.hash.finish() {
                        return Err(invalid("lz4 frame: content checksum mismatch"));
                    }
                }
                self.between_frames = true;
                continue;
            }
            let stored = word & UNCOMPRESSED_BIT != 0;
            let size = (word & !UNCOMPRESSED_BIT) as usize;
            if size > self.block_size || size == 0 {
                return Err(invalid(format!("lz4 frame: block of {size} bytes in a {} byte frame", self.block_size)));
            }
            if stored {
                read_exact(&mut self.source, &mut self.output[..size])?;
                self.len = size;
            } else {
                if self.input.len() < size {
                    self.input = vec![0u8; self.block_size];
                }
                let mut input = std::mem::take(&mut self.input);
                read_exact(&mut self.source, &mut input[..size])?;
                if self.block_checksum {
                    let Some(want) = read_u32(&mut self.source)? else { return Err(invalid("lz4 frame: truncated block checksum")) };
                    if want != xxh32(&input[..size], 0) {
                        return Err(invalid("lz4 frame: block checksum mismatch"));
                    }
                }
                let decoded = decompress_safe(&input[..size], &mut self.output).map_err(|e| invalid(format!("lz4 frame: {e}")));
                self.input = input;
                self.len = decoded?;
            }
            if stored && self.block_checksum {
                let Some(want) = read_u32(&mut self.source)? else { return Err(invalid("lz4 frame: truncated block checksum")) };
                if want != xxh32(&self.output[..size], 0) {
                    return Err(invalid("lz4 frame: block checksum mismatch"));
                }
            }
            if self.content_checksum {
                self.hash.update(&self.output[..self.len]);
            }
            self.pos = 0;
            return Ok(true);
        }
    }
}

impl<R: Read> Read for FrameDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        while self.pos == self.len {
            if self.done {
                return Ok(0);
            }
            if !self.next_block()? {
                self.done = true;
                return Ok(0);
            }
        }
        let n = buf.len().min(self.len - self.pos);
        buf[..n].copy_from_slice(&self.output[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// `data` as one frame in memory.
pub fn frame_compress(data: &[u8]) -> Vec<u8> {
    let mut enc = FrameEncoder::with_content_size(Vec::new(), data.len() as u64);
    enc.write_all(data).expect("Vec sink");
    enc.finish().expect("Vec sink")
}

/// The content of a frame held in memory.
pub fn frame_decompress(frame: &[u8]) -> io::Result<Vec<u8>> {
    let mut dec = FrameDecoder::new(frame)?;
    let mut out = Vec::with_capacity(dec.content_size().unwrap_or(0).min(1 << 30) as usize);
    dec.read_to_end(&mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xxh32_reference_vectors() {
        assert_eq!(xxh32(b"", 0), 0x02CC_5D05);
        assert_eq!(xxh32(b"a", 0), 0x550D_7456);
        assert_eq!(xxh32(b"abc", 0), 0x32D1_53FF);
        assert_eq!(xxh32(b"Nobody inspects the spammish repetition", 0), 0xE229_3B2F);
    }

    #[test]
    fn xxh32_streams_the_same_as_one_call() {
        let data: Vec<u8> = (0..10_007u32).map(|n| (n.wrapping_mul(2654435761) >> 13) as u8).collect();
        let whole = xxh32(&data, 7);
        for chunk in [1usize, 3, 15, 16, 17, 64, 1000] {
            let mut h = Xxh32::new(7);
            for piece in data.chunks(chunk) {
                h.update(piece);
            }
            assert_eq!(h.finish(), whole, "chunk {chunk}");
        }
    }

    fn sample(len: usize) -> Vec<u8> {
        // Half text (compressible), half noise (stored blocks).
        let mut v = Vec::with_capacity(len);
        let mut x: u32 = 0x1234_5678;
        while v.len() < len {
            if v.len() % (2 << 20) < (1 << 20) {
                v.extend_from_slice(b"the quick brown fox jumps over the lazy dog ");
            } else {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                v.push(x as u8);
            }
        }
        v.truncate(len);
        v
    }

    #[test]
    fn frames_round_trip_across_block_boundaries() {
        for len in [0usize, 1, 100, DEFAULT_BLOCK_SIZE - 1, DEFAULT_BLOCK_SIZE, DEFAULT_BLOCK_SIZE + 1, 9 << 20] {
            let data = sample(len);
            let frame = frame_compress(&data);
            assert_eq!(&frame[..4], &MAGIC.to_le_bytes());
            let back = frame_decompress(&frame).unwrap();
            assert_eq!(back.len(), data.len(), "len {len}");
            assert!(back == data, "len {len}");
            let dec = FrameDecoder::new(&frame[..]).unwrap();
            assert_eq!(dec.content_size(), Some(len as u64));
        }
    }

    #[test]
    fn small_reads_and_unknown_content_size() {
        let data = sample(3 << 20);
        let mut enc = FrameEncoder::with_block_size(Vec::new(), 256 << 10, None);
        for piece in data.chunks(777) {
            enc.write_all(piece).unwrap();
        }
        let frame = enc.finish().unwrap();
        let mut dec = FrameDecoder::new(&frame[..]).unwrap();
        assert_eq!(dec.content_size(), None);
        let mut out = Vec::new();
        let mut buf = [0u8; 1000];
        loop {
            let n = dec.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        assert!(out == data);
    }

    #[test]
    fn concatenated_and_skippable_frames_read_through() {
        let a = frame_compress(b"hello ");
        let b = frame_compress(b"world");
        let mut joined = Vec::new();
        joined.extend_from_slice(&(SKIPPABLE_MAGIC | 3).to_le_bytes());
        joined.extend_from_slice(&5u32.to_le_bytes());
        joined.extend_from_slice(b"12345");
        joined.extend_from_slice(&a);
        joined.extend_from_slice(&b);
        assert_eq!(frame_decompress(&joined).unwrap(), b"hello world");
    }

    #[test]
    fn damage_is_reported() {
        let data = sample(300_000);
        let frame = frame_compress(&data);
        // Truncated before the end mark.
        assert!(frame_decompress(&frame[..frame.len() - 8]).is_err());
        // A flipped content byte fails the content checksum (or the block).
        let mut bad = frame.clone();
        let i = bad.len() / 2;
        bad[i] ^= 0x55;
        assert!(frame_decompress(&bad).is_err());
        // Wrong magic.
        assert!(FrameDecoder::new(&b"PK\x03\x04junk"[..]).is_err());
        // A block-linked descriptor is refused up front.
        let mut linked = frame.clone();
        linked[4] &= !0b0010_0000;
        let hc = (xxh32(&linked[4..4 + 2 + 8], 0) >> 8) as u8;
        linked[4 + 2 + 8] = hc;
        let err = FrameDecoder::new(&linked[..]).err().unwrap();
        assert!(err.to_string().contains("block-linked"), "{err}");
    }

    #[test]
    fn a_frame_assembled_from_the_spec_decodes() {
        // Descriptor: FLG = version 01, independent blocks, content
        // checksum; BD = 4 MB blocks; HC = the descriptor's xxh32 >> 8.
        // Block: "hello " literal, a 12-byte match at offset 6, then the
        // last literals "hello\n" (token 0x68 = 6 literals, match 8 + 4).
        let content = b"hello hello hello hello\n";
        let desc = [0x64u8, 0x70];
        let mut frame = MAGIC.to_le_bytes().to_vec();
        frame.extend_from_slice(&desc);
        frame.push((xxh32(&desc, 0) >> 8) as u8);
        let block = [0x68, b'h', b'e', b'l', b'l', b'o', b' ', 0x06, 0x00, 0x60, b'h', b'e', b'l', b'l', b'o', b'\n'];
        frame.extend_from_slice(&(block.len() as u32).to_le_bytes());
        frame.extend_from_slice(&block);
        frame.extend_from_slice(&0u32.to_le_bytes());
        frame.extend_from_slice(&xxh32(content, 0).to_le_bytes());
        assert_eq!(frame_decompress(&frame).unwrap(), content);
        // And the encoder's own header agrees with that descriptor.
        let ours = frame_compress(content);
        assert_eq!(ours[4] & !0b0000_1000, 0x64);
        assert_eq!(ours[5], 0x70);
    }
}
