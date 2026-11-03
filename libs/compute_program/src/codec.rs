//! How an output's bytes travel: elements of 1, 2 or 4 bytes (u8 levels, f16, words), coded in
//! blocks of [`BLOCK`] elements, each block on its own so blocks encode and decode on several
//! cores and land in disjoint ranges of one buffer.
//!
//! - `raw`: the bytes as they are (loopback).
//! - `lz4`: each block's bytes, LZ4.
//! - `shuffle-lz4`: each block's bytes split into planes (byte 0 of every element, then byte 1,
//!   ..), then LZ4: smooth floats compress where plain LZ4 does not.
//! - `delta-lz4` (the default): each element minus the one before it (wrapping, the block's
//!   first against zero), shuffled, then LZ4: quantised and integer fields.
//! - `delta-deflate`: the whole output delta-coded and shuffled as one stream, then raw
//!   DEFLATE, the kernel wire's tile form (the best ratio, one core).
//!
//! A block-coded payload: `u32 blocks`, `u32 bytes` per block, then the blocks' LZ4 data.

use std::sync::atomic::{AtomicUsize, Ordering};

/// Elements per block (256 KiB of words).
pub const BLOCK: usize = 1 << 16;
/// Blocks per worker below which a payload codes on the calling thread.
const BLOCKS_PER_WORKER: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Raw,
    Lz4,
    ShuffleLz4,
    DeltaLz4,
    DeltaDeflate,
}

