//! Vorbis codebooks: canonical Huffman decode plus the VQ vector lookup.

use crate::bitread::{float32_unpack, ilog, lookup1_values, BitReader};
use crate::AudioError;
use std::collections::HashMap;

/// Entries with no codeword are marked with this length.
const NO_CODE: u8 = 255;

pub struct Codebook {
    pub dimensions: u32,
    pub entries: u32,
    /// (length << 32 | code) -> entry index.
    decode_map: HashMap<u64, u32>,
    max_len: u8,
    /// Decoded VQ vectors, `entries * dimensions` long when lookup_type != 0.
    pub vectors: Vec<f32>,
    pub lookup_type: u8,
}

impl Codebook {
    pub fn read(r: &mut BitReader) -> Result<Codebook, AudioError> {
        let sync = r.read(24).ok_or(AudioError::Truncated)?;
        if sync != 0x564342 {
            return Err(AudioError::Malformed);
        }
        let dimensions = r.read(16).ok_or(AudioError::Truncated)?;
        let entries = r.read(24).ok_or(AudioError::Truncated)?;
        if entries == 0 || dimensions == 0 || entries > 1 << 24 {
            return Err(AudioError::Malformed);
        }

        let mut lengths = vec![NO_CODE; entries as usize];
        let ordered = r.read_bit().ok_or(AudioError::Truncated)?;
        if !ordered {
            let sparse = r.read_bit().ok_or(AudioError::Truncated)?;
            for len in lengths.iter_mut() {
                let present = if sparse {
                    r.read_bit().ok_or(AudioError::Truncated)?
                } else {
                    true
                };
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
                let end = current_entry
                    .checked_add(number)
                    .ok_or(AudioError::Malformed)?;
                if end > entries {
                    return Err(AudioError::Malformed);
                }
                for i in current_entry..end {
                    lengths[i as usize] = current_length.min(32) as u8;
                }
                current_entry = end;
                current_length += 1;
                if current_length > 32 {
                    // Only legal if we finished exactly.
                    if current_entry < entries {
                        return Err(AudioError::Malformed);
                    }
                }
            }
        }

        let (decode_map, max_len) = build_codes(&lengths)?;

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
                    entries
                        .checked_mul(dimensions)
                        .ok_or(AudioError::Malformed)?
                };
                if lookup_values > 1 << 24 {
                    return Err(AudioError::Malformed);
                }
                let mut multiplicands = Vec::with_capacity(lookup_values as usize);
                for _ in 0..lookup_values {
                    multiplicands.push(r.read(value_bits).ok_or(AudioError::Truncated)? as f32);
                }
                let total = (entries as usize)
                    .checked_mul(dimensions as usize)
                    .ok_or(AudioError::Malformed)?;
                if total > 1 << 26 {
                    return Err(AudioError::Malformed);
                }
                vectors = vec![0.0f32; total];
                if lookup_type == 1 {
                    for i in 0..entries as usize {
                        let mut last = 0.0f32;
                        let mut index_divisor = 1u32;
                        for j in 0..dimensions as usize {
                            let offset = ((i as u32 / index_divisor) % lookup_values) as usize;
                            let v = multiplicands[offset] * delta + min + last;
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
                            let v = multiplicands[offset + j] * delta + min + last;
                            vectors[offset + j] = v;
                            if sequence_p {
                                last = v;
                            }
                        }
                    }
                }
            }
            _ => return Err(AudioError::Malformed),
        }

        Ok(Codebook {
            dimensions,
            entries,
            decode_map,
            max_len,
            vectors,
            lookup_type,
        })
    }

    /// Read one codeword and return its entry index.
    pub fn decode(&self, r: &mut BitReader) -> Result<u32, AudioError> {
        let mut code: u32 = 0;
        for len in 1..=self.max_len {
            let bit = r.read(1).ok_or(AudioError::Truncated)?;
            code = (code << 1) | bit;
            if let Some(&entry) = self.decode_map.get(&key(len, code)) {
                return Ok(entry);
            }
        }
        Err(AudioError::Malformed)
    }

    /// Decode a codeword and return its VQ vector.
    pub fn decode_vector(&self, r: &mut BitReader) -> Result<&[f32], AudioError> {
        let entry = self.decode(r)? as usize;
        let d = self.dimensions as usize;
        let start = entry.checked_mul(d).ok_or(AudioError::Malformed)?;
        self.vectors
            .get(start..start + d)
            .ok_or(AudioError::Malformed)
    }
}

fn key(len: u8, code: u32) -> u64 {
    ((len as u64) << 32) | code as u64
}

/// Canonical code assignment (the `available` walk from the spec).
fn build_codes(lengths: &[u8]) -> Result<(HashMap<u64, u32>, u8), AudioError> {
    let mut map: HashMap<u64, u32> = HashMap::new();
    let mut available = [0u32; 33];
    let mut max_len = 0u8;

    let first = lengths.iter().position(|&l| l != NO_CODE);
    let Some(first) = first else {
        // A codebook with no usable entries is legal but undecodable; leave
        // the map empty and let any decode attempt error.
        return Ok((map, 0));
    };
    let flen = lengths[first];
    if flen == 0 || flen > 32 {
        return Err(AudioError::Malformed);
    }
    map.insert(key(flen, 0), first as u32);
    max_len = max_len.max(flen);
    for i in 1..=flen as usize {
        available[i] = 1u32 << (32 - i);
    }

    for (i, &len) in lengths.iter().enumerate().skip(first + 1) {
        if len == NO_CODE {
            continue;
        }
        if len == 0 || len > 32 {
            return Err(AudioError::Malformed);
        }
        let mut z = len as usize;
        while z > 0 && available[z] == 0 {
            z -= 1;
        }
        if z == 0 {
            return Err(AudioError::Malformed);
        }
        let res = available[z];
        available[z] = 0;
        let code = res >> (32 - len as u32);
        map.insert(key(len, code), i as u32);
        max_len = max_len.max(len);
        if z != len as usize {
            for y in ((z + 1)..=len as usize).rev() {
                available[y] = res + (1u32 << (32 - y));
            }
        }
    }
    Ok((map, max_len))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_codes_are_prefix_free_and_ordered() {
        // Lengths 1,2,3,3 -> codes 0, 10, 110, 111
        let (map, max) = build_codes(&[1, 2, 3, 3]).unwrap();
        assert_eq!(max, 3);
        assert_eq!(map.get(&key(1, 0b0)), Some(&0));
        assert_eq!(map.get(&key(2, 0b10)), Some(&1));
        assert_eq!(map.get(&key(3, 0b110)), Some(&2));
        assert_eq!(map.get(&key(3, 0b111)), Some(&3));
    }

    #[test]
    fn sparse_entries_are_skipped() {
        let (map, _) = build_codes(&[NO_CODE, 1, NO_CODE, 2, 2]).unwrap();
        // First usable entry gets code 0 at its own length.
        assert_eq!(map.get(&key(1, 0)), Some(&1));
        assert_eq!(map.len(), 3);
    }

    #[test]
    fn overfull_tree_is_rejected() {
        // Three length-1 codes cannot fit in a binary tree.
        assert!(build_codes(&[1, 1, 1]).is_err());
    }

    #[test]
    fn empty_codebook_does_not_panic() {
        let (map, max) = build_codes(&[NO_CODE, NO_CODE]).unwrap();
        assert!(map.is_empty());
        assert_eq!(max, 0);
    }
}
