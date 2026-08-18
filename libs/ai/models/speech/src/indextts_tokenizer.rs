//! IndexTTS-2.5 text tokenizer: a whisper-style tiktoken BPE, encode-only.
//!
//! The vocab file (`multilingual_zh_ja_yue_char_del.tiktoken`) is lines of
//! `base64(token_bytes) <space> rank`; 58836 base entries for this model.
//! Special tokens are NOT in the file — they are appended in a fixed order
//! copied from the reference (`indextts/utils/tokenizer.py::get_encoding`):
//! `<|endoftext|>`, `<|startoftranscript|>`, the 99 whisper languages
//! (`<|en|>` first), audio events, emotions, task tokens, `<|SPECIAL_TOKEN_1..30|>`,
//! TTS vocal tokens, and 1501 timestamp tokens — 60509 ids total, matching
//! `number_text_tokens` in config.yaml.
//!
//! Pre-tokenization is the GPT-2 pattern
//! `'s|'t|'re|'ve|'m|'ll|'d| ?\p{L}+| ?\p{N}+| ?[^\s\p{L}\p{N}]+|\s+(?!\S)|\s+`
//! hand-rolled below (no regex dependency). `\p{L}`/`\p{N}` are approximated
//! with `char::is_alphabetic`/`char::is_numeric`, which match exactly on the
//! model's target languages (en/zh/ja/es); combining marks (e.g. Arabic
//! harakat) can differ from the reference — acceptable for now and called
//! out here on purpose.
//!
//! Validated against the reference oracle: the fixed sentence in
//! `local/indextts_ref/dumps/meta.json` token-exact (see tests).

use crate::error::{DiffusionError, Result};
use std::collections::HashMap;
use std::path::Path;

/// Whisper's 99 languages in dictionary order — the order defines the
/// special-token ids (`<|en|>` is base+2).
const LANGUAGES: [&str; 99] = [
    "en", "zh", "de", "es", "ru", "ko", "fr", "ja", "pt", "tr", "pl", "ca", "nl", "ar", "sv",
    "it", "id", "hi", "fi", "vi", "he", "uk", "el", "ms", "cs", "ro", "da", "hu", "ta", "no",
    "th", "ur", "hr", "bg", "lt", "la", "mi", "ml", "cy", "sk", "te", "fa", "lv", "bn", "sr",
    "az", "sl", "kn", "et", "mk", "br", "eu", "is", "hy", "ne", "mn", "bs", "kk", "sq", "sw",
    "gl", "mr", "pa", "si", "km", "sn", "yo", "so", "af", "oc", "ka", "be", "tg", "sd", "gu",
    "am", "yi", "lo", "uz", "fo", "ht", "ps", "tk", "nn", "mt", "sa", "lb", "my", "bo", "tl",
    "mg", "as", "tt", "haw", "ln", "ha", "ba", "jw", "su",
];

const AUDIO_EVENTS: [&str; 11] = [
    "ASR", "AED", "SER", "Speech", "/Speech", "BGM", "/BGM", "Laughter", "/Laughter",
    "Applause", "/Applause",
];

const EMOTIONS: [&str; 4] = ["HAPPY", "SAD", "ANGRY", "NEUTRAL"];

const TASKS: [&str; 6] = [
    "translate", "transcribe", "startoflm", "startofprev", "nospeech", "notimestamps",
];

const TTS_VOCAL: [&str; 20] = [
    "TTS/B", "TTS/O", "TTS/Q", "TTS/A", "TTS/CO", "TTS/CL", "TTS/H", "TTS/SP01", "TTS/SP02",
    "TTS/SP03", "TTS/SP04", "TTS/SP05", "TTS/SP06", "TTS/SP07", "TTS/SP08", "TTS/SP09",
    "TTS/SP10", "TTS/SP11", "TTS/SP12", "TTS/SP13",
];

pub struct IndexTtsTokenizer {
    ranks: HashMap<Vec<u8>, u32>,
    specials: HashMap<String, u32>,
    n_vocab: u32,
}

