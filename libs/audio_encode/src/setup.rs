//! The fixed codec shape and the three header packets.
//!
//! One block size (1024), one mode, one mapping without coupling, floor 1
//! with a fixed 30-point grid, residue type 1 in 32-bin partitions with eight
//! classes graded by amplitude. Everything a packet needs is decided here;
//! only the Huffman code lengths vary per file (built from the file's own
//! symbol counts in pass one).
//!
//! The floor configuration is parsed back through the *decoder's*
//! `Floor::read` at construction, so the curve the encoder quantizes against
//! is synthesized by the identical integer line renderer the decoder will
//! run — the two cannot drift.

use crate::bits::BitWriter;
use crate::huffman::HuffBook;
use makepad_audio_decode::vorbis::bits::BitReader;
use makepad_audio_decode::vorbis::floor::{Floor, Floor1};

/// Time-domain block size; every packet codes one of these.
pub const BLOCK: usize = 1024;
/// Spectral coefficients per block.
pub const HALF: usize = BLOCK / 2;
/// Hop between block starts; also the samples one packet contributes.
pub const HOP: usize = HALF;
/// Residue partition width in bins.
pub const PARTITION: usize = 32;
/// Partitions per channel per block.
pub const PARTS: usize = HALF / PARTITION;
/// Partitions classified per classbook codeword.
pub const CLASSWORDS: usize = 2;
/// Number of residue classes.
pub const N_CLASSES: usize = 10;
/// Floor points: two implicit ends plus the interior grid.
pub const FLOOR_POINTS: usize = 2 + FLOOR_INTERIOR.len();
/// Floor Y range (multiplier 2).
pub const FLOOR_RANGE: i32 = 128;
/// Bits per raw floor Y value.
pub const FLOOR_Y_BITS: u32 = 7;

/// Interior floor X positions (bins), log-spaced denser at the low end.
/// In list order after the implicit `0` and `512`; ascending, all unique.
pub const FLOOR_INTERIOR: [u32; 28] = [
    2, 4, 6, 8, 10, 13, 16, 20, 24, 29, 35, 42, 50, 60, 72, 86, 102, 121, 143, 169, 200, 236,
    278, 328, 386, 425, 456, 490,
];

/// Largest |quantized residue| each class covers; class 0 is the free
/// all-zero class (no codebook, no bits beyond its classword share).
pub const CLASS_R: [i32; N_CLASSES] = [0, 1, 2, 4, 8, 16, 32, 64, 128, 256];
/// VQ dimension of each class's codebook (class 0 has none).
pub const CLASS_DIM: [usize; N_CLASSES] = [0, 4, 4, 2, 2, 2, 2, 1, 1, 1];
/// The hard ceiling quantized residues are clamped to.
pub const Q_LIMIT: i32 = CLASS_R[N_CLASSES - 1];

/// Codebook indices inside the setup header.
pub const BOOK_FLOOR: usize = 0;
pub const BOOK_CLASS: usize = 1;
/// Residue class `c` (1-based) uses codebook `BOOK_RES0 + c - 1`.
pub const BOOK_RES0: usize = 2;
pub const N_BOOKS: usize = BOOK_RES0 + (N_CLASSES - 1);

/// Symbol counts gathered in pass one; indices match the codebooks.
pub struct Histograms {
    pub floor: Vec<u64>,
    pub class: Vec<u64>,
    pub res: [Vec<u64>; N_CLASSES - 1],
}

impl Default for Histograms {
    fn default() -> Self {
        Self::new()
    }
}

impl Histograms {
    pub fn new() -> Histograms {
        Histograms {
            floor: vec![0; FLOOR_RANGE as usize],
            class: vec![0; N_CLASSES.pow(CLASSWORDS as u32)],
            res: std::array::from_fn(|i| {
                let c = i + 1;
                let lv = (2 * CLASS_R[c] + 1) as usize;
                vec![0; lv.pow(CLASS_DIM[c] as u32)]
            }),
        }
    }

    pub fn merge(&mut self, other: &Histograms) {
        for (a, b) in self.floor.iter_mut().zip(&other.floor) {
            *a += b;
        }
        for (a, b) in self.class.iter_mut().zip(&other.class) {
            *a += b;
        }
        for (ra, rb) in self.res.iter_mut().zip(&other.res) {
            for (a, b) in ra.iter_mut().zip(rb) {
                *a += b;
            }
        }
    }
}

