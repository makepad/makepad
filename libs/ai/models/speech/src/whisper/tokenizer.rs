//! Whisper's text encoder: text to token ids over the ggml file's own vocab.
//!
//! OpenAI's whisper tokenizes with tiktoken (`gpt2.tiktoken` for the English
//! models, `multilingual.tiktoken` for the rest), and the ggml converters
//! write those tokens in rank order, as raw bytes: vocab id N *is* tiktoken
//! rank N for every ordinary token (the ids below `<|endoftext|>`). A
//! tiktoken encoding has no separate merges table: BPE joins, again and
//! again, the adjacent pair whose concatenation has the lowest rank. So the
//! vocab alone reproduces whisper's tokenization exactly, pre-split by the
//! GPT-2 pattern ([`crate::gpt2_split`]) as tiktoken does.
//!
//! (whisper.cpp's own `tokenize` is a greedy longest-prefix match instead, and
//! disagrees with the model's training tokenization on some words; the
//! forced alignment wants the tokens the model would have written itself.)
//!
//! Special tokens (`<|en|>`, timestamps) are never produced from text: a
//! literal `<|en|>` in the input encodes as ordinary characters.

use crate::gpt2_split::Gpt2Splitter;
use std::collections::HashMap;
use std::fmt;

pub struct WhisperTokenizer {
    /// Ordinary tokens' bytes -> id (= tiktoken rank).
    ranks: HashMap<Vec<u8>, i32>,
    /// id -> bytes, for decoding (ordinary tokens only).
    bytes: Vec<Vec<u8>>,
}

impl fmt::Debug for WhisperTokenizer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WhisperTokenizer").field("tokens", &self.bytes.len()).finish()
    }
}

impl WhisperTokenizer {
    /// Over `token_bytes[id]` for the ordinary ids `0..n_text` (`n_text` is
    /// the `<|endoftext|>` id; everything from it on is special).
    pub fn new(token_bytes: &[Vec<u8>], n_text: usize) -> Self {
        let bytes: Vec<Vec<u8>> = token_bytes.iter().take(n_text).cloned().collect();
        let mut ranks = HashMap::with_capacity(bytes.len());
        for (id, token) in bytes.iter().enumerate() {
            if !token.is_empty() {
                // First id wins should a converter ever write a duplicate.
                ranks.entry(token.clone()).or_insert(id as i32);
            }
        }
        Self { ranks, bytes }
    }

    /// Number of ordinary (text) tokens.
    pub fn n_text(&self) -> usize {
        self.bytes.len()
    }

    /// The raw bytes of one ordinary token (empty for a special id).
    pub fn token_bytes(&self, id: i32) -> &[u8] {
        usize::try_from(id).ok().and_then(|id| self.bytes.get(id)).map_or(&[], |b| b.as_slice())
    }

    pub fn encode(&self, text: &str) -> Vec<i32> {
        self.encode_with_offsets(text).into_iter().map(|(id, _, _)| id).collect()
    }

    /// [`Self::encode`], each id with the byte range of `text` it covers.
    /// Ranges tile `text` in order; one may end or start inside a multi-byte
    /// character, since tokens are bytes.
    pub fn encode_with_offsets(&self, text: &str) -> Vec<(i32, usize, usize)> {
        let mut out = Vec::new();
        let mut at = 0;
        for piece in Gpt2Splitter::new(text) {
            self.bpe(piece.as_bytes(), at, &mut out);
            at += piece.len();
        }
        out
    }

    /// Exact inverse of [`Self::encode`]: the tokens' bytes joined, as text
    /// (lossy only where the ids split a character the sequence never
    /// completes). Special ids contribute nothing.
    pub fn decode(&self, ids: &[i32]) -> String {
        let mut bytes = Vec::new();
        for id in ids {
            bytes.extend_from_slice(self.token_bytes(*id));
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// The tiktoken merge: repeatedly join the adjacent pair with the lowest
    /// rank until no adjacent pair is a token.
    fn bpe(&self, piece: &[u8], base: usize, out: &mut Vec<(i32, usize, usize)>) {
        if piece.is_empty() {
            return;
        }
        if let Some(&id) = self.ranks.get(piece) {
            out.push((id, base, base + piece.len()));
            return;
        }
        // parts[i] = start offset of part i, with the end as a sentinel.
        let mut parts: Vec<usize> = (0..=piece.len()).collect();
        loop {
            let mut best: Option<(i32, usize)> = None;
            for i in 0..parts.len().saturating_sub(2) {
                if let Some(&rank) = self.ranks.get(&piece[parts[i]..parts[i + 2]]) {
                    if best.map_or(true, |(r, _)| rank < r) {
                        best = Some((rank, i));
                    }
                }
            }
            let Some((_, index)) = best else { break };
            parts.remove(index + 1);
        }
        for pair in parts.windows(2) {
            // Every single byte is a token in both whisper vocabs, so a part
            // is always found; one that is not is dropped rather than guessed.
            if let Some(&id) = self.ranks.get(&piece[pair[0]..pair[1]]) {
                out.push((id, base + pair[0], base + pair[1]));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A miniature tiktoken vocab: all 256 bytes first (as tiktoken's are),
    /// then merges in rank order.
    fn tiny() -> WhisperTokenizer {
        let mut tokens: Vec<Vec<u8>> = (0..=255u8).map(|b| vec![b]).collect();
        for merged in ["he", "ll", "llo", " w", "or", " wor", " world", "hello", "é", " é"] {
            tokens.push(merged.as_bytes().to_vec());
        }
        let n = tokens.len();
        tokens.push(b"<|endoftext|>".to_vec());
        WhisperTokenizer::new(&tokens, n)
    }

    #[test]
    fn merges_follow_rank_order() {
        let tok = tiny();
        let id = |s: &str| tok.ranks[s.as_bytes()];
        // "hello" is a token outright; " world" merges " w"+"or" -> " wor" -> " world".
        assert_eq!(tok.encode("hello world"), vec![id("hello"), id(" world")]);
        // "hell": "he" (rank 256) beats "ll" (257): he + l + l -> he + ll.
        assert_eq!(tok.encode("hell"), vec![id("he"), id("ll")]);
        // "yellow" -> y, e, llo, w (no "ye" or "ow" token).
        assert_eq!(tok.encode("yellow"), vec![id("y"), id("e"), id("llo"), id("w")]);
        // Special tokens are text like any other: never the special id.
        assert!(!tok.encode("<|endoftext|>").contains(&(tok.n_text() as i32)));
    }

    #[test]
    fn offsets_tile_the_text_and_decode_round_trips() {
        let tok = tiny();
        for text in [
            "hello world",
            "  Café au lait, s'il vous plaît!\n",
            "日本語 — テスト 123 ½ 😀",
            "It's 3.14… «quoted» (yes) [no] {maybe}",
            "",
        ] {
            let pieces = tok.encode_with_offsets(text);
            let mut at = 0;
            for (_, start, end) in &pieces {
                assert_eq!(*start, at, "{text:?}");
                assert!(end > start);
                at = *end;
            }
            assert_eq!(at, text.len());
            let ids: Vec<i32> = pieces.iter().map(|(id, _, _)| *id).collect();
            assert_eq!(tok.decode(&ids), text);
        }
        // A multi-byte token and its spaced form.
        let id = |s: &str| tok.ranks[s.as_bytes()];
        assert_eq!(tok.encode("é é"), vec![id("é"), id(" é")]);
    }
}
