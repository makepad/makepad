//! RoBERTa-large byte-level BPE tokenizer for the Woosh text conditioner.
//!
//! Parity target: HuggingFace `AutoTokenizer("roberta-large")` with
//! `padding="max_length", truncation=True, max_length=77,
//! add_special_tokens=True` — the exact call in Woosh's
//! `SFXCLAPTextConditioner.tokenize_text`. Oracle fixtures:
//! `local/woosh_ref/dumps/*/tok_ids_{pos,neg}.npy`.
//!
//! Pinned from the HF `tokenizer.json` (FacebookAI/roberta-large):
//! - no normalizer;
//! - pre_tokenizer: `ByteLevel{add_prefix_space: false}` — i.e. the GPT-2
//!   split regex `'s|'t|'re|'ve|'m|'ll|'d| ?\p{L}+| ?\p{N}+|` +
//!   `` ?[^\s\p{L}\p{N}]+|\s+(?!\S)|\s+`` applied internally (use_regex
//!   defaults true), then GPT-2 byte mapping. This differs from the Qwen2
//!   splitter in `h3_tokenizer`: contractions are case-SENSITIVE, the letter
//!   run may only take a single leading SPACE (not any symbol), digit RUNS
//!   stay together, and symbol runs do not swallow trailing newlines;
//! - model: plain BPE, merges as `"left right"` strings;
//! - post_processor: RobertaProcessing -> `<s>`(0) + ids + `</s>`(2), then
//!   pad with `<pad>`(1) to 77; truncation keeps the first 75 word ids.
//!
//! The BPE merge loop and byte mapping are the same as `h3_tokenizer` (which
//! is byte-exact vs llama.cpp); only the splitter and post-processing differ.

use crate::error::{DiffusionError, Result};
use makepad_ai_h3::h3_tokenizer::{gpt2_byte_to_unicode, Json};
use crate::woosh::{WOOSH_DESC_TOKENS, WOOSH_TE_BOS_ID, WOOSH_TE_EOS_ID, WOOSH_TE_PAD_ID};
use std::collections::HashMap;
use std::path::Path;

pub struct WooshTokenizer {
    token_to_id: HashMap<String, u32>,
    merge_ranks: HashMap<(String, String), usize>,
    byte_encoder: [char; 256],
}

impl WooshTokenizer {
    /// Loads `tokenizer.json` (the roberta-large HF file) from `path`.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|err| DiffusionError::io(path, err.to_string()))?;
        let root = Json::parse(&text).map_err(|msg| DiffusionError::json(path, msg))?;

        match root.get("normalizer") {
            None | Some(Json::Null) => {}
            Some(other) => {
                return Err(DiffusionError::model(format!(
                    "woosh tokenizer: unexpected normalizer {:?}",
                    other.get("type").and_then(Json::as_str).unwrap_or("?")
                )))
            }
        }
        let pre = root
            .get("pre_tokenizer")
            .ok_or_else(|| DiffusionError::model("woosh tokenizer: no pre_tokenizer"))?;
        if pre.get("type").and_then(Json::as_str) != Some("ByteLevel")
            || pre.get("add_prefix_space").and_then(Json::as_bool) == Some(true)
        {
            return Err(DiffusionError::model(
                "woosh tokenizer: pre_tokenizer must be ByteLevel{add_prefix_space: false}",
            ));
        }