impl Codec {
    pub fn from_name(name: &str) -> Option<Codec> {
        Some(match name {
            "raw" => Codec::Raw,
            "lz4" => Codec::Lz4,
            "shuffle-lz4" => Codec::ShuffleLz4,
            "delta-lz4" => Codec::DeltaLz4,
            "delta-deflate" => Codec::DeltaDeflate,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Codec::Raw => "raw",
            Codec::Lz4 => "lz4",
            Codec::ShuffleLz4 => "shuffle-lz4",
            Codec::DeltaLz4 => "delta-lz4",
            Codec::DeltaDeflate => "delta-deflate",
        }
    }

    pub fn id(self) -> u8 {
        match self {
            Codec::Raw => 0,
            Codec::Lz4 => 1,
            Codec::ShuffleLz4 => 2,
            Codec::DeltaLz4 => 3,
            Codec::DeltaDeflate => 4,
        }
    }

    pub fn from_id(id: u8) -> Option<Codec> {
        Some(match id {
            0 => Codec::Raw,
            1 => Codec::Lz4,
            2 => Codec::ShuffleLz4,
            3 => Codec::DeltaLz4,
            4 => Codec::DeltaDeflate,
            _ => return None,
        })
    }

    fn delta(self) -> bool {
        matches!(self, Codec::DeltaLz4 | Codec::DeltaDeflate)
    }

    fn shuffle(self) -> bool {
        matches!(self, Codec::ShuffleLz4 | Codec::DeltaLz4 | Codec::DeltaDeflate)
    }
}

/// `bytes` (whole elements of `width` bytes) coded with `codec` on up to `threads` cores.
pub fn encode(bytes: &[u8], width: usize, codec: Codec, threads: usize) -> Vec<u8> {
    debug_assert!(matches!(width, 1 | 2 | 4) && bytes.len() % width == 0);
    match codec {
        Codec::Raw => bytes.to_vec(),
        Codec::DeltaDeflate => {
            let mut planes = vec![0u8; bytes.len()];
            transform(bytes, &mut planes, width, true, true);
            makepad_fast_inflate::deflate_compress(&planes, 1)
        }
        _ => {
            let span = BLOCK * width;
            let blocks = bytes.len().div_ceil(span);
            let coded: Vec<Vec<u8>> = parallel(blocks, threads, |b| {
                let src = &bytes[b * span..((b + 1) * span).min(bytes.len())];
                let mut planes = vec![0u8; src.len()];
                transform(src, &mut planes, width, codec.delta(), codec.shuffle());
                // An input this size always fits LZ4's bound.
                makepad_lz4::compress_default(&planes).unwrap_or_default()
            });
            let mut out = Vec::with_capacity(4 + 4 * blocks + coded.iter().map(Vec::len).sum::<usize>());
            out.extend_from_slice(&(blocks as u32).to_le_bytes());
            for c in &coded {
                out.extend_from_slice(&(c.len() as u32).to_le_bytes());
            }
            for c in &coded {
                out.extend_from_slice(c);
            }
            out
        }
    }
}

/// `out` (exactly the elements' bytes) back from `payload`, on up to `threads` cores.
pub fn decode(payload: &[u8], width: usize, codec: Codec, out: &mut [u8], threads: usize) -> Result<(), String> {
    if !matches!(width, 1 | 2 | 4) || out.len() % width != 0 {
        return Err(format!("{}: elements of {width} bytes", codec.name()));
    }
    match codec {
        Codec::Raw => {
            if payload.len() != out.len() {
                return Err(format!("raw: {} bytes for {}", payload.len(), out.len()));
            }
            out.copy_from_slice(payload);
            Ok(())
        }
        Codec::DeltaDeflate => {
            let mut planes = vec![0u8; out.len()];
            let (_, written) = makepad_fast_inflate::deflate_decompress(payload, &mut planes).map_err(|e| format!("delta-deflate: {e:?}"))?;
            if written != planes.len() {
                return Err(format!("delta-deflate: {written} bytes for {}", planes.len()));
            }
            untransform(&planes, out, width, true, true);
            Ok(())
        }
        _ => {
            let span = BLOCK * width;
            let blocks = out.len().div_ceil(span);
            let mut r = Reader { bytes: payload, at: 0 };
            if r.u32()? as usize != blocks {
                return Err(format!("{}: block count for {} bytes", codec.name(), out.len()));
            }
            let mut ranges = Vec::with_capacity(blocks);
            let mut at = 4 + 4 * blocks;
            for _ in 0..blocks {
                let n = r.u32()? as usize;
                let end = at.checked_add(n).filter(|e| *e <= payload.len()).ok_or_else(|| format!("{}: a block past the payload", codec.name()))?;
                ranges.push(at..end);
                at = end;
            }
            if at != payload.len() {
                return Err(format!("{}: bytes after the last block", codec.name()));
            }
            let chunks: Vec<&mut [u8]> = out.chunks_mut(span).collect();
            let slots: Vec<std::sync::Mutex<Option<&mut [u8]>>> = chunks.into_iter().map(|c| std::sync::Mutex::new(Some(c))).collect();
            let results = parallel(blocks, threads, |b| {
                let dst = slots[b].lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_default();
                let mut planes = vec![0u8; dst.len()];
                match makepad_lz4::decompress_safe(&payload[ranges[b].clone()], &mut planes) {
                    Ok(n) if n == planes.len() => {
                        untransform(&planes, dst, width, codec.delta(), codec.shuffle());
                        Ok(())
                    }
                    Ok(n) => Err(format!("{}: block {b} gave {n} bytes of {}", codec.name(), planes.len())),
                    Err(e) => Err(format!("{}: block {b}: {e:?}", codec.name())),
                }
            });
            results.into_iter().collect()
        }
    }
}

/// Elements (little-endian, `width` bytes) to their deltas and planes.
fn transform(src: &[u8], dst: &mut [u8], width: usize, delta: bool, shuffle: bool) {
    let n = src.len() / width;
    let mut prev = 0u32;
    for i in 0..n {
        let mut v = 0u32;
        for k in 0..width {
            v |= (src[i * width + k] as u32) << (8 * k);
        }
        let d = if delta { v.wrapping_sub(prev) } else { v };
        prev = v;
        for k in 0..width {
            let byte = (d >> (8 * k)) as u8;
            if shuffle {
                dst[k * n + i] = byte;
            } else {
                dst[i * width + k] = byte;
            }
        }
    }
}

fn untransform(src: &[u8], dst: &mut [u8], width: usize, delta: bool, shuffle: bool) {
    let n = src.len() / width;
    let mask = if width == 4 { u32::MAX } else { (1u32 << (8 * width)) - 1 };
    let mut prev = 0u32;
    for i in 0..n {
        let mut d = 0u32;
        for k in 0..width {
            let byte = if shuffle { src[k * n + i] } else { src[i * width + k] };
            d |= (byte as u32) << (8 * k);
        }
        let v = if delta { prev.wrapping_add(d) & mask } else { d };
        prev = v;
        for k in 0..width {
            dst[i * width + k] = (v >> (8 * k)) as u8;
        }
    }
}

/// `f(0..n)` on up to `threads` scoped workers, in index order.
fn parallel<R: Send>(n: usize, threads: usize, f: impl Fn(usize) -> R + Sync) -> Vec<R> {
    let workers = threads.min(n / BLOCKS_PER_WORKER).max(1);
    if workers == 1 {
        return (0..n).map(&f).collect();
    }
    let next = AtomicUsize::new(0);
    let slots: Vec<std::sync::Mutex<Option<R>>> = (0..n).map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let b = next.fetch_add(1, Ordering::Relaxed);
                if b >= n {
                    break;
                }
                let r = f(b);
                *slots[b].lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
            });
        }
    });
    slots.into_iter().map(|s| s.into_inner().unwrap_or_else(|e| e.into_inner()).expect("every block ran")).collect()
}

/// Little-endian reads that fail instead of running past the end.
pub(crate) struct Reader<'a> {
    pub bytes: &'a [u8],
    pub at: usize,
}

impl<'a> Reader<'a> {
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).filter(|e| *e <= self.bytes.len()).ok_or("a frame ends early")?;
        let s = &self.bytes[self.at..end];
        self.at = end;
        Ok(s)
    }
    pub fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    pub fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_bits(self.u32()?))
    }
    pub fn f64(&mut self) -> Result<f64, String> {
        Ok(f64::from_bits(self.u64()?))
    }
    pub fn str(&mut self) -> Result<&'a str, String> {
        let n = self.u32()? as usize;
        std::str::from_utf8(self.take(n)?).map_err(|_| "a frame's text is not UTF-8".to_string())
    }
}
