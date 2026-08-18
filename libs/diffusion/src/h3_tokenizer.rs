//! Qwen2-style byte-level BPE tokenizer for the MiniMax H3 text-encoder path.
//!
//! Parity target: HuggingFace transformers `AutoTokenizer` on the MiniMax-H3
//! tokenizer dir, `tokenizer(prompt, add_special_tokens=False)["input_ids"]`
//! (no BOS/EOS, no chat template).
//!
//! The pre-tokenizer splitter and the BPE merge loop are ported from
//! `libs/llama/src/vocab.rs` (`split_qwen_words` / `encode_bpe_word` /
//! `gpt2_byte_encoder`), which tokenizes Qwen2/3.5 GGUFs byte-exact against
//! llama.cpp. This module is self-contained: only the loading differs —
//! libs/llama reads vocab/merges from GGUF metadata, here we read the HF
//! `tokenizer.json` directly.
//!
//! Why `tokenizer.json` and not `vocab.json` + `merges.txt`: `tokenizer.json`
//! is the file transformers' fast tokenizer actually reads, and in the
//! MiniMax-H3 tokenizer dir the sibling `merges.txt` is corrupt — all 96
//! merge rules whose left symbol is `#` (lines starting with `"# "`) were
//! dropped as comments by whatever exported it (151291 rules vs 151387 in
//! `tokenizer.json`). Loading `merges.txt` therefore cannot reproduce the
//! reference ids; `tokenizer.json` is the single source of truth.
//!
//! Pinned behavior from `tokenizer.json`:
//! - `normalizer`: `{"type": "NFC"}`. We do NOT implement NFC (no Unicode
//!   composition tables without a dependency); prompts are assumed to already
//!   be NFC, which holds for ordinary text (all parity fixtures verified
//!   NFC-stable). Any other normalizer type is rejected at load.
//! - `pre_tokenizer`: `Sequence[ Split{Regex: <Qwen2 pattern>, Isolated},
//!   ByteLevel{add_prefix_space: false, use_regex: false} ]` — the Split regex
//!   is verified verbatim against [`QWEN2_SPLIT_REGEX`] at load so a different
//!   model can never silently mis-tokenize.
//! - `model`: plain BPE (no dropout / unk fusing / subword prefix / byte
//!   fallback), all verified at load.
//! - `added_tokens` (26 entries, `<|endoftext|>` .. `</think>`) are matched in
//!   the input text before pre-tokenization, longest match first, exactly like
//!   the HF added-token trie. `tokenizer_config.json`'s
//!   `additional_special_tokens` that are not already known (MiniMax adds
//!   `<d>`, `</d>`, `<|cutoff|>`, lyrics/caption markers) get fresh sequential
//!   ids after the existing maximum, mirroring what transformers does when it
//!   loads that config.
//!
//! Known deviations from the exact `\p{L}` / `\p{N}` regex classes (documented,
//! irrelevant for ordinary prompts, and covered by the parity fixtures):
//! - Rust `char::is_alphabetic()` is the `Alphabetic` property, a superset of
//!   `\p{L}` that also contains `Nl` and `Other_Alphabetic` marks. `Nl` is
//!   excluded here via an `is_numeric()` check (so Roman-numeral chars stay
//!   `\p{N}`-like), but `Other_Alphabetic` combining marks (e.g. Devanagari
//!   matras) are treated as letters where the regex would treat them as
//!   symbols.
//! - `libs/llama`'s splitter breaks symbol runs at combining marks (a Qwen3.5
//!   `\p{M}` behavior); the H3 regex has no `\p{M}`, so this port follows the
//!   literal regex instead and keeps marks inside `[^\s\p{L}\p{N}]+` runs.

use crate::error::{DiffusionError, Result};
use std::collections::HashMap;
use std::path::Path;

/// The exact Split regex pinned in MiniMax-H3 `tokenizer.json` (Qwen2 pattern).
const QWEN2_SPLIT_REGEX: &str = "(?i:'s|'t|'re|'ve|'m|'ll|'d)|[^\\r\\n\\p{L}\\p{N}]?\\p{L}+|\\p{N}| ?[^\\s\\p{L}\\p{N}]+[\\r\\n]*|\\s*[\\r\\n]+|\\s+(?!\\S)|\\s+";

pub struct H3Tokenizer {
    /// Byte-level-encoded piece (e.g. "Ġworld") -> token id.
    token_to_id: HashMap<String, u32>,
    /// BPE merge rule -> rank (lower merges first).
    merge_ranks: HashMap<(String, String), usize>,
    /// Added tokens matched verbatim before pre-tokenization, longest first.
    added_tokens: Vec<(String, u32)>,
    /// GPT-2 byte -> printable unicode char mapping.
    byte_encoder: [char; 256],
}

