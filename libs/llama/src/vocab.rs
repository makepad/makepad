use crate::error::{LlamaError, Result};
use crate::gguf::{GgufArray, GgufFile, GgufValue};
use crate::model::LlamaModel;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::sync::OnceLock;

const GGUF_KEY_TOKENIZER_TOKENS: &str = "tokenizer.ggml.tokens";
const GGUF_KEY_TOKENIZER_MODEL: &str = "tokenizer.ggml.model";
const GGUF_KEY_TOKENIZER_PRE: &str = "tokenizer.ggml.pre";
const GGUF_KEY_TOKENIZER_TOKEN_TYPE: &str = "tokenizer.ggml.token_type";
const GGUF_KEY_TOKENIZER_SCORES: &str = "tokenizer.ggml.scores";
const GGUF_KEY_TOKENIZER_MERGES: &str = "tokenizer.ggml.merges";
const GGUF_KEY_TOKENIZER_BOS_TOKEN_ID: &str = "tokenizer.ggml.bos_token_id";
const GGUF_KEY_TOKENIZER_EOS_TOKEN_ID: &str = "tokenizer.ggml.eos_token_id";
const GGUF_KEY_TOKENIZER_UNK_TOKEN_ID: &str = "tokenizer.ggml.unknown_token_id";
const GGUF_KEY_TOKENIZER_PADDING_TOKEN_ID: &str = "tokenizer.ggml.padding_token_id";
const GGUF_KEY_TOKENIZER_ADD_BOS_TOKEN: &str = "tokenizer.ggml.add_bos_token";
const GGUF_KEY_TOKENIZER_ADD_EOS_TOKEN: &str = "tokenizer.ggml.add_eos_token";
const GGUF_KEY_TOKENIZER_ADD_SPACE_PREFIX: &str = "tokenizer.ggml.add_space_prefix";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LlamaTokenizerKind {
    Plain,
    Gpt2,
    SentencePiece,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LlamaBpePreTokenizer {
    Gpt2,
    Qwen2,
    Qwen35,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LlamaTokenType {
    Undefined,
    Normal,
    Unknown,
    Control,
    UserDefined,
    Unused,
    Byte,
}

#[derive(Clone, Debug)]
pub struct LlamaVocab {
    pieces: Vec<String>,
    token_types: Vec<LlamaTokenType>,
    token_scores: Vec<f32>,
    token_to_id: HashMap<String, i32>,
    tokenizer_kind: LlamaTokenizerKind,
    bpe_pre: Option<LlamaBpePreTokenizer>,
    tokenizer_pre: Option<String>,
    bos_token_id: Option<i32>,
    eos_token_id: Option<i32>,
    unk_token_id: Option<i32>,
    padding_token_id: Option<i32>,
    add_bos_token: bool,
    add_eos_token: bool,
    add_space_prefix: bool,
    bpe_ranks: HashMap<(String, String), usize>,
}

pub struct LlamaTextDecoder {
    tokenizer_kind: LlamaTokenizerKind,
    gpt2_byte_decoder: Option<HashMap<char, u8>>,
    pending_bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
enum TokenizeFragment {
    Text(String),
    Token(i32),
}

#[derive(Clone, Copy, Debug)]
struct TextChar {
    ch: char,
    start: usize,
}

#[derive(Clone, Debug)]
struct SentencePieceSymbol {
    start: usize,
    len: usize,
    prev: Option<usize>,
    next: Option<usize>,
}

#[derive(Clone, Debug)]
struct SentencePieceBigram {
    left: usize,
    right: usize,
    score: f32,
    size: usize,
}

impl PartialEq for SentencePieceBigram {
    fn eq(&self, other: &Self) -> bool {
        self.left == other.left
            && self.right == other.right
            && self.score.to_bits() == other.score.to_bits()
            && self.size == other.size
    }
}

impl Eq for SentencePieceBigram {}

impl PartialOrd for SentencePieceBigram {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SentencePieceBigram {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .total_cmp(&other.score)
            .then_with(|| other.left.cmp(&self.left))
            .then_with(|| other.right.cmp(&self.right))
            .then_with(|| other.size.cmp(&self.size))
    }
}

impl LlamaVocab {
    pub fn from_model(model: &LlamaModel) -> Result<Self> {
        Self::from_gguf(&model.gguf)
    }

    pub fn from_gguf(gguf: &GgufFile) -> Result<Self> {
        let pieces = required_string_array(gguf, GGUF_KEY_TOKENIZER_TOKENS)?;
        let token_types =
            optional_token_type_array(gguf, GGUF_KEY_TOKENIZER_TOKEN_TYPE, pieces.len())?;
        let token_scores = optional_f32_array(gguf, GGUF_KEY_TOKENIZER_SCORES, pieces.len())?;
        let token_to_id = pieces
            .iter()
            .enumerate()
            .map(|(index, piece)| {
                let token_id = i32::try_from(index).map_err(|_| {
                    LlamaError::format(format!("token index {} does not fit in i32", index))
                })?;
                Ok((piece.clone(), token_id))
            })
            .collect::<Result<HashMap<_, _>>>()?;

        let tokenizer_model = optional_utf8_string(gguf, GGUF_KEY_TOKENIZER_MODEL)?;
        let tokenizer_pre = optional_utf8_string(gguf, GGUF_KEY_TOKENIZER_PRE)?;
        let (tokenizer_kind, bpe_pre) =
            tokenizer_kind_from_gguf(tokenizer_model.as_deref(), tokenizer_pre.as_deref());
        let add_space_prefix = optional_bool(gguf, GGUF_KEY_TOKENIZER_ADD_SPACE_PREFIX)
            .unwrap_or(tokenizer_kind == LlamaTokenizerKind::SentencePiece);

        let bpe_ranks = if tokenizer_kind == LlamaTokenizerKind::Gpt2 {
            load_bpe_ranks(gguf)?
        } else {
            HashMap::new()
        };

        Ok(Self {
            pieces,
            token_types,
            token_scores,
            token_to_id,
            tokenizer_kind,
            bpe_pre,
            tokenizer_pre,
            bos_token_id: optional_u32(gguf, GGUF_KEY_TOKENIZER_BOS_TOKEN_ID)
                .and_then(|id| i32::try_from(id).ok()),
            eos_token_id: optional_u32(gguf, GGUF_KEY_TOKENIZER_EOS_TOKEN_ID)
                .and_then(|id| i32::try_from(id).ok()),
            unk_token_id: optional_u32(gguf, GGUF_KEY_TOKENIZER_UNK_TOKEN_ID)
                .and_then(|id| i32::try_from(id).ok()),
            padding_token_id: optional_u32(gguf, GGUF_KEY_TOKENIZER_PADDING_TOKEN_ID)
                .and_then(|id| i32::try_from(id).ok()),
            add_bos_token: optional_bool(gguf, GGUF_KEY_TOKENIZER_ADD_BOS_TOKEN).unwrap_or(false),
            add_eos_token: optional_bool(gguf, GGUF_KEY_TOKENIZER_ADD_EOS_TOKEN).unwrap_or(false),
            add_space_prefix,
            bpe_ranks,
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

    pub fn bos_token_id(&self) -> Option<i32> {
        self.bos_token_id
    }

    pub fn eos_token_id(&self) -> Option<i32> {
        self.eos_token_id
    }

    pub fn padding_token_id(&self) -> Option<i32> {
        self.padding_token_id
    }

    pub fn add_bos_token(&self) -> bool {
        self.add_bos_token
    }

    pub fn add_eos_token(&self) -> bool {
        self.add_eos_token
    }

    pub fn add_space_prefix(&self) -> bool {
        self.add_space_prefix
    }

    pub fn text_decoder(&self) -> LlamaTextDecoder {
        LlamaTextDecoder::new(self.tokenizer_kind.clone())
    }

    pub fn tokenize(&self, text: &str, add_special: bool, parse_special: bool) -> Result<Vec<i32>> {
        match self.tokenizer_kind {
            LlamaTokenizerKind::Plain => Err(LlamaError::unsupported(
                "native tokenization is not implemented for this tokenizer model",
            )),
            LlamaTokenizerKind::Gpt2 => self.tokenize_bpe(text, add_special, parse_special),
            LlamaTokenizerKind::SentencePiece => {
                self.tokenize_sentencepiece(text, add_special, parse_special)
            }
        }
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

    fn tokenize_bpe(&self, text: &str, add_special: bool, parse_special: bool) -> Result<Vec<i32>> {
        let bpe_pre = self.bpe_pre.ok_or_else(|| {
            LlamaError::unsupported("missing BPE pre-tokenizer configuration in GGUF metadata")
        })?;
        let fragments = self.partition_special_fragments(text, parse_special);
        let mut output = Vec::new();
        if add_special && self.add_bos_token {
            output.push(self.bos_token_id.ok_or_else(|| {
                LlamaError::format("tokenizer requested BOS insertion but no bos_token_id exists")
            })?);
        }
        for fragment in fragments {
            match fragment {
                TokenizeFragment::Text(fragment_text) => {
                    for word in split_bpe_words(&fragment_text, bpe_pre) {
                        self.encode_bpe_word(&word, &mut output)?;
                    }
                }
                TokenizeFragment::Token(token_id) => output.push(token_id),
            }
        }
        if add_special && self.add_eos_token {
            output.push(self.eos_token_id.ok_or_else(|| {
                LlamaError::format("tokenizer requested EOS insertion but no eos_token_id exists")
            })?);
        }
        Ok(output)
    }

    fn tokenize_sentencepiece(
        &self,
        text: &str,
        add_special: bool,
        parse_special: bool,
    ) -> Result<Vec<i32>> {
        let fragments = self.partition_special_fragments(text, parse_special);
        let mut output = Vec::new();
        let mut prev_special = true;
        if add_special && self.add_bos_token {
            output.push(self.bos_token_id.ok_or_else(|| {
                LlamaError::format("tokenizer requested BOS insertion but no bos_token_id exists")
            })?);
            prev_special = true;
        }

        for fragment in fragments {
            match fragment {
                TokenizeFragment::Text(fragment_text) => {
                    let mut raw = String::new();
                    if self.add_space_prefix && prev_special {
                        raw.push(' ');
                    }
                    raw.push_str(&fragment_text);
                    let escaped = sentencepiece_escape_whitespace(&raw);
                    self.encode_sentencepiece_text(&escaped, &mut output)?;
                    prev_special = false;
                }
                TokenizeFragment::Token(token_id) => {
                    output.push(token_id);
                    prev_special = true;
                }
            }
        }

        if add_special && self.add_eos_token {
            output.push(self.eos_token_id.ok_or_else(|| {
                LlamaError::format("tokenizer requested EOS insertion but no eos_token_id exists")
            })?);
        }
        Ok(output)
    }

    fn partition_special_fragments(
        &self,
        text: &str,
        parse_special: bool,
    ) -> Vec<TokenizeFragment> {
        let mut specials = self.special_tokens(parse_special);
        if specials.is_empty() || text.is_empty() {
            return vec![TokenizeFragment::Text(text.to_owned())];
        }

        specials.sort_by(|left, right| {
            right
                .0
                .len()
                .cmp(&left.0.len())
                .then_with(|| left.1.cmp(&right.1))
        });

        let mut out = Vec::new();
        let mut cursor = 0;
        let mut text_start = 0;
        while cursor < text.len() {
            let suffix = &text[cursor..];
            let matched = specials
                .iter()
                .find(|(piece, _)| suffix.starts_with(piece.as_str()));
            if let Some((piece, token_id)) = matched {
                if text_start < cursor {
                    out.push(TokenizeFragment::Text(text[text_start..cursor].to_owned()));
                }
                out.push(TokenizeFragment::Token(*token_id));
                cursor += piece.len();
                text_start = cursor;
                continue;
            }
            let next = suffix.chars().next().map(|ch| ch.len_utf8()).unwrap_or(1);
            cursor += next;
        }
        if text_start < text.len() {
            out.push(TokenizeFragment::Text(text[text_start..].to_owned()));
        }
        if out.is_empty() {
            out.push(TokenizeFragment::Text(String::new()));
        }
        out
    }

    fn special_tokens(&self, parse_special: bool) -> Vec<(String, i32)> {
        self.pieces
            .iter()
            .enumerate()
            .filter_map(|(index, piece)| {
                let token_type = self
                    .token_types
                    .get(index)
                    .copied()
                    .unwrap_or(LlamaTokenType::Normal);
                let include = match token_type {
                    LlamaTokenType::Control
                    | LlamaTokenType::UserDefined
                    | LlamaTokenType::Unused => true,
                    LlamaTokenType::Unknown => parse_special,
                    _ => false,
                };
                if !include {
                    return None;
                }
                let token_id = i32::try_from(index).ok()?;
                Some((piece.clone(), token_id))
            })
            .collect()
    }

    fn encode_bpe_word(&self, word: &str, output: &mut Vec<i32>) -> Result<()> {
        if word.is_empty() {
            return Ok(());
        }

        let encoded_word = encode_bpe_word_bytes(word);
        if encoded_word.is_empty() {
            return Ok(());
        }

        let mut symbols = if self.token_to_id.contains_key(&encoded_word)
            && matches!(self.bpe_pre, Some(LlamaBpePreTokenizer::Gpt2))
        {
            vec![encoded_word]
        } else {
            encoded_word
                .chars()
                .map(|ch| ch.to_string())
                .collect::<Vec<_>>()
        };

        while symbols.len() > 1 {
            let mut best_pair = None;
            for pair_index in 0..(symbols.len() - 1) {
                let rank = self
                    .bpe_ranks
                    .get(&(symbols[pair_index].clone(), symbols[pair_index + 1].clone()));
                if let Some(rank) = rank {
                    match best_pair {
                        None => best_pair = Some((pair_index, *rank)),
                        Some((_, best_rank)) if *rank < best_rank => {
                            best_pair = Some((pair_index, *rank))
                        }
                        _ => {}
                    }
                }
            }
            let Some((pair_index, _)) = best_pair else {
                break;
            };
            let merged = symbols[pair_index].clone() + &symbols[pair_index + 1];
            symbols[pair_index] = merged;
            symbols.remove(pair_index + 1);
        }

        for symbol in symbols {
            if let Some(token_id) = self.token_to_id.get(&symbol).copied() {
                output.push(token_id);
                continue;
            }

            let mut emitted = false;
            for ch in symbol.chars() {
                let piece = ch.to_string();
                if let Some(token_id) = self.token_to_id.get(&piece).copied() {
                    output.push(token_id);
                    emitted = true;
                } else {
                    emitted = false;
                    break;
                }
            }
            if emitted {
                continue;
            }

            if let Some(unk_token_id) = self.unk_token_id {
                output.push(unk_token_id);
                continue;
            }

            return Err(LlamaError::unsupported(format!(
                "tokenizer could not resolve BPE symbol {:?}",
                symbol
            )));
        }

        Ok(())
    }

    fn encode_sentencepiece_text(&self, text: &str, output: &mut Vec<i32>) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }

        let mut symbols = Vec::new();
        let mut head = None;
        let mut prev = None;
        let mut iter = text.char_indices().peekable();
        while let Some((start, _)) = iter.next() {
            let end = iter.peek().map(|(offset, _)| *offset).unwrap_or(text.len());
            let index = symbols.len();
            symbols.push(SentencePieceSymbol {
                start,
                len: end - start,
                prev,
                next: None,
            });
            if let Some(prev_index) = prev {
                symbols[prev_index].next = Some(index);
            } else {
                head = Some(index);
            }
            prev = Some(index);
        }

        let Some(head_index) = head else {
            return Ok(());
        };

        let mut work_queue = BinaryHeap::new();
        let mut rev_merge = HashMap::new();
        for index in 1..symbols.len() {
            self.try_add_sentencepiece_bigram(
                text,
                &symbols,
                index - 1,
                index,
                &mut work_queue,
                &mut rev_merge,
            );
        }

        while let Some(bigram) = work_queue.pop() {
            let left_len = symbols[bigram.left].len;
            let right_len = symbols[bigram.right].len;
            if left_len == 0
                || right_len == 0
                || left_len.checked_add(right_len).ok_or_else(|| {
                    LlamaError::format("overflow merging sentencepiece symbol lengths")
                })? != bigram.size
            {
                continue;
            }

            let right_next = symbols[bigram.right].next;
            symbols[bigram.left].len += symbols[bigram.right].len;
            symbols[bigram.left].next = right_next;
            symbols[bigram.right].len = 0;
            if let Some(next_index) = right_next {
                symbols[next_index].prev = Some(bigram.left);
            }

            if let Some(prev_index) = symbols[bigram.left].prev {
                self.try_add_sentencepiece_bigram(
                    text,
                    &symbols,
                    prev_index,
                    bigram.left,
                    &mut work_queue,
                    &mut rev_merge,
                );
            }
            if let Some(next_index) = symbols[bigram.left].next {
                self.try_add_sentencepiece_bigram(
                    text,
                    &symbols,
                    bigram.left,
                    next_index,
                    &mut work_queue,
                    &mut rev_merge,
                );
            }
        }

        let mut cursor = Some(head_index);
        while let Some(index) = cursor {
            self.resegment_sentencepiece_symbol(text, &symbols, &rev_merge, index, output)?;
            cursor = symbols[index].next;
        }
        Ok(())
    }

    fn try_add_sentencepiece_bigram(
        &self,
        text: &str,
        symbols: &[SentencePieceSymbol],
        left: usize,
        right: usize,
        work_queue: &mut BinaryHeap<SentencePieceBigram>,
        rev_merge: &mut HashMap<String, (usize, usize)>,
    ) {
        let left_symbol = &symbols[left];
        let right_symbol = &symbols[right];
        if left_symbol.len == 0 || right_symbol.len == 0 {
            return;
        }
        let Some(end) = right_symbol.start.checked_add(right_symbol.len) else {
            return;
        };
        let merged = &text[left_symbol.start..end];
        let Some(token_id) = self.lookup_token(merged) else {
            return;
        };
        let Some(score) = self.token_score(token_id) else {
            return;
        };
        work_queue.push(SentencePieceBigram {
            left,
            right,
            score,
            size: merged.len(),
        });
        rev_merge.insert(merged.to_owned(), (left, right));
    }

    fn resegment_sentencepiece_symbol(
        &self,
        text: &str,
        symbols: &[SentencePieceSymbol],
        rev_merge: &HashMap<String, (usize, usize)>,
        symbol_index: usize,
        output: &mut Vec<i32>,
    ) -> Result<()> {
        let symbol = symbols
            .get(symbol_index)
            .ok_or_else(|| LlamaError::format("sentencepiece symbol index out of range"))?;
        let end = symbol
            .start
            .checked_add(symbol.len)
            .ok_or_else(|| LlamaError::format("overflow computing sentencepiece slice"))?;
        let fragment = &text[symbol.start..end];

        if let Some(token_id) = self.lookup_token(fragment) {
            output.push(token_id);
            return Ok(());
        }

        if let Some((left, right)) = rev_merge.get(fragment).copied() {
            self.resegment_sentencepiece_symbol(text, symbols, rev_merge, left, output)?;
            self.resegment_sentencepiece_symbol(text, symbols, rev_merge, right, output)?;
            return Ok(());
        }

        for &byte in fragment.as_bytes() {
            output.push(self.byte_fallback_token(byte)?);
        }
        Ok(())
    }

    fn lookup_token(&self, piece: &str) -> Option<i32> {
        self.token_to_id.get(piece).copied()
    }

    fn token_score(&self, token_id: i32) -> Option<f32> {
        usize::try_from(token_id)
            .ok()
            .and_then(|index| self.token_scores.get(index))
            .copied()
    }

    fn token_type(&self, token_id: i32) -> Option<LlamaTokenType> {
        usize::try_from(token_id)
            .ok()
            .and_then(|index| self.token_types.get(index))
            .copied()
    }

    fn byte_fallback_token(&self, byte: u8) -> Result<i32> {
        let piece = format!("<0x{:02X}>", byte);
        if let Some(token_id) = self.lookup_token(&piece) {
            return Ok(token_id);
        }
        let single = char::from(byte).to_string();
        if let Some(token_id) = self.lookup_token(&single) {
            return Ok(token_id);
        }
        self.unk_token_id.ok_or_else(|| {
            LlamaError::unsupported(format!(
                "sentencepiece tokenizer has no byte fallback for 0x{:02X}",
                byte
            ))
        })
    }
}

impl LlamaTextDecoder {
    pub fn new(tokenizer_kind: LlamaTokenizerKind) -> Self {
        let gpt2_byte_decoder = match tokenizer_kind {
            LlamaTokenizerKind::Gpt2 => Some(gpt2_byte_decoder()),
            LlamaTokenizerKind::SentencePiece => None,
            LlamaTokenizerKind::Plain => None,
        };
        Self {
            tokenizer_kind,
            gpt2_byte_decoder,
            pending_bytes: Vec::new(),
        }
    }

    pub fn push_token(&mut self, vocab: &LlamaVocab, token_id: i32) -> Option<String> {
        let piece = vocab.piece(token_id)?;
        match self.tokenizer_kind {
            LlamaTokenizerKind::SentencePiece => {
                let token_type = vocab.token_type(token_id)?;
                Some(match token_type {
                    LlamaTokenType::Normal => sentencepiece_unescape_whitespace(piece),
                    LlamaTokenType::Byte => {
                        decode_sentencepiece_byte_piece(piece).unwrap_or_else(|| piece.to_owned())
                    }
                    LlamaTokenType::Undefined
                    | LlamaTokenType::Unknown
                    | LlamaTokenType::Control
                    | LlamaTokenType::UserDefined
                    | LlamaTokenType::Unused => piece.to_owned(),
                })
            }
            _ => Some(self.push_piece(piece)),
        }
    }

    pub fn push_piece(&mut self, piece: &str) -> String {
        match self.tokenizer_kind {
            LlamaTokenizerKind::Plain => piece.to_owned(),
            LlamaTokenizerKind::SentencePiece => sentencepiece_unescape_whitespace(piece),
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

fn tokenizer_kind_from_gguf(
    tokenizer_model: Option<&str>,
    tokenizer_pre: Option<&str>,
) -> (LlamaTokenizerKind, Option<LlamaBpePreTokenizer>) {
    match tokenizer_model {
        Some(model) if model.eq_ignore_ascii_case("gpt2") => (
            LlamaTokenizerKind::Gpt2,
            Some(match tokenizer_pre {
                Some(pre) if pre.eq_ignore_ascii_case("qwen35") => LlamaBpePreTokenizer::Qwen35,
                Some(pre) if pre.eq_ignore_ascii_case("qwen2") => LlamaBpePreTokenizer::Qwen2,
                _ => LlamaBpePreTokenizer::Gpt2,
            }),
        ),
        Some(model) if model.eq_ignore_ascii_case("gemma4") => {
            (LlamaTokenizerKind::SentencePiece, None)
        }
        _ => (LlamaTokenizerKind::Plain, None),
    }
}

fn sentencepiece_escape_whitespace(text: &str) -> String {
    text.replace(' ', "\u{2581}")
}

fn sentencepiece_unescape_whitespace(piece: &str) -> String {
    piece.replace('\u{2581}', " ")
}

fn decode_sentencepiece_byte_piece(piece: &str) -> Option<String> {
    let hex = piece
        .strip_prefix("<0x")
        .and_then(|rest| rest.strip_suffix('>'))?;
    if hex.len() != 2 {
        return None;
    }
    let byte = u8::from_str_radix(hex, 16).ok()?;
    Some(String::from_utf8_lossy(&[byte]).into_owned())
}

fn load_bpe_ranks(gguf: &GgufFile) -> Result<HashMap<(String, String), usize>> {
    let Some(merges) = optional_string_array(gguf, GGUF_KEY_TOKENIZER_MERGES)? else {
        return Ok(HashMap::new());
    };
    let mut ranks = HashMap::with_capacity(merges.len());
    for (rank, merge) in merges.into_iter().enumerate() {
        let Some(split_at) = merge.find(' ').filter(|split_at| *split_at > 0) else {
            continue;
        };
        let left = merge[..split_at].to_owned();
        let right = merge[split_at + 1..].to_owned();
        ranks.insert((left, right), rank);
    }
    Ok(ranks)
}

fn split_bpe_words(text: &str, pre: LlamaBpePreTokenizer) -> Vec<String> {
    match pre {
        LlamaBpePreTokenizer::Gpt2 => split_gpt2_words(text),
        LlamaBpePreTokenizer::Qwen2 => split_qwen_words(text, false),
        LlamaBpePreTokenizer::Qwen35 => split_qwen_words(text, true),
    }
}

fn split_gpt2_words(text: &str) -> Vec<String> {
    let chars = text_chars(text);
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < chars.len() {
        let ch = chars[pos].ch;

        if ch == '\'' && pos + 1 < chars.len() {
            let next = chars[pos + 1].ch.to_ascii_lowercase();
            if matches!(next, 's' | 't' | 'm' | 'd') {
                push_char_range(text, &chars, pos, pos + 2, &mut out);
                pos += 2;
                continue;
            }
            if pos + 2 < chars.len() {
                let next_next = chars[pos + 2].ch.to_ascii_lowercase();
                if matches!((next, next_next), ('r', 'e') | ('v', 'e') | ('l', 'l')) {
                    push_char_range(text, &chars, pos, pos + 3, &mut out);
                    pos += 3;
                    continue;
                }
            }
        }

        let mut next_pos = pos;
        let current = chars[pos].ch;
        let check = if current == ' ' {
            char_flags(next_char(&chars, pos))
        } else {
            char_flags(Some(current))
        };

        if check.is_letter {
            if current == ' ' {
                next_pos += 1;
            }
            while next_pos < chars.len() && char_flags(Some(chars[next_pos].ch)).is_letter {
                next_pos += 1;
            }
            push_char_range(text, &chars, pos, next_pos, &mut out);
            pos = next_pos;
            continue;
        }

        if check.is_number {
            if current == ' ' {
                next_pos += 1;
            }
            while next_pos < chars.len() && char_flags(Some(chars[next_pos].ch)).is_number {
                next_pos += 1;
            }
            push_char_range(text, &chars, pos, next_pos, &mut out);
            pos = next_pos;
            continue;
        }

        if check.is_present && !check.is_whitespace && !check.is_letter && !check.is_number {
            if current == ' ' {
                next_pos += 1;
            }
            while next_pos < chars.len() {
                let flags = char_flags(Some(chars[next_pos].ch));
                if flags.is_whitespace || flags.is_letter || flags.is_number {
                    break;
                }
                next_pos += 1;
            }
            push_char_range(text, &chars, pos, next_pos, &mut out);
            pos = next_pos;
            continue;
        }

        let whitespace_count = whitespace_run_len(&chars, pos);
        if whitespace_count > 1 && pos + whitespace_count < chars.len() {
            push_char_range(text, &chars, pos, pos + whitespace_count - 1, &mut out);
            pos += whitespace_count - 1;
            continue;
        }
        if whitespace_count > 0 {
            push_char_range(text, &chars, pos, pos + whitespace_count, &mut out);
            pos += whitespace_count;
            continue;
        }

        push_char_range(text, &chars, pos, pos + 1, &mut out);
        pos += 1;
    }
    out
}

fn split_qwen_words(text: &str, allow_marks: bool) -> Vec<String> {
    let chars = text_chars(text);
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < chars.len() {
        let ch = chars[pos].ch;

        if ch == '\'' && pos + 1 < chars.len() {
            let next = chars[pos + 1].ch.to_ascii_lowercase();
            if matches!(next, 's' | 't' | 'm' | 'd') {
                push_char_range(text, &chars, pos, pos + 2, &mut out);
                pos += 2;
                continue;
            }
            if pos + 2 < chars.len() {
                let next_next = chars[pos + 2].ch.to_ascii_lowercase();
                if matches!((next, next_next), ('r', 'e') | ('v', 'e') | ('l', 'l')) {
                    push_char_range(text, &chars, pos, pos + 3, &mut out);
                    pos += 3;
                    continue;
                }
            }
        }

        if !is_crlf(ch) && !ch.is_numeric() {
            let next_is_letter = next_char(&chars, pos)
                .map(|next| is_letter_or_mark(next, allow_marks))
                .unwrap_or(false);
            if is_letter_or_mark(ch, allow_marks) || next_is_letter {
                let mut next_pos = pos + 1;
                while next_pos < chars.len() && is_letter_or_mark(chars[next_pos].ch, allow_marks) {
                    next_pos += 1;
                }
                push_char_range(text, &chars, pos, next_pos, &mut out);
                pos = next_pos;
                continue;
            }
        }

        if ch.is_numeric() {
            push_char_range(text, &chars, pos, pos + 1, &mut out);
            pos += 1;
            continue;
        }

        let mut symbol_pos = pos;
        let symbol_flags = if ch == ' ' {
            char_flags(next_char(&chars, pos))
        } else {
            char_flags(Some(ch))
        };
        if symbol_flags.is_present
            && !symbol_flags.is_whitespace
            && !symbol_flags.is_letter
            && !symbol_flags.is_number
            && !symbol_flags.is_mark
        {
            if ch == ' ' {
                symbol_pos += 1;
            }
            while symbol_pos < chars.len() {
                let flags = char_flags(Some(chars[symbol_pos].ch));
                if flags.is_whitespace || flags.is_letter || flags.is_number || flags.is_mark {
                    break;
                }
                symbol_pos += 1;
            }
            while symbol_pos < chars.len() && is_crlf(chars[symbol_pos].ch) {
                symbol_pos += 1;
            }
            push_char_range(text, &chars, pos, symbol_pos, &mut out);
            pos = symbol_pos;
            continue;
        }

        let mut whitespace_count = 0;
        let mut newline_end = None;
        while pos + whitespace_count < chars.len()
            && chars[pos + whitespace_count].ch.is_whitespace()
        {
            if is_crlf(chars[pos + whitespace_count].ch) {
                newline_end = Some(pos + whitespace_count + 1);
            }
            whitespace_count += 1;
        }

        if let Some(newline_end) = newline_end {
            push_char_range(text, &chars, pos, newline_end, &mut out);
            pos = newline_end;
            continue;
        }

        if whitespace_count > 1 && pos + whitespace_count < chars.len() {
            push_char_range(text, &chars, pos, pos + whitespace_count - 1, &mut out);
            pos += whitespace_count - 1;
            continue;
        }

        if whitespace_count > 0 {
            push_char_range(text, &chars, pos, pos + whitespace_count, &mut out);
            pos += whitespace_count;
            continue;
        }

        push_char_range(text, &chars, pos, pos + 1, &mut out);
        pos += 1;
    }
    out
}

fn text_chars(text: &str) -> Vec<TextChar> {
    text.char_indices()
        .map(|(start, ch)| TextChar { ch, start })
        .collect()
}

fn push_char_range(
    text: &str,
    chars: &[TextChar],
    start_index: usize,
    end_index: usize,
    out: &mut Vec<String>,
) {
    if start_index >= end_index || start_index >= chars.len() {
        return;
    }
    let start = chars[start_index].start;
    let end = if end_index >= chars.len() {
        text.len()
    } else {
        chars[end_index].start
    };
    if start < end {
        out.push(text[start..end].to_owned());
    }
}

fn next_char(chars: &[TextChar], index: usize) -> Option<char> {
    chars.get(index + 1).map(|info| info.ch)
}

fn whitespace_run_len(chars: &[TextChar], start: usize) -> usize {
    let mut count = 0;
    while start + count < chars.len() && chars[start + count].ch.is_whitespace() {
        count += 1;
    }
    count
}

fn is_crlf(ch: char) -> bool {
    matches!(ch, '\r' | '\n')
}

fn is_letter_or_mark(ch: char, allow_marks: bool) -> bool {
    ch.is_alphabetic() || (allow_marks && is_combining_mark(ch))
}

fn is_combining_mark(ch: char) -> bool {
    let c = u32::from(ch);
    matches!(
        c,
        0x0300..=0x036F
            | 0x0483..=0x0489
            | 0x0591..=0x05BD
            | 0x05BF
            | 0x05C1..=0x05C2
            | 0x05C4..=0x05C5
            | 0x05C7
            | 0x0610..=0x061A
            | 0x064B..=0x065F
            | 0x0670
            | 0x06D6..=0x06DC
            | 0x06DF..=0x06E4
            | 0x06E7..=0x06E8
            | 0x06EA..=0x06ED
            | 0x0711
            | 0x0730..=0x074A
            | 0x07A6..=0x07B0
            | 0x07EB..=0x07F3
            | 0x0816..=0x0819
            | 0x081B..=0x0823
            | 0x0825..=0x0827
            | 0x0829..=0x082D
            | 0x0859..=0x085B
            | 0x08D3..=0x08E1
            | 0x08E3..=0x0902
            | 0x093A
            | 0x093C
            | 0x0941..=0x0948
            | 0x094D
            | 0x0951..=0x0957
            | 0x0962..=0x0963
            | 0x0981
            | 0x09BC
            | 0x09C1..=0x09C4
            | 0x09CD
            | 0x09E2..=0x09E3
            | 0x0A01..=0x0A02
            | 0x0A3C
            | 0x0A41..=0x0A42
            | 0x0A47..=0x0A48
            | 0x0A4B..=0x0A4D
            | 0x0A51
            | 0x0A70..=0x0A71
            | 0x0A75
            | 0x0A81..=0x0A82
            | 0x0ABC
            | 0x0AC1..=0x0AC5
            | 0x0AC7..=0x0AC8
            | 0x0ACD
            | 0x0AE2..=0x0AE3
            | 0x0B01
            | 0x0B3C
            | 0x0B3F
            | 0x0B41..=0x0B44
            | 0x0B4D
            | 0x0B56
            | 0x0B62..=0x0B63
            | 0x0B82
            | 0x0BC0
            | 0x0BCD
            | 0x0C00
            | 0x0C04
            | 0x0C3E..=0x0C40
            | 0x0C46..=0x0C48
            | 0x0C4A..=0x0C4D
            | 0x0C55..=0x0C56
            | 0x0C62..=0x0C63
            | 0x0C81
            | 0x0CBC
            | 0x0CBF
            | 0x0CC6
            | 0x0CCC..=0x0CCD
            | 0x0CE2..=0x0CE3
            | 0x0D00..=0x0D01
            | 0x0D3B..=0x0D3C
            | 0x0D41..=0x0D44
            | 0x0D4D
            | 0x0D62..=0x0D63
            | 0x0DCA
            | 0x0DD2..=0x0DD4
            | 0x0DD6
            | 0x0E31
            | 0x0E34..=0x0E3A
            | 0x0E47..=0x0E4E
            | 0x0EB1
            | 0x0EB4..=0x0EBC
            | 0x0EC8..=0x0ECD
            | 0x0F18..=0x0F19
            | 0x0F35
            | 0x0F37
            | 0x0F39
            | 0x0F71..=0x0F7E
            | 0x0F80..=0x0F84
            | 0x0F86..=0x0F87
            | 0x0F8D..=0x0F97
            | 0x0F99..=0x0FBC
            | 0x0FC6
            | 0x102D..=0x1030
            | 0x1032..=0x1037
            | 0x1039..=0x103A
            | 0x103D..=0x103E
            | 0x1058..=0x1059
            | 0x105E..=0x1060
            | 0x1071..=0x1074
            | 0x1082
            | 0x1085..=0x1086
            | 0x108D
            | 0x109D
            | 0x135D..=0x135F
            | 0x1712..=0x1714
            | 0x1732..=0x1734
            | 0x1752..=0x1753
            | 0x1772..=0x1773
            | 0x17B4..=0x17B5
            | 0x17B7..=0x17BD
            | 0x17C6
            | 0x17C9..=0x17D3
            | 0x17DD
            | 0x180B..=0x180F
            | 0x1885..=0x1886
            | 0x18A9
            | 0x1AB0..=0x1AFF
            | 0x1DC0..=0x1DFF
            | 0x20D0..=0x20FF
            | 0xFE20..=0xFE2F
    )
}

#[derive(Clone, Copy, Debug, Default)]
struct CharFlags {
    is_present: bool,
    is_whitespace: bool,
    is_letter: bool,
    is_number: bool,
    is_mark: bool,
}

fn char_flags(ch: Option<char>) -> CharFlags {
    let Some(ch) = ch else {
        return CharFlags::default();
    };
    CharFlags {
        is_present: true,
        is_whitespace: ch.is_whitespace(),
        is_letter: ch.is_alphabetic(),
        is_number: ch.is_numeric(),
        is_mark: is_combining_mark(ch),
    }
}

fn encode_bpe_word_bytes(word: &str) -> String {
    let mut encoded = String::new();
    for &byte in word.as_bytes() {
        encoded.push_str(
            gpt2_byte_encoder()
                .get(usize::from(byte))
                .map(String::as_str)
                .unwrap_or(""),
        );
    }
    encoded
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

fn required_string_array(gguf: &GgufFile, key: &str) -> Result<Vec<String>> {
    optional_string_array(gguf, key)?
        .ok_or_else(|| LlamaError::format(format!("missing required gguf key '{}'", key)))
}

fn optional_string_array(gguf: &GgufFile, key: &str) -> Result<Option<Vec<String>>> {
    match gguf.get_value(key) {
        None => Ok(None),
        Some(GgufValue::Array(GgufArray::String(strings))) => strings
            .iter()
            .map(|value| value.try_utf8().map(str::to_owned))
            .collect::<Result<Vec<_>>>()
            .map(Some),
        Some(other) => Err(LlamaError::format(format!(
            "gguf key '{}' has type {}, expected string array",
            key,
            other.value_type().name()
        ))),
    }
}

fn optional_token_type_array(
    gguf: &GgufFile,
    key: &str,
    expected_len: usize,
) -> Result<Vec<LlamaTokenType>> {
    let Some(value) = gguf.get_value(key) else {
        return Ok(vec![LlamaTokenType::Normal; expected_len]);
    };
    let values = match value {
        GgufValue::Array(GgufArray::Int32(values)) => values
            .iter()
            .copied()
            .map(token_type_from_i32)
            .collect::<Result<Vec<_>>>()?,
        GgufValue::Array(GgufArray::Uint32(values)) => values
            .iter()
            .copied()
            .map(|value| {
                i32::try_from(value)
                    .map_err(|_| {
                        LlamaError::format(format!(
                            "token type value {} does not fit in i32",
                            value
                        ))
                    })
                    .and_then(token_type_from_i32)
            })
            .collect::<Result<Vec<_>>>()?,
        other => {
            return Err(LlamaError::format(format!(
                "gguf key '{}' has type {}, expected int32 array",
                key,
                other.value_type().name()
            )))
        }
    };
    if values.len() != expected_len {
        return Err(LlamaError::format(format!(
            "gguf key '{}' length mismatch: got {}, expected {}",
            key,
            values.len(),
            expected_len
        )));
    }
    Ok(values)
}

fn optional_f32_array(gguf: &GgufFile, key: &str, expected_len: usize) -> Result<Vec<f32>> {
    let Some(value) = gguf.get_value(key) else {
        return Ok(vec![0.0; expected_len]);
    };
    let values = match value {
        GgufValue::Array(GgufArray::Float32(values)) => values.clone(),
        other => {
            return Err(LlamaError::format(format!(
                "gguf key '{}' has type {}, expected f32 array",
                key,
                other.value_type().name()
            )))
        }
    };
    if values.len() != expected_len {
        return Err(LlamaError::format(format!(
            "gguf key '{}' length mismatch: got {}, expected {}",
            key,
            values.len(),
            expected_len
        )));
    }
    Ok(values)
}

fn token_type_from_i32(value: i32) -> Result<LlamaTokenType> {
    Ok(match value {
        0 => LlamaTokenType::Undefined,
        1 => LlamaTokenType::Normal,
        2 => LlamaTokenType::Unknown,
        3 => LlamaTokenType::Control,
        4 => LlamaTokenType::UserDefined,
        5 => LlamaTokenType::Unused,
        6 => LlamaTokenType::Byte,
        other => {
            return Err(LlamaError::format(format!(
                "unsupported tokenizer token_type {}",
                other
            )))
        }
    })
}

fn optional_u32(gguf: &GgufFile, key: &str) -> Option<u32> {
    gguf.get_value(key).and_then(value_to_u32)
}

fn optional_bool(gguf: &GgufFile, key: &str) -> Option<bool> {
    match gguf.get_value(key) {
        Some(GgufValue::Bool(value)) => Some(*value),
        _ => None,
    }
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

fn gpt2_byte_encoder() -> &'static Vec<String> {
    static ENCODER: OnceLock<Vec<String>> = OnceLock::new();
    ENCODER.get_or_init(|| {
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

        let mut map = vec![String::new(); 256];
        for (byte, codepoint) in bs.into_iter().zip(cs) {
            map[byte as usize] = char::from_u32(u32::from(codepoint))
                .expect("invalid gpt2 codepoint")
                .to_string();
        }
        map
    })
}

fn gpt2_byte_decoder() -> HashMap<char, u8> {
    gpt2_byte_encoder()
        .iter()
        .enumerate()
        .map(|(byte, piece)| {
            (
                piece.chars().next().expect("empty gpt2 byte piece"),
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
    use super::{
        decode_sentencepiece_byte_piece, encode_bpe_word_bytes, gpt2_byte_decoder,
        sentencepiece_escape_whitespace, sentencepiece_unescape_whitespace, split_gpt2_words,
        split_qwen_words, LlamaBpePreTokenizer, LlamaTextDecoder, LlamaTokenType,
        LlamaTokenizerKind, LlamaVocab, TokenizeFragment,
    };
    use std::collections::HashMap;

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

    #[test]
    fn qwen35_split_keeps_space_prefixed_words_and_single_digits() {
        assert_eq!(
            split_qwen_words("Hello world 42", true),
            vec!["Hello", " world", " ", "4", "2"]
        );
    }

    #[test]
    fn gpt2_split_keeps_digit_runs() {
        assert_eq!(
            split_gpt2_words("Hello world 42"),
            vec!["Hello", " world", " 42"]
        );
    }

    #[test]
    fn bpe_byte_encoding_maps_space_prefix() {
        assert_eq!(encode_bpe_word_bytes(" world"), "Ġworld");
    }

    #[test]
    fn sentencepiece_whitespace_round_trip() {
        assert_eq!(
            sentencepiece_escape_whitespace(" Hello world"),
            "▁Hello▁world"
        );
        assert_eq!(
            sentencepiece_unescape_whitespace("▁Hello▁world"),
            " Hello world"
        );
    }

    #[test]
    fn sentencepiece_byte_piece_decodes_hex_token() {
        assert_eq!(
            decode_sentencepiece_byte_piece("<0x0A>"),
            Some("\n".to_owned())
        );
    }

    #[test]
    fn control_tokens_are_partitioned_without_parse_special() {
        let vocab = mock_special_vocab();
        let fragments = vocab.partition_special_fragments("<|im_start|>user", false);
        assert_eq!(fragments.len(), 2);
        assert!(matches!(fragments[0], TokenizeFragment::Token(0)));
        assert!(matches!(&fragments[1], TokenizeFragment::Text(text) if text == "user"));
    }

    #[test]
    fn unknown_tokens_only_partition_when_parse_special_is_enabled() {
        let vocab = mock_special_vocab();

        let fragments_without_parse = vocab.partition_special_fragments("<unk>x", false);
        assert!(matches!(
            &fragments_without_parse[..],
            [TokenizeFragment::Text(text)] if text == "<unk>x"
        ));

        let fragments_with_parse = vocab.partition_special_fragments("<unk>x", true);
        assert!(matches!(
            &fragments_with_parse[..],
            [TokenizeFragment::Token(1), TokenizeFragment::Text(text)] if text == "x"
        ));
    }

    fn mock_special_vocab() -> LlamaVocab {
        let pieces = vec![
            "<|im_start|>".to_owned(),
            "<unk>".to_owned(),
            "user".to_owned(),
        ];
        let token_to_id = pieces
            .iter()
            .enumerate()
            .map(|(index, piece)| (piece.clone(), index as i32))
            .collect::<HashMap<_, _>>();
        LlamaVocab {
            pieces,
            token_types: vec![
                LlamaTokenType::Control,
                LlamaTokenType::Unknown,
                LlamaTokenType::Normal,
            ],
            token_scores: vec![0.0; 3],
            token_to_id,
            tokenizer_kind: LlamaTokenizerKind::Gpt2,
            bpe_pre: Some(LlamaBpePreTokenizer::Gpt2),
            tokenizer_pre: Some("gpt2".to_owned()),
            bos_token_id: None,
            eos_token_id: None,
            unk_token_id: None,
            padding_token_id: None,
            add_bos_token: false,
            add_eos_token: false,
            add_space_prefix: false,
            bpe_ranks: HashMap::new(),
        }
    }
}
