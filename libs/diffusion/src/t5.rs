use crate::assets::T5_TOKENIZER_JSON;
use crate::{DiffusionError, Result};
use makepad_micro_serde::{DeJson, JsonValue};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct T5TokenizedPrompt {
    pub raw_token_ids: Vec<i32>,
    pub token_ids: Vec<i32>,
    pub attention_mask: Vec<i32>,
    pub eos_index: usize,
}

#[derive(Clone, Debug)]
pub struct T5Tokenizer {
    metaspace_replacement: String,
    add_prefix_space: bool,
    pieces: Vec<String>,
    scores: Vec<f32>,
    piece_char_lens: Vec<usize>,
    token_to_id: HashMap<String, i32>,
    prefix_index: HashMap<char, Vec<i32>>,
    special_tokens: Vec<String>,
    pad_token_id: i32,
    eos_token_id: i32,
    unk_token_id: i32,
}

impl T5Tokenizer {
    pub fn new() -> Result<Self> {
        Self::from_json(T5_TOKENIZER_JSON)
    }

    pub fn from_json(text: &str) -> Result<Self> {
        let root = HashMap::<String, JsonValue>::deserialize_json(text).map_err(|err| {
            DiffusionError::model(format!("invalid t5 tokenizer json: {:?}", err))
        })?;

        let pre_tokenizer = json_object(root.get("pre_tokenizer"), "t5.pre_tokenizer")?;
        let pre_tokenizer_type = json_string(pre_tokenizer.get("type"), "t5.pre_tokenizer.type")?;
        if pre_tokenizer_type != "Metaspace" {
            return Err(DiffusionError::model(format!(
                "unsupported t5 pre_tokenizer {}",
                pre_tokenizer_type
            )));
        }
        let metaspace_replacement = json_string(
            pre_tokenizer.get("replacement"),
            "t5.pre_tokenizer.replacement",
        )?;
        let add_prefix_space = json_bool(
            pre_tokenizer.get("add_prefix_space"),
            "t5.pre_tokenizer.add_prefix_space",
        )?;

        let model = json_object(root.get("model"), "t5.model")?;
        let model_type = json_string(model.get("type"), "t5.model.type")?;
        if model_type != "Unigram" {
            return Err(DiffusionError::model(format!(
                "unsupported t5 tokenizer model {}",
                model_type
            )));
        }
        let unk_token_id = json_i32(model.get("unk_id"), "t5.model.unk_id")?;
        let vocab = json_array(model.get("vocab"), "t5.model.vocab")?;

        let mut pieces = Vec::with_capacity(vocab.len());
        let mut scores = Vec::with_capacity(vocab.len());
        let mut piece_char_lens = Vec::with_capacity(vocab.len());
        let mut token_to_id = HashMap::with_capacity(vocab.len());
        let mut prefix_index: HashMap<char, Vec<i32>> = HashMap::new();
        for (index, entry) in vocab.iter().enumerate() {
            let pair = json_array(Some(entry), &format!("t5.model.vocab[{index}]"))?;
            if pair.len() != 2 {
                return Err(DiffusionError::model(format!(
                    "t5.model.vocab[{index}] must have 2 elements"
                )));
            }
            let piece = json_string(pair.first(), &format!("t5.model.vocab[{index}][0]"))?;
            let score = json_f32(pair.get(1), &format!("t5.model.vocab[{index}][1]"))?;
            let token_id = i32::try_from(index).map_err(|_| {
                DiffusionError::model(format!("t5 vocab index {} exceeds i32", index))
            })?;
            piece_char_lens.push(piece.chars().count());
            if let Some(first_char) = piece.chars().next() {
                prefix_index.entry(first_char).or_default().push(token_id);
            }
            token_to_id.insert(piece.clone(), token_id);
            pieces.push(piece);
            scores.push(score);
        }
        for token_ids in prefix_index.values_mut() {
            token_ids.sort_unstable_by(|lhs, rhs| {
                piece_char_lens[*rhs as usize]
                    .cmp(&piece_char_lens[*lhs as usize])
                    .then_with(|| lhs.cmp(rhs))
            });
        }

        let added_tokens = json_array(root.get("added_tokens"), "t5.added_tokens")?;
        let mut special_tokens = Vec::new();
        for (index, token) in added_tokens.iter().enumerate() {
            let token = json_object(Some(token), &format!("t5.added_tokens[{index}]"))?;
            if !json_bool(
                token.get("special"),
                &format!("t5.added_tokens[{index}].special"),
            )? {
                continue;
            }
            special_tokens.push(json_string(
                token.get("content"),
                &format!("t5.added_tokens[{index}].content"),
            )?);
        }
        special_tokens.sort_by(|lhs, rhs| rhs.len().cmp(&lhs.len()).then_with(|| lhs.cmp(rhs)));
        special_tokens.dedup();

        let pad_token_id = *token_to_id
            .get("<pad>")
            .ok_or_else(|| DiffusionError::model("t5 tokenizer is missing <pad>".to_string()))?;
        let eos_token_id = *token_to_id
            .get("</s>")
            .ok_or_else(|| DiffusionError::model("t5 tokenizer is missing </s>".to_string()))?;

        Ok(Self {
            metaspace_replacement,
            add_prefix_space,
            pieces,
            scores,
            piece_char_lens,
            token_to_id,
            prefix_index,
            special_tokens,
            pad_token_id,
            eos_token_id,
            unk_token_id,
        })
    }