impl H3Tokenizer {
    /// Load from a HF tokenizer directory containing `tokenizer.json`
    /// (and optionally `tokenizer_config.json` for extra special tokens).
    pub fn load(tokenizer_dir: &Path) -> Result<Self> {
        let path = tokenizer_dir.join("tokenizer.json");
        let text = std::fs::read_to_string(&path)
            .map_err(|err| DiffusionError::io(&path, err.to_string()))?;
        let root = Json::parse(&text).map_err(|msg| DiffusionError::json(&path, msg))?;

        verify_normalizer(&root)?;
        verify_pre_tokenizer(&root)?;

        let model = root
            .get("model")
            .ok_or_else(|| DiffusionError::model("tokenizer.json has no model section"))?;
        verify_bpe_model(model)?;

        let vocab = model
            .get("vocab")
            .and_then(Json::as_obj)
            .ok_or_else(|| DiffusionError::model("tokenizer.json model.vocab is not an object"))?;
        let mut token_to_id = HashMap::with_capacity(vocab.len());
        for (piece, id) in vocab {
            let id = id.as_u32().ok_or_else(|| {
                DiffusionError::model(format!("vocab id for {:?} is not a u32", piece))
            })?;
            token_to_id.insert(piece.clone(), id);
        }

        let merges = model
            .get("merges")
            .and_then(Json::as_arr)
            .ok_or_else(|| DiffusionError::model("tokenizer.json model.merges is not an array"))?;
        let mut merge_ranks = HashMap::with_capacity(merges.len());
        for (rank, entry) in merges.iter().enumerate() {
            // A merge is either "left right" (split at the FIRST space: the
            // left symbol never contains a space in the GPT-2 byte alphabet,
            // the right one may be e.g. "----") or a ["left", "right"] pair
            // (newer tokenizers releases). Malformed entries are hard errors:
            // silently skipping merges is exactly the failure mode that
            // corrupted this model's merges.txt.
            let (left, right) = match entry {
                Json::Str(merge) => {
                    let split_at = merge.find(' ').filter(|at| *at > 0).ok_or_else(|| {
                        DiffusionError::model(format!("malformed merge rule {}: {:?}", rank, merge))
                    })?;
                    (merge[..split_at].to_owned(), merge[split_at + 1..].to_owned())
                }
                Json::Arr(pair) if pair.len() == 2 => {
                    match (pair[0].as_str(), pair[1].as_str()) {
                        (Some(left), Some(right)) => (left.to_owned(), right.to_owned()),
                        _ => {
                            return Err(DiffusionError::model(format!(
                                "malformed merge pair at rank {}",
                                rank
                            )))
                        }
                    }
                }
                _ => {
                    return Err(DiffusionError::model(format!(
                        "malformed merge entry at rank {}",
                        rank
                    )))
                }
            };
            merge_ranks.insert((left, right), rank);
        }

        let mut added_tokens = Vec::new();
        if let Some(entries) = root.get("added_tokens").and_then(Json::as_arr) {
            for entry in entries {
                let content = entry.get("content").and_then(Json::as_str).ok_or_else(|| {
                    DiffusionError::model("added_tokens entry has no content string")
                })?;
                let id = entry.get("id").and_then(Json::as_u32).ok_or_else(|| {
                    DiffusionError::model(format!("added token {:?} has no u32 id", content))
                })?;
                added_tokens.push((content.to_owned(), id));
            }
        }

        // transformers appends unknown `additional_special_tokens` from
        // tokenizer_config.json with fresh ids after the current vocabulary
        // end. MiniMax-H3 relies on this for <d>, </d>, <|cutoff|>,
        // <|lyrics_start|>, <|lyrics_end|>, <|caption_start|>,
        // <|caption_end|> (=> ids 151669..151675).
        let config_path = tokenizer_dir.join("tokenizer_config.json");
        if let Ok(config_text) = std::fs::read_to_string(&config_path) {
            let config = Json::parse(&config_text)
                .map_err(|msg| DiffusionError::json(&config_path, msg))?;
            if let Some(extra) = config.get("additional_special_tokens").and_then(Json::as_arr) {
                let mut next_id = token_to_id
                    .values()
                    .copied()
                    .chain(added_tokens.iter().map(|(_, id)| *id))
                    .max()
                    .map_or(0, |id| id + 1);
                for entry in extra {
                    // Entries are plain strings here, but configs may also
                    // serialize AddedToken objects.
                    let Some(content) = entry
                        .as_str()
                        .or_else(|| entry.get("content").and_then(Json::as_str))
                    else {
                        continue;
                    };
                    if token_to_id.contains_key(content)
                        || added_tokens.iter().any(|(existing, _)| existing == content)
                    {
                        continue;
                    }
                    added_tokens.push((content.to_owned(), next_id));
                    next_id += 1;
                }
            }
        }

        let tokenizer = Self::from_parts(token_to_id, merge_ranks, added_tokens);

        // Byte-level BPE needs every single mapped-byte piece in the vocab;
        // this makes encode() total (the per-char fallback always resolves).
        for byte in 0..=255_u8 {
            let piece = tokenizer.byte_encoder[byte as usize].to_string();
            if !tokenizer.token_to_id.contains_key(&piece) {
                return Err(DiffusionError::model(format!(
                    "vocab is missing the single-byte piece for 0x{:02X}",
                    byte
                )));
            }
        }
        Ok(tokenizer)
    }

