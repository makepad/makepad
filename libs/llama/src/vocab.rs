use crate::error::{LlamaError, Result};
use crate::gguf::{GgufArray, GgufFile, GgufValue};
use crate::model::LlamaModel;

const GGUF_KEY_TOKENIZER_TOKENS: &str = "tokenizer.ggml.tokens";

#[derive(Clone, Debug)]
pub struct LlamaVocab {
    pieces: Vec<String>,
}

impl LlamaVocab {
    pub fn from_model(model: &LlamaModel) -> Result<Self> {
        Self::from_gguf(&model.gguf)
    }

    pub fn from_gguf(gguf: &GgufFile) -> Result<Self> {
        let value = gguf
            .get_value(GGUF_KEY_TOKENIZER_TOKENS)
            .ok_or_else(|| {
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
        Ok(Self { pieces })
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
}
