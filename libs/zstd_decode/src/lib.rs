//! Zstandard decoder written from RFC 8878.
//!
//! `decode` accepts any number of concatenated Zstandard and skippable frames and returns the concatenated
//! content. Supported: raw, RLE and compressed blocks; raw, RLE, Huffman (1 or 4 streams) and treeless
//! literals; predefined, RLE, FSE-compressed and repeat sequence tables; repeat offsets; the content checksum
//! (XXH64, verified when present); the frame content size (verified when present). Dictionaries are not
//! supported: a frame's dictionary id is reported by `read_frame_header`, and decoding fails only when the
//! frame actually references data before its start.

mod bits;
mod fse;
mod huffman;
mod xxh64;

#[cfg(test)]
mod tests;

pub use xxh64::xxh64;

use bits::BackReader;
use fse::FseTable;
use huffman::HufTable;

const MAGIC: u32 = 0xFD2F_B528;
const SKIPPABLE_MASK: u32 = 0xFFFF_FFF0;
const SKIPPABLE_MAGIC: u32 = 0x184D_2A50;
const BLOCK_MAX: usize = 128 * 1024;
/// Largest window this decoder accepts (window log 41 is the format's limit).
const WINDOW_LOG_MAX: u32 = 41;

/// A decoded frame header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameHeader {
    pub window_size: u64,
    /// 0 = none. Dictionaries are not supported; this is reported only.
    pub dictionary_id: u32,
    pub content_size: Option<u64>,
    pub single_segment: bool,
    pub has_checksum: bool,
}

#[inline]
fn le(src: &[u8], n: usize) -> u64 {
    src[..n].iter().rev().fold(0u64, |a, &b| (a << 8) | b as u64)
}

/// Parses a frame header (after the 4-byte magic). Returns the header and its size in bytes.
pub fn read_frame_header(src: &[u8]) -> Result<(FrameHeader, usize), String> {
    let fhd = *src.first().ok_or("truncated frame header")?;
    let fcs_flag = fhd >> 6;
    let single_segment = fhd & 0x20 != 0;
    if fhd & 0x08 != 0 {
        return Err("reserved frame header bit is set".into());
    }
    let has_checksum = fhd & 0x04 != 0;
    let did_size = [0usize, 1, 2, 4][(fhd & 3) as usize];
    let fcs_size = match fcs_flag {
        0 => single_segment as usize,
        1 => 2,
        2 => 4,
        _ => 8,
    };
    let wd_size = (!single_segment) as usize;
    let total = 1 + wd_size + did_size + fcs_size;
    if src.len() < total {
        return Err("truncated frame header".into());
    }
    let mut p = 1;
    let mut window_size = 0u64;
    if !single_segment {
        let wd = src[p];
        let log = 10 + (wd >> 3) as u32;
        if log > WINDOW_LOG_MAX {
            return Err(format!("window log {} is too large", log));
        }
        let base = 1u64 << log;
        window_size = base + (base / 8) * (wd & 7) as u64;
        p += 1;
    }
    let dictionary_id = le(&src[p..], did_size) as u32;
    p += did_size;
    let content_size = match fcs_size {
        0 => None,
        2 => Some(le(&src[p..], 2) + 256),
        n => Some(le(&src[p..], n)),
    };
    if single_segment {
        window_size = content_size.unwrap_or(0);
    }
    Ok((FrameHeader { window_size, dictionary_id, content_size, single_segment, has_checksum }, total))
}

/// Decodes every frame in `input`; fails if the output would exceed `max` bytes.
pub fn decode(input: &[u8], max: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut p = 0usize;
    let mut frames = 0usize;
    while p < input.len() {
        let magic = input.get(p..p + 4).ok_or("truncated frame magic")?;
        let magic = u32::from_le_bytes(magic.try_into().unwrap());
        p += 4;
        if magic & SKIPPABLE_MASK == SKIPPABLE_MAGIC {
            let size = input.get(p..p + 4).ok_or("truncated skippable frame")?;
            let size = u32::from_le_bytes(size.try_into().unwrap()) as usize;
            p += 4;
            if input.len() - p < size {
                return Err("truncated skippable frame".into());
            }
            p += size;
        } else if magic == MAGIC {
            p += decode_frame(&input[p..], max, &mut out).map_err(|e| format!("frame {}: {}", frames, e))?;
        } else {
            return Err(format!("unknown frame magic {:08x} at byte {}", magic, p - 4));
        }
        frames += 1;
    }
    if frames == 0 {
        return Err("no frames".into());
    }
    Ok(out)
}

/// Per-frame decoding state that persists from block to block.
struct Frame {
    start: usize,
    window: u64,
    dictionary_id: u32,
    huf: Option<HufTable>,
    ll: Option<FseTable>,
    of: Option<FseTable>,
    ml: Option<FseTable>,
    reps: [usize; 3],
    literals: Vec<u8>,
}