    /// `add_special_tokens=False` semantics: no BOS/EOS/template, but added
    /// tokens appearing verbatim in the text are still matched to their ids.
    pub fn encode(&self, text: &str) -> Vec<u32> {
        let mut output = Vec::new();
        for fragment in self.split_added_tokens(text) {
            match fragment {
                Fragment::Special(id) => output.push(id),
                Fragment::Text(span) => {
                    for word in split_qwen2_words(span) {
                        self.encode_bpe_word(word, &mut output);
                    }
                }
            }
        }
        output
    }

    /// Resolve a full piece (e.g. `<|im_end|>` or a byte-level piece) to its id.
    pub fn token_id(&self, piece: &str) -> Option<u32> {
        self.added_tokens
            .iter()
            .find(|(content, _)| content == piece)
            .map(|(_, id)| *id)
            .or_else(|| self.token_to_id.get(piece).copied())
    }

    fn from_parts(
        token_to_id: HashMap<String, u32>,
        merge_ranks: HashMap<(String, String), usize>,
        mut added_tokens: Vec<(String, u32)>,
    ) -> Self {
        // Longest match first (ties: lowest id), as in libs/llama
        // partition_special_fragments.
        added_tokens.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.1.cmp(&b.1)));
        Self {
            token_to_id,
            merge_ranks,
            added_tokens,
            byte_encoder: gpt2_byte_to_unicode(),
        }
    }

    /// Split out added tokens before pre-tokenization (HF added-token trie).
    /// Port of libs/llama/src/vocab.rs partition_special_fragments.
    fn split_added_tokens<'a>(&self, text: &'a str) -> Vec<Fragment<'a>> {
        if self.added_tokens.is_empty() || text.is_empty() {
            return vec![Fragment::Text(text)];
        }
        let mut out = Vec::new();
        let mut cursor = 0;
        let mut text_start = 0;
        while cursor < text.len() {
            let suffix = &text[cursor..];
            let matched = self
                .added_tokens
                .iter()
                .find(|(content, _)| suffix.starts_with(content.as_str()));
            if let Some((content, id)) = matched {
                if text_start < cursor {
                    out.push(Fragment::Text(&text[text_start..cursor]));
                }
                out.push(Fragment::Special(*id));
                cursor += content.len();
                text_start = cursor;
                continue;
            }
            cursor += suffix.chars().next().map_or(1, |ch| ch.len_utf8());
        }
        if text_start < text.len() {
            out.push(Fragment::Text(&text[text_start..]));
        }
        out
    }

    /// Byte-level BPE over one pre-tokenized word.
    /// Port of libs/llama/src/vocab.rs encode_bpe_word (Qwen2 path: the word
    /// always starts as single mapped-byte chars, no whole-word shortcut).
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
            // Lowest rank wins; ties go to the leftmost pair (HF order).
            let mut best: Option<(usize, usize)> = None;
            for index in 0..(symbols.len() - 1) {
                let rank = self
                    .merge_ranks
                    .get(&(symbols[index].clone(), symbols[index + 1].clone()));
                if let Some(&rank) = rank {
                    if best.map_or(true, |(_, best_rank)| rank < best_rank) {
                        best = Some((index, rank));
                    }
                }
            }
            let Some((index, _)) = best else {
                break;
            };
            let merged = symbols[index].clone() + &symbols[index + 1];
            symbols[index] = merged;
            symbols.remove(index + 1);
        }

        for symbol in &symbols {
            if let Some(&id) = self.token_to_id.get(symbol) {
                output.push(id);
                continue;
            }
            // load() verified every single mapped-byte char resolves, so for a
            // loaded tokenizer this fallback always covers the whole symbol.
            for ch in symbol.chars() {
                if let Some(&id) = self.token_to_id.get(ch.to_string().as_str()) {
                    output.push(id);
                }
            }
        }
    }
}

