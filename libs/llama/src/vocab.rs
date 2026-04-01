use crate::error::{LlamaError, Result};
use crate::gguf::{GgufArray, GgufFile, GgufValue};
use crate::model::LlamaModel;
use std::collections::HashMap;

const GGUF_KEY_TOKENIZER_TOKENS: &str = "tokenizer.ggml.tokens";
const GGUF_KEY_TOKENIZER_MODEL: &str = "tokenizer.ggml.model";
const GGUF_KEY_TOKENIZER_PRE: &str = "tokenizer.ggml.pre";
const GGUF_KEY_TOKENIZER_EOS_TOKEN_ID: &str = "tokenizer.ggml.eos_token_id";
const GGUF_KEY_TOKENIZER_PADDING_TOKEN_ID: &str = "tokenizer.ggml.padding_token_id";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LlamaTokenizerKind {
    Plain,
    Gpt2,
}

#[derive(Clone, Debug)]
pub struct LlamaVocab {
    pieces: Vec<String>,
    tokenizer_kind: LlamaTokenizerKind,
    tokenizer_pre: Option<String>,
    eos_token_id: Option<i32>,
    padding_token_id: Option<i32>,
}

pub struct LlamaTextDecoder {
    tokenizer_kind: LlamaTokenizerKind,
    gpt2_byte_decoder: Option<HashMap<char, u8>>,
    pending_bytes: Vec<u8>,
}

impl LlamaVocab {
    pub fn from_model(model: &LlamaModel) -> Result<Self> {
        Self::from_gguf(&model.gguf)
    }

    pub fn from_gguf(gguf: &GgufFile) -> Result<Self> {
        let value = gguf.get_value(GGUF_KEY_TOKENIZER_TOKENS).ok_or_else(|| {
            LlamaError::format(format!(
                "missing required gguf key '{}'",
                GGUF_KEY_TOKENIZER_TOKENS
            ))
        })?;
        let pieces = match value {
            GgufValue::Array(GgufArray::String(tokens)) => tokens
                .iter()
                .map(|token| token.to_string_lossy().into_owned())
                .collect(),
            other => {
                return Err(LlamaError::format(format!(
                    "gguf key '{}' has type {}, expected string array",
                    GGUF_KEY_TOKENIZER_TOKENS,
                    other.value_type().name()
                )))
            }
        };

        let tokenizer_kind = match optional_utf8_string(gguf, GGUF_KEY_TOKENIZER_MODEL)? {
            Some(model) if model.eq_ignore_ascii_case("gpt2") => LlamaTokenizerKind::Gpt2,
            _ => LlamaTokenizerKind::Plain,
        };

        Ok(Self {
            pieces,
            tokenizer_kind,
            tokenizer_pre: optional_utf8_string(gguf, GGUF_KEY_TOKENIZER_PRE)?,
            eos_token_id: optional_u32(gguf, GGUF_KEY_TOKENIZER_EOS_TOKEN_ID)
                .and_then(|id| i32::try_from(id).ok()),
            padding_token_id: optional_u32(gguf, GGUF_KEY_TOKENIZER_PADDING_TOKEN_ID)
                .and_then(|id| i32::try_from(id).ok()),
        })
    }

    pub fn len(&self) -> usize {
        self.pieces.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pieces.is_empty()
    }

    pub fn piece(&self, token_id: i32) -> Option<&str> {
        usize::try_from(token_id)
            .ok()
            .and_then(|index| self.pieces.get(index))
            .map(String::as_str)
    }

    pub fn escaped_piece(&self, token_id: i32) -> Option<String> {
        self.piece(token_id)
            .map(|piece| piece.chars().flat_map(|ch| ch.escape_default()).collect())
    }

    pub fn tokenizer_kind(&self) -> &LlamaTokenizerKind {
        &self.tokenizer_kind
    }

    pub fn tokenizer_pre(&self) -> Option<&str> {
        self.tokenizer_pre.as_deref()
    }

    pub fn eos_token_id(&self) -> Option<i32> {
        self.eos_token_id
    }

    pub fn padding_token_id(&self) -> Option<i32> {
        self.padding_token_id
    }

    pub fn text_decoder(&self) -> LlamaTextDecoder {
        LlamaTextDecoder::new(self.tokenizer_kind.clone())
    }

    pub fn decode_tokens(&self, token_ids: &[i32]) -> Result<String> {
        let mut decoder = self.text_decoder();
        let mut text = String::new();
        for token_id in token_ids.iter().copied() {
            let chunk = decoder.push_token(self, token_id).ok_or_else(|| {
                LlamaError::format(format!("token id {} is outside the vocabulary", token_id))
            })?;
            text.push_str(&chunk);
        }
        text.push_str(&decoder.finish());
        Ok(text)
    }
}

impl LlamaTextDecoder {
    pub fn new(tokenizer_kind: LlamaTokenizerKind) -> Self {
        let gpt2_byte_decoder = match tokenizer_kind {
            LlamaTokenizerKind::Gpt2 => Some(gpt2_byte_decoder()),
            LlamaTokenizerKind::Plain => None,
        };
        Self {
            tokenizer_kind,
            gpt2_byte_decoder,
            pending_bytes: Vec::new(),
        }
    }

