//! MPEG-1 Layer III (MP3) encoder, written from ISO/IEC 11172-3 beside this
//! repo's own decoder, which is also its round-trip oracle.
//!
//! Constant bitrate (160..320 kbit/s is its intended range; 192 is the
//! default), mono or stereo with per-frame mid/side, long and short blocks
//! chosen by an attack detector, the bit reservoir, and the standard's two
//! nested quantization loops against a compact psychoacoustic model. Sources
//! at rates MPEG-1 cannot carry are resampled to 44.1 or 48 kHz first.
//!
//! The song is cut into chunks of frames that are encoded on all cores: every
//! stage before the reservoir is a pure function of the input samples, and
//! each chunk starts with an empty reservoir, so the output is the same for
//! any thread count. [`id3::id3v2`] makes the tag that goes in front.
//!
//! ```no_run
//! let pcm = vec![0f32; 44_100 * 2]; // one second of interleaved stereo
//! let mp3 = makepad_audio_encode::mp3::encode_mp3(44_100, 2, &pcm, &Default::default()).unwrap();
//! ```

mod analysis;
mod bands;
mod bits;
pub mod id3;
mod psy;
mod quantize;
pub mod resample;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use analysis::{block, Analysis, LINES, SLOTS, SUBBANDS};
use bands::Bands;
use bits::BitWriter;
use quantize::{Granule, Huffman};

const BITRATES: [u32; 15] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320];
/// Frames per independently encoded chunk (about 0.6 s at 44.1 kHz).
const CHUNK_FRAMES: usize = 24;
/// Largest `main_data_begin` (9 bits).
const MAX_RESERVOIR: usize = 511;

#[derive(Clone, Debug)]
pub struct Mp3Options {
    /// One of the MPEG-1 Layer III bitrates, in kbit/s.
    pub bitrate_kbps: u32,
    /// Worker threads; 0 uses every core.
    pub threads: usize,
}

impl Default for Mp3Options {
    fn default() -> Self {
        Self { bitrate_kbps: 192, threads: 0 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mp3Error {
    NoChannels,
    UnsupportedBitrate(u32),
}

impl std::fmt::Display for Mp3Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Mp3Error::NoChannels => write!(f, "no audio channels"),
            Mp3Error::UnsupportedBitrate(b) => write!(f, "{b} kbit/s is not an MPEG-1 Layer III bitrate"),
        }
    }
}

/// The MPEG-1 rate a source at `rate` is coded at.
pub fn coded_rate(rate: u32) -> u32 {
    match rate {
        32_000 | 44_100 | 48_000 => rate,
        r if r % 11_025 == 0 => 44_100,
        _ => 48_000,
    }
}

/// Encode interleaved `pcm` (`channels` values per frame, [-1, 1]) to an MP3
/// elementary stream. More than two channels keep the first two (front left
/// and right); an empty input gives a stream of one silent frame.
pub fn encode_mp3(rate: u32, channels: u16, pcm: &[f32], opts: &Mp3Options) -> Result<Vec<u8>, Mp3Error> {
    if channels == 0 {
        return Err(Mp3Error::NoChannels);
    }
    let bitrate_index = BITRATES
        .iter()
        .position(|b| *b == opts.bitrate_kbps && *b != 0)
        .ok_or(Mp3Error::UnsupportedBitrate(opts.bitrate_kbps))?;
    let src_ch = channels as usize;
    let nch = src_ch.min(2);
    let frames_in = pcm.len() / src_ch;
    let coded = coded_rate(rate);
    let planes: Vec<Vec<f32>> = (0..nch)
        .map(|c| {
            let plane: Vec<f32> = (0..frames_in).map(|i| pcm[i * src_ch + c]).collect();
            resample::resample(&plane, rate, coded)
        })
        .collect();
    let samples = planes[0].len();
    // Enough granules to flush the filterbank delay out of the decoder too.
    let granules = (samples + 1152 + LINES - 1) / LINES;
    let frames = (granules + 1) / 2;
    let enc = Encoder {
        analysis: Analysis::new(),
        huffman: Huffman::new(),
        long: Bands::new(coded, false),
        short: Bands::new(coded, true),
        planes,
        rate: coded,
        bitrate_index,
        side_bytes: if nch == 1 { 17 } else { 32 },
    };
    let chunks = frames.div_ceil(CHUNK_FRAMES);
    let threads = if opts.threads == 0 { std::thread::available_parallelism().map_or(1, |n| n.get()) } else { opts.threads };
    let threads = threads.clamp(1, chunks.max(1));
    let next = AtomicUsize::new(0);
    let done: Mutex<Vec<(usize, Vec<u8>)>> = Mutex::new(Vec::with_capacity(chunks));
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let c = next.fetch_add(1, Ordering::Relaxed);
                if c >= chunks {
                    break;
                }
                let f0 = c * CHUNK_FRAMES;
                let f1 = (f0 + CHUNK_FRAMES).min(frames);
                let bytes = enc.chunk(f0, f1);
                done.lock().unwrap().push((c, bytes));
            });
        }
    });
    let mut done = done.into_inner().unwrap();
    done.sort_by_key(|(c, _)| *c);
    let mut out = Vec::with_capacity(done.iter().map(|(_, b)| b.len()).sum());
    for (_, b) in done {
        out.extend_from_slice(&b);
    }
    Ok(out)
}