enum Fragment<'a> {
    Text(&'a str),
    Special(u32),
}

// --- tokenizer.json verification ---------------------------------------------

fn verify_normalizer(root: &Json) -> Result<()> {
    match root.get("normalizer") {
        None | Some(Json::Null) => Ok(()),
        Some(normalizer) => {
            let kind = normalizer.get("type").and_then(Json::as_str).unwrap_or("?");
            if kind == "NFC" {
                // Treated as a no-op: prompts are assumed NFC already (see
                // module docs).
                Ok(())
            } else {
                Err(DiffusionError::model(format!(
                    "unsupported tokenizer normalizer {:?}",
                    kind
                )))
            }
        }
    }
}

fn verify_pre_tokenizer(root: &Json) -> Result<()> {
    let pre = root
        .get("pre_tokenizer")
        .ok_or_else(|| DiffusionError::model("tokenizer.json has no pre_tokenizer"))?;
    let entries: Vec<&Json> = if pre.get("type").and_then(Json::as_str) == Some("Sequence") {
        pre.get("pretokenizers")
            .and_then(Json::as_arr)
            .map(|arr| arr.iter().collect())
            .unwrap_or_default()
    } else {
        vec![pre]
    };

    let mut saw_split = false;
    let mut saw_byte_level = false;
    for entry in entries {
        match entry.get("type").and_then(Json::as_str) {
            Some("Split") => {
                let regex = entry
                    .get("pattern")
                    .and_then(|pattern| pattern.get("Regex"))
                    .and_then(Json::as_str)
                    .unwrap_or("");
                if regex != QWEN2_SPLIT_REGEX {
                    return Err(DiffusionError::model(format!(
                        "pre_tokenizer Split regex differs from the Qwen2 pattern this \
                         port implements: {:?}",
                        regex
                    )));
                }
                if entry.get("behavior").and_then(Json::as_str) != Some("Isolated")
                    || entry.get("invert").and_then(Json::as_bool) == Some(true)
                {
                    return Err(DiffusionError::model(
                        "pre_tokenizer Split must be behavior=Isolated, invert=false",
                    ));
                }
                saw_split = true;
            }
            Some("ByteLevel") => {
                if entry.get("add_prefix_space").and_then(Json::as_bool) == Some(true) {
                    return Err(DiffusionError::model(
                        "pre_tokenizer ByteLevel add_prefix_space=true is not supported",
                    ));
                }
                if entry.get("use_regex").and_then(Json::as_bool) == Some(true) {
                    return Err(DiffusionError::model(
                        "pre_tokenizer ByteLevel use_regex=true is not supported",
                    ));
                }
                saw_byte_level = true;
            }
            other => {
                return Err(DiffusionError::model(format!(
                    "unsupported pre_tokenizer entry {:?}",
                    other.unwrap_or("?")
                )));
            }
        }
    }
    if !saw_split || !saw_byte_level {
        return Err(DiffusionError::model(
            "pre_tokenizer must contain a Split(Qwen2 regex) and a ByteLevel step",
        ));
    }
    Ok(())
}

fn verify_bpe_model(model: &Json) -> Result<()> {
    if model.get("type").and_then(Json::as_str) != Some("BPE") {
        return Err(DiffusionError::model(format!(
            "unsupported tokenizer model type {:?}",
            model.get("type").and_then(Json::as_str).unwrap_or("?")
        )));
    }
    // Options our merge loop does not implement must be off.
    let empty_or_null =
        |key: &str| matches!(model.get(key), None | Some(Json::Null)) || model.get(key).and_then(Json::as_str) == Some("");
    let falsy = |key: &str| {
        matches!(model.get(key), None | Some(Json::Null)) || model.get(key).and_then(Json::as_bool) == Some(false)
    };
    if !matches!(model.get("dropout"), None | Some(Json::Null)) {
        return Err(DiffusionError::model("BPE dropout is not supported"));
    }
    if !empty_or_null("continuing_subword_prefix") || !empty_or_null("end_of_word_suffix") {
        return Err(DiffusionError::model(
            "BPE subword prefix/suffix is not supported",
        ));
    }
    if !falsy("fuse_unk") || !falsy("byte_fallback") {
        return Err(DiffusionError::model(
            "BPE fuse_unk/byte_fallback are not supported",
        ));
    }
    Ok(())
}