        let model = root
            .get("model")
            .ok_or_else(|| DiffusionError::model("woosh tokenizer: no model section"))?;
        let vocab = model
            .get("vocab")
            .and_then(Json::as_obj)
            .ok_or_else(|| DiffusionError::model("woosh tokenizer: model.vocab missing"))?;
        let mut token_to_id = HashMap::with_capacity(vocab.len());
        for (piece, id) in vocab {
            let id = id.as_u32().ok_or_else(|| {
                DiffusionError::model(format!("woosh tokenizer: vocab id for {piece:?}"))
            })?;
            token_to_id.insert(piece.clone(), id);
        }
        let merges = model
            .get("merges")
            .and_then(Json::as_arr)
            .ok_or_else(|| DiffusionError::model("woosh tokenizer: model.merges missing"))?;
        let mut merge_ranks = HashMap::with_capacity(merges.len());
        for (rank, entry) in merges.iter().enumerate() {
            let (left, right) = match entry {
                Json::Str(merge) => {
                    let at = merge.find(' ').filter(|at| *at > 0).ok_or_else(|| {
                        DiffusionError::model(format!("woosh tokenizer: bad merge {rank}"))
                    })?;
                    (merge[..at].to_owned(), merge[at + 1..].to_owned())
                }
                Json::Arr(pair) if pair.len() == 2 => match (pair[0].as_str(), pair[1].as_str()) {
                    (Some(left), Some(right)) => (left.to_owned(), right.to_owned()),
                    _ => {
                        return Err(DiffusionError::model(format!(
                            "woosh tokenizer: bad merge pair {rank}"
                        )))
                    }
                },
                _ => {
                    return Err(DiffusionError::model(format!(
                        "woosh tokenizer: bad merge entry {rank}"
                    )))
                }
            };
            merge_ranks.insert((left, right), rank);
        }
        let tokenizer = Self {
            token_to_id,
            merge_ranks,
            byte_encoder: gpt2_byte_to_unicode(),
        };
        for byte in 0..=255_u8 {
            let piece = tokenizer.byte_encoder[byte as usize].to_string();
            if !tokenizer.token_to_id.contains_key(&piece) {
                return Err(DiffusionError::model(format!(
                    "woosh tokenizer: vocab missing byte piece 0x{byte:02X}"
                )));
            }
        }
        Ok(tokenizer)
    }

    /// Raw BPE ids without special tokens or padding.
    pub fn encode(&self, text: &str) -> Vec<u32> {
        let mut out = Vec::new();
        for word in split_gpt2_words(text) {
            self.encode_bpe_word(word, &mut out);
        }
        out
    }

    /// The full Woosh TE tokenization: `<s>` + ids (truncated to 75) + `</s>`,
    /// right-padded with `<pad>` to 77. Returns (ids, attention_mask).
    pub fn encode_padded(&self, text: &str) -> (Vec<u32>, Vec<f32>) {
        let mut ids = self.encode(text);
        ids.truncate(WOOSH_DESC_TOKENS - 2);
        let mut out = Vec::with_capacity(WOOSH_DESC_TOKENS);
        out.push(WOOSH_TE_BOS_ID);
        out.extend_from_slice(&ids);
        out.push(WOOSH_TE_EOS_ID);
        let valid = out.len();
        out.resize(WOOSH_DESC_TOKENS, WOOSH_TE_PAD_ID);
        let mut mask = vec![0f32; WOOSH_DESC_TOKENS];
        mask[..valid].fill(1.0);
        (out, mask)
    }

    /// Byte-level BPE over one pre-tokenized word (same loop as h3_tokenizer,
    /// byte-exact vs the HF BPE: lowest merge rank first, ties leftmost).
    fn encode_bpe_word(&self, word: &str, output: &mut Vec<u32>) {
        if word.is_empty() {
            return;
        }
        let mut symbols: Vec<String> = word
            .as_bytes()
            .iter()
            .map(|&byte| self.byte_encoder[byte as usize].to_string())
            .collect();
        while symbols.len() > 1 {
            let mut best: Option<(usize, usize)> = None;
            for index in 0..(symbols.len() - 1) {
                if let Some(&rank) = self
                    .merge_ranks
                    .get(&(symbols[index].clone(), symbols[index + 1].clone()))
                {
                    if best.map_or(true, |(_, best_rank)| rank < best_rank) {
                        best = Some((index, rank));
                    }
                }
            }
            let Some((index, _)) = best else { break };
            let merged = symbols[index].clone() + &symbols[index + 1];
            symbols[index] = merged;
            symbols.remove(index + 1);
        }
        for symbol in &symbols {
            if let Some(&id) = self.token_to_id.get(symbol) {
                output.push(id);
                continue;
            }
            for ch in symbol.chars() {
                if let Some(&id) = self.token_to_id.get(ch.to_string().as_str()) {
                    output.push(id);
                }
            }
        }
    }
}

/// `\p{L}` approximation (same stance as h3_tokenizer: Alphabetic minus
/// numeric keeps Nl out of letter runs; ordinary prompts are unaffected).
fn is_letter(ch: char) -> bool {
    ch.is_alphabetic() && !ch.is_numeric()
}

