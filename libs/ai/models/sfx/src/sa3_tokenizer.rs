//! Gemma SentencePiece tokenizer for the SA3 T5Gemma conditioner, loaded from
//! the HF `tokenizer.model` protobuf. Mirrors the reference GemmaTokenizer
//! (raw sentencepiece encode): whitespace escape (' ' -> U+2581) only, NO
//! dummy prefix, NO bos/eos, byte fallback (<0xXX> pieces), pad id 0 to the
//! conditioner's 256-token window.
//!
//! The merge algorithm is the llama.cpp-style greedy score merge (identical
//! to libs/llama vocab.rs, which is llama.cpp-parity tested); parity against
//! the reference tokenizer is covered by local/sa3_ref/dumps fixtures via
//! `sa3-validate`/unit test.

use crate::sa3::SA3_TEXT_TOKENS;
use crate::{DiffusionError, Result};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::path::Path;

pub const SA3_PAD_TOKEN: u32 = 0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PieceType {
    Normal,
    Unknown,
    Control,
    UserDefined,
    Unused,
    Byte,
}

pub struct Sa3Tokenizer {
    pieces: Vec<String>,
    scores: Vec<f32>,
    /// Pieces eligible for text lookup (controls/unknown excluded).
    token_to_id: HashMap<String, u32>,
    byte_to_id: HashMap<u8, u32>,
    unk_id: u32,
}

struct Symbol {
    start: usize,
    len: usize,
    prev: Option<usize>,
    next: Option<usize>,
}

struct Bigram {
    left: usize,
    right: usize,
    score: f32,
    size: usize,
}

impl PartialEq for Bigram {
    fn eq(&self, other: &Self) -> bool {
        self.left == other.left
            && self.right == other.right
            && self.score.to_bits() == other.score.to_bits()
            && self.size == other.size
    }
}
impl Eq for Bigram {}
impl PartialOrd for Bigram {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Bigram {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .total_cmp(&other.score)
            .then_with(|| other.left.cmp(&self.left))
            .then_with(|| other.right.cmp(&self.right))
            .then_with(|| other.size.cmp(&self.size))
    }
}

// --- minimal protobuf reader for the sentencepiece ModelProto ---------------

struct ProtoReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> ProtoReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn done(&self) -> bool {
        self.pos >= self.data.len()
    }

    fn varint(&mut self) -> Result<u64> {
        let mut out = 0u64;
        let mut shift = 0;
        loop {
            let byte = *self
                .data
                .get(self.pos)
                .ok_or_else(|| DiffusionError::model("sa3 tokenizer: truncated varint"))?;
            self.pos += 1;
            out |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(out);
            }
            shift += 7;
            if shift > 63 {
                return Err(DiffusionError::model("sa3 tokenizer: varint overflow"));
            }
        }
    }

    /// Returns (field_number, wire_type).
    fn tag(&mut self) -> Result<(u64, u8)> {
        let v = self.varint()?;
        Ok((v >> 3, (v & 7) as u8))
    }

    fn bytes(&mut self) -> Result<&'a [u8]> {
        let len = self.varint()? as usize;
        let end = self
            .pos
            .checked_add(len)
            .filter(|&e| e <= self.data.len())
            .ok_or_else(|| DiffusionError::model("sa3 tokenizer: truncated bytes"))?;
        let out = &self.data[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    fn skip(&mut self, wire_type: u8) -> Result<()> {
        match wire_type {
            0 => {
                self.varint()?;
            }
            1 => self.pos += 8,
            2 => {
                self.bytes()?;
            }
            5 => self.pos += 4,
            other => {
                return Err(DiffusionError::model(format!(
                    "sa3 tokenizer: unsupported wire type {other}"
                )))
            }
        }
        if self.pos > self.data.len() {
            return Err(DiffusionError::model("sa3 tokenizer: truncated field"));
        }
        Ok(())
    }
}

