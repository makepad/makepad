//! The encode pipeline: two passes over independent 1024-sample blocks, then
//! serial pagination.
//!
//! Pass one (parallel over block ranges): window, forward MDCT, masking
//! floor, floor fit, residue quantize and classify — plus symbol histograms.
//! Between passes the per-file Huffman books are built from the merged
//! histograms. Pass two (parallel again): each block's packet is bit-packed
//! independently, because with a single mode every packet stands alone once
//! the books are fixed. Pagination is the only serial stage and is a copy.
//!
//! Output is deterministic for a given input and options — thread count only
//! moves slab boundaries, histogram totals are order-free sums, and every
//! per-block decision is local — so re-encoding the same PCM yields the same
//! bytes and the same content address.

use crate::bits::BitWriter;
use crate::floor_enc::FloorFitter;
use crate::ogg::OggWriter;
use crate::psy::{Psy, PsyTuning};
use crate::setup::{
    comment_packet, ident_packet, setup_packet, BookSet, Histograms, BLOCK,
    CLASSWORDS, CLASS_DIM, CLASS_R, FLOOR_POINTS, FLOOR_Y_BITS, HALF, HOP, N_CLASSES, PARTITION,
    PARTS, Q_LIMIT,
};
use makepad_audio_decode::vorbis::mdct::window;

#[derive(Clone, Debug)]
pub struct EncodeOptions {
    /// 0.0 (smallest) to 1.0 (finest). The default (0.85) lands around
    /// 250-300 kbit/s on typical 44.1 kHz stereo music (dense full-band 48 kHz
    /// material runs higher) at a round-trip SNR in the mid-30s dB.
    pub quality: f32,
    /// Worker threads; 0 means all available cores.
    pub threads: usize,
    /// Vorbis comments (`KEY`, `value`) for the comment header.
    pub tags: Vec<(String, String)>,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        EncodeOptions { quality: 0.85, threads: 0, tags: Vec::new() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncodeError {
    /// Argument shape errors; the message names the field.
    BadArgs(&'static str),
}

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EncodeError::BadArgs(what) => write!(f, "bad encode arguments: {what}"),
        }
    }
}

impl std::error::Error for EncodeError {}

/// Encode interleaved f32 PCM to a complete Ogg Vorbis stream.
pub fn encode_vorbis(
    rate: u32,
    channels: u16,
    pcm: &[f32],
    opts: &EncodeOptions,
) -> Result<Vec<u8>, EncodeError> {
    if channels == 0 || channels > 8 {
        return Err(EncodeError::BadArgs("channel count"));
    }
    if !(8_000..=192_000).contains(&rate) {
        return Err(EncodeError::BadArgs("sample rate"));
    }
    let ch = channels as usize;
    if pcm.is_empty() || pcm.len() % ch != 0 {
        return Err(EncodeError::BadArgs("pcm length"));
    }
    let frames = pcm.len() / ch;
    let blocks = frames.div_ceil(HOP) + 1;

    // The one quality lever, expanded into the psy model's four dB anchors.
    let tuning = PsyTuning::from_quality(opts.quality);

    let threads = if opts.threads == 0 {
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
    } else {
        opts.threads
    };
    // Below a few hundred blocks the spawn overhead beats the win.
    let threads = threads.clamp(1, blocks.div_ceil(64).max(1));

    let ranges = split_ranges(blocks, threads);

    // -- pass one: analyse + quantize + histogram --
    let slabs: Vec<Slab> = if threads == 1 {
        vec![pass1(pcm, frames, ch, rate, tuning, ranges[0].clone())]
    } else {
        std::thread::scope(|scope| {
            let handles: Vec<_> = ranges
                .iter()
                .map(|range| {
                    let range = range.clone();
                    scope.spawn(move || pass1(pcm, frames, ch, rate, tuning, range))
                })
                .collect();
            handles.into_iter().map(|h| h.join().expect("pass1 worker")).collect()
        })
    };

    // -- books from the merged histograms --
    let mut hist = Histograms::new();
    for slab in &slabs {
        hist.merge(&slab.hist);
    }
    let books = BookSet::build(&hist);

    // -- pass two: pack packets --
    let packets: Vec<Vec<Vec<u8>>> = if threads == 1 {
        vec![pass2(&books, ch, &slabs[0])]
    } else {
        std::thread::scope(|scope| {
            let books = &books;
            let handles: Vec<_> =
                slabs.iter().map(|slab| scope.spawn(move || pass2(books, ch, slab))).collect();
            handles.into_iter().map(|h| h.join().expect("pass2 worker")).collect()
        })
    };

    // -- serial: headers and pagination --
    let serial = stream_serial(rate, channels, pcm);
    let mut ogg = OggWriter::new(serial);
    ogg.packet(&ident_packet(rate, channels), 0, false);
    ogg.flush();
    ogg.packet(&comment_packet(&opts.tags), 0, false);
    ogg.packet(&setup_packet(&books, channels), 0, false);
    ogg.flush();
    let mut b = 0usize;
    for slab_packets in &packets {
        for packet in slab_packets {
            let granule = (b * HOP).min(frames) as u64;
            ogg.packet(packet, granule, b + 1 == blocks);
            if b == 0 {
                // The first audio page must carry granule 0 on its own: a
                // page's granule is end-truncated on the final page, and for
                // a short file the first and final page would otherwise
                // coincide, making the decoder's front-trim scan read the
                // truncation as extra encoder delay and shift the whole
                // stream. (libvorbis flushes here for the same reason.)
                ogg.flush();
            }
            b += 1;
        }
    }
    debug_assert_eq!(b, blocks);
    Ok(ogg.finish())
}

