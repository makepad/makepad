//! Vorbis codebooks: canonical Huffman decode plus the VQ vector lookup.
//!
//! Adapted from the same author's sandbox decoder, with the per-codeword
//! `HashMap` lookup replaced by a direct-mapped table over the first
//! [`FAST_BITS`] bits — codeword decode is the innermost loop of residue
//! decode, and hashing once per bit of every codeword dominated the profile.

use super::bits::{float32_unpack, ilog, lookup1_values, BitReader};
use crate::error::AudioError;

/// Entries with no codeword are marked with this length.
const NO_CODE: u8 = 255;
/// Width of the direct-mapped codeword table (4 KiB per codebook).
const FAST_BITS: u32 = 10;

pub struct Codebook {
    pub dimensions: u32,
    pub entries: u32,
    pub lookup_type: u8,
    /// Decoded VQ vectors, `entries * dimensions` long when `lookup_type != 0`.
    pub vectors: Vec<f32>,
    fast_bits: u32,
    /// `(entry << 5) | len` for codewords up to `fast_bits` long, indexed by the
    /// next `fast_bits` bits of the stream; 0 where no short codeword matches.
    fast: Vec<u32>,
    /// Codewords longer than `fast_bits`, sorted by (length, code).
    long_codes: Vec<u32>,
    long_entries: Vec<u32>,
    /// `long_range[len]..long_range[len + 1]` selects one length's codewords.
    long_range: [u32; 34],
    max_len: u8,
}

impl Codebook {
    /// Read one codebook. `budget` is the number of `f32`/entry slots the whole
    /// setup header may still spend, so a header cannot name a gigabyte of
    /// codebook it does not contain.
    pub fn read(r: &mut BitReader, budget: &mut usize) -> Result<Codebook, AudioError> {
        let sync = r.read(24).ok_or(AudioError::Truncated)?;
        if sync != 0x564342 {
            return Err(AudioError::Corrupt("codebook sync"));
        }
        let dimensions = r.read(16).ok_or(AudioError::Truncated)?;
        let entries = r.read(24).ok_or(AudioError::Truncated)?;
        if entries == 0 || dimensions == 0 {
            return Err(AudioError::Corrupt("empty codebook"));
        }
        spend(budget, entries as usize)?;

        let mut lengths = vec![NO_CODE; entries as usize];
        let ordered = r.read_bit().ok_or(AudioError::Truncated)?;
        if !ordered {
            let sparse = r.read_bit().ok_or(AudioError::Truncated)?;
            // Each entry costs at least one bit, so a header cannot describe
            // more entries than the packet has room for.
            if entries as usize > r.bits_left() {
                return Err(AudioError::Truncated);
            }
            for len in lengths.iter_mut() {
                let present =
                    if sparse { r.read_bit().ok_or(AudioError::Truncated)? } else { true };
                if present {
                    *len = (r.read(5).ok_or(AudioError::Truncated)? + 1) as u8;
                }
            }
        } else {
            let mut current_entry = 0u32;
            let mut current_length = r.read(5).ok_or(AudioError::Truncated)? + 1;
            while current_entry < entries {
                let bits = ilog((entries - current_entry) as i32);
                let number = r.read(bits).ok_or(AudioError::Truncated)?;
                let end = current_entry.checked_add(number).ok_or(AudioError::Corrupt("codebook run"))?;
                if end > entries {
                    return Err(AudioError::Corrupt("codebook run"));
                }
                for len in lengths[current_entry as usize..end as usize].iter_mut() {
                    *len = current_length as u8;
                }
                current_entry = end;
                current_length += 1;
                if current_length > 32 && current_entry < entries {
                    return Err(AudioError::Corrupt("codebook run"));
                }
            }
        }

        let assigned = build_codes(&lengths)?;

        // VQ lookup.
        let lookup_type = r.read(4).ok_or(AudioError::Truncated)? as u8;
        let mut vectors = Vec::new();
        match lookup_type {
            0 => {}
            1 | 2 => {
                let min = float32_unpack(r.read(32).ok_or(AudioError::Truncated)?);
                let delta = float32_unpack(r.read(32).ok_or(AudioError::Truncated)?);
                let value_bits = r.read(4).ok_or(AudioError::Truncated)? + 1;
                let sequence_p = r.read_bit().ok_or(AudioError::Truncated)?;
                let lookup_values = if lookup_type == 1 {
                    lookup1_values(entries, dimensions)
                } else {
                    entries.checked_mul(dimensions).ok_or(AudioError::Corrupt("codebook size"))?
                };
                // Every multiplicand is `value_bits` wide in the packet.
                if (lookup_values as usize).saturating_mul(value_bits as usize) > r.bits_left() {
                    return Err(AudioError::Truncated);
                }
                let mut multiplicands = Vec::with_capacity(lookup_values as usize);
                for _ in 0..lookup_values {
                    multiplicands.push(r.read(value_bits).ok_or(AudioError::Truncated)? as f32);
                }
                let total = (entries as usize)
                    .checked_mul(dimensions as usize)
                    .ok_or(AudioError::Corrupt("codebook size"))?;
                spend(budget, total)?;
                vectors = vec![0.0f32; total];
                if lookup_type == 1 {
                    for i in 0..entries as usize {
                        let mut last = 0.0f32;
                        let mut index_divisor = 1u32;
                        for j in 0..dimensions as usize {
                            let offset = ((i as u32 / index_divisor) % lookup_values.max(1)) as usize;
                            let m = multiplicands.get(offset).copied().unwrap_or(0.0);
                            let v = m * delta + min + last;
                            vectors[i * dimensions as usize + j] = v;
                            if sequence_p {
                                last = v;
                            }
                            index_divisor = index_divisor.saturating_mul(lookup_values.max(1));
                        }
                    }
                } else {
                    for i in 0..entries as usize {
                        let mut last = 0.0f32;
                        let offset = i * dimensions as usize;
                        for j in 0..dimensions as usize {
                            let m = multiplicands.get(offset + j).copied().unwrap_or(0.0);
                            let v = m * delta + min + last;
                            vectors[offset + j] = v;
                            if sequence_p {
                                last = v;
                            }
                        }
                    }
                }
            }
            _ => return Err(AudioError::Corrupt("codebook lookup type")),
        }

        Ok(Codebook::build(dimensions, entries, lookup_type, vectors, &assigned))
    }

