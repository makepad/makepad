//! Per-file Huffman codes: histogram in, canonical Vorbis codewords out.
//!
//! The encoder runs two passes — quantize everything and count symbols, then
//! build codes from the real counts — so every file carries codebooks fitted
//! to its own distribution instead of a shipped approximation. Trees are
//! always *exactly* full (Kraft sum 1): libvorbis-family decoders reject
//! under- or over-specified trees, so unused symbols get no codeword at all
//! (written sparse) rather than a wasted leaf.
//!
//! Code assignment must match the spec's `available`-list walk bit for bit —
//! the decoder derives codes from lengths alone, so the encoder recomputes the
//! same walk and the tests drive the decoder's own `Codebook` over the result.

use crate::bits::reverse_bits;

/// Longest codeword the builder will emit. Well under the spec's 32 and the
/// decoder's fast path handles up to 10 in one lookup; rare symbols may cost a
/// slow-path walk, which is fine.
const MAX_CODE_LEN: u32 = 24;

/// One symbol's codeword, pre-reversed for the LSB-first writer.
#[derive(Clone, Copy, Default)]
pub struct Codeword {
    /// Reversed bits, ready for `BitWriter::push`.
    pub bits: u32,
    /// Length in bits; 0 means "symbol has no codeword" (never occurs).
    pub len: u32,
}

/// A built codebook: lengths for the header, codewords for the stream.
pub struct HuffBook {
    /// Codeword length per entry, 0 for entries with no codeword.
    pub lengths: Vec<u8>,
    /// Encode table, indexed by entry.
    pub codes: Vec<Codeword>,
}

impl HuffBook {
    /// Build from occurrence counts, one per codebook entry.
    ///
    /// Entries with a zero count get no codeword. If fewer than two entries
    /// are used, codewords are forced onto the first entries so the tree is
    /// still exactly full — a degenerate track (digital silence) must still
    /// produce a stream every decoder accepts.
    pub fn build(counts: &[u64]) -> HuffBook {
        assert!(counts.len() >= 2, "a codebook needs at least two entries");
        let mut counts = counts.to_vec();
        let used = counts.iter().filter(|&&c| c > 0).count();
        if used < 2 {
            // Force two length-1 codewords: entry 0 and the used one (or 1).
            let hot = counts.iter().position(|&c| c > 0).unwrap_or(1).max(1);
            counts[0] = counts[0].max(1);
            counts[hot] = counts[hot].max(1);
        }
        let mut lengths = huffman_lengths(&counts, MAX_CODE_LEN);
        debug_assert_eq!(kraft_scaled(&lengths), 1u64 << 32, "tree must be exactly full");
        // Guard against the impossible: if the tree were not exactly full every
        // decoder would refuse the file, so flatten to a trivial full tree.
        if kraft_scaled(&lengths) != 1u64 << 32 {
            lengths = fallback_flat(counts.len());
        }
        let codes = assign_codes(&lengths);
        HuffBook { lengths, codes }
    }

    /// Mean code length in bits under `counts`, for reporting.
    pub fn mean_bits(&self, counts: &[u64]) -> f64 {
        let mut bits = 0u64;
        let mut n = 0u64;
        for (i, &c) in counts.iter().enumerate() {
            bits += c * self.lengths.get(i).copied().unwrap_or(0) as u64;
            n += c;
        }
        if n == 0 {
            0.0
        } else {
            bits as f64 / n as f64
        }
    }
}

/// Kraft sum scaled by 2^32: sum of 2^(32 - len) over coded entries.
fn kraft_scaled(lengths: &[u8]) -> u64 {
    lengths.iter().filter(|&&l| l > 0).map(|&l| 1u64 << (32 - l as u32)).sum()
}

fn fallback_flat(n: usize) -> Vec<u8> {
    // Smallest full tree over all n entries: lengths ceil(log2 n), with the
    // spare leaves shortened. Simplest correct: give everything equal length
    // by rounding n up to a power of two and shortening the first entries.
    let bits = (n.max(2) as u64).next_power_of_two().trailing_zeros() as usize;
    let mut lengths = vec![bits as u8; n];
    let spare = (1usize << bits) - n;
    // Each shortened leaf frees exactly one same-length sibling slot.
    for len in lengths.iter_mut().take(spare) {
        *len -= 1;
    }
    lengths
}