/// [`encode_mp3`] with an ID3v2 tag in front.
pub fn encode_mp3_tagged(rate: u32, channels: u16, pcm: &[f32], tags: &id3::Id3Tags, opts: &Mp3Options) -> Result<Vec<u8>, Mp3Error> {
    let stream = encode_mp3(rate, channels, pcm, opts)?;
    let mut out = id3::id3v2(tags);
    out.extend_from_slice(&stream);
    Ok(out)
}

struct Encoder {
    analysis: Analysis,
    huffman: Huffman,
    long: Bands,
    short: Bands,
    planes: Vec<Vec<f32>>,
    rate: u32,
    bitrate_index: usize,
    side_bytes: usize,
}

type Subbands = [[f32; SUBBANDS]; SLOTS];

impl Encoder {
    fn nch(&self) -> usize {
        self.planes.len()
    }

    /// Bytes of frame `f`, padding included: the running total of the exact
    /// frame length 144 * bitrate / rate.
    fn frame_bytes(&self, f: usize) -> (usize, bool) {
        let num = 144 * BITRATES[self.bitrate_index] as u64 * 1000;
        let den = self.rate as u64;
        let base = (num / den) as usize;
        let size = (((f as u64 + 1) * num / den) - (f as u64 * num / den)) as usize;
        (size, size > base)
    }

    fn attack(&self, g: i64) -> bool {
        g >= 0 && self.planes.iter().any(|p| psy::attack(p, g))
    }

    /// The window sequence: short around attacks, start/stop windows into
    /// and out of them. A pure function of the input, so chunks agree.
    fn block_type(&self, g: i64) -> u8 {
        let short = |g: i64| self.attack(g) || (self.attack(g - 1) && self.attack(g + 1));
        if short(g) {
            block::SHORT
        } else if short(g + 1) {
            block::START
        } else if g > 0 && short(g - 1) {
            block::STOP
        } else {
            block::NORMAL
        }
    }

    fn bands(&self, bt: u8) -> &Bands {
        if bt == block::SHORT {
            &self.short
        } else {
            &self.long
        }
    }