    pub fn vocab_size(&self) -> usize {
        self.pieces.len()
    }

    pub fn token_to_id(&self, token: &str) -> Option<i32> {
        self.token_to_id.get(token).copied()
    }

    pub fn encode(&self, text: &str) -> Result<Vec<i32>> {
        let mut token_ids = Vec::new();
        for chunk in split_with_special_tokens(text, &self.special_tokens) {
            if chunk.is_empty() {
                continue;
            }
            if self.special_tokens.iter().any(|token| token == &chunk) {
                let token_id = self.token_to_id(&chunk).ok_or_else(|| {
                    DiffusionError::model(format!("missing t5 special token '{}'", chunk))
                })?;
                token_ids.push(token_id);
                continue;
            }
            token_ids.extend(self.encode_plain_text(&chunk)?);
        }
        Ok(token_ids)
    }

    pub fn tokenize(
        &self,
        text: &str,
        max_length: usize,
        padding: bool,
    ) -> Result<T5TokenizedPrompt> {
        if max_length == 0 {
            return Err(DiffusionError::workflow("t5 max_length must be at least 1"));
        }

        let raw_token_ids = self.encode(text)?;
        let mut token_ids = raw_token_ids.clone();
        if token_ids.len() >= max_length {
            token_ids.truncate(max_length - 1);
        }
        let eos_index = token_ids.len();
        token_ids.push(self.eos_token_id);

        let mut attention_mask = vec![1; token_ids.len()];
        if padding {
            token_ids.resize(max_length, self.pad_token_id);
            attention_mask.resize(max_length, 0);
        }

        Ok(T5TokenizedPrompt {
            raw_token_ids,
            token_ids,
            attention_mask,
            eos_index,
        })
    }

    fn encode_plain_text(&self, text: &str) -> Result<Vec<i32>> {
        let normalized = basic_t5_normalize(text);
        if normalized.is_empty() {
            return Ok(Vec::new());
        }

        let mut metaspace = normalized.replace(' ', &self.metaspace_replacement);
        if self.add_prefix_space && !metaspace.starts_with(&self.metaspace_replacement) {
            metaspace.insert_str(0, &self.metaspace_replacement);
        }
        self.encode_unigram(&metaspace)
    }

    fn encode_unigram(&self, text: &str) -> Result<Vec<i32>> {
        if text.is_empty() {
            return Ok(Vec::new());
        }

        let mut char_offsets = text
            .char_indices()
            .map(|(offset, _)| offset)
            .collect::<Vec<_>>();
        char_offsets.push(text.len());
        let char_count = char_offsets.len() - 1;
        let mut best_scores = vec![f32::NEG_INFINITY; char_count + 1];
        let mut best_paths = vec![None::<(i32, usize)>; char_count];
        best_scores[char_count] = 0.0;

        for index in (0..char_count).rev() {
            let start = char_offsets[index];
            let rest = &text[start..];
            let first_char = rest.chars().next().ok_or_else(|| {
                DiffusionError::model("t5 unigram tokenizer saw invalid input slice")
            })?;

            if let Some(candidates) = self.prefix_index.get(&first_char) {
                for &token_id in candidates {
                    let piece = &self.pieces[token_id as usize];
                    if !rest.starts_with(piece) {
                        continue;
                    }
                    let next = index + self.piece_char_lens[token_id as usize];
                    let score = self.scores[token_id as usize] + best_scores[next];
                    if score > best_scores[index] {
                        best_scores[index] = score;
                        best_paths[index] = Some((token_id, next));
                    }
                }
            }

            if best_paths[index].is_none() {
                best_scores[index] =
                    self.scores[self.unk_token_id as usize] + best_scores[index + 1];
                best_paths[index] = Some((self.unk_token_id, index + 1));
            }
        }

        let mut token_ids = Vec::new();
        let mut cursor = 0usize;
        while cursor < char_count {
            let (token_id, next) = best_paths[cursor].ok_or_else(|| {
                DiffusionError::model(format!("t5 unigram tokenizer lost path at char {}", cursor))
            })?;
            token_ids.push(token_id);
            cursor = next;
        }
        Ok(token_ids)
    }
}

fn basic_t5_normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_was_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            out.push(ch);
            last_was_space = false;
        }
    }
    out.trim_matches(' ').to_string()
}