/// Package-merge-free Huffman: classic two-queue merge over sorted counts,
/// then depth extraction; counts are damped and rebuilt if the tree exceeds
/// `max_len`. Deterministic: ties break on entry order.
fn huffman_lengths(counts: &[u64], max_len: u32) -> Vec<u8> {
    let mut damped: Vec<u64> = counts.to_vec();
    loop {
        let lengths = huffman_lengths_once(&damped);
        let deepest = lengths.iter().copied().max().unwrap_or(0) as u32;
        if deepest <= max_len {
            return lengths;
        }
        // Halve the dynamic range and retry: flattens the tree.
        for c in damped.iter_mut() {
            if *c > 0 {
                *c = (*c >> 2) + 1;
            }
        }
    }
}

fn huffman_lengths_once(counts: &[u64]) -> Vec<u8> {
    #[derive(Clone, Copy)]
    struct Node {
        count: u64,
        /// Index into the node arena of the two children, or usize::MAX.
        left: usize,
        right: usize,
        symbol: usize,
    }
    let mut leaves: Vec<(u64, usize)> =
        counts.iter().enumerate().filter(|&(_, &c)| c > 0).map(|(i, &c)| (c, i)).collect();
    let mut lengths = vec![0u8; counts.len()];
    if leaves.len() < 2 {
        if let Some(&(_, sym)) = leaves.first() {
            lengths[sym] = 1;
        }
        return lengths;
    }
    // Sort ascending by (count, symbol): deterministic merges.
    leaves.sort_unstable();
    let mut arena: Vec<Node> = leaves
        .iter()
        .map(|&(count, symbol)| Node { count, left: usize::MAX, right: usize::MAX, symbol })
        .collect();
    // Two queues: leaf queue (already sorted) and merge queue (created in
    // nondecreasing count order), giving O(n) merging.
    let mut leaf_at = 0usize;
    let mut merge: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    let take_min = |leaf_at: &mut usize,
                    merge: &mut std::collections::VecDeque<usize>,
                    arena: &Vec<Node>|
     -> usize {
        let leaf_ok = *leaf_at < leaves.len();
        let merge_ok = !merge.is_empty();
        let pick_leaf = match (leaf_ok, merge_ok) {
            (true, true) => arena[*leaf_at].count <= arena[*merge.front().unwrap()].count,
            (true, false) => true,
            (false, true) => false,
            (false, false) => unreachable!("huffman queue underflow"),
        };
        if pick_leaf {
            let idx = *leaf_at;
            *leaf_at += 1;
            idx
        } else {
            merge.pop_front().unwrap()
        }
    };
    let total = leaves.len();
    let mut root = usize::MAX;
    for _ in 0..total - 1 {
        let a = take_min(&mut leaf_at, &mut merge, &arena);
        let b = take_min(&mut leaf_at, &mut merge, &arena);
        let node = Node {
            count: arena[a].count + arena[b].count,
            left: a,
            right: b,
            symbol: usize::MAX,
        };
        arena.push(node);
        root = arena.len() - 1;
        merge.push_back(root);
    }
    // Depth-first depth assignment, iterative to keep the stack shallow.
    let mut stack: Vec<(usize, u8)> = vec![(root, 0)];
    while let Some((idx, depth)) = stack.pop() {
        let node = arena[idx];
        if node.symbol != usize::MAX {
            lengths[node.symbol] = depth.max(1);
        } else {
            stack.push((node.left, depth + 1));
            stack.push((node.right, depth + 1));
        }
    }
    lengths
}