// --- Qwen2 pre-tokenizer splitter ---------------------------------------------
// Port of libs/llama/src/vocab.rs split_qwen_words(text, allow_marks=false),
// i.e. the regex-equivalent hand-rolled splitter for QWEN2_SPLIT_REGEX.

/// `\p{L}` approximation: Rust `Alphabetic` minus `\p{N}` (keeps Nl out of
/// letter runs; see module docs for the remaining Other_Alphabetic caveat).
fn is_letter(ch: char) -> bool {
    ch.is_alphabetic() && !ch.is_numeric()
}

fn is_crlf(ch: char) -> bool {
    matches!(ch, '\r' | '\n')
}

fn split_qwen2_words(text: &str) -> Vec<&str> {
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

        // (?i:'s|'t|'re|'ve|'m|'ll|'d)
        if ch == '\'' && pos + 1 < chars.len() {
            let next = chars[pos + 1].1.to_ascii_lowercase();
            if matches!(next, 's' | 't' | 'm' | 'd') {
                out.push(slice(pos, pos + 2));
                pos += 2;
                continue;
            }
            if pos + 2 < chars.len() {
                let next_next = chars[pos + 2].1.to_ascii_lowercase();
                if matches!((next, next_next), ('r', 'e') | ('v', 'e') | ('l', 'l')) {
                    out.push(slice(pos, pos + 3));
                    pos += 3;
                    continue;
                }
            }
        }

        // [^\r\n\p{L}\p{N}]?\p{L}+
        if !is_crlf(ch) && !ch.is_numeric() {
            let next_is_letter = pos + 1 < chars.len() && is_letter(chars[pos + 1].1);
            if is_letter(ch) || next_is_letter {
                let mut next_pos = pos + 1;
                while next_pos < chars.len() && is_letter(chars[next_pos].1) {
                    next_pos += 1;
                }
                out.push(slice(pos, next_pos));
                pos = next_pos;
                continue;
            }
        }

        // \p{N} (single numeric char — Qwen2 splits digit runs)
        if ch.is_numeric() {
            out.push(slice(pos, pos + 1));
            pos += 1;
            continue;
        }

        // ` ?[^\s\p{L}\p{N}]+[\r\n]*`
        let is_symbol =
            |c: char| !c.is_whitespace() && !is_letter(c) && !c.is_numeric();
        let probe = if ch == ' ' {
            (pos + 1 < chars.len()).then(|| chars[pos + 1].1)
        } else {
            Some(ch)
        };
        if probe.map_or(false, is_symbol) {
            let mut next_pos = pos + usize::from(ch == ' ');
            while next_pos < chars.len() && is_symbol(chars[next_pos].1) {
                next_pos += 1;
            }
            while next_pos < chars.len() && is_crlf(chars[next_pos].1) {
                next_pos += 1;
            }
            out.push(slice(pos, next_pos));
            pos = next_pos;
            continue;
        }

        // `\s*[\r\n]+` | `\s+(?!\S)` | `\s+`
        let mut whitespace_count = 0;
        let mut newline_end = None;
        while pos + whitespace_count < chars.len() && chars[pos + whitespace_count].1.is_whitespace()
        {
            if is_crlf(chars[pos + whitespace_count].1) {
                newline_end = Some(pos + whitespace_count + 1);
            }
            whitespace_count += 1;
        }
        if let Some(newline_end) = newline_end {
            out.push(slice(pos, newline_end));
            pos = newline_end;
            continue;
        }
        if whitespace_count > 1 && pos + whitespace_count < chars.len() {
            // `\s+(?!\S)`: leave the last whitespace to prefix the next word.
            out.push(slice(pos, pos + whitespace_count - 1));
            pos += whitespace_count - 1;
            continue;
        }
        if whitespace_count > 0 {
            out.push(slice(pos, pos + whitespace_count));
            pos += whitespace_count;
            continue;
        }

        out.push(slice(pos, pos + 1));
        pos += 1;
    }
    out
}

/// GPT-2 byte -> printable unicode mapping (port of libs/llama/src/vocab.rs
/// gpt2_byte_encoder): printable bytes map to themselves, the rest to
/// U+0100.. in byte order. Shared with the Woosh RoBERTa tokenizer.
pub(crate) fn gpt2_byte_to_unicode() -> [char; 256] {
    let mut used = [false; 256];
    for byte in 33..=126_usize {
        used[byte] = true;
    }
    for byte in 161..=172_usize {
        used[byte] = true;
    }
    for byte in 174..=255_usize {
        used[byte] = true;
    }
    let mut map = ['\0'; 256];
    let mut extra = 0_u32;
    for byte in 0..256_usize {
        map[byte] = if used[byte] {
            char::from_u32(byte as u32).expect("printable byte")
        } else {
            let ch = char::from_u32(256 + extra).expect("remapped byte");
            extra += 1;
            ch
        };
    }
    map
}