    fn chunk(&self, f0: usize, f1: usize) -> Vec<u8> {
        let nch = self.nch();
        let sfb_short = &makepad_audio_decode::mp3::tables::SFB_SHORT_MPEG1[bands::rate_index(self.rate)];
        let mut prev: Vec<Box<Subbands>> = (0..nch).map(|_| Box::new([[0.0f32; SUBBANDS]; SLOTS])).collect();
        for (c, p) in prev.iter_mut().enumerate() {
            self.analysis.granule_subbands(&self.planes[c], 2 * f0 as i64 - 1, p);
        }
        let mut cur: Box<Subbands> = Box::new([[0.0f32; SUBBANDS]; SLOTS]);
        let mut reservoir = 0usize; // bytes
        let mut avg_pe = 0.0f32;
        let mut frames: Vec<(Vec<u8>, Vec<u8>, usize, usize)> = Vec::with_capacity(f1 - f0);
        for f in f0..f1 {
            let (size, padding) = self.frame_bytes(f);
            let slot = size - 4 - self.side_bytes;
            // spectra of both granules
            let mut bt = [0u8; 2];
            let mut xr = vec![[[0.0f32; LINES]; 2]; 2]; // [gr][ch]
            let mut tmp = [0.0f32; LINES];
            for gr in 0..2 {
                let g = (2 * f + gr) as i64;
                bt[gr] = self.block_type(g);
                for c in 0..nch {
                    self.analysis.granule_subbands(&self.planes[c], g, &mut cur);
                    self.analysis.mdct(&prev[c], &cur, bt[gr], &mut tmp);
                    if bt[gr] == block::SHORT {
                        analysis::short_to_bitstream(sfb_short, &tmp, &mut xr[gr][c]);
                    } else {
                        xr[gr][c] = tmp;
                    }
                    std::mem::swap(&mut prev[c], &mut cur);
                }
            }
            // mid/side when the channels are alike enough to gain from it
            let mut ms = false;
            if nch == 2 {
                let (mut em, mut es) = (0.0f64, 0.0f64);
                for gr in 0..2 {
                    for i in 0..LINES {
                        let (l, r) = (xr[gr][0][i] as f64, xr[gr][1][i] as f64);
                        em += (l + r) * (l + r);
                        es += (l - r) * (l - r);
                    }
                }
                ms = es < 0.5 * em;
                if ms {
                    let k = std::f32::consts::FRAC_1_SQRT_2;
                    for gr in xr.iter_mut() {
                        for i in 0..LINES {
                            let (l, r) = (gr[0][i], gr[1][i]);
                            gr[0][i] = (l + r) * k;
                            gr[1][i] = (l - r) * k;
                        }
                    }
                }
            }
            // allocation against the reservoir, granule by granule
            let mean = slot * 8 / 2;
            let mut free = reservoir as i64 * 8;
            let mut coded: Vec<Granule> = Vec::with_capacity(2 * nch);
            for gr in 0..2 {
                let bands = self.bands(bt[gr]);
                let masks: Vec<psy::Masking> = (0..nch).map(|c| psy::masking(&xr[gr][c], bands, self.rate)).collect();
                let pe: f32 = masks.iter().map(|m| m.pe).sum();
                avg_pe = if avg_pe == 0.0 { pe } else { 0.95 * avg_pe + 0.05 * pe };
                let draw = if bt[gr] == block::SHORT {
                    0.6
                } else if pe > 1.3 * avg_pe {
                    0.3
                } else {
                    0.1
                };
                let target = mean as i64 + (free as f32 * draw) as i64;
                let mut left = target;
                for c in 0..nch {
                    let share = if c + 1 == nch {
                        left
                    } else {
                        let w = (masks[c].pe + 1.0) / (pe + nch as f32);
                        ((target as f32 * w.clamp(0.25, 0.75)) as i64).min(left)
                    };
                    let g = self.huffman.encode(&xr[gr][c], bands, bt[gr], &masks[c].allowed, share.max(0) as u32);
                    left -= g.part2_3_length as i64;
                    coded.push(g);
                }
                free += mean as i64 - (target - left);
            }
            let used: usize = coded.iter().map(|g| g.part2_3_length as usize).sum();
            let data_bytes = used.div_ceil(8);
            debug_assert!(data_bytes <= reservoir + slot);
            // header + side info
            let mut w = BitWriter::new();
            w.put(0xfff, 12);
            w.put(1, 1); // MPEG-1
            w.put(1, 2); // Layer III
            w.put(1, 1); // no CRC
            w.put(self.bitrate_index as u32, 4);
            w.put(bands::rate_index(self.rate) as u32, 2);
            w.put(padding as u32, 1);
            w.put(0, 1);
            w.put(if nch == 1 { 3 } else { 1 }, 2); // mono / joint stereo
            w.put(if ms { 2 } else { 0 }, 2);
            w.put(0, 1);
            w.put(1, 1); // original
            w.put(0, 2);
            w.put(reservoir as u32, 9);
            w.put(0, if nch == 1 { 5 } else { 3 });
            w.put(0, 4 * nch as u32); // scfsi
            for g in &coded {
                let c = &g.coding;
                w.put(g.part2_3_length, 12);
                w.put(c.big_values, 9);
                w.put(g.global_gain, 8);
                w.put(g.scalefac_compress, 4);
                if g.block_type != block::NORMAL {
                    w.put(1, 1);
                    w.put(g.block_type as u32, 2);
                    w.put(0, 1); // not mixed
                    w.put(c.table_select[0] as u32, 5);
                    w.put(c.table_select[1] as u32, 5);
                    w.put(0, 9); // subblock gains
                } else {
                    w.put(0, 1);
                    for t in c.table_select {
                        w.put(t as u32, 5);
                    }
                    w.put(c.region0_count as u32, 4);
                    w.put(c.region1_count as u32, 3);
                }
                w.put(0, 1); // preflag
                w.put(0, 1); // scalefac_scale
                w.put(c.count1_select as u32, 1);
            }
            let head = w.finish();
            debug_assert_eq!(head.len(), 4 + self.side_bytes);
            let mut m = BitWriter::new();
            for (i, g) in coded.iter().enumerate() {
                self.huffman.write(g, self.bands(g.block_type), &mut m);
                let _ = i;
            }
            let main = m.finish();
            frames.push((head, main, reservoir, size));
            reservoir = (reservoir + slot - data_bytes).min(MAX_RESERVOIR);
        }
        // Lay the main data into the slots: frame f's data starts
        // `main_data_begin` bytes before its own slot, in earlier frames' slots.
        let total: usize = frames.iter().map(|(_, _, _, size)| size).sum();
        let mut out = vec![0u8; total];
        let mut slot_map: Vec<usize> = Vec::with_capacity(total);
        let mut slot_starts = Vec::with_capacity(frames.len());
        let mut at = 0usize;
        for (head, _, _, size) in &frames {
            out[at..at + head.len()].copy_from_slice(head);
            slot_starts.push(slot_map.len());
            slot_map.extend(at + head.len()..at + size);
            at += size;
        }
        for (f, (_, main, begin, _)) in frames.iter().enumerate() {
            let start = slot_starts[f] - begin;
            for (k, byte) in main.iter().enumerate() {
                out[slot_map[start + k]] = *byte;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests;