/// The spec's canonical assignment: identical walk to the decoder's
/// `build_codes`, producing (entry order preserved) the codes a decoder will
/// derive from the lengths alone. Codes come back already reversed for the
/// writer.
fn assign_codes(lengths: &[u8]) -> Vec<Codeword> {
    let mut out = vec![Codeword::default(); lengths.len()];
    let mut available = [0u32; 33];
    let Some(first) = lengths.iter().position(|&l| l != 0) else {
        return out;
    };
    let flen = lengths[first] as usize;
    out[first] = Codeword { bits: 0, len: flen as u32 };
    for (i, slot) in available.iter_mut().enumerate().take(flen + 1).skip(1) {
        *slot = 1u32 << (32 - i);
    }
    for (i, &len) in lengths.iter().enumerate().skip(first + 1) {
        if len == 0 {
            continue;
        }
        let len = len as usize;
        let mut z = len;
        while z > 0 && available[z] == 0 {
            z -= 1;
        }
        assert!(z != 0, "overfull huffman tree in encoder");
        let res = available[z];
        available[z] = 0;
        let code = res >> (32 - len);
        out[i] = Codeword { bits: reverse_bits(code, len as u32), len: len as u32 };
        if z != len {
            for y in ((z + 1)..=len).rev() {
                available[y] = res + (1u32 << (32 - y));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::BitWriter;
    use makepad_audio_decode::vorbis::bits::BitReader;
    use makepad_audio_decode::vorbis::codebook::Codebook;

    /// Serialize lengths as a sparse unordered codebook and hand it to the
    /// real decoder: the decoder is the authority on canonical assignment.
    fn decoder_book(lengths: &[u8]) -> Codebook {
        let mut w = BitWriter::new();
        w.push(0x564342, 24);
        w.push(1, 16); // dimensions
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
        w.push(0, 4); // lookup type 0
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes);
        let mut budget = 1 << 22;
        Codebook::read(&mut r, &mut budget).expect("decoder must accept our codebook")
    }

    fn roundtrip(counts: &[u64], stream: &[usize]) {
        let book = HuffBook::build(counts);
        let decoder = decoder_book(&book.lengths);
        let mut w = BitWriter::new();
        for &sym in stream {
            let cw = book.codes[sym];
            assert!(cw.len > 0, "symbol {sym} has no codeword");
            w.push(cw.bits, cw.len);
        }
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes);
        for &sym in stream {
            assert_eq!(decoder.decode(&mut r).unwrap(), sym as u32);
        }
    }

    #[test]
    fn codes_decode_through_the_real_decoder() {
        let counts = [900u64, 400, 200, 100, 50, 25, 12, 6, 3, 1];
        let stream: Vec<usize> = (0..10).chain([0, 0, 3, 9, 5, 0, 1]).collect();
        roundtrip(&counts, &stream);
    }

    #[test]
    fn sparse_symbols_get_no_codeword() {
        let counts = [10u64, 0, 5, 0, 1];
        let book = HuffBook::build(&counts);
        assert_eq!(book.lengths[1], 0);
        assert_eq!(book.lengths[3], 0);
        assert_eq!(kraft_scaled(&book.lengths), 1u64 << 32);
        roundtrip(&counts, &[0, 2, 4, 0, 0, 2]);
    }

    #[test]
    fn all_zero_counts_still_yield_a_full_tree() {
        let counts = [0u64; 6];
        let book = HuffBook::build(&counts);
        assert_eq!(kraft_scaled(&book.lengths), 1u64 << 32);
        // The forced entries decode.
        roundtrip(&counts, &[0]);
    }

    #[test]
    fn one_hot_counts_still_yield_a_full_tree() {
        let counts = [0u64, 0, 1_000_000, 0];
        let book = HuffBook::build(&counts);
        assert_eq!(kraft_scaled(&book.lengths), 1u64 << 32);
        roundtrip(&counts, &[2, 2, 2]);
    }

    #[test]
    fn skewed_counts_respect_the_length_cap() {
        // Fibonacci-ish counts force maximal depth in a plain Huffman build.
        let mut counts = vec![0u64; 40];
        let (mut a, mut b) = (1u64, 1u64);
        for slot in counts.iter_mut() {
            *slot = a;
            let next = a + b;
            a = b;
            b = next;
        }
        let book = HuffBook::build(&counts);
        assert!(book.lengths.iter().all(|&l| l as u32 <= MAX_CODE_LEN));
        assert_eq!(kraft_scaled(&book.lengths), 1u64 << 32);
        let stream: Vec<usize> = (0..40).collect();
        roundtrip(&counts, &stream);
    }

    #[test]
    fn build_is_deterministic() {
        let counts = [5u64, 5, 5, 5, 3, 3, 2, 2, 1, 1];
        let a = HuffBook::build(&counts);
        let b = HuffBook::build(&counts);
        assert_eq!(a.lengths, b.lengths);
    }

    #[test]
    fn mean_bits_tracks_entropy() {
        let counts = [800u64, 100, 50, 25, 15, 10];
        let book = HuffBook::build(&counts);
        let mean = book.mean_bits(&counts);
        // Entropy of this distribution is ~1.1 bits; Huffman must be close.
        assert!(mean < 1.6, "mean {mean}");
        assert!(mean >= 1.0);
    }
}