/// The per-file codebooks, built from the histograms.
pub struct BookSet {
    pub floor: HuffBook,
    pub class: HuffBook,
    pub res: [HuffBook; N_CLASSES - 1],
}

impl BookSet {
    pub fn build(hist: &Histograms) -> BookSet {
        BookSet {
            floor: HuffBook::build(&hist.floor),
            class: HuffBook::build(&hist.class),
            res: std::array::from_fn(|i| HuffBook::build(&hist.res[i])),
        }
    }

    /// Encode-table index of a residue vector for class `c` (1-based):
    /// dimension 0 is the low digit, matching the decoder's lattice unrolling.
    #[inline]
    pub fn res_symbol(c: usize, vals: &[i16]) -> usize {
        let r = CLASS_R[c];
        let lv = (2 * r + 1) as usize;
        debug_assert_eq!(vals.len(), CLASS_DIM[c]);
        let mut idx = 0usize;
        for &v in vals.iter().rev() {
            debug_assert!((v as i32).abs() <= r);
            idx = idx * lv + (v as i32 + r) as usize;
        }
        idx
    }
}

/// Vorbis' packed float for small integers: mantissa * 2^(exp-788), sign bit.
fn float32_pack(v: i32) -> u32 {
    let sign = if v < 0 { 0x8000_0000u32 } else { 0 };
    let mantissa = v.unsigned_abs();
    assert!(mantissa < (1 << 21));
    sign | (788u32 << 21) | mantissa
}

/// `ilog` from the spec.
fn ilog(x: i32) -> u32 {
    if x <= 0 {
        0
    } else {
        32 - (x as u32).leading_zeros()
    }
}

/// A scalar (lookup type 0) codebook: Huffman lengths only.
fn write_scalar_book(w: &mut BitWriter, dimensions: u32, lengths: &[u8]) {
    w.push(0x564342, 24);
    w.push(dimensions, 16);
    w.push(lengths.len() as u32, 24);
    w.push(0, 1); // not ordered
    w.push(1, 1); // sparse: unused entries carry no codeword
    for &l in lengths {
        if l == 0 {
            w.push(0, 1);
        } else {
            w.push(1, 1);
            w.push(l as u32 - 1, 5);
        }
    }
    w.push(0, 4); // lookup type 0
}

/// A lattice (lookup type 1) codebook over the integer grid [-r, r]^dim.
fn write_lattice_book(w: &mut BitWriter, dim: usize, r: i32, lengths: &[u8]) {
    let lv = (2 * r + 1) as u32;
    assert_eq!(lengths.len(), (lv as usize).pow(dim as u32));
    w.push(0x564342, 24);
    w.push(dim as u32, 16);
    w.push(lengths.len() as u32, 24);
    w.push(0, 1); // not ordered
    w.push(1, 1); // sparse
    for &l in lengths {
        if l == 0 {
            w.push(0, 1);
        } else {
            w.push(1, 1);
            w.push(l as u32 - 1, 5);
        }
    }
    w.push(1, 4); // lookup type 1
    w.push(float32_pack(-r), 32); // minimum
    w.push(float32_pack(1), 32); // delta
    let value_bits = ilog(lv as i32 - 1);
    w.push(value_bits - 1, 4);
    w.push(0, 1); // sequence_p
    for m in 0..lv {
        w.push(m, value_bits);
    }
}

/// The floor 1 configuration, without the leading floor-type field.
fn floor_config(w: &mut BitWriter) {
    let partitions = FLOOR_INTERIOR.len() / 4;
    w.push(partitions as u32, 5);
    for _ in 0..partitions {
        w.push(0, 4); // every partition uses class 0
    }
    // Class 0: four dimensions, no subclasses, one book for everything.
    w.push(3, 3); // dimensions - 1
    w.push(0, 2); // subclasses
    w.push(BOOK_FLOOR as u32 + 1, 8); // subclass book + 1
    w.push(1, 2); // multiplier - 1  => multiplier 2, range 128
    w.push(9, 4); // rangebits: X values are 9 bits, top point at 512
    for &x in &FLOOR_INTERIOR {
        w.push(x, 9);
    }
}