    pub fn push_token(&mut self, vocab: &LlamaVocab, token_id: i32) -> Option<String> {
        Some(self.push_piece(vocab.piece(token_id)?))
    }

    pub fn push_piece(&mut self, piece: &str) -> String {
        match self.tokenizer_kind {
            LlamaTokenizerKind::Plain => piece.to_owned(),
            LlamaTokenizerKind::Gpt2 => {
                let decoder = self
                    .gpt2_byte_decoder
                    .as_ref()
                    .expect("gpt2 decoder missing");
                for ch in piece.chars() {
                    if let Some(byte) = decoder.get(&ch) {
                        self.pending_bytes.push(*byte);
                    } else {
                        let mut utf8 = [0_u8; 4];
                        self.pending_bytes
                            .extend_from_slice(ch.encode_utf8(&mut utf8).as_bytes());
                    }
                }
                drain_utf8_prefix(&mut self.pending_bytes)
            }
        }
    }

    pub fn finish(mut self) -> String {
        if self.pending_bytes.is_empty() {
            String::new()
        } else {
            let text = String::from_utf8_lossy(&self.pending_bytes).into_owned();
            self.pending_bytes.clear();
            text
        }
    }
}

fn optional_utf8_string(gguf: &GgufFile, key: &str) -> Result<Option<String>> {
    match gguf.get_value(key) {
        None => Ok(None),
        Some(GgufValue::String(value)) => value.try_utf8().map(|s| Some(s.to_owned())),
        Some(other) => Err(LlamaError::format(format!(
            "gguf key '{}' has type {}, expected string",
            key,
            other.value_type().name()
        ))),
    }
}

fn optional_u32(gguf: &GgufFile, key: &str) -> Option<u32> {
    gguf.get_value(key).and_then(value_to_u32)
}

fn value_to_u32(value: &GgufValue) -> Option<u32> {
    match value {
        GgufValue::Uint32(v) => Some(*v),
        GgufValue::Uint64(v) => u32::try_from(*v).ok(),
        GgufValue::Int32(v) => u32::try_from(*v).ok(),
        GgufValue::Int64(v) => u32::try_from(*v).ok(),
        _ => None,
    }
}

fn gpt2_byte_decoder() -> HashMap<char, u8> {
    let mut used = [false; 256];
    let mut bs = Vec::new();

    for value in 33_u16..=126 {
        used[value as usize] = true;
        bs.push(value);
    }
    for value in 161_u16..=172 {
        used[value as usize] = true;
        bs.push(value);
    }
    for value in 174_u16..=255 {
        used[value as usize] = true;
        bs.push(value);
    }

    let mut cs = bs.clone();
    let mut extra = 0_u16;
    for byte in 0_u16..=255 {
        if !used[byte as usize] {
            bs.push(byte);
            cs.push(256 + extra);
            extra += 1;
        }
    }

    bs.into_iter()
        .zip(cs)
        .map(|(byte, codepoint)| {
            (
                char::from_u32(u32::from(codepoint)).expect("invalid gpt2 codepoint"),
                byte as u8,
            )
        })
        .collect()
}

fn drain_utf8_prefix(pending_bytes: &mut Vec<u8>) -> String {
    let mut out = String::new();
    loop {
        match std::str::from_utf8(pending_bytes) {
            Ok(valid) => {
                out.push_str(valid);
                pending_bytes.clear();
                return out;
            }
            Err(err) => {
                let valid_up_to = err.valid_up_to();
                if valid_up_to > 0 {
                    out.push_str(&String::from_utf8_lossy(&pending_bytes[..valid_up_to]));
                    pending_bytes.drain(..valid_up_to);
                    continue;
                }
                match err.error_len() {
                    None => return out,
                    Some(invalid_len) => {
                        out.push('\u{FFFD}');
                        pending_bytes.drain(..invalid_len);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{gpt2_byte_decoder, LlamaTextDecoder, LlamaTokenizerKind};

    #[test]
    fn gpt2_byte_decoder_maps_space_marker() {
        let decoder = gpt2_byte_decoder();
        assert_eq!(decoder.get(&'Ġ'), Some(&b' '));
        assert_eq!(decoder.get(&'H'), Some(&b'H'));
    }

    #[test]
    fn gpt2_text_decoder_decodes_ascii_and_space() {
        let mut decoder = LlamaTextDecoder::new(LlamaTokenizerKind::Gpt2);
        assert_eq!(decoder.push_piece("Hello"), "Hello");
        assert_eq!(decoder.push_piece("Ġworld"), " world");
        assert_eq!(decoder.finish(), "");
    }

    #[test]
    fn gpt2_text_decoder_handles_utf8_split_across_tokens() {
        let mut decoder = LlamaTextDecoder::new(LlamaTokenizerKind::Gpt2);
        assert_eq!(decoder.push_piece("Ã"), "");
        assert_eq!(decoder.push_piece("©"), "é");
        assert_eq!(decoder.finish(), "");
    }
}