/// Hand-rolled GPT-2 pre-tokenizer regex:
/// `'s|'t|'re|'ve|'m|'ll|'d| ?\p{L}+| ?\p{N}+| ?[^\s\p{L}\p{N}]+|\s+(?!\S)|\s+`
fn split_gpt2_words(text: &str) -> Vec<&str> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let slice = |start_index: usize, end_index: usize| -> &str {
        let start = chars[start_index].0;
        let end = if end_index >= chars.len() {
            text.len()
        } else {
            chars[end_index].0
        };
        &text[start..end]
    };

    let mut out = Vec::new();
    let mut pos = 0;
    while pos < chars.len() {
        let ch = chars[pos].1;

        // 's|'t|'re|'ve|'m|'ll|'d — case sensitive (no (?i:) in GPT-2).
        if ch == '\'' && pos + 1 < chars.len() {
            let next = chars[pos + 1].1;
            if matches!(next, 's' | 't' | 'm' | 'd') {
                out.push(slice(pos, pos + 2));
                pos += 2;
                continue;
            }
            if pos + 2 < chars.len() {
                let pair = (next, chars[pos + 2].1);
                if matches!(pair, ('r', 'e') | ('v', 'e') | ('l', 'l')) {
                    out.push(slice(pos, pos + 3));
                    pos += 3;
                    continue;
                }
            }
        }

        // ` ?\p{L}+` | ` ?\p{N}+` | ` ?[^\s\p{L}\p{N}]+` — one optional
        // leading space, then a homogeneous run.
        let probe = if ch == ' ' {
            (pos + 1 < chars.len()).then(|| chars[pos + 1].1)
        } else {
            Some(ch)
        };
        if let Some(first) = probe {
            let start_run = pos + usize::from(ch == ' ');
            let class: Option<fn(char) -> bool> = if is_letter(first) {
                Some(is_letter)
            } else if first.is_numeric() {
                Some(|c: char| c.is_numeric())
            } else if !first.is_whitespace() {
                Some(|c: char| !c.is_whitespace() && !is_letter(c) && !c.is_numeric())
            } else {
                None
            };
            if let Some(in_class) = class {
                let mut next_pos = start_run;
                while next_pos < chars.len() && in_class(chars[next_pos].1) {
                    next_pos += 1;
                }
                out.push(slice(pos, next_pos));
                pos = next_pos;
                continue;
            }
        }

        // `\s+(?!\S)` | `\s+` — whitespace runs; if more input follows, the
        // final whitespace char is left to prefix the next word.
        let mut count = 0;
        while pos + count < chars.len() && chars[pos + count].1.is_whitespace() {
            count += 1;
        }
        debug_assert!(count > 0);
        if count > 1 && pos + count < chars.len() {
            out.push(slice(pos, pos + count - 1));
            pos += count - 1;
        } else {
            out.push(slice(pos, pos + count));
            pos += count;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpt2_splitter_reference_cases() {
        assert_eq!(
            split_gpt2_words("Hello world 42"),
            vec!["Hello", " world", " 42"]
        );
        assert_eq!(split_gpt2_words("don't stop"), vec!["don", "'t", " stop"]);
        // Case-sensitive contractions: 'T does not match, so ' joins a symbol
        // run and T starts a letter run.
        assert_eq!(split_gpt2_words("DON'T"), vec!["DON", "'", "T"]);
        assert_eq!(
            split_gpt2_words("sword clash, metallic"),
            vec!["sword", " clash", ",", " metallic"]
        );
        assert_eq!(split_gpt2_words("a  b"), vec!["a", " ", " b"]);
        assert_eq!(split_gpt2_words("a   b"), vec!["a", "  ", " b"]);
        assert_eq!(split_gpt2_words("ab12cd"), vec!["ab", "12", "cd"]);
        assert_eq!(split_gpt2_words("x...y"), vec!["x", "...", "y"]);
        assert_eq!(split_gpt2_words("tail  "), vec!["tail", "  "]);
        assert_eq!(split_gpt2_words(""), Vec::<&str>::new());
    }

    #[test]
    fn empty_prompt_pads_to_77() {
        // encode_padded("") must yield [<s>, </s>, <pad> x 75] without
        // needing a loaded vocab (encode of "" is empty).
        let tokenizer = WooshTokenizer {
            token_to_id: HashMap::new(),
            merge_ranks: HashMap::new(),
            byte_encoder: gpt2_byte_to_unicode(),
        };
        let (ids, mask) = tokenizer.encode_padded("");
        assert_eq!(ids.len(), 77);
        assert_eq!(&ids[..2], &[0, 2]);
        assert!(ids[2..].iter().all(|&id| id == 1));
        assert_eq!(mask.iter().sum::<f32>(), 2.0);
    }
}
