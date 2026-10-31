//! Huffman literal coding (RFC 8878 section 4.2): tree descriptions, decoding tables, streams.

use crate::bits::BackReader;
use crate::fse::FseTable;

pub(crate) const MAX_BITS: u32 = 11;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HufTable {
    pub(crate) max_bits: u32,
    /// Indexed by the next `max_bits` stream bits: (symbol, code length).
    pub(crate) entries: Vec<(u8, u8)>,
}

impl HufTable {
    /// Builds the table from the transmitted weights (the last symbol's weight is implied).
    pub(crate) fn from_weights(mut weights: Vec<u8>) -> Result<HufTable, String> {
        if weights.len() > 255 {
            return Err("too many Huffman weights".into());
        }
        let mut sum: u32 = 0;
        for &w in &weights {
            if w as u32 > MAX_BITS {
                return Err(format!("Huffman weight {} exceeds {}", w, MAX_BITS));
            }
            if w > 0 {
                sum += 1 << (w - 1);
            }
        }
        if sum == 0 {
            return Err("Huffman weights are all zero".into());
        }
        let max_bits = 32 - sum.leading_zeros(); // highest bit + 1
        if max_bits > MAX_BITS {
            return Err("Huffman code too long".into());
        }
        let left = (1u32 << max_bits) - sum;
        if !left.is_power_of_two() {
            return Err("Huffman weights do not complete a prefix code".into());
        }
        weights.push((left.trailing_zeros() + 1) as u8);
        HufTable::build(&weights, max_bits)
    }

    /// Builds the table from complete weights for every symbol.
    pub(crate) fn build(weights: &[u8], max_bits: u32) -> Result<HufTable, String> {
        let size = 1usize << max_bits;
        // starting cell per weight: lower weights (longer codes) first, symbols in order inside a weight
        let mut rank_count = [0usize; MAX_BITS as usize + 2];
        for &w in weights {
            rank_count[w as usize] += 1;
        }
        let mut start = [0usize; MAX_BITS as usize + 2];
        let mut next = 0usize;
        for w in 1..=max_bits as usize {
            start[w] = next;
            next += rank_count[w] << (w - 1);
        }
        if next != size {
            return Err("Huffman weights do not fill the table".into());
        }
        let mut entries = vec![(0u8, 0u8); size];
        for (sym, &w) in weights.iter().enumerate() {
            if w == 0 {
                continue;
            }
            let len = 1usize << (w - 1);
            let nb = (max_bits + 1 - w as u32) as u8;
            for e in &mut entries[start[w as usize]..start[w as usize] + len] {
                *e = (sym as u8, nb);
            }
            start[w as usize] += len;
        }
        Ok(HufTable { max_bits, entries })
    }

    /// Parses a Huffman tree description; returns the table and the bytes used.
    pub(crate) fn read(src: &[u8]) -> Result<(HufTable, usize), String> {
        let header = *src.first().ok_or("missing Huffman tree description")? as usize;
        let (weights, used) = if header < 128 {
            let body = src.get(1..1 + header).ok_or("Huffman tree description is truncated")?;
            (fse_weights(body)?, 1 + header)
        } else {
            let n = header - 127;
            let bytes = n.div_ceil(2);
            let body = src.get(1..1 + bytes).ok_or("Huffman tree description is truncated")?;
            let mut w = Vec::with_capacity(n);
            for i in 0..n {
                let b = body[i / 2];
                w.push(if i % 2 == 0 { b >> 4 } else { b & 15 });
            }
            (w, 1 + bytes)
        };
        Ok((HufTable::from_weights(weights)?, used))
    }

    /// Decodes exactly `count` symbols from one backward stream that must be consumed exactly.
    pub(crate) fn decode_stream(&self, src: &[u8], count: usize, out: &mut Vec<u8>) -> Result<(), String> {
        let mut r = BackReader::new(src)?;
        for _ in 0..count {
            let (sym, nb) = self.entries[r.peek(self.max_bits) as usize];
            r.consume(nb as u32);
            out.push(sym);
        }
        if r.remaining() != 0 {
            return Err("Huffman stream size does not match its symbols".into());
        }
        Ok(())
    }
}

/// FSE-compressed weights: two interleaved states over one backward stream (accuracy log <= 6).
fn fse_weights(src: &[u8]) -> Result<Vec<u8>, String> {
    let (table, used) = FseTable::read(src, 255, 6)?;
    let mut r = BackReader::new(&src[used..])?;
    let mut s1 = table.init_state(&mut r);
    let mut s2 = table.init_state(&mut r);
    if r.overflowed() {
        return Err("Huffman weight stream is truncated".into());
    }
    let mut out = Vec::new();
    loop {
        out.push(table.entries[s1].symbol);
        s1 = table.update(s1, &mut r);
        if r.overflowed() {
            out.push(table.entries[s2].symbol);
            break;
        }
        out.push(table.entries[s2].symbol);
        s2 = table.update(s2, &mut r);
        if r.overflowed() {
            out.push(table.entries[s1].symbol);
            break;
        }
        if out.len() > 255 {
            return Err("too many Huffman weights".into());
        }
    }
    Ok(out)
}