    fn build(
        dimensions: u32,
        entries: u32,
        lookup_type: u8,
        vectors: Vec<f32>,
        assigned: &[(u32, u8, u32)],
    ) -> Codebook {
        let max_len = assigned.iter().map(|&(_, l, _)| l).max().unwrap_or(0);
        let fast_bits = (max_len as u32).min(FAST_BITS);
        let mut fast = vec![0u32; if fast_bits == 0 { 0 } else { 1usize << fast_bits }];
        let mut long: Vec<(u8, u32, u32)> = Vec::new();
        for &(entry, len, code) in assigned {
            if (len as u32) <= fast_bits && fast_bits > 0 {
                // The first bit read is the codeword's high bit, so the table
                // index is the codeword reversed.
                let rev = reverse_bits(code, len as u32);
                let packed = (entry << 5) | len as u32;
                let step = 1usize << len;
                let mut idx = rev as usize;
                while idx < fast.len() {
                    fast[idx] = packed;
                    idx += step;
                }
            } else {
                long.push((len, code, entry));
            }
        }
        // Sorted by (length, code) so one length's codewords are a contiguous,
        // ordered run that `decode_long` can binary-search.
        long.sort_unstable();
        let long_codes: Vec<u32> = long.iter().map(|&(_, code, _)| code).collect();
        let long_entries: Vec<u32> = long.iter().map(|&(_, _, entry)| entry).collect();
        let mut long_range = [0u32; 34];
        let mut at = 0usize;
        for (len, slot) in long_range.iter_mut().enumerate().take(33) {
            *slot = at as u32;
            while at < long.len() && long[at].0 as usize == len {
                at += 1;
            }
        }
        long_range[33] = at as u32;
        Codebook {
            dimensions,
            entries,
            lookup_type,
            vectors,
            fast_bits,
            fast,
            long_codes,
            long_entries,
            long_range,
            max_len,
        }
    }

    /// Read one codeword and return its entry index.
    #[inline]
    pub fn decode(&self, r: &mut BitReader) -> Result<u32, AudioError> {
        if self.fast_bits == 0 {
            return Err(AudioError::Corrupt("undecodable codebook"));
        }
        let packed = self.fast[r.peek(self.fast_bits) as usize];
        if packed != 0 {
            let len = packed & 31;
            if !r.skip(len) {
                return Err(AudioError::Truncated);
            }
            return Ok(packed >> 5);
        }
        self.decode_long(r)
    }

    /// Codewords past the fast table: walk bit by bit, binary-searching the
    /// codewords of each length. Rare enough not to matter.
    fn decode_long(&self, r: &mut BitReader) -> Result<u32, AudioError> {
        let mut code = 0u32;
        for len in 1..=self.max_len as u32 {
            let bit = r.read(1).ok_or(AudioError::Truncated)?;
            code = (code << 1) | bit;
            if len > self.fast_bits {
                let a = self.long_range[len as usize] as usize;
                let b = self.long_range[len as usize + 1] as usize;
                if let Some(slice) = self.long_codes.get(a..b) {
                    if let Ok(k) = slice.binary_search(&code) {
                        return Ok(self.long_entries[a + k]);
                    }
                }
            }
        }
        Err(AudioError::Corrupt("vorbis huffman code"))
    }