/// Deterministic stream serial from the content, so identical input encodes
/// to identical bytes.
fn stream_serial(rate: u32, channels: u16, pcm: &[f32]) -> u32 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut mix = |v: u64| {
        h ^= v;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    mix(rate as u64);
    mix(channels as u64);
    mix(pcm.len() as u64);
    for &v in pcm.iter().take(256) {
        mix(v.to_bits() as u64);
    }
    (h ^ (h >> 32)) as u32
}

fn split_ranges(blocks: usize, threads: usize) -> Vec<std::ops::Range<usize>> {
    let per = blocks.div_ceil(threads);
    (0..threads)
        .map(|t| (t * per).min(blocks)..((t + 1) * per).min(blocks))
        .filter(|r| !r.is_empty())
        .collect()
}

/// One worker's share of pass one: everything pass two needs to pack the
/// packets, plus this range's symbol counts.
struct Slab {
    /// Per (block, channel).
    silent: Vec<bool>,
    /// `FLOOR_POINTS` raw floor values per (block, channel).
    vals: Vec<u8>,
    /// `PARTS` class ids per (block, channel).
    cls: Vec<u8>,
    /// `HALF` quantized residues per (block, channel).
    q: Vec<i16>,
    hist: Histograms,
    blocks: usize,
}