// --- minimal JSON parser -------------------------------------------------------
// Hand-rolled because makepad-micro-serde's DeJson maps `\uXXXX` surrogate
// pairs to '?' (char::from_u32(hi).unwrap_or('?')), which would corrupt
// emoji in fixture files; this one handles surrogate pairs and is reused by
// the h3-tokenize bin for tok_ref.json.

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn parse(text: &str) -> std::result::Result<Json, String> {
        let mut parser = JsonParser {
            bytes: text.as_bytes(),
            pos: 0,
        };
        parser.skip_ws();
        let value = parser.parse_value(0)?;
        parser.skip_ws();
        if parser.pos != parser.bytes.len() {
            return Err(format!("trailing data at byte {}", parser.pos));
        }
        Ok(value)
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(entries) => entries
                .iter()
                .find(|(entry_key, _)| entry_key == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_u32(&self) -> Option<u32> {
        self.as_f64()
            .filter(|n| *n >= 0.0 && n.fract() == 0.0 && *n <= u32::MAX as f64)
            .map(|n| n as u32)
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_arr(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(values) => Some(values),
            _ => None,
        }
    }

    pub fn as_obj(&self) -> Option<&[(String, Json)]> {
        match self {
            Json::Obj(entries) => Some(entries),
            _ => None,
        }
    }
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl JsonParser<'_> {
    fn skip_ws(&mut self) {
        while matches!(
            self.bytes.get(self.pos),
            Some(b' ' | b'\t' | b'\n' | b'\r')
        ) {
            self.pos += 1;
        }
    }

    fn parse_value(&mut self, depth: usize) -> std::result::Result<Json, String> {
        if depth > 256 {
            return Err("json nesting too deep".to_owned());
        }
        match self.bytes.get(self.pos) {
            Some(b'{') => {
                self.pos += 1;
                let mut entries = Vec::new();
                self.skip_ws();
                if self.bytes.get(self.pos) == Some(&b'}') {
                    self.pos += 1;
                    return Ok(Json::Obj(entries));
                }
                loop {
                    self.skip_ws();
                    let key = self.parse_string()?;
                    self.skip_ws();
                    if self.bytes.get(self.pos) != Some(&b':') {
                        return Err(format!("expected ':' at byte {}", self.pos));
                    }
                    self.pos += 1;
                    self.skip_ws();
                    let value = self.parse_value(depth + 1)?;
                    entries.push((key, value));
                    self.skip_ws();
                    match self.bytes.get(self.pos) {
                        Some(b',') => self.pos += 1,
                        Some(b'}') => {
                            self.pos += 1;
                            return Ok(Json::Obj(entries));
                        }
                        _ => return Err(format!("expected ',' or '}}' at byte {}", self.pos)),
                    }
                }
            }
            Some(b'[') => {
                self.pos += 1;
                let mut values = Vec::new();
                self.skip_ws();
                if self.bytes.get(self.pos) == Some(&b']') {
                    self.pos += 1;
                    return Ok(Json::Arr(values));
                }
                loop {
                    self.skip_ws();
                    values.push(self.parse_value(depth + 1)?);
                    self.skip_ws();
                    match self.bytes.get(self.pos) {
                        Some(b',') => self.pos += 1,
                        Some(b']') => {
                            self.pos += 1;
                            return Ok(Json::Arr(values));
                        }
                        _ => return Err(format!("expected ',' or ']' at byte {}", self.pos)),
                    }
                }
            }
            Some(b'"') => Ok(Json::Str(self.parse_string()?)),
            Some(b't') => self.parse_literal("true", Json::Bool(true)),
            Some(b'f') => self.parse_literal("false", Json::Bool(false)),
            Some(b'n') => self.parse_literal("null", Json::Null),
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            _ => Err(format!("unexpected input at byte {}", self.pos)),
        }
    }

    fn parse_literal(
        &mut self,
        literal: &str,
        value: Json,
    ) -> std::result::Result<Json, String> {
        if self.bytes[self.pos..].starts_with(literal.as_bytes()) {
            self.pos += literal.len();
            Ok(value)
        } else {
            Err(format!("expected {} at byte {}", literal, self.pos))
        }
    }

    fn parse_number(&mut self) -> std::result::Result<Json, String> {
        let start = self.pos;
        while matches!(
            self.bytes.get(self.pos),
            Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
        ) {
            self.pos += 1;
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| format!("invalid number at byte {}", start))?;
        text.parse::<f64>()
            .map(Json::Num)
            .map_err(|_| format!("invalid number {:?} at byte {}", text, start))
    }

    fn parse_string(&mut self) -> std::result::Result<String, String> {
        if self.bytes.get(self.pos) != Some(&b'"') {
            return Err(format!("expected string at byte {}", self.pos));
        }
        self.pos += 1;
        let mut out = String::new();
        loop {
            let run_start = self.pos;
            while let Some(&byte) = self.bytes.get(self.pos) {
                if byte == b'"' || byte == b'\\' || byte < 0x20 {
                    break;
                }
                self.pos += 1;
            }
            // Runs are delimited by ASCII bytes, so they sit on UTF-8
            // boundaries of the source &str.
            out.push_str(
                std::str::from_utf8(&self.bytes[run_start..self.pos])
                    .map_err(|_| format!("invalid utf-8 in string at byte {}", run_start))?,
            );
            match self.bytes.get(self.pos) {
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    let escape = *self
                        .bytes
                        .get(self.pos)
                        .ok_or_else(|| "unterminated escape".to_owned())?;
                    self.pos += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{0008}'),
                        b'f' => out.push('\u{000C}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let unit = self.parse_hex4()?;
                            let ch = if (0xD800..=0xDBFF).contains(&unit) {
                                // Surrogate pair: require the low half.
                                if self.bytes.get(self.pos) != Some(&b'\\')
                                    || self.bytes.get(self.pos + 1) != Some(&b'u')
                                {
                                    return Err(format!(
                                        "lone high surrogate at byte {}",
                                        self.pos
                                    ));
                                }
                                self.pos += 2;
                                let low = self.parse_hex4()?;
                                if !(0xDC00..=0xDFFF).contains(&low) {
                                    return Err(format!(
                                        "invalid low surrogate at byte {}",
                                        self.pos
                                    ));
                                }
                                let code =
                                    0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00);
                                char::from_u32(code)
                                    .ok_or_else(|| "invalid surrogate pair".to_owned())?
                            } else if (0xDC00..=0xDFFF).contains(&unit) {
                                return Err(format!("lone low surrogate at byte {}", self.pos));
                            } else {
                                char::from_u32(unit)
                                    .ok_or_else(|| "invalid \\u escape".to_owned())?
                            };
                            out.push(ch);
                        }
                        other => {
                            return Err(format!(
                                "unsupported escape '\\{}' at byte {}",
                                other as char, self.pos
                            ))
                        }
                    }
                }
                Some(_) => return Err(format!("control byte in string at byte {}", self.pos)),
                None => return Err("unterminated string".to_owned()),
            }
        }
    }

    fn parse_hex4(&mut self) -> std::result::Result<u32, String> {
        let end = self.pos + 4;
        if end > self.bytes.len() {
            return Err("truncated \\u escape".to_owned());
        }
        let text = std::str::from_utf8(&self.bytes[self.pos..end])
            .map_err(|_| "invalid \\u escape".to_owned())?;
        let value =
            u32::from_str_radix(text, 16).map_err(|_| format!("invalid \\u escape {:?}", text))?;
        self.pos = end;
        Ok(value)
    }
}