impl IndexTtsTokenizer {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let text = std::fs::read_to_string(path.as_ref())
            .map_err(|e| DiffusionError::io(path.as_ref(), format!("tokenizer: {e}")))?;
        let mut ranks = HashMap::new();
        for line in text.lines() {
            if line.is_empty() {
                continue;
            }
            let mut parts = line.split_ascii_whitespace();
            let (Some(b64), Some(rank)) = (parts.next(), parts.next()) else {
                return Err(DiffusionError::model(format!(
                    "tokenizer: bad vocab line {line:?}"
                )));
            };
            let bytes = base64_decode(b64).ok_or_else(|| {
                DiffusionError::model(format!("tokenizer: bad base64 {b64:?}"))
            })?;
            let rank: u32 = rank.parse().map_err(|_| {
                DiffusionError::model(format!("tokenizer: bad rank in {line:?}"))
            })?;
            ranks.insert(bytes, rank);
        }
        let mut n_vocab = ranks.len() as u32;
        let mut specials = HashMap::new();
        let push = |name: String, n: &mut u32, specials: &mut HashMap<String, u32>| {
            specials.insert(name, *n);
            *n += 1;
        };
        push("<|endoftext|>".into(), &mut n_vocab, &mut specials);
        push("<|startoftranscript|>".into(), &mut n_vocab, &mut specials);
        for lang in LANGUAGES {
            push(format!("<|{lang}|>"), &mut n_vocab, &mut specials);
        }
        for event in AUDIO_EVENTS {
            push(format!("<|{event}|>"), &mut n_vocab, &mut specials);
        }
        for emotion in EMOTIONS {
            push(format!("<|{emotion}|>"), &mut n_vocab, &mut specials);
        }
        for task in TASKS {
            push(format!("<|{task}|>"), &mut n_vocab, &mut specials);
        }
        for i in 1..=30 {
            push(format!("<|SPECIAL_TOKEN_{i}|>"), &mut n_vocab, &mut specials);
        }
        for tts in TTS_VOCAL {
            push(format!("<|{tts}|>"), &mut n_vocab, &mut specials);
        }
        for i in 0..1501u32 {
            push(
                format!("<|{:.2}|>", i as f64 * 0.02),
                &mut n_vocab,
                &mut specials,
            );
        }
        Ok(Self {
            ranks,
            specials,
            n_vocab,
        })
    }

    pub fn n_vocab(&self) -> u32 {
        self.n_vocab
    }

    /// The language index used for the GPT `lang_embedding` (whisper language
    /// dictionary position; unknown languages fall back to "common" upstream —
    /// here `None`).
    pub fn lang_index(lang: &str) -> Option<u32> {
        let lang = lang.to_ascii_lowercase();
        LANGUAGES
            .iter()
            .position(|l| **l == lang)
            .map(|index| index as u32)
    }

    /// Encodes with all special tokens enabled (`allowed_special='all'`):
    /// exact `<|...|>` occurrences of known specials become single ids.
    pub fn encode(&self, text: &str) -> Vec<u32> {
        let mut out = Vec::new();
        let mut rest = text;
        while !rest.is_empty() {
            // Find the earliest special-token occurrence.
            let mut best: Option<(usize, &str, u32)> = None;
            if let Some(start) = rest.find("<|") {
                if let Some(end_rel) = rest[start..].find("|>") {
                    let candidate = &rest[start..start + end_rel + 2];
                    if let Some(&id) = self.specials.get(candidate) {
                        best = Some((start, candidate, id));
                    }
                }
            }
            match best {
                Some((start, candidate, id)) => {
                    self.encode_ordinary(&rest[..start], &mut out);
                    out.push(id);
                    rest = &rest[start + candidate.len()..];
                }
                None => {
                    self.encode_ordinary(rest, &mut out);
                    rest = "";
                }
            }
        }
        out
    }

    fn encode_ordinary(&self, text: &str, out: &mut Vec<u32>) {
        for piece in Gpt2Splitter::new(text) {
            self.bpe(piece.as_bytes(), out);
        }
    }

    /// Standard tiktoken merge: repeatedly join the adjacent pair with the
    /// lowest rank until no mergeable pair remains.
    fn bpe(&self, piece: &[u8], out: &mut Vec<u32>) {
        if piece.is_empty() {
            return;
        }
        if let Some(&rank) = self.ranks.get(piece) {
            out.push(rank);
            return;
        }
        // parts[i] = start offset of part i; sentinel at the end.
        let mut parts: Vec<usize> = (0..=piece.len()).collect();
        loop {
            let mut best_rank = u32::MAX;
            let mut best_index = usize::MAX;
            for i in 0..parts.len().saturating_sub(2) {
                let bytes = &piece[parts[i]..parts[i + 2]];
                if let Some(&rank) = self.ranks.get(bytes) {
                    if rank < best_rank {
                        best_rank = rank;
                        best_index = i;
                    }
                }
            }
            if best_index == usize::MAX {
                break;
            }
            parts.remove(best_index + 1);
        }
        for i in 0..parts.len() - 1 {
            let bytes = &piece[parts[i]..parts[i + 1]];
            match self.ranks.get(bytes) {
                Some(&rank) => out.push(rank),
                // A byte sequence outside the vocab (shouldn't happen: the
                // base vocab contains all single bytes); drop it.
                None => {}
            }
        }
    }
}

/// Hand-rolled GPT-2 pre-tokenizer:
/// `'s|'t|'re|'ve|'m|'ll|'d| ?\p{L}+| ?\p{N}+| ?[^\s\p{L}\p{N}]+|\s+(?!\S)|\s+`
struct Gpt2Splitter<'a> {
    text: &'a str,
    pos: usize,
}

impl<'a> Gpt2Splitter<'a> {
    fn new(text: &'a str) -> Self {
        Self { text, pos: 0 }
    }
}

fn is_letter(c: char) -> bool {
    c.is_alphabetic()
}

fn is_number(c: char) -> bool {
    c.is_numeric()
}