    /// Decode a codeword and return its VQ vector.
    #[inline]
    pub fn decode_vector(&self, r: &mut BitReader) -> Result<&[f32], AudioError> {
        let entry = self.decode(r)? as usize;
        let d = self.dimensions as usize;
        let start = entry.checked_mul(d).ok_or(AudioError::Corrupt("codebook entry"))?;
        self.vectors.get(start..start + d).ok_or(AudioError::Corrupt("codebook entry"))
    }
}

fn spend(budget: &mut usize, n: usize) -> Result<(), AudioError> {
    match budget.checked_sub(n) {
        Some(left) => {
            *budget = left;
            Ok(())
        }
        None => Err(AudioError::TooLarge("vorbis setup header")),
    }
}

fn reverse_bits(code: u32, len: u32) -> u32 {
    let mut out = 0u32;
    for i in 0..len {
        out |= ((code >> (len - 1 - i)) & 1) << i;
    }
    out
}

/// Canonical code assignment (the `available` walk from the spec), returning
/// `(entry, length, code)` for every entry that has a codeword.
fn build_codes(lengths: &[u8]) -> Result<Vec<(u32, u8, u32)>, AudioError> {
    let mut out: Vec<(u32, u8, u32)> = Vec::new();
    let mut available = [0u32; 33];

    let Some(first) = lengths.iter().position(|&l| l != NO_CODE) else {
        // A codebook with no usable entries is legal but undecodable; leave the
        // table empty and let any decode attempt error.
        return Ok(out);
    };
    let flen = lengths[first];
    if flen == 0 || flen > 32 {
        return Err(AudioError::Corrupt("codeword length"));
    }
    out.push((first as u32, flen, 0));
    for (i, slot) in available.iter_mut().enumerate().take(flen as usize + 1).skip(1) {
        *slot = 1u32 << (32 - i);
    }

    for (i, &len) in lengths.iter().enumerate().skip(first + 1) {
        if len == NO_CODE {
            continue;
        }
        if len == 0 || len > 32 {
            return Err(AudioError::Corrupt("codeword length"));
        }
        let mut z = len as usize;
        while z > 0 && available[z] == 0 {
            z -= 1;
        }
        if z == 0 {
            // No free node of any usable length: the tree is overfull.
            return Err(AudioError::Corrupt("overfull huffman tree"));
        }
        let res = available[z];
        available[z] = 0;
        let code = res >> (32 - len as u32);
        out.push((i as u32, len, code));
        if z != len as usize {
            for y in ((z + 1)..=len as usize).rev() {
                available[y] = res + (1u32 << (32 - y));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(lengths: &[u8]) -> Vec<(u32, u8, u32)> {
        build_codes(lengths).unwrap()
    }

    #[test]
    fn canonical_codes_are_prefix_free_and_ordered() {
        // Lengths 1,2,3,3 -> codes 0, 10, 110, 111
        let c = codes(&[1, 2, 3, 3]);
        assert_eq!(c, vec![(0, 1, 0b0), (1, 2, 0b10), (2, 3, 0b110), (3, 3, 0b111)]);
    }

    #[test]
    fn out_of_order_lengths_follow_the_spec_walk() {
        // The spec assigns in entry order off a free list, which is NOT the
        // same as sorting by length: entry 0 keeps the first length-2 code.
        let c = codes(&[2, 1, 2]);
        assert_eq!(c, vec![(0, 2, 0b00), (1, 1, 0b1), (2, 2, 0b01)]);
    }

    #[test]
    fn assigned_codes_are_a_prefix_code() {
        // Exhaustive-ish: every pair must not prefix the other.
        for lengths in [
            vec![1u8, 2, 3, 3],
            vec![2, 2, 2, 2],
            vec![1, 3, 3, 3, 3],
            vec![4, 4, 3, 2, 2, 3],
        ] {
            let c = codes(&lengths);
            for (_, la, ca) in &c {
                for (_, lb, cb) in &c {
                    if (la, ca) == (lb, cb) {
                        continue;
                    }
                    let (short, long, sc, lc) =
                        if la <= lb { (la, lb, ca, cb) } else { (lb, la, cb, ca) };
                    assert_ne!(lc >> (long - short), *sc, "{lengths:?} is not prefix free");
                }
            }
        }
    }

    #[test]
    fn sparse_entries_are_skipped() {
        let c = codes(&[NO_CODE, 1, NO_CODE, 2, 2]);
        assert_eq!(c, vec![(1, 1, 0), (3, 2, 0b10), (4, 2, 0b11)]);
    }

    #[test]
    fn overfull_tree_is_rejected() {
        // Three length-1 codes cannot fit in a binary tree.
        assert!(build_codes(&[1, 1, 1]).is_err());
    }

    #[test]
    fn empty_codebook_does_not_panic() {
        assert!(codes(&[NO_CODE, NO_CODE]).is_empty());
        let book = Codebook::build(1, 2, 0, Vec::new(), &[]);
        let mut r = BitReader::new(&[0xFF, 0xFF]);
        assert!(book.decode(&mut r).is_err());
    }

    /// Pack codewords MSB-first into an LSB-first bitstream, the way a Vorbis
    /// encoder does, so the decoder can be driven by hand.
    fn pack(words: &[(u8, u32)]) -> Vec<u8> {
        let mut bits: Vec<bool> = Vec::new();
        for &(len, code) in words {
            for i in (0..len).rev() {
                bits.push((code >> i) & 1 == 1);
            }
        }
        let mut bytes = vec![0u8; bits.len().div_ceil(8) + 1];
        for (i, b) in bits.iter().enumerate() {
            if *b {
                bytes[i / 8] |= 1 << (i % 8);
            }
        }
        bytes
    }

    #[test]
    fn decode_round_trips_short_and_long_codewords() {
        // 1..=14 bit codewords: crosses the fast table boundary at 10.
        let lengths: Vec<u8> = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 14];
        let assigned = codes(&lengths);
        let book = Codebook::build(1, lengths.len() as u32, 0, Vec::new(), &assigned);
        let words: Vec<(u8, u32)> = assigned.iter().map(|&(_, l, c)| (l, c)).collect();
        let bytes = pack(&words);
        let mut r = BitReader::new(&bytes);
        for &(entry, _, _) in &assigned {
            assert_eq!(book.decode(&mut r).unwrap(), entry);
        }
    }

    #[test]
    fn decode_round_trips_in_a_shuffled_order() {
        let lengths: Vec<u8> = vec![3, 3, 3, 3, 3, 3, 3, 3];
        let assigned = codes(&lengths);
        let book = Codebook::build(1, 8, 0, Vec::new(), &assigned);
        let order = [5usize, 0, 7, 7, 1, 3, 2, 6, 4, 0];
        let words: Vec<(u8, u32)> =
            order.iter().map(|&i| (assigned[i].1, assigned[i].2)).collect();
        let bytes = pack(&words);
        let mut r = BitReader::new(&bytes);
        for &i in &order {
            assert_eq!(book.decode(&mut r).unwrap(), i as u32);
        }
    }

    #[test]
    fn running_out_of_bits_is_truncated_not_a_panic() {
        let lengths: Vec<u8> = vec![12, 12, 12, 12];
        let assigned = codes(&lengths);
        let book = Codebook::build(1, 4, 0, Vec::new(), &assigned);
        let mut r = BitReader::new(&[0u8]);
        assert!(matches!(book.decode(&mut r), Err(AudioError::Truncated)));
    }

    /// Pack fields LSB-first, the way a Vorbis header is written.
    fn bitstream(fields: &[(u32, u32)]) -> Vec<u8> {
        let mut bits: Vec<bool> = Vec::new();
        for &(val, n) in fields {
            for i in 0..n {
                bits.push((val >> i) & 1 == 1);
            }
        }
        let mut bytes = vec![0u8; bits.len().div_ceil(8) + 8];
        for (i, b) in bits.iter().enumerate() {
            if *b {
                bytes[i / 8] |= 1 << (i % 8);
            }
        }
        bytes
    }

    #[test]
    fn budget_refuses_a_codebook_bigger_than_the_file() {
        let mut budget = 1000usize;
        let bytes = bitstream(&[(0x564342, 24), (4, 16), (0xFF_FFFF, 24)]);
        let mut r = BitReader::new(&bytes);
        assert!(matches!(Codebook::read(&mut r, &mut budget), Err(AudioError::TooLarge(_))));
    }

    #[test]
    fn entry_count_beyond_the_packet_is_truncated_not_allocated() {
        let mut budget = 1 << 22;
        // 100_000 entries, unordered, non-sparse: needs 500 kbit the file lacks.
        let bytes = bitstream(&[(0x564342, 24), (2, 16), (100_000, 24), (0, 1), (0, 1)]);
        let mut r = BitReader::new(&bytes);
        assert!(matches!(Codebook::read(&mut r, &mut budget), Err(AudioError::Truncated)));
    }

    #[test]
    fn bad_sync_is_refused() {
        let bytes = bitstream(&[(0x123456, 24)]);
        let mut r = BitReader::new(&bytes);
        let mut budget = 1 << 20;
        assert!(matches!(Codebook::read(&mut r, &mut budget), Err(AudioError::Corrupt(_))));
    }
}