/// Parse our own floor configuration through the decoder, yielding the exact
/// `Floor1` whose `synthesize` the decoder will run on our packets.
pub fn decoder_floor() -> Floor1 {
    let mut w = BitWriter::new();
    w.push(1, 16); // floor type 1
    floor_config(&mut w);
    let bytes = w.finish();
    let mut r = BitReader::new(&bytes);
    match Floor::read(&mut r, N_BOOKS).expect("our own floor config must parse") {
        Floor::Type1(f) => f,
    }
}

/// Identification header packet.
pub fn ident_packet(rate: u32, channels: u16) -> Vec<u8> {
    let mut p = vec![1u8];
    p.extend_from_slice(b"vorbis");
    let mut w = BitWriter::new();
    w.push(0, 32); // version
    w.push(channels as u32, 8);
    w.push(rate, 32);
    w.push(0, 32); // bitrate max
    w.push(0, 32); // bitrate nominal (VBR: unhinted)
    w.push(0, 32); // bitrate min
    let bs = BLOCK.trailing_zeros();
    w.push(bs, 4);
    w.push(bs, 4);
    w.push(1, 1); // framing
    p.extend(w.finish());
    p
}

/// Comment header packet: vendor plus caller tags, framing byte.
pub fn comment_packet(tags: &[(String, String)]) -> Vec<u8> {
    let mut p = vec![3u8];
    p.extend_from_slice(b"vorbis");
    let vendor = b"makepad-audio-encode 0.1";
    p.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    p.extend_from_slice(vendor);
    p.extend_from_slice(&(tags.len() as u32).to_le_bytes());
    for (k, v) in tags {
        let text = format!("{k}={v}");
        p.extend_from_slice(&(text.len() as u32).to_le_bytes());
        p.extend_from_slice(text.as_bytes());
    }
    p.push(1); // framing
    p
}