// --- tests ----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_tokenizer(
        vocab: &[(&str, u32)],
        merges: &[(&str, &str)],
        added: &[(&str, u32)],
    ) -> H3Tokenizer {
        let token_to_id = vocab
            .iter()
            .map(|(piece, id)| (piece.to_string(), *id))
            .collect();
        let merge_ranks = merges
            .iter()
            .enumerate()
            .map(|(rank, (left, right))| ((left.to_string(), right.to_string()), rank))
            .collect();
        let added_tokens = added
            .iter()
            .map(|(content, id)| (content.to_string(), *id))
            .collect();
        H3Tokenizer::from_parts(token_to_id, merge_ranks, added_tokens)
    }

    #[test]
    fn json_parses_escapes_and_surrogate_pairs() {
        let value = Json::parse(r#"{"text": "a\nb\t\"\\ é 🐉", "n": [1, 2.5, -3]}"#)
            .expect("parse");
        assert_eq!(
            value.get("text").and_then(Json::as_str),
            Some("a\nb\t\"\\ \u{e9} \u{1F409}")
        );
        let numbers: Vec<f64> = value
            .get("n")
            .and_then(Json::as_arr)
            .expect("arr")
            .iter()
            .filter_map(Json::as_f64)
            .collect();
        assert_eq!(numbers, vec![1.0, 2.5, -3.0]);
        assert!(Json::parse(r#""\ud83d""#).is_err());
    }

    #[test]
    fn byte_mapping_roundtrip_covers_all_bytes() {
        let encoder = gpt2_byte_to_unicode();
        let mut decoder = HashMap::new();
        for (byte, ch) in encoder.iter().enumerate() {
            assert!(decoder.insert(*ch, byte as u8).is_none(), "duplicate char");
        }
        assert_eq!(decoder.len(), 256);
        assert_eq!(encoder[b' ' as usize], '\u{120}'); // Ġ
        assert_eq!(encoder[b'H' as usize], 'H');
        assert_eq!(encoder[b'\n' as usize], '\u{10A}'); // Ċ
    }

    #[test]
    fn qwen2_splitter_matches_reference_cases() {
        assert_eq!(
            split_qwen2_words("Hello world 42"),
            vec!["Hello", " world", " ", "4", "2"]
        );
        assert_eq!(split_qwen2_words("don't stop"), vec!["don", "'t", " stop"]);
        assert_eq!(
            split_qwen2_words("crema,\" it"),
            vec!["crema", ",\"", " it"]
        );
        assert_eq!(split_qwen2_words("a  \n b"), vec!["a", "  \n", " b"]);
        assert_eq!(split_qwen2_words("x\ttabs"), vec!["x", "\ttabs"]);
        assert_eq!(
            split_qwen2_words("dots...   three   sp"),
            vec!["dots", "...", "  ", " three", "  ", " sp"]
        );
        assert_eq!(split_qwen2_words("東京の夜"), vec!["東京の夜"]);
        assert_eq!(split_qwen2_words(" 🐉🔥"), vec![" 🐉🔥"]);
    }

    #[test]
    fn tiny_vocab_bpe_merges_best_rank_first() {
        let tokenizer = tiny_tokenizer(
            &[("a", 0), ("b", 1), ("c", 2), ("ab", 3), ("abc", 4)],
            &[("a", "b"), ("ab", "c")],
            &[],
        );
        assert_eq!(tokenizer.encode("abc"), vec![4]);
        assert_eq!(tokenizer.encode("abab"), vec![3, 3]);
        assert_eq!(tokenizer.encode("cba"), vec![2, 1, 0]);
        assert_eq!(tokenizer.encode(""), Vec::<u32>::new());
    }

    #[test]
    fn space_prefixed_word_maps_to_g_piece() {
        let tokenizer = tiny_tokenizer(
            &[("a", 0), ("\u{120}", 1), ("\u{120}a", 2)],
            &[("\u{120}", "a")],
            &[],
        );
        assert_eq!(tokenizer.encode(" a"), vec![2]);
    }

    #[test]
    fn added_tokens_split_before_pretokenization() {
        let tokenizer = tiny_tokenizer(
            &[("h", 0), ("i", 1), ("hi", 2), ("<", 3)],
            &[("h", "i")],
            &[("<|x|>", 9), ("<|xx|>", 10)],
        );
        assert_eq!(tokenizer.encode("hi<|x|>hi"), vec![2, 9, 2]);
        // Longest added token wins.
        assert_eq!(tokenizer.encode("<|xx|>"), vec![10]);
        assert_eq!(tokenizer.token_id("<|x|>"), Some(9));
        assert_eq!(tokenizer.token_id("hi"), Some(2));
    }

    /// Full parity gate against transformers reference ids. Skips silently
    /// unless both env vars point at the fixture data, e.g.
    ///   H3_TOK_DIR=.../fixtures/tok H3_TOK_FIXTURE=.../fixtures/tok_ref.json \
    ///     cargo test -p makepad-diffusion --lib
    #[test]
    fn fixture_parity_when_env_set() {
        let (Ok(dir), Ok(fixture)) = (
            std::env::var("H3_TOK_DIR"),
            std::env::var("H3_TOK_FIXTURE"),
        ) else {
            return;
        };
        let tokenizer = H3Tokenizer::load(Path::new(&dir)).expect("load tokenizer");
        let text = std::fs::read_to_string(&fixture).expect("read fixture");
        let root = Json::parse(&text).expect("parse fixture");
        let prompts = root.get("prompts").and_then(Json::as_arr).expect("prompts");
        assert!(!prompts.is_empty());
        for prompt in prompts {
            let name = prompt.get("name").and_then(Json::as_str).unwrap_or("?");
            let text = prompt.get("text").and_then(Json::as_str).expect("text");
            let expected: Vec<u32> = prompt
                .get("ids")
                .and_then(Json::as_arr)
                .expect("ids")
                .iter()
                .map(|id| id.as_u32().expect("u32 id"))
                .collect();
            assert_eq!(tokenizer.encode(text), expected, "prompt {}", name);
        }
    }
}