impl Sa3Tokenizer {
    /// Loads pieces/scores/types from a sentencepiece `tokenizer.model`.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let data = std::fs::read(path.as_ref()).map_err(|err| {
            DiffusionError::model(format!("sa3 tokenizer {}: {err}", path.as_ref().display()))
        })?;
        let mut pieces = Vec::new();
        let mut scores = Vec::new();
        let mut types = Vec::new();
        let mut reader = ProtoReader::new(&data);
        while !reader.done() {
            let (field, wire) = reader.tag()?;
            if field == 1 && wire == 2 {
                // repeated SentencePiece pieces = 1
                let msg = reader.bytes()?;
                let mut piece = String::new();
                let mut score = 0f32;
                let mut ptype = PieceType::Normal;
                let mut inner = ProtoReader::new(msg);
                while !inner.done() {
                    let (f, w) = inner.tag()?;
                    match (f, w) {
                        (1, 2) => {
                            piece = String::from_utf8_lossy(inner.bytes()?).into_owned();
                        }
                        (2, 5) => {
                            let b = &inner.data[inner.pos..inner.pos + 4];
                            score = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                            inner.pos += 4;
                        }
                        (3, 0) => {
                            ptype = match inner.varint()? {
                                2 => PieceType::Unknown,
                                3 => PieceType::Control,
                                4 => PieceType::UserDefined,
                                5 => PieceType::Unused,
                                6 => PieceType::Byte,
                                _ => PieceType::Normal,
                            };
                        }
                        (_, w) => inner.skip(w)?,
                    }
                }
                pieces.push(piece);
                scores.push(score);
                types.push(ptype);
            } else {
                reader.skip(wire)?;
            }
        }
        if pieces.is_empty() {
            return Err(DiffusionError::model("sa3 tokenizer: no pieces parsed"));
        }
        let mut token_to_id = HashMap::with_capacity(pieces.len());
        let mut byte_to_id = HashMap::new();
        let mut unk_id = 3u32;
        for (id, piece) in pieces.iter().enumerate() {
            match types[id] {
                PieceType::Control | PieceType::Unused => {}
                PieceType::Unknown => unk_id = id as u32,
                PieceType::Byte => {
                    // "<0xNN>"
                    if let Some(hex) = piece
                        .strip_prefix("<0x")
                        .and_then(|rest| rest.strip_suffix('>'))
                    {
                        if let Ok(byte) = u8::from_str_radix(hex, 16) {
                            byte_to_id.insert(byte, id as u32);
                        }
                    }
                }
                PieceType::Normal | PieceType::UserDefined => {
                    token_to_id.entry(piece.clone()).or_insert(id as u32);
                }
            }
        }
        Ok(Self {
            pieces,
            scores,
            token_to_id,
            byte_to_id,
            unk_id,
        })
    }

    pub fn piece(&self, id: u32) -> Option<&str> {
        self.pieces.get(id as usize).map(String::as_str)
    }

    /// Raw sentencepiece encode of a prompt (no bos/eos, no dummy prefix).
    pub fn tokenize(&self, text: &str) -> Vec<u32> {
        let escaped = text.replace(' ', "\u{2581}");
        let mut output = Vec::new();
        self.encode_text(&escaped, &mut output);
        output
    }

    /// Tokenizes, truncates to and pads (id 0) to the SA3 conditioner window.
    /// Returns (ids, mask) of length `SA3_TEXT_TOKENS`.
    pub fn tokenize_padded(&self, text: &str) -> (Vec<u32>, Vec<bool>) {
        let mut ids = self.tokenize(text);
        ids.truncate(SA3_TEXT_TOKENS);
        let valid = ids.len();
        ids.resize(SA3_TEXT_TOKENS, SA3_PAD_TOKEN);
        let mut mask = vec![false; SA3_TEXT_TOKENS];
        for m in mask[..valid].iter_mut() {
            *m = true;
        }
        (ids, mask)
    }

    fn encode_text(&self, text: &str, output: &mut Vec<u32>) {
        if text.is_empty() {
            return;
        }
        let mut symbols: Vec<Symbol> = Vec::new();
        let mut prev: Option<usize> = None;
        let mut iter = text.char_indices().peekable();
        while let Some((start, _)) = iter.next() {
            let end = iter.peek().map(|(offset, _)| *offset).unwrap_or(text.len());
            let index = symbols.len();
            symbols.push(Symbol {
                start,
                len: end - start,
                prev,
                next: None,
            });
            if let Some(prev_index) = prev {
                symbols[prev_index].next = Some(index);
            }
            prev = Some(index);
        }

        let mut work_queue = BinaryHeap::new();
        let mut rev_merge: HashMap<String, (usize, usize)> = HashMap::new();
        for index in 1..symbols.len() {
            self.try_add_bigram(text, &symbols, index - 1, index, &mut work_queue, &mut rev_merge);
        }
        while let Some(bigram) = work_queue.pop() {
            let left_len = symbols[bigram.left].len;
            let right_len = symbols[bigram.right].len;
            if left_len == 0 || right_len == 0 || left_len + right_len != bigram.size {
                continue;
            }
            let right_next = symbols[bigram.right].next;
            symbols[bigram.left].len += right_len;
            symbols[bigram.left].next = right_next;
            symbols[bigram.right].len = 0;
            if let Some(next_index) = right_next {
                symbols[next_index].prev = Some(bigram.left);
            }
            if let Some(prev_index) = symbols[bigram.left].prev {
                self.try_add_bigram(text, &symbols, prev_index, bigram.left, &mut work_queue, &mut rev_merge);
            }
            if let Some(next_index) = symbols[bigram.left].next {
                self.try_add_bigram(text, &symbols, bigram.left, next_index, &mut work_queue, &mut rev_merge);
            }
        }

        let mut cursor = if symbols.is_empty() { None } else { Some(0) };
        while let Some(index) = cursor {
            self.resegment(text, &symbols, &rev_merge, index, output);
            cursor = symbols[index].next;
        }
    }

    fn try_add_bigram(
        &self,
        text: &str,
        symbols: &[Symbol],
        left: usize,
        right: usize,
        work_queue: &mut BinaryHeap<Bigram>,
        rev_merge: &mut HashMap<String, (usize, usize)>,
    ) {
        let left_symbol = &symbols[left];
        let right_symbol = &symbols[right];
        if left_symbol.len == 0 || right_symbol.len == 0 {
            return;
        }
        let end = right_symbol.start + right_symbol.len;
        let merged = &text[left_symbol.start..end];
        let Some(&token_id) = self.token_to_id.get(merged) else {
            return;
        };
        work_queue.push(Bigram {
            left,
            right,
            score: self.scores[token_id as usize],
            size: merged.len(),
        });
        rev_merge.insert(merged.to_owned(), (left, right));
    }

    fn resegment(
        &self,
        text: &str,
        symbols: &[Symbol],
        rev_merge: &HashMap<String, (usize, usize)>,
        index: usize,
        output: &mut Vec<u32>,
    ) {
        let symbol = &symbols[index];
        let fragment = &text[symbol.start..symbol.start + symbol.len];
        if let Some(&token_id) = self.token_to_id.get(fragment) {
            output.push(token_id);
            return;
        }
        if let Some(&(left, right)) = rev_merge.get(fragment) {
            self.resegment(text, symbols, rev_merge, left, output);
            self.resegment(text, symbols, rev_merge, right, output);
            return;
        }
        for &byte in fragment.as_bytes() {
            match self.byte_to_id.get(&byte) {
                Some(&id) => output.push(id),
                None => output.push(self.unk_id),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Parity vs the reference GemmaTokenizer fixtures. Skips (with a note)
    /// when the local reference weights are absent (e.g. plain CI checkout).
    #[test]
    fn reference_fixture_parity() {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let model = repo.join("local/sa3_ref/weights/stable-audio-3-small-sfx/t5gemma-b-b-ul2/tokenizer.model");
        let fixtures = repo.join("local/sa3_ref/dumps/tokenizer_fixtures.json");
        if !model.is_file() || !fixtures.is_file() {
            eprintln!("sa3 tokenizer fixtures absent; skipping parity test");
            return;
        }
        let tokenizer = Sa3Tokenizer::load(&model).unwrap();
        let text = std::fs::read_to_string(&fixtures).unwrap();
        // Tiny JSON pull: [{"text": ..., "ids": [...]}, ...]
        let value: Vec<(String, Vec<u32>)> = parse_fixtures(&text);
        assert!(!value.is_empty());
        for (prompt, expected) in value {
            let got = tokenizer.tokenize(&prompt);
            assert_eq!(got, expected, "prompt {prompt:?}");
        }
    }

    /// Minimal parser for the exact fixture file shape written by sa3_dump.
    fn parse_fixtures(text: &str) -> Vec<(String, Vec<u32>)> {
        let mut out = Vec::new();
        let mut rest = text;
        while let Some(t_pos) = rest.find("\"text\":") {
            rest = &rest[t_pos + 7..];
            let start = rest.find('"').unwrap() + 1;
            let mut prompt = String::new();
            let mut chars = rest[start..].chars();
            loop {
                match chars.next().unwrap() {
                    '"' => break,
                    '\\' => match chars.next().unwrap() {
                        'n' => prompt.push('\n'),
                        't' => prompt.push('\t'),
                        'u' => {
                            let hex: String = (0..4).map(|_| chars.next().unwrap()).collect();
                            let code = u32::from_str_radix(&hex, 16).unwrap();
                            // Handle surrogate pairs for emoji fixtures.
                            if (0xD800..0xDC00).contains(&code) {
                                let escaped: String = (0..6).map(|_| chars.next().unwrap()).collect();
                                assert!(escaped.starts_with("\\u"));
                                let low = u32::from_str_radix(&escaped[2..], 16).unwrap();
                                let combined =
                                    0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                                prompt.push(char::from_u32(combined).unwrap());
                            } else {
                                prompt.push(char::from_u32(code).unwrap());
                            }
                        }
                        other => prompt.push(other),
                    },
                    other => prompt.push(other),
                }
            }
            let ids_pos = rest.find("\"ids\":").unwrap();
            let ids_rest = &rest[ids_pos + 6..];
            let open = ids_rest.find('[').unwrap();
            let close = ids_rest.find(']').unwrap();
            let ids: Vec<u32> = ids_rest[open + 1..close]
                .split(',')
                .filter_map(|part| part.trim().parse().ok())
                .collect();
            out.push((prompt, ids));
            rest = &ids_rest[close..];
        }
        out
    }
}