/// Decodes one frame after its magic; returns the bytes it used.
fn decode_frame(src: &[u8], max: usize, out: &mut Vec<u8>) -> Result<usize, String> {
    let (hdr, mut p) = read_frame_header(src)?;
    if let Some(size) = hdr.content_size {
        if size > (max - out.len()) as u64 {
            return Err(format!("frame content size {} exceeds the output limit", size));
        }
        out.reserve(size as usize);
    }
    let mut f = Frame {
        start: out.len(),
        window: hdr.window_size,
        dictionary_id: hdr.dictionary_id,
        huf: None,
        ll: None,
        of: None,
        ml: None,
        reps: [1, 4, 8],
        literals: Vec::new(),
    };
    loop {
        let bh = src.get(p..p + 3).ok_or("truncated block header")?;
        let bh = le(bh, 3) as usize;
        p += 3;
        let last = bh & 1 != 0;
        let size = bh >> 3;
        if size > BLOCK_MAX {
            return Err(format!("block size {} exceeds 128 KiB", size));
        }
        match (bh >> 1) & 3 {
            0 => {
                let body = src.get(p..p + size).ok_or("truncated raw block")?;
                check_room(out, size, max)?;
                out.extend_from_slice(body);
                p += size;
            }
            1 => {
                let b = *src.get(p).ok_or("truncated RLE block")?;
                check_room(out, size, max)?;
                out.resize(out.len() + size, b);
                p += 1;
            }
            2 => {
                let body = src.get(p..p + size).ok_or("truncated compressed block")?;
                decode_block(&mut f, body, max, out)?;
                p += size;
            }
            _ => return Err("reserved block type".into()),
        }
        if last {
            break;
        }
    }
    let produced = (out.len() - f.start) as u64;
    if let Some(size) = hdr.content_size {
        if size != produced {
            return Err(format!("frame decoded to {} bytes but declares {}", produced, size));
        }
    }
    if hdr.has_checksum {
        let stored = src.get(p..p + 4).ok_or("truncated content checksum")?;
        let stored = u32::from_le_bytes(stored.try_into().unwrap());
        let actual = xxh64(&out[f.start..], 0) as u32;
        if stored != actual {
            return Err(format!("content checksum mismatch: stored {:08x}, computed {:08x}", stored, actual));
        }
        p += 4;
    }
    Ok(p)
}

#[inline]
fn check_room(out: &[u8], n: usize, max: usize) -> Result<(), String> {
    if n > max - out.len() {
        Err(format!("output exceeds the limit of {} bytes", max))
    } else {
        Ok(())
    }
}

