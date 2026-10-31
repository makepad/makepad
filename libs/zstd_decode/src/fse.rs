//! Finite State Entropy tables (RFC 8878 section 4.1): table descriptions and decoding tables.

use crate::bits::{bits_at, BackReader};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FseEntry {
    pub(crate) symbol: u8,
    pub(crate) nb_bits: u8,
    pub(crate) base: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FseTable {
    pub(crate) log: u32,
    pub(crate) entries: Vec<FseEntry>,
}

impl FseTable {
    /// A one-state table that always yields `symbol` and reads no bits (RLE mode).
    pub(crate) fn rle(symbol: u8) -> FseTable {
        FseTable { log: 0, entries: vec![FseEntry { symbol, nb_bits: 0, base: 0 }] }
    }

    /// Builds the decoding table from normalized counts (-1 = "less than one", one cell at the table end).
    pub(crate) fn build(counts: &[i16], log: u32) -> Result<FseTable, String> {
        if counts.len() > 256 {
            return Err("FSE table has too many symbols".into());
        }
        let size = 1usize << log;
        let total: usize = counts.iter().map(|&c| c.unsigned_abs() as usize).sum();
        if total != size {
            return Err(format!("FSE counts sum to {} instead of {}", total, size));
        }
        let mut symbols = vec![0u8; size];
        let mut next = vec![0u32; counts.len()];
        let mut high = size as isize - 1;
        for (s, &c) in counts.iter().enumerate() {
            if c == -1 {
                symbols[high as usize] = s as u8;
                high -= 1;
                next[s] = 1;
            } else if c < -1 {
                return Err("invalid FSE count".into());
            } else {
                next[s] = c as u32;
            }
        }
        let step = (size >> 1) + (size >> 3) + 3;
        let mask = size - 1;
        let mut pos = 0usize;
        for (s, &c) in counts.iter().enumerate() {
            for _ in 0..c.max(0) {
                symbols[pos] = s as u8;
                loop {
                    pos = (pos + step) & mask;
                    if pos as isize <= high {
                        break;
                    }
                }
            }
        }
        if pos != 0 {
            return Err("FSE symbol spread did not cover the table".into());
        }
        let mut entries = Vec::with_capacity(size);
        for &symbol in &symbols {
            let x = next[symbol as usize];
            next[symbol as usize] += 1;
            let nb = log - (31 - x.leading_zeros());
            let base = ((x << nb) as usize - size) as u16;
            entries.push(FseEntry { symbol, nb_bits: nb as u8, base });
        }
        Ok(FseTable { log, entries })
    }

    /// Reads a table description and builds it; returns the table and the bytes it used.
    pub(crate) fn read(src: &[u8], max_symbol: usize, max_log: u32) -> Result<(FseTable, usize), String> {
        let (counts, log, used) = read_counts(src, max_symbol, max_log)?;
        Ok((FseTable::build(&counts, log)?, used))
    }

    #[inline]
    pub(crate) fn init_state(&self, r: &mut BackReader) -> usize {
        r.read(self.log) as usize
    }

    #[inline]
    pub(crate) fn update(&self, state: usize, r: &mut BackReader) -> usize {
        let e = self.entries[state];
        e.base as usize + r.read(e.nb_bits as u32) as usize
    }
}

/// Parses an FSE table description (RFC 8878 4.1.1). Returns (normalized counts, accuracy log, bytes used).
pub(crate) fn read_counts(src: &[u8], max_symbol: usize, max_log: u32) -> Result<(Vec<i16>, u32, usize), String> {
    let first = *src.first().ok_or("missing FSE table description")?;
    let log = (first & 15) as u32 + 5;
    if log > max_log {
        return Err(format!("FSE accuracy log {} exceeds {}", log, max_log));
    }
    let mut bit = 4usize;
    let mut remaining: i32 = (1 << log) + 1;
    let mut threshold: i32 = 1 << log;
    let mut nb_bits = log + 1;
    let mut counts: Vec<i16> = Vec::new();
    let mut prev_zero = false;
    while remaining > 1 {
        if prev_zero {
            loop {
                let repeat = bits_at(src, bit, 2) as usize;
                bit += 2;
                counts.resize(counts.len() + repeat, 0);
                if repeat != 3 {
                    break;
                }
                if counts.len() > max_symbol + 1 {
                    break;
                }
            }
        }
        if counts.len() > max_symbol || bit > src.len() * 8 {
            return Err("FSE table description is corrupt".into());
        }
        let max = 2 * threshold - 1 - remaining;
        let low = bits_at(src, bit, nb_bits - 1) as i32;
        let value = if low < max {
            bit += nb_bits as usize - 1;
            low
        } else {
            let v = bits_at(src, bit, nb_bits) as i32;
            bit += nb_bits as usize;
            if v >= threshold {
                v - max
            } else {
                v
            }
        };
        let count = value - 1;
        remaining -= count.abs();
        counts.push(count as i16);
        prev_zero = count == 0;
        if remaining < 1 {
            break;
        }
        while remaining < threshold {
            nb_bits -= 1;
            threshold >>= 1;
        }
    }
    if remaining != 1 {
        return Err("FSE table description counts do not sum to the table size".into());
    }
    if bit > src.len() * 8 {
        return Err("FSE table description is truncated".into());
    }
    Ok((counts, log, bit.div_ceil(8)))
}
