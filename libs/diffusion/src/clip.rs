use crate::assets::CLIP_MERGES_UTF8;
use crate::{DiffusionError, Result};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipTokenChunk {
    pub token_ids: Vec<i32>,
    pub eos_index: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipTokenizedPrompt {
    pub raw_token_ids: Vec<i32>,
    pub chunks: Vec<ClipTokenChunk>,
}

#[derive(Clone, Debug)]
pub struct ClipTokenizer {
    byte_encoder: Vec<String>,
    encoder: HashMap<String, i32>,
    decoder: HashMap<i32, String>,
    bpe_ranks: HashMap<(String, String), i32>,
    special_tokens: Vec<String>,
}

impl ClipTokenizer {
    pub const UNK_TOKEN: &'static str = "<|endoftext|>";
    pub const BOS_TOKEN: &'static str = "<|startoftext|>";
    pub const EOS_TOKEN: &'static str = "<|endoftext|>";
    pub const PAD_TOKEN: &'static str = "<|endoftext|>";

    pub const UNK_TOKEN_ID: i32 = 49_407;
    pub const BOS_TOKEN_ID: i32 = 49_406;
    pub const EOS_TOKEN_ID: i32 = 49_407;
    pub const PAD_TOKEN_ID: i32 = 49_407;

    pub fn new() -> Result<Self> {
        Self::from_merges_utf8(CLIP_MERGES_UTF8)
    }

    pub fn from_merges_utf8(merges_utf8: &str) -> Result<Self> {
        let byte_unicode_pairs = bytes_to_unicode()?;
        let mut byte_encoder = vec![String::new(); 256];
        let mut byte_decoder = HashMap::new();
        for (byte, value) in &byte_unicode_pairs {
            byte_encoder[*byte as usize] = value.clone();
            byte_decoder.insert(value.clone(), *byte);
        }

        let mut merges: Vec<&str> = merges_utf8.lines().collect();
        if merges.len() != 48_895 {
            return Err(DiffusionError::model(format!(
                "clip merge count mismatch: expected 48895 lines, got {}",
                merges.len()
            )));
        }
        merges.remove(0);

        let mut merge_pairs = Vec::with_capacity(merges.len());
        for merge in merges {
            let (left, right) = merge.split_once(' ').ok_or_else(|| {
                DiffusionError::model(format!("invalid clip merge entry '{}'", merge))
            })?;
            merge_pairs.push((left.to_string(), right.to_string()));
        }

        let mut vocab = Vec::with_capacity(byte_unicode_pairs.len() * 2 + merge_pairs.len() + 2);
        for (_, value) in &byte_unicode_pairs {
            vocab.push(value.clone());
        }
        for (_, value) in &byte_unicode_pairs {
            vocab.push(format!("{value}</w>"));
        }
        for (left, right) in &merge_pairs {
            vocab.push(format!("{left}{right}"));
        }
        vocab.push(Self::BOS_TOKEN.to_string());
        vocab.push(Self::EOS_TOKEN.to_string());

        let mut encoder = HashMap::with_capacity(vocab.len());
        let mut decoder = HashMap::with_capacity(vocab.len());
        for (index, token) in vocab.into_iter().enumerate() {
            encoder.insert(token.clone(), index as i32);
            decoder.insert(index as i32, token);
        }

        let mut bpe_ranks = HashMap::with_capacity(merge_pairs.len());
        for (rank, pair) in merge_pairs.into_iter().enumerate() {
            bpe_ranks.insert(pair, rank as i32);
        }

        Ok(Self {
            byte_encoder,
            encoder,
            decoder,
            bpe_ranks,
            special_tokens: vec![Self::BOS_TOKEN.to_string(), Self::EOS_TOKEN.to_string()],
        })
    }

    pub fn vocab_size(&self) -> usize {
        self.encoder.len()
    }

    pub fn encode(&self, text: &str) -> Result<Vec<i32>> {
        let mut text = whitespace_clean(text);
        text.make_ascii_lowercase();

        let mut token_ids = Vec::new();
        for chunk in split_with_special_tokens(&text, &self.special_tokens) {
            if chunk.is_empty() {
                continue;
            }
            if self.is_special_token(&chunk) {
                let token_id = self.encoder.get(&chunk).copied().ok_or_else(|| {
                    DiffusionError::model(format!("missing special clip token '{}'", chunk))
                })?;
                token_ids.push(token_id);
                continue;
            }

            for token in token_split(&chunk) {
                let mut byte_encoded = String::new();
                for byte in token.as_bytes() {
                    byte_encoded.push_str(&self.byte_encoder[*byte as usize]);
                }
                let bpe = self.bpe(&byte_encoded);
                for piece in bpe.split(' ') {
                    let piece_id = self.encoder.get(piece).copied().ok_or_else(|| {
                        DiffusionError::model(format!("missing clip vocab entry '{}'", piece))
                    })?;
                    token_ids.push(piece_id);
                }
            }
        }
        Ok(token_ids)
    }

    pub fn tokenize(&self, text: &str, max_length: usize, padding: bool) -> Result<Vec<i32>> {
        if max_length == 1 {
            return Err(DiffusionError::workflow(
                "clip max_length must be at least 2",
            ));
        }

        let mut tokens = self.encode(text)?;
        tokens.insert(0, Self::BOS_TOKEN_ID);
        if max_length > 0 {
            if tokens.len() > max_length - 1 {
                tokens.truncate(max_length - 1);
                tokens.push(Self::EOS_TOKEN_ID);
            } else {
                tokens.push(Self::EOS_TOKEN_ID);
                if padding {
                    tokens.resize(max_length, Self::PAD_TOKEN_ID);
                }
            }
        }
        Ok(tokens)
    }

    pub fn tokenize_chunks(
        &self,
        text: &str,
        max_length: usize,
        padding: bool,
    ) -> Result<ClipTokenizedPrompt> {
        if max_length < 2 {
            return Err(DiffusionError::workflow(
                "clip chunk length must be at least 2",
            ));
        }

        let raw_token_ids = self.encode(text)?;
        let flat = if padding {
            pad_chunked_token_ids(&raw_token_ids, max_length)?
        } else {
            self.tokenize(text, max_length, false)?
        };

        let mut chunks = Vec::new();
        if padding {
            for token_ids in flat.chunks(max_length) {
                if token_ids.is_empty() {
                    continue;
                }
                chunks.push(ClipTokenChunk {
                    token_ids: token_ids.to_vec(),
                    eos_index: first_eos_index(token_ids),
                });
            }
        } else if !flat.is_empty() {
            let eos_index = first_eos_index(&flat);
            chunks.push(ClipTokenChunk {
                token_ids: flat,
                eos_index,
            });
        }

        Ok(ClipTokenizedPrompt {
            raw_token_ids,
            chunks,
        })
    }

    pub fn decode_approx(&self, token_ids: &[i32]) -> Result<String> {
        let mut text = String::new();
        for token_id in token_ids {
            if *token_id == Self::BOS_TOKEN_ID || *token_id == Self::EOS_TOKEN_ID {
                continue;
            }
            let token = self.decoder.get(token_id).ok_or_else(|| {
                DiffusionError::model(format!("unknown clip token id {}", token_id))
            })?;
            if let Some(stripped) = token.strip_suffix("</w>") {
                text.push_str(stripped);
                text.push(' ');
            } else {
                text.push_str(token);
            }
        }
        Ok(text.trim().replace(" ,", ","))
    }

    fn is_special_token(&self, token: &str) -> bool {
        self.special_tokens.iter().any(|value| value == token)
    }

    fn bpe(&self, token: &str) -> String {
        let mut word = split_chars(token);
        if word.is_empty() {
            return String::new();
        }
        let last = word.pop().unwrap();
        word.push(format!("{last}</w>"));

        let mut pairs = get_pairs(&word);
        if pairs.is_empty() {
            return format!("{token}</w>");
        }

        loop {
            let mut best: Option<(&(String, String), &i32)> = None;
            for pair in &pairs {
                if let Some(rank) = self.bpe_ranks.get(pair) {
                    match best {
                        Some((_, best_rank)) if rank >= best_rank => {}
                        _ => best = Some((pair, rank)),
                    }
                }
            }

            let Some(((first, second), _)) = best else {
                break;
            };

            let mut new_word = Vec::with_capacity(word.len());
            let mut index = 0usize;
            while index < word.len() {
                if word[index] == *first
                    && index + 1 < word.len()
                    && word[index + 1] == *second
                {
                    new_word.push(format!("{first}{second}"));
                    index += 2;
                } else {
                    new_word.push(word[index].clone());
                    index += 1;
                }
            }

            word = new_word;
            if word.len() == 1 {
                break;
            }
            pairs = get_pairs(&word);
        }

        word.join(" ")
    }
}

fn unicode_string(value: u32) -> Result<String> {
    char::from_u32(value)
        .map(|value| value.to_string())
        .ok_or_else(|| DiffusionError::model(format!("invalid unicode scalar U+{value:04X}")))
}

fn bytes_to_unicode() -> Result<Vec<(u8, String)>> {
    let mut pairs = Vec::new();
    let mut byte_set = HashSet::new();

    for byte in b'!'..=b'~' {
        byte_set.insert(byte);
        pairs.push((byte, unicode_string(byte as u32)?));
    }
    for byte in 161u8..=172u8 {
        byte_set.insert(byte);
        pairs.push((byte, unicode_string(byte as u32)?));
    }
    for byte in 174u8..=255u8 {
        byte_set.insert(byte);
        pairs.push((byte, unicode_string(byte as u32)?));
    }

    let mut extra = 0u32;
    for byte in 0u8..=255u8 {
        if byte_set.contains(&byte) {
            continue;
        }
        pairs.push((byte, unicode_string(256 + extra)?));
        extra += 1;
    }

    Ok(pairs)
}

fn whitespace_clean(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn get_pairs(pieces: &[String]) -> HashSet<(String, String)> {
    let mut pairs = HashSet::new();
    for window in pieces.windows(2) {
        pairs.insert((window[0].clone(), window[1].clone()));
    }
    pairs
}

fn split_chars(text: &str) -> Vec<String> {
    text.chars().map(|value| value.to_string()).collect()
}

fn split_with_special_tokens(text: &str, special_tokens: &[String]) -> Vec<String> {
    let mut result = Vec::new();
    let mut cursor = 0usize;
    while cursor < text.len() {
        let mut next_pos = text.len();
        let mut matched = None::<&str>;

        for token in special_tokens {
            if let Some(offset) = text[cursor..].find(token) {
                let pos = cursor + offset;
                if pos < next_pos {
                    next_pos = pos;
                    matched = Some(token.as_str());
                }
            }
        }

        if next_pos > cursor {
            result.push(text[cursor..next_pos].to_string());
        }
        if let Some(token) = matched {
            result.push(token.to_string());
            cursor = next_pos + token.len();
        } else {
            break;
        }
    }
    result
}

fn token_split(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens = Vec::new();
    let mut index = 0usize;

    while index < chars.len() {
        let ch = chars[index];
        if ch.is_whitespace() {
            index += 1;
            continue;
        }

        if ch == '\'' {
            if let Some((suffix, next)) = apostrophe_suffix(&chars, index) {
                tokens.push(suffix.to_string());
                index = next;
                continue;
            }
        }

        if ch.is_alphabetic() {
            let start = index;
            index += 1;
            while index < chars.len() && chars[index].is_alphabetic() {
                index += 1;
            }
            tokens.push(chars[start..index].iter().collect());
            continue;
        }

        if ch.is_numeric() {
            tokens.push(ch.to_string());
            index += 1;
            continue;
        }

        let start = index;
        index += 1;
        while index < chars.len()
            && !chars[index].is_whitespace()
            && !chars[index].is_alphabetic()
            && !chars[index].is_numeric()
        {
            index += 1;
        }
        tokens.push(chars[start..index].iter().collect());
    }

    tokens
}

fn apostrophe_suffix(chars: &[char], index: usize) -> Option<(&'static str, usize)> {
    let rest: String = chars[index..].iter().take(3).collect();
    for suffix in ["'re", "'ve", "'ll", "'s", "'t", "'m", "'d"] {
        if rest.starts_with(suffix) {
            return Some((suffix, index + suffix.chars().count()));
        }
    }
    None
}

fn pad_chunked_token_ids(raw_token_ids: &[i32], max_length: usize) -> Result<Vec<i32>> {
    if max_length < 2 {
        return Err(DiffusionError::workflow(
            "clip chunk length must be at least 2",
        ));
    }

    let payload = max_length - 2;
    let chunk_count = raw_token_ids.len().div_ceil(payload).max(1);
    let total_len = max_length
        .checked_mul(chunk_count)
        .ok_or_else(|| DiffusionError::workflow("clip chunked token length overflow"))?;

    let mut token_ids = Vec::with_capacity(total_len);
    token_ids.push(ClipTokenizer::BOS_TOKEN_ID);
    let mut raw_index = 0usize;
    for position in 1..total_len {
        if raw_index >= raw_token_ids.len() {
            break;
        }
        if position % max_length == 0 {
            token_ids.push(ClipTokenizer::BOS_TOKEN_ID);
        } else if position % max_length == max_length - 1 {
            token_ids.push(ClipTokenizer::EOS_TOKEN_ID);
        } else {
            token_ids.push(raw_token_ids[raw_index]);
            raw_index += 1;
        }
    }
    token_ids.push(ClipTokenizer::EOS_TOKEN_ID);
    token_ids.resize(total_len, ClipTokenizer::PAD_TOKEN_ID);
    Ok(token_ids)
}

fn first_eos_index(token_ids: &[i32]) -> usize {
    token_ids
        .iter()
        .position(|value| *value == ClipTokenizer::EOS_TOKEN_ID)
        .unwrap_or(token_ids.len().saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::{ClipTokenizer, ClipTokenizedPrompt};
    use crate::assets::{CLIP_MERGES_UTF8, T5_TOKENIZER_JSON};

    #[test]
    fn exposes_generated_tokenizer_assets() {
        assert!(CLIP_MERGES_UTF8.starts_with("#version: 0.2"));
        assert!(T5_TOKENIZER_JSON.starts_with('{'));
    }

    #[test]
    fn builds_reference_clip_vocab() {
        let tokenizer = ClipTokenizer::new().unwrap();
        assert_eq!(tokenizer.vocab_size(), 49_408);
        assert_eq!(ClipTokenizer::BOS_TOKEN_ID, 49_406);
        assert_eq!(ClipTokenizer::EOS_TOKEN_ID, 49_407);
        assert_eq!(ClipTokenizer::UNK_TOKEN, ClipTokenizer::EOS_TOKEN);
        assert_eq!(ClipTokenizer::PAD_TOKEN, ClipTokenizer::EOS_TOKEN);
    }

    #[test]
    fn tokenizes_and_pads_single_flux_chunk() {
        let tokenizer = ClipTokenizer::new().unwrap();
        let tokenized = tokenizer.tokenize_chunks("test", 77, true).unwrap();
        assert_eq!(tokenized.raw_token_ids.len(), 1);
        assert_eq!(tokenized.chunks.len(), 1);
        assert_eq!(tokenized.chunks[0].token_ids.len(), 77);
        assert_eq!(tokenized.chunks[0].token_ids[0], ClipTokenizer::BOS_TOKEN_ID);
        assert_eq!(tokenized.chunks[0].eos_index, 2);
        assert_eq!(
            &tokenized.chunks[0].token_ids[..5],
            &[49_406, 1_628, 49_407, 49_407, 49_407]
        );
    }

    #[test]
    fn chunk_padding_matches_reference_boundaries() {
        let tokenizer = ClipTokenizer::new().unwrap();
        let long_prompt = vec!["test"; 90].join(" ");
        let tokenized = tokenizer.tokenize_chunks(&long_prompt, 77, true).unwrap();
        assert_eq!(tokenized.chunks.len(), 2);
        assert_eq!(tokenized.chunks[0].token_ids[0], ClipTokenizer::BOS_TOKEN_ID);
        assert_eq!(tokenized.chunks[1].token_ids[0], ClipTokenizer::BOS_TOKEN_ID);
        assert_eq!(tokenized.chunks[0].token_ids[76], ClipTokenizer::EOS_TOKEN_ID);
        assert!(tokenized.chunks[1].eos_index < 76);
    }

    #[test]
    fn decode_approx_round_trips_simple_prompt() {
        let tokenizer = ClipTokenizer::new().unwrap();
        let ClipTokenizedPrompt { chunks, .. } = tokenizer.tokenize_chunks("hello world", 77, true).unwrap();
        let decoded = tokenizer.decode_approx(&chunks[0].token_ids[..chunks[0].eos_index]).unwrap();
        assert_eq!(decoded, "hello world");
    }
}