fn decode_block(f: &mut Frame, src: &[u8], max: usize, out: &mut Vec<u8>) -> Result<(), String> {
    let block_start = out.len();
    let used = decode_literals(f, src)?;
    let used = used + decode_sequences(f, &src[used..], max, out)?;
    debug_assert_eq!(used, src.len());
    if out.len() - block_start > BLOCK_MAX {
        return Err("block decodes to more than 128 KiB".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------
// Literals section (RFC 8878 3.1.1.3.1)

/// Decodes the literals section into `f.literals`; returns the bytes it used.
fn decode_literals(f: &mut Frame, src: &[u8]) -> Result<usize, String> {
    let b0 = *src.first().ok_or("missing literals section")? as usize;
    let ty = b0 & 3;
    let format = (b0 >> 2) & 3;
    f.literals.clear();
    match ty {
        0 | 1 => {
            let (regen, hsize) = match format {
                0 | 2 => (b0 >> 3, 1),
                1 => (le(src.get(..2).ok_or("truncated literals header")?, 2) as usize >> 4, 2),
                _ => (le(src.get(..3).ok_or("truncated literals header")?, 3) as usize >> 4, 3),
            };
            if regen > BLOCK_MAX {
                return Err("literals exceed 128 KiB".into());
            }
            if ty == 0 {
                let body = src.get(hsize..hsize + regen).ok_or("truncated raw literals")?;
                f.literals.extend_from_slice(body);
                Ok(hsize + regen)
            } else {
                let b = *src.get(hsize).ok_or("truncated RLE literals")?;
                f.literals.resize(regen, b);
                Ok(hsize + 1)
            }
        }
        _ => {
            let (hsize, bits) = match format {
                0 | 1 => (3, 10),
                2 => (4, 14),
                _ => (5, 18),
            };
            let streams = if format == 0 { 1 } else { 4 };
            let v = le(src.get(..hsize).ok_or("truncated literals header")?, hsize);
            let mask = (1u64 << bits) - 1;
            let regen = ((v >> 4) & mask) as usize;
            let csize = ((v >> (4 + bits)) & mask) as usize;
            if regen > BLOCK_MAX {
                return Err("literals exceed 128 KiB".into());
            }
            let mut body = src.get(hsize..hsize + csize).ok_or("truncated compressed literals")?;
            if ty == 2 {
                let (table, used) = HufTable::read(body)?;
                body = &body[used..];
                f.huf = Some(table);
            }
            let table = f.huf.as_ref().ok_or("treeless literals without a previous Huffman table")?;
            f.literals.reserve(regen);
            if streams == 1 {
                table.decode_stream(body, regen, &mut f.literals)?;
            } else {
                if body.len() < 6 {
                    return Err("truncated literals jump table".into());
                }
                let s1 = le(body, 2) as usize;
                let s2 = le(&body[2..], 2) as usize;
                let s3 = le(&body[4..], 2) as usize;
                let rest = &body[6..];
                if s1 + s2 + s3 > rest.len() {
                    return Err("literals jump table exceeds the section".into());
                }
                let seg = regen.div_ceil(4);
                if 3 * seg > regen {
                    return Err("too few literals for four streams".into());
                }
                let (a, rest) = rest.split_at(s1);
                let (b, rest) = rest.split_at(s2);
                let (c, d) = rest.split_at(s3);
                table.decode_stream(a, seg, &mut f.literals)?;
                table.decode_stream(b, seg, &mut f.literals)?;
                table.decode_stream(c, seg, &mut f.literals)?;
                table.decode_stream(d, regen - 3 * seg, &mut f.literals)?;
            }
            Ok(hsize + csize)
        }
    }
}

// ---------------------------------------------------------------------------------------------------------
// Sequences section (RFC 8878 3.1.1.3.2)

const LL_MAX_SYMBOL: usize = 35;
const ML_MAX_SYMBOL: usize = 52;
const OF_MAX_SYMBOL: usize = 31;

const LL_DEFAULT: [i16; 36] = [
    4, 3, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 3, 2, 1, 1, 1, 1, 1, -1, -1, -1, -1,
];
const ML_DEFAULT: [i16; 53] = [
    1, 4, 3, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, -1, -1, -1, -1, -1, -1, -1,
];
const OF_DEFAULT: [i16; 29] =
    [1, 1, 1, 1, 1, 1, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, -1, -1, -1, -1, -1];

/// (baseline, extra bits) per literals length code.
const LL_CODES: [(u32, u32); 36] = [
    (0, 0), (1, 0), (2, 0), (3, 0), (4, 0), (5, 0), (6, 0), (7, 0), (8, 0), (9, 0), (10, 0), (11, 0),
    (12, 0), (13, 0), (14, 0), (15, 0), (16, 1), (18, 1), (20, 1), (22, 1), (24, 2), (28, 2), (32, 3),
    (40, 3), (48, 4), (64, 6), (128, 7), (256, 8), (512, 9), (1024, 10), (2048, 11), (4096, 12),
    (8192, 13), (16384, 14), (32768, 15), (65536, 16),
];

/// (baseline, extra bits) per match length code.
const ML_CODES: [(u32, u32); 53] = [
    (3, 0), (4, 0), (5, 0), (6, 0), (7, 0), (8, 0), (9, 0), (10, 0), (11, 0), (12, 0), (13, 0), (14, 0),
    (15, 0), (16, 0), (17, 0), (18, 0), (19, 0), (20, 0), (21, 0), (22, 0), (23, 0), (24, 0), (25, 0),
    (26, 0), (27, 0), (28, 0), (29, 0), (30, 0), (31, 0), (32, 0), (33, 0), (34, 0), (35, 1), (37, 1),
    (39, 1), (41, 1), (43, 2), (47, 2), (51, 3), (59, 3), (67, 4), (83, 4), (99, 5), (131, 7), (259, 8),
    (515, 9), (1027, 10), (2051, 11), (4099, 12), (8195, 13), (16387, 14), (32771, 15), (65539, 16),
];

/// Resolves one table of the sequences section header; returns the bytes it used.
fn read_table(
    mode: u8,
    src: &[u8],
    slot: &mut Option<FseTable>,
    default: &[i16],
    default_log: u32,
    max_symbol: usize,
    max_log: u32,
    name: &str,
) -> Result<usize, String> {
    match mode {
        0 => {
            *slot = Some(FseTable::build(default, default_log)?);
            Ok(0)
        }
        1 => {
            let sym = *src.first().ok_or_else(|| format!("truncated {} RLE symbol", name))?;
            if sym as usize > max_symbol {
                return Err(format!("{} RLE symbol {} out of range", name, sym));
            }
            *slot = Some(FseTable::rle(sym));
            Ok(1)
        }
        2 => {
            let (table, used) = FseTable::read(src, max_symbol, max_log).map_err(|e| format!("{} table: {}", name, e))?;
            *slot = Some(table);
            Ok(used)
        }
        _ => {
            if slot.is_none() {
                return Err(format!("{} repeat mode without a previous table", name));
            }
            Ok(0)
        }
    }
}

/// Decodes and executes the sequences section; returns the bytes it used (all of `src`).
fn decode_sequences(f: &mut Frame, src: &[u8], max: usize, out: &mut Vec<u8>) -> Result<usize, String> {
    let b0 = *src.first().ok_or("missing sequences section")? as usize;
    let (count, mut p) = if b0 < 128 {
        (b0, 1)
    } else if b0 < 255 {
        (((b0 - 128) << 8) + *src.get(1).ok_or("truncated sequence count")? as usize, 2)
    } else {
        (le(src.get(1..3).ok_or("truncated sequence count")?, 2) as usize + 0x7F00, 3)
    };
    if count == 0 {
        if p != src.len() {
            return Err("data after an empty sequences section".into());
        }
        check_room(out, f.literals.len(), max)?;
        out.extend_from_slice(&f.literals);
        return Ok(p);
    }
    let modes = *src.get(p).ok_or("truncated sequence modes")?;
    p += 1;
    if modes & 3 != 0 {
        return Err("reserved sequence mode bits are set".into());
    }
    p += read_table(modes >> 6, &src[p..], &mut f.ll, &LL_DEFAULT, 6, LL_MAX_SYMBOL, 9, "literals length")?;
    p += read_table((modes >> 4) & 3, &src[p..], &mut f.of, &OF_DEFAULT, 5, OF_MAX_SYMBOL, 8, "offset")?;
    p += read_table((modes >> 2) & 3, &src[p..], &mut f.ml, &ML_DEFAULT, 6, ML_MAX_SYMBOL, 9, "match length")?;

    let (ll_t, of_t, ml_t) = (f.ll.as_ref().unwrap(), f.of.as_ref().unwrap(), f.ml.as_ref().unwrap());
    let mut r = BackReader::new(&src[p..])?;
    let mut ll_s = ll_t.init_state(&mut r);
    let mut of_s = of_t.init_state(&mut r);
    let mut ml_s = ml_t.init_state(&mut r);
    let mut lit = 0usize;
    let mut reps = f.reps;
    for i in 0..count {
        let of_code = of_t.entries[of_s].symbol as u32;
        let (ml_base, ml_bits) = ML_CODES[ml_t.entries[ml_s].symbol as usize];
        let (ll_base, ll_bits) = LL_CODES[ll_t.entries[ll_s].symbol as usize];
        let of_value = (1u64 << of_code) + r.read(of_code);
        let ml = (ml_base as u64 + r.read(ml_bits)) as usize;
        let ll = (ll_base as u64 + r.read(ll_bits)) as usize;
        if i + 1 < count {
            ll_s = ll_t.update(ll_s, &mut r);
            ml_s = ml_t.update(ml_s, &mut r);
            of_s = of_t.update(of_s, &mut r);
        }
        if r.overflowed() {
            return Err("sequence bitstream is truncated".into());
        }

        let offset = if of_value > 3 {
            let o = (of_value - 3) as usize;
            reps = [o, reps[0], reps[1]];
            o
        } else {
            let idx = of_value as usize - 1 + (ll == 0) as usize;
            match idx {
                0 => reps[0],
                1 => {
                    let o = reps[1];
                    reps = [o, reps[0], reps[2]];
                    o
                }
                2 => {
                    let o = reps[2];
                    reps = [o, reps[0], reps[1]];
                    o
                }
                _ => {
                    let o = reps[0].wrapping_sub(1);
                    reps = [o, reps[0], reps[1]];
                    o
                }
            }
        };

        let lits = f.literals.get(lit..lit + ll).ok_or("sequences use more literals than decoded")?;
        check_room(out, ll + ml, max)?;
        out.extend_from_slice(lits);
        lit += ll;
        let produced = out.len() - f.start;
        if offset == 0 || offset > produced {
            if f.dictionary_id != 0 {
                return Err(format!("offset {} reaches into dictionary {:08x}, which is not supported", offset, f.dictionary_id));
            }
            return Err(format!("offset {} exceeds the {} bytes decoded so far", offset, produced));
        }
        if offset as u64 > f.window {
            return Err(format!("offset {} exceeds the window size {}", offset, f.window));
        }
        let from = out.len() - offset;
        if offset >= ml {
            out.extend_from_within(from..from + ml);
        } else {
            for k in 0..ml {
                let b = out[from + k];
                out.push(b);
            }
        }
    }
    if r.remaining() != 0 {
        return Err("sequence bitstream has unused bits".into());
    }
    f.reps = reps;
    let rest = &f.literals[lit..];
    check_room(out, rest.len(), max)?;
    out.extend_from_slice(rest);
    Ok(src.len())
}