fn split_with_special_tokens(text: &str, special_tokens: &[String]) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut plain = String::new();
    let mut cursor = 0usize;
    while cursor < text.len() {
        let rest = &text[cursor..];
        if let Some(token) = special_tokens
            .iter()
            .find(|token| rest.starts_with(token.as_str()))
        {
            if !plain.is_empty() {
                chunks.push(std::mem::take(&mut plain));
            }
            chunks.push(token.clone());
            cursor += token.len();
            continue;
        }

        let ch = rest.chars().next().unwrap();
        plain.push(ch);
        cursor += ch.len_utf8();
    }
    if !plain.is_empty() {
        chunks.push(plain);
    }
    chunks
}

fn json_object<'a>(
    value: Option<&'a JsonValue>,
    path: &str,
) -> Result<&'a HashMap<String, JsonValue>> {
    match value {
        Some(JsonValue::Object(object)) => Ok(object),
        _ => Err(DiffusionError::model(format!(
            "expected {} to be an object",
            path
        ))),
    }
}

fn json_array<'a>(value: Option<&'a JsonValue>, path: &str) -> Result<&'a [JsonValue]> {
    match value {
        Some(JsonValue::Array(array)) => Ok(array),
        _ => Err(DiffusionError::model(format!(
            "expected {} to be an array",
            path
        ))),
    }
}

fn json_string(value: Option<&JsonValue>, path: &str) -> Result<String> {
    match value {
        Some(JsonValue::String(text)) => Ok(text.clone()),
        Some(JsonValue::BareIdent(text)) => Ok(text.clone()),
        _ => Err(DiffusionError::model(format!(
            "expected {} to be a string",
            path
        ))),
    }
}

fn json_bool(value: Option<&JsonValue>, path: &str) -> Result<bool> {
    match value {
        Some(JsonValue::Bool(flag)) => Ok(*flag),
        _ => Err(DiffusionError::model(format!(
            "expected {} to be a bool",
            path
        ))),
    }
}

fn json_i32(value: Option<&JsonValue>, path: &str) -> Result<i32> {
    match value {
        Some(JsonValue::I64(number)) => i32::try_from(*number)
            .map_err(|_| DiffusionError::model(format!("{} does not fit in i32", path))),
        Some(JsonValue::I128(number)) => i32::try_from(*number)
            .map_err(|_| DiffusionError::model(format!("{} does not fit in i32", path))),
        Some(JsonValue::U64(number)) => i32::try_from(*number)
            .map_err(|_| DiffusionError::model(format!("{} does not fit in i32", path))),
        Some(JsonValue::U128(number)) => i32::try_from(*number)
            .map_err(|_| DiffusionError::model(format!("{} does not fit in i32", path))),
        _ => Err(DiffusionError::model(format!(
            "expected {} to be an integer",
            path
        ))),
    }
}

fn json_f32(value: Option<&JsonValue>, path: &str) -> Result<f32> {
    match value {
        Some(JsonValue::F64(number)) => Ok(*number as f32),
        Some(JsonValue::I64(number)) => Ok(*number as f32),
        Some(JsonValue::I128(number)) => Ok(*number as f32),
        Some(JsonValue::U64(number)) => Ok(*number as f32),
        Some(JsonValue::U128(number)) => Ok(*number as f32),
        _ => Err(DiffusionError::model(format!(
            "expected {} to be a number",
            path
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::T5Tokenizer;

    #[test]
    fn builds_reference_t5_tokenizer() {
        let tokenizer = T5Tokenizer::new().unwrap();
        assert_eq!(tokenizer.vocab_size(), 32_100);
        assert_eq!(tokenizer.token_to_id("<pad>"), Some(0));
        assert_eq!(tokenizer.token_to_id("</s>"), Some(1));
        assert_eq!(tokenizer.token_to_id("<unk>"), Some(2));
    }

    #[test]
    fn tokenizes_and_pads_simple_prompt() {
        let tokenizer = T5Tokenizer::new().unwrap();
        let tokenized = tokenizer.tokenize("test", 8, true).unwrap();
        assert_eq!(tokenized.raw_token_ids, vec![794]);
        assert_eq!(tokenized.eos_index, 1);
        assert_eq!(tokenized.token_ids, vec![794, 1, 0, 0, 0, 0, 0, 0]);
        assert_eq!(tokenized.attention_mask, vec![1, 1, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn preserves_added_special_tokens() {
        let tokenizer = T5Tokenizer::new().unwrap();
        assert_eq!(tokenizer.encode("<extra_id_0>").unwrap(), vec![32_099]);
    }

    #[test]
    fn splits_special_tokens_out_of_plain_text() {
        let tokenizer = T5Tokenizer::new().unwrap();
        let token_ids = tokenizer.encode("left<extra_id_0>right").unwrap();
        assert!(token_ids.contains(&32_099));
        assert_eq!(token_ids.iter().filter(|&&id| id == 32_099).count(), 1);
    }
}