impl<'a> Iterator for Gpt2Splitter<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        let rest = &self.text[self.pos..];
        if rest.is_empty() {
            return None;
        }
        let start = self.pos;
        let mut chars = rest.chars();
        let first = chars.next().unwrap();

        // Contractions: 's 't 're 've 'm 'll 'd (case-sensitive, as in the
        // reference pattern; input is lowercased upstream anyway).
        if first == '\'' {
            for suffix in ["'s", "'t", "'re", "'ve", "'m", "'ll", "'d"] {
                if rest.starts_with(suffix) {
                    self.pos += suffix.len();
                    return Some(&self.text[start..self.pos]);
                }
            }
        }

        // ` ?\p{L}+`, ` ?\p{N}+`, ` ?[^\s\p{L}\p{N}]+` — one optional leading
        // ASCII space, then a run of one class.
        let (lead_space, class_first) = if first == ' ' {
            match chars.next() {
                Some(c) => (true, c),
                None => {
                    // Lone trailing space: falls through to the whitespace arm.
                    self.pos += 1;
                    return Some(&self.text[start..self.pos]);
                }
            }
        } else {
            (false, first)
        };

        if !class_first.is_whitespace() {
            let class: fn(char) -> bool = if is_letter(class_first) {
                is_letter
            } else if is_number(class_first) {
                is_number
            } else {
                |c: char| !c.is_whitespace() && !is_letter(c) && !is_number(c)
            };
            let mut end = start + lead_space as usize + class_first.len_utf8();
            for c in self.text[end..].chars() {
                if class(c) && !c.is_whitespace() {
                    end += c.len_utf8();
                } else {
                    break;
                }
            }
            self.pos = end;
            return Some(&self.text[start..end]);
        }

        // Whitespace run (first char is whitespace, or the lone-space case
        // above already returned). `\s+(?!\S)` keeps the final whitespace
        // char for the next token when non-space follows.
        let mut end = start;
        for c in self.text[start..].chars() {
            if c.is_whitespace() {
                end += c.len_utf8();
            } else {
                break;
            }
        }
        let followed_by_nonspace = end < self.text.len();
        if followed_by_nonspace {
            // Leave the last whitespace char to prefix the next token
            // (`\s+(?!\S)` semantics) — unless the run is a single char, in
            // which case `\s+` takes it whole.
            let last_len = self.text[start..end].chars().last().unwrap().len_utf8();
            if end - last_len > start {
                end -= last_len;
            }
        }
        self.pos = end;
        Some(&self.text[start..end])
    }
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(b: u8) -> Option<u32> {
        match b {
            b'A'..=b'Z' => Some((b - b'A') as u32),
            b'a'..=b'z' => Some((b - b'a' + 26) as u32),
            b'0'..=b'9' => Some((b - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let mut chunk = [0u32; 4];
    let mut n = 0;
    for &b in bytes {
        if b == b'=' {
            break;
        }
        chunk[n] = val(b)?;
        n += 1;
        if n == 4 {
            let v = (chunk[0] << 18) | (chunk[1] << 12) | (chunk[2] << 6) | chunk[3];
            out.extend_from_slice(&[(v >> 16) as u8, (v >> 8) as u8, v as u8]);
            n = 0;
        }
    }
    match n {
        0 => {}
        2 => {
            let v = (chunk[0] << 18) | (chunk[1] << 12);
            out.push((v >> 16) as u8);
        }
        3 => {
            let v = (chunk[0] << 18) | (chunk[1] << 12) | (chunk[2] << 6);
            out.extend_from_slice(&[(v >> 16) as u8, (v >> 8) as u8]);
        }
        _ => return None,
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn splitter(text: &str) -> Vec<&str> {
        Gpt2Splitter::new(text).collect()
    }

    #[test]
    fn gpt2_pattern_splits() {
        assert_eq!(splitter("hello world"), vec!["hello", " world"]);
        assert_eq!(splitter("it's 42 items."), vec!["it", "'s", " 42", " items", "."]);
        assert_eq!(splitter("a  b"), vec!["a", " ", " b"]);
        assert_eq!(splitter("a \n"), vec!["a", " \n"]);
        assert_eq!(splitter("ab12cd"), vec!["ab", "12", "cd"]);
    }

    #[test]
    fn oracle_sentence_token_exact() {
        // Vocab file lives with the reference checkout; skip when absent
        // (CI machines without the reference env).
        let path = crate::indextts::reference_checkpoints_dir()
            .join("multilingual_zh_ja_yue_char_del.tiktoken");
        if !path.is_file() {
            eprintln!("skipping oracle_sentence_token_exact: {path:?} missing");
            return;
        }
        let tok = IndexTtsTokenizer::load(&path).unwrap();
        assert_eq!(tok.n_vocab(), 60509);
        assert_eq!(IndexTtsTokenizer::lang_index("EN"), Some(0));
        // Reference: dumps/meta.json text_tokens for
        // "<|en|> the old lighthouse keeper smiled as the storm finally passed."
        // (stop token 1 appended by the pipeline, not the tokenizer).
        let ids = tok.encode("<|en|> the old lighthouse keeper smiled as the storm finally passed.");
        assert_eq!(
            ids,
            vec![58838, 264, 1331, 45800, 37415, 33981, 382, 264, 7555, 2707, 4630, 13]
        );
    }
}
