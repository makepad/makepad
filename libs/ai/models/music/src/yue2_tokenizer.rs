//! YuE2 text/ABC tokenizer: tiktoken-style byte BPE over `qwen.tiktoken`.
//!
//! Reference: `yue2.tokenization_yue2.YuE2TextTokenizer` — 151643 mergeable
//! ranks (base64 bytes + rank per line), the Qwen2 split pattern, NFC, and
//! `encode_ordinary` (special tokens in the text are NOT matched; the
//! protocol inserts its control ids itself). BPE = tiktoken's
//! `byte_pair_merge`: repeatedly merge the adjacent pair whose concatenated
//! bytes have the lowest rank.
//!
//! NFC: like the H3 tokenizer we carry no Unicode composition tables, so
//! input is taken as already NFC (typed Latin, CJK and ABC text are). Text
//! with combining marks after a base letter is the one case that can differ.

use crate::{DiffusionError, Result};
use makepad_ai_h3::h3_tokenizer::split_qwen2_words;
use std::collections::HashMap;
use std::path::Path;

pub const YUE2_ORDINARY_TOKENS: usize = 151_643;

pub struct Yue2Tokenizer {
    ranks: HashMap<Vec<u8>, u32>,
    pieces: Vec<Vec<u8>>,
}

fn b64_value(byte: u8) -> Option<u32> {
    Some(match byte {
        b'A'..=b'Z' => (byte - b'A') as u32,
        b'a'..=b'z' => (byte - b'a') as u32 + 26,
        b'0'..=b'9' => (byte - b'0') as u32 + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    })
}

fn b64_decode(text: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for &byte in text {
        if byte == b'=' {
            break;
        }
        acc = (acc << 6) | b64_value(byte)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

impl Yue2Tokenizer {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let data = std::fs::read(path)
            .map_err(|e| DiffusionError::model(format!("yue2 tokenizer {}: {e}", path.display())))?;
        Self::from_tiktoken(&data)
    }

    pub fn from_tiktoken(data: &[u8]) -> Result<Self> {
        let mut ranks = HashMap::with_capacity(YUE2_ORDINARY_TOKENS);
        let mut pieces = vec![Vec::new(); YUE2_ORDINARY_TOKENS];
        for line in data.split(|&b| b == b'\n') {
            if line.is_empty() {
                continue;
            }
            let mut fields = line.split(|&b| b == b' ');
            let (Some(token), Some(rank)) = (fields.next(), fields.next()) else {
                return Err(DiffusionError::model("yue2 tokenizer: malformed qwen.tiktoken line"));
            };
            let bytes = b64_decode(token)
                .ok_or_else(|| DiffusionError::model("yue2 tokenizer: bad base64"))?;
            let rank: u32 = std::str::from_utf8(rank)
                .ok()
                .and_then(|r| r.trim().parse().ok())
                .ok_or_else(|| DiffusionError::model("yue2 tokenizer: bad rank"))?;
            if rank as usize >= YUE2_ORDINARY_TOKENS {
                return Err(DiffusionError::model(format!("yue2 tokenizer: rank {rank} out of range")));
            }
            pieces[rank as usize] = bytes.clone();
            ranks.insert(bytes, rank);
        }
        if ranks.len() != YUE2_ORDINARY_TOKENS {
            return Err(DiffusionError::model(format!(
                "yue2 tokenizer: expected {YUE2_ORDINARY_TOKENS} ordinary tokens, found {}",
                ranks.len()
            )));
        }
        Ok(Self { ranks, pieces })
    }

    /// `encode_ordinary(NFC(text))`.
    pub fn encode(&self, text: &str) -> Vec<u32> {
        let mut out = Vec::new();
        for word in split_qwen2_words(text) {
            let bytes = word.as_bytes();
            if let Some(&rank) = self.ranks.get(bytes) {
                out.push(rank);
                continue;
            }
            self.byte_pair_merge(bytes, &mut out);
        }
        out
    }

    fn byte_pair_merge(&self, bytes: &[u8], out: &mut Vec<u32>) {
        // Part boundaries; parts[i]..parts[i+1] is one current symbol.
        let mut parts: Vec<usize> = (0..=bytes.len()).collect();
        let rank_of = |parts: &[usize], i: usize| -> u32 {
            if i + 2 < parts.len() {
                self.ranks
                    .get(&bytes[parts[i]..parts[i + 2]])
                    .copied()
                    .unwrap_or(u32::MAX)
            } else {
                u32::MAX
            }
        };
        let mut pair_ranks: Vec<u32> = (0..parts.len()).map(|i| rank_of(&parts, i)).collect();
        loop {
            let mut best = (u32::MAX, 0usize);
            for (i, &r) in pair_ranks.iter().enumerate().take(parts.len().saturating_sub(2)) {
                if r < best.0 {
                    best = (r, i);
                }
            }
            if best.0 == u32::MAX {
                break;
            }
            let i = best.1;
            parts.remove(i + 1);
            pair_ranks.remove(i + 1);
            pair_ranks[i] = rank_of(&parts, i);
            if i > 0 {
                pair_ranks[i - 1] = rank_of(&parts, i - 1);
            }
        }
        for w in parts.windows(2) {
            // Every single byte is a rank, so each final part resolves.
            out.push(self.ranks.get(&bytes[w[0]..w[1]]).copied().unwrap_or(0));
        }
    }

    /// Decode ordinary ids (others skipped), invalid UTF-8 replaced.
    pub fn decode(&self, ids: &[u32]) -> String {
        let mut bytes = Vec::new();
        for &id in ids {
            if let Some(piece) = self.pieces.get(id as usize) {
                bytes.extend_from_slice(piece);
            }
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_roundtrip() {
        assert_eq!(b64_decode(b"IQ==").unwrap(), b"!");
        assert_eq!(b64_decode(b"aGVsbG8=").unwrap(), b"hello");
        assert_eq!(b64_decode(b"5L2g").unwrap(), "你".as_bytes());
    }
}