/// Setup header packet: the codebooks, then floor, residue, mapping, mode.
pub fn setup_packet(books: &BookSet, channels: u16) -> Vec<u8> {
    let _ = channels; // the shape is channel-count independent
    let mut p = vec![5u8];
    p.extend_from_slice(b"vorbis");
    let mut w = BitWriter::new();

    // -- codebooks --
    w.push(N_BOOKS as u32 - 1, 8);
    write_scalar_book(&mut w, 1, &books.floor.lengths);
    write_scalar_book(&mut w, CLASSWORDS as u32, &books.class.lengths);
    for c in 1..N_CLASSES {
        write_lattice_book(&mut w, CLASS_DIM[c], CLASS_R[c], &books.res[c - 1].lengths);
    }

    // -- time transforms: one null placeholder --
    w.push(0, 6);
    w.push(0, 16);

    // -- floors: one, type 1 --
    w.push(0, 6);
    w.push(1, 16);
    floor_config(&mut w);

    // -- residues: one, type 1 --
    w.push(0, 6);
    w.push(1, 16);
    w.push(0, 24); // begin
    w.push(HALF as u32, 24); // end
    w.push(PARTITION as u32 - 1, 24); // partition size - 1
    w.push(N_CLASSES as u32 - 1, 6); // classifications - 1
    w.push(BOOK_CLASS as u32, 8); // classbook
    for c in 0..N_CLASSES {
        // Cascade: class 0 has no books at all; the rest use pass 0 only.
        let low = if c == 0 { 0 } else { 1 };
        w.push(low, 3);
        w.push(0, 1); // no high bits
    }
    for c in 1..N_CLASSES {
        w.push((BOOK_RES0 + c - 1) as u32, 8);
    }

    // -- mappings: one, type 0, one submap, no coupling --
    w.push(0, 6);
    w.push(0, 16); // mapping type
    w.push(0, 1); // submaps flag: one submap
    w.push(0, 1); // no coupling
    w.push(0, 2); // reserved
    w.push(0, 8); // submap: unused
    w.push(0, 8); // submap floor
    w.push(0, 8); // submap residue

    // -- modes: one, short blocks only --
    w.push(0, 6);
    w.push(0, 1); // blockflag
    w.push(0, 16); // windowtype
    w.push(0, 16); // transformtype
    w.push(0, 8); // mapping
    w.push(1, 1); // framing

    p.extend(w.finish());
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_audio_decode::vorbis::codebook::Codebook;

    #[test]
    fn floor_interior_is_ascending_unique_and_in_range() {
        for pair in FLOOR_INTERIOR.windows(2) {
            assert!(pair[0] < pair[1]);
        }
        assert!(*FLOOR_INTERIOR.first().unwrap() > 0);
        assert!((*FLOOR_INTERIOR.last().unwrap() as usize) < HALF);
        assert_eq!(FLOOR_INTERIOR.len() % 4, 0, "partitions of dimension 4");
    }

    #[test]
    fn decoder_accepts_our_floor_config() {
        let f = decoder_floor();
        assert_eq!(f.points(), FLOOR_POINTS);
    }

    #[test]
    fn decoder_accepts_every_codebook_shape() {
        // Uniform counts: every entry coded.
        let hist = {
            let mut h = Histograms::new();
            h.floor.iter_mut().for_each(|c| *c = 1);
            h.class.iter_mut().for_each(|c| *c = 1);
            for r in h.res.iter_mut() {
                r.iter_mut().for_each(|c| *c = 1);
            }
            h
        };
        let books = BookSet::build(&hist);
        let packet = setup_packet(&books, 2);
        // Walk the codebooks off the front of the setup packet with the real
        // decoder: count, then N_BOOKS codebooks.
        let mut r = BitReader::new(&packet[7..]);
        let count = r.read(8).unwrap() as usize + 1;
        assert_eq!(count, N_BOOKS);
        let mut budget = 1 << 22;
        for i in 0..count {
            let cb = Codebook::read(&mut r, &mut budget)
                .unwrap_or_else(|e| panic!("codebook {i}: {e:?}"));
            match i {
                BOOK_FLOOR => assert_eq!(cb.entries, FLOOR_RANGE as u32),
                BOOK_CLASS => {
                    assert_eq!(cb.entries as usize, N_CLASSES.pow(CLASSWORDS as u32));
                    assert_eq!(cb.dimensions as usize, CLASSWORDS);
                }
                _ => {
                    let c = i - BOOK_RES0 + 1;
                    let lv = (2 * CLASS_R[c] + 1) as u32;
                    assert_eq!(cb.dimensions as usize, CLASS_DIM[c]);
                    assert_eq!(cb.entries, lv.pow(CLASS_DIM[c] as u32));
                    // The lattice must dequantize to the exact integer grid.
                    for entry in 0..cb.entries as usize {
                        let mut idx = entry;
                        for d in 0..CLASS_DIM[c] {
                            let want = (idx % lv as usize) as i32 - CLASS_R[c];
                            let got = cb.vectors[entry * CLASS_DIM[c] + d];
                            assert_eq!(got, want as f32, "book {i} entry {entry} dim {d}");
                            idx /= lv as usize;
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn res_symbol_matches_the_decoder_lattice() {
        // Symbol index round-trips through the decoder's dequantized vectors.
        let hist = {
            let mut h = Histograms::new();
            for r in h.res.iter_mut() {
                r.iter_mut().for_each(|c| *c = 1);
            }
            h.floor[0] = 1;
            h.class[0] = 1;
            h
        };
        let books = BookSet::build(&hist);
        let packet = setup_packet(&books, 2);
        let mut r = BitReader::new(&packet[7..]);
        let _count = r.read(8).unwrap();
        let mut budget = 1 << 22;
        let mut cbs = Vec::new();
        for _ in 0..N_BOOKS {
            cbs.push(Codebook::read(&mut r, &mut budget).unwrap());
        }
        for c in 1..N_CLASSES {
            let cb = &cbs[BOOK_RES0 + c - 1];
            let probes: &[&[i16]] = match CLASS_DIM[c] {
                4 => &[&[0, 0, 0, 0], &[1, -1, 0, 1], &[-1, -1, -1, -1]],
                2 => &[&[0, 0], &[-3, 3], &[1, -2]],
                _ => &[&[0], &[5], &[-7]],
            };
            for vals in probes {
                let ok = vals.iter().all(|&v| (v as i32).abs() <= CLASS_R[c]);
                if !ok {
                    continue;
                }
                let sym = BookSet::res_symbol(c, vals);
                for (d, &v) in vals.iter().enumerate() {
                    assert_eq!(
                        cb.vectors[sym * CLASS_DIM[c] + d],
                        v as f32,
                        "class {c} vals {vals:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn ident_packet_is_the_decoders_shape() {
        let p = ident_packet(44_100, 2);
        assert_eq!(p[0], 1);
        assert_eq!(&p[1..7], b"vorbis");
        assert_eq!(p.len(), 30);
    }
}