fn pass1(
    pcm: &[f32],
    frames: usize,
    ch: usize,
    rate: u32,
    tuning: PsyTuning,
    range: std::ops::Range<usize>,
) -> Slab {
    let count = range.len();
    let mut slab = Slab {
        silent: vec![false; count * ch],
        vals: vec![0; count * ch * FLOOR_POINTS],
        cls: vec![0; count * ch * PARTS],
        q: vec![0; count * ch * HALF],
        hist: Histograms::new(),
        blocks: count,
    };
    let win = window(BLOCK);
    let mut mdct = crate::mdct::MdctFwd::new(BLOCK);
    let mut psy = Psy::new(rate);
    let fitter = FloorFitter::new();
    let tables = tuning.tables();
    let cutoff = psy.cutoff();
    // Class of a partition from its max |q|: lookup by magnitude.
    let class_of = {
        let mut table = [0u8; Q_LIMIT as usize + 1];
        for (m, slot) in table.iter_mut().enumerate() {
            *slot = CLASS_R.iter().position(|&r| r >= m as i32).unwrap() as u8;
        }
        table
    };

    let mut time = vec![0f32; BLOCK];
    let mut spec = vec![0f32; HALF];
    let mut curve = vec![0f32; HALF];
    let mut desired = [0i32; FLOOR_POINTS];
    let mut vals_i32 = [0i32; FLOOR_POINTS];

    let mut t_mdct = 0f64;
    let mut t_psy = 0f64;
    let mut t_quant = 0f64;
    let mut t_gather = 0f64;
    #[cfg(not(target_arch = "wasm32"))]
    let timing = std::env::var("OGGENC_PHASES").is_ok();
    #[cfg(target_arch = "wasm32")]
    let timing = false;
    for (bi, b) in range.enumerate() {
        for c in 0..ch {
            #[cfg(not(target_arch = "wasm32"))]
            let t0 = if timing { Some(std::time::Instant::now()) } else { None };
            // Windowed block: block `b` covers input frames
            // [b*HOP - HOP, b*HOP + HOP), zero-padded outside the input.
            let base = b as isize * HOP as isize - HOP as isize;
            let lo = (-base).clamp(0, BLOCK as isize) as usize;
            let hi = ((frames as isize - base).clamp(0, BLOCK as isize)) as usize;
            time[..lo].fill(0.0);
            time[hi..].fill(0.0);
            let start = (base + lo as isize) as usize;
            for i in lo..hi {
                time[i] = pcm[(start + i - lo) * ch + c] * win[i];
            }

            #[cfg(not(target_arch = "wasm32"))]
            if let Some(t) = t0 { t_gather += t.elapsed().as_secs_f64(); }
            #[cfg(not(target_arch = "wasm32"))]
            let t0 = if timing { Some(std::time::Instant::now()) } else { None };
            mdct.mdct(&time, &mut spec);
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(t) = t0 { t_mdct += t.elapsed().as_secs_f64(); }
            #[cfg(not(target_arch = "wasm32"))]
            let t0 = if timing { Some(std::time::Instant::now()) } else { None };
            psy.desired_floor(&spec, &tables, &mut desired);
            fitter.encode(&desired, &mut vals_i32);
            fitter.synthesize(&vals_i32, &mut curve);
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(t) = t0 { t_psy += t.elapsed().as_secs_f64(); }
            #[cfg(not(target_arch = "wasm32"))]
            let t0 = if timing { Some(std::time::Instant::now()) } else { None };

            let at = bi * ch + c;
            let q = &mut slab.q[at * HALF..(at + 1) * HALF];
            let mut any = false;
            for k in 0..cutoff {
                let r = spec[k] / curve[k];
                let qv = (r.round() as i32).clamp(-Q_LIMIT, Q_LIMIT);
                q[k] = qv as i16;
                any |= qv != 0;
            }
            q[cutoff..].fill(0);

            if !any {
                slab.silent[at] = true;
                continue;
            }
            let vals = &mut slab.vals[at * FLOOR_POINTS..(at + 1) * FLOOR_POINTS];
            for (dst, &v) in vals.iter_mut().zip(vals_i32.iter()) {
                debug_assert!((0..256).contains(&v));
                *dst = v as u8;
            }
            for &v in &vals[2..] {
                slab.hist.floor[v as usize] += 1;
            }
            let cls = &mut slab.cls[at * PARTS..(at + 1) * PARTS];
            for p in 0..PARTS {
                let part = &q[p * PARTITION..(p + 1) * PARTITION];
                let max = part.iter().map(|&v| (v as i32).unsigned_abs()).max().unwrap_or(0);
                cls[p] = class_of[max as usize];
            }
            for pair in cls.chunks_exact(CLASSWORDS) {
                let sym = pair[0] as usize * N_CLASSES + pair[1] as usize;
                slab.hist.class[sym] += 1;
            }
            for p in 0..PARTS {
                let c = cls[p] as usize;
                if c == 0 {
                    continue;
                }
                let part = &q[p * PARTITION..(p + 1) * PARTITION];
                for chunk in part.chunks_exact(CLASS_DIM[c]) {
                    slab.hist.res[c - 1][BookSet::res_symbol(c, chunk)] += 1;
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(t) = t0 { t_quant += t.elapsed().as_secs_f64(); }
        }
    }
    if timing {
        eprintln!("phases: gather {t_gather:.3}s mdct {t_mdct:.3}s psy+floor {t_psy:.3}s quant+hist {t_quant:.3}s");
    }
    slab
}

/// Pack one slab's packets. The bit layout mirrors the decoder's
/// `decode_spectra` walk exactly: floors per channel, then the residue's
/// interleaved classword/partition order.
fn pass2(books: &BookSet, ch: usize, slab: &Slab) -> Vec<Vec<u8>> {
    let mut out = Vec::with_capacity(slab.blocks);
    for bi in 0..slab.blocks {
        let mut w = BitWriter::with_capacity(512);
        w.push(0, 1); // audio packet
        // Single mode: zero mode bits, blockflag 0, no window flags.

        let at0 = bi * ch;
        for c in 0..ch {
            let at = at0 + c;
            if slab.silent[at] {
                w.push(0, 1);
                continue;
            }
            w.push(1, 1);
            let vals = &slab.vals[at * FLOOR_POINTS..(at + 1) * FLOOR_POINTS];
            w.push(vals[0] as u32, FLOOR_Y_BITS);
            w.push(vals[1] as u32, FLOOR_Y_BITS);
            for &v in &vals[2..] {
                let cw = books.floor.codes[v as usize];
                debug_assert!(cw.len > 0, "floor symbol {v} has no codeword");
                w.push(cw.bits, cw.len);
            }
        }

        if !(0..ch).all(|c| slab.silent[at0 + c]) {
            for group in (0..PARTS).step_by(CLASSWORDS) {
                for c in 0..ch {
                    let at = at0 + c;
                    if slab.silent[at] {
                        continue;
                    }
                    let cls = &slab.cls[at * PARTS..(at + 1) * PARTS];
                    let sym = cls[group] as usize * N_CLASSES + cls[group + 1] as usize;
                    let cw = books.class.codes[sym];
                    debug_assert!(cw.len > 0, "class symbol {sym} has no codeword");
                    w.push(cw.bits, cw.len);
                }
                for p in group..group + CLASSWORDS {
                    for c in 0..ch {
                        let at = at0 + c;
                        if slab.silent[at] {
                            continue;
                        }
                        let cls = slab.cls[at * PARTS + p] as usize;
                        if cls == 0 {
                            continue;
                        }
                        let q = &slab.q[at * HALF..(at + 1) * HALF];
                        let part = &q[p * PARTITION..(p + 1) * PARTITION];
                        let book = &books.res[cls - 1];
                        for chunk in part.chunks_exact(CLASS_DIM[cls]) {
                            let sym = BookSet::res_symbol(cls, chunk);
                            let cw = book.codes[sym];
                            debug_assert!(cw.len > 0, "res c{cls} symbol {sym} has no codeword");
                            w.push(cw.bits, cw.len);
                        }
                    }
                }
            }
        }
        out.push(w.finish());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_cover_everything_once() {
        for (blocks, threads) in [(1usize, 1usize), (10, 3), (100, 8), (7, 16), (64, 2)] {
            let ranges = split_ranges(blocks, threads);
            let mut covered = vec![false; blocks];
            for r in &ranges {
                for i in r.clone() {
                    assert!(!covered[i]);
                    covered[i] = true;
                }
            }
            assert!(covered.iter().all(|&c| c), "{blocks} blocks {threads} threads");
        }
    }

    #[test]
    fn bad_arguments_are_refused() {
        let opts = EncodeOptions::default();
        assert!(encode_vorbis(44_100, 0, &[0.0], &opts).is_err());
        assert!(encode_vorbis(44_100, 9, &[0.0; 18], &opts).is_err());
        assert!(encode_vorbis(100, 1, &[0.0], &opts).is_err());
        assert!(encode_vorbis(44_100, 2, &[0.0; 3], &opts).is_err());
        assert!(encode_vorbis(44_100, 1, &[], &opts).is_err());
    }
}
