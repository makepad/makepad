//! Mistral "Tekken" byte-level BPE tokenizer for the FLUX.2 text-encoder path.
//!
//! Parity target: HuggingFace `tokenizers` on the FLUX.2-dev `tokenizer/`
//! dir (byte-identical to mistralai/Mistral-Small-3.1-24B-Instruct-2503's
//! tokenizer.json, LFS sha b76085f9...), `tokenizer.encode(prompt,
//! add_special_tokens=False).ids`. Inline fixtures in the tests below were
//! generated with `tokenizers` 0.23.1 against the real tokenizer.json.
//!
//! This is a sibling of [`crate::h3_tokenizer`] (Qwen2 pattern); the Tekken
//! deltas, each pinned by verification at load:
//! - pre-tokenizer Split regex groups digit runs `\p{N}{1,3}` (Qwen2 splits
//!   single digits); everything else in the pattern is identical.
//! - NO normalizer at all (h3 had NFC; here `normalizer: null` in the json).
//! - `model.ignore_merges: true` — a pre-tokenized word that exists in the
//!   vocab verbatim is emitted directly without running the merge loop.
//! - 1000 added special tokens (`<unk>`=0 .. `<SPECIAL_999>`), all with
//!   lstrip/rstrip/normalized/single_word = false (verified uniform via the
//!   real file), so verbatim trie matching is exact.
//!
//! The shared pieces (GPT-2 byte alphabet, the surrogate-safe JSON parser)
//! are reused from `h3_tokenizer`; the BPE merge loop is re-implemented here
//! because it needs the ignore_merges shortcut.
//!
//! Conditioning ids: FLUX.2's pipeline formats the prompt through the
//! Mistral-3 chat template (see [`render_flux2_t2i_prompt`]) and tokenizes
//! the rendered string — the template's literal `<s>`/`[SYSTEM_PROMPT]`/
//! `[INST]` markers hit the added-token trie, so `encode` covers the whole
//! flow. Padding: fixed 512 tokens with `<pad>` (id 11), truncation at 512.

use crate::error::{DiffusionError, Result};
use crate::h3_tokenizer::{gpt2_byte_to_unicode, Json};
use std::collections::HashMap;
use std::path::Path;

/// The exact Split regex pinned in Tekken `tokenizer.json`. Differs from the
/// Qwen2 pattern only in `\p{N}{1,3}` (vs `\p{N}`).
const TEKKEN_SPLIT_REGEX: &str = "(?i:'s|'t|'re|'ve|'m|'ll|'d)|[^\\r\\n\\p{L}\\p{N}]?\\p{L}+|\\p{N}{1,3}| ?[^\\s\\p{L}\\p{N}]+[\\r\\n]*|\\s*[\\r\\n]+|\\s+(?!\\S)|\\s+";

/// Special-token ids pinned from the FLUX.2 tokenizer (verified at load by
/// [`Flux2Tokenizer::verify_special_ids`]).
pub const FLUX2_TOKEN_UNK: u32 = 0;
pub const FLUX2_TOKEN_BOS: u32 = 1; // <s>
pub const FLUX2_TOKEN_EOS: u32 = 2; // </s>
pub const FLUX2_TOKEN_INST: u32 = 3; // [INST]
pub const FLUX2_TOKEN_INST_END: u32 = 4; // [/INST]
pub const FLUX2_TOKEN_PAD: u32 = 11; // <pad>
pub const FLUX2_TOKEN_SYSTEM: u32 = 17; // [SYSTEM_PROMPT]
pub const FLUX2_TOKEN_SYSTEM_END: u32 = 18; // [/SYSTEM_PROMPT]

/// FLUX.2's conditioning window: `max_sequence_length=512`, always padded to
/// full length (`padding="max_length", truncation=True` in the reference
/// pipeline).
pub const FLUX2_MAX_SEQUENCE_LENGTH: usize = 512;

/// Render the FLUX.2 t2i chat-template string for one prompt.
///
/// Mistral V7-Tekken instruct format: the template is applied by the
/// reference pipeline via `apply_chat_template` with a system + user
/// message; the rendered string (which our `encode` then tokenizes,
/// matching the literal special markers through the added-token trie) is:
///
/// `<s>[SYSTEM_PROMPT]{system}[/SYSTEM_PROMPT][INST]{prompt}[/INST]`
///
/// ASSUMPTION PINNED PENDING ORACLE: this matches FLUX.2-dev's bundled
/// `tokenizer/chat_template.jinja` (2,670 bytes, gated) for the
/// one-system-one-user case. Verify the rendered ids against the reference
/// dump on the first .169 oracle run (`ref_dump_flux2.py` dumps input_ids);
/// the raw `encode` below is HF-parity-tested independently of this.
pub fn render_flux2_t2i_prompt(system_message: &str, prompt: &str) -> String {
    format!("<s>[SYSTEM_PROMPT]{system_message}[/SYSTEM_PROMPT][INST]{prompt}[/INST]")
}

/// A tokenized FLUX.2 conditioning window: always exactly
/// [`FLUX2_MAX_SEQUENCE_LENGTH`] ids, `<pad>`-filled after `real_len`.
#[derive(Clone, Debug)]
pub struct Flux2TokenizedPrompt {
    pub token_ids: Vec<u32>,
    /// Number of non-pad ids (the attention-mask boundary).
    pub real_len: usize,
}

pub struct Flux2Tokenizer {
    /// Byte-level-encoded piece (e.g. "Ġworld") -> token id.
    token_to_id: HashMap<String, u32>,
    /// BPE merge rule -> rank (lower merges first).
    merge_ranks: HashMap<(String, String), usize>,
    /// Added tokens matched verbatim before pre-tokenization, longest first.
    added_tokens: Vec<(String, u32)>,
    /// GPT-2 byte -> printable unicode char mapping.
    byte_encoder: [char; 256],
}

impl Flux2Tokenizer {
    /// Load from a HF tokenizer directory containing `tokenizer.json`.
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
            // Tekken serializes merges as ["left", "right"] pairs; accept the
            // legacy "left right" string form too. Malformed entries are hard
            // errors (see h3_tokenizer for the merges-corruption war story).
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
                // The verbatim trie assumes no lstrip/rstrip whitespace
                // absorption; the real Tekken file is uniformly false.
                for flag in ["lstrip", "rstrip", "single_word"] {
                    if entry.get(flag).and_then(Json::as_bool) == Some(true) {
                        return Err(DiffusionError::model(format!(
                            "added token {:?} sets {}=true, which this port does not implement",
                            content, flag
                        )));
                    }
                }
                added_tokens.push((content.to_owned(), id));
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

        tokenizer.verify_special_ids()?;
        Ok(tokenizer)
    }

    /// The pinned special ids this module exports as constants must match the
    /// loaded file — a different tokenizer can never silently mis-pad.
    fn verify_special_ids(&self) -> Result<()> {
        for (piece, expected) in [
            ("<unk>", FLUX2_TOKEN_UNK),
            ("<s>", FLUX2_TOKEN_BOS),
            ("</s>", FLUX2_TOKEN_EOS),
            ("[INST]", FLUX2_TOKEN_INST),
            ("[/INST]", FLUX2_TOKEN_INST_END),
            ("<pad>", FLUX2_TOKEN_PAD),
            ("[SYSTEM_PROMPT]", FLUX2_TOKEN_SYSTEM),
            ("[/SYSTEM_PROMPT]", FLUX2_TOKEN_SYSTEM_END),
        ] {
            match self.token_id(piece) {
                Some(id) if id == expected => {}
                other => {
                    return Err(DiffusionError::model(format!(
                        "special token {:?} has id {:?}, expected {}",
                        piece, other, expected
                    )))
                }
            }
        }
        Ok(())
    }

    /// `add_special_tokens=False` semantics: no BOS/EOS/template, but added
    /// tokens appearing verbatim in the text are still matched to their ids.
    pub fn encode(&self, text: &str) -> Vec<u32> {
        let mut output = Vec::new();
        for fragment in self.split_added_tokens(text) {
            match fragment {
                Fragment::Special(id) => output.push(id),
                Fragment::Text(span) => {
                    for word in split_tekken_words(span) {
                        self.encode_bpe_word(word, &mut output);
                    }
                }
            }
        }
        output
    }

    /// Tokenize one t2i prompt through the chat template into the fixed
    /// 512-token conditioning window (truncate at 512, `<pad>`-fill).
    pub fn encode_t2i(&self, system_message: &str, prompt: &str) -> Flux2TokenizedPrompt {
        let rendered = render_flux2_t2i_prompt(system_message, prompt);
        let mut token_ids = self.encode(&rendered);
        token_ids.truncate(FLUX2_MAX_SEQUENCE_LENGTH);
        let real_len = token_ids.len();
        token_ids.resize(FLUX2_MAX_SEQUENCE_LENGTH, FLUX2_TOKEN_PAD);
        Flux2TokenizedPrompt {
            token_ids,
            real_len,
        }
    }

    /// Tokenize one t2i prompt through the chat template WITHOUT padding —
    /// the ComfyUI reference semantics (batch-1 unpadded; the DiT's 512-row
    /// text window comes from zero-left-padding the conditioning, not the
    /// ids). Still truncates at 512.
    pub fn encode_t2i_unpadded(&self, system_message: &str, prompt: &str) -> Vec<u32> {
        let rendered = render_flux2_t2i_prompt(system_message, prompt);
        let mut token_ids = self.encode(&rendered);
        token_ids.truncate(FLUX2_MAX_SEQUENCE_LENGTH);
        token_ids
    }

    /// Resolve a full piece (added token or byte-level piece) to its id.
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
        // Longest match first (ties: lowest id), as in the HF added-token trie.
        added_tokens.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.1.cmp(&b.1)));
        Self {
            token_to_id,
            merge_ranks,
            added_tokens,
            byte_encoder: gpt2_byte_to_unicode(),
        }
    }

    /// Split out added tokens before pre-tokenization (HF added-token trie).
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

    /// Byte-level BPE over one pre-tokenized word, with the Tekken
    /// `ignore_merges` shortcut: a word whose byte-level form is already a
    /// vocab entry is emitted whole without running the merge loop.
    fn encode_bpe_word(&self, word: &str, output: &mut Vec<u32>) {
        if word.is_empty() {
            return;
        }
        let mapped: String = word
            .as_bytes()
            .iter()
            .map(|&byte| self.byte_encoder[byte as usize])
            .collect();

        // ignore_merges=true (verified at load).
        if let Some(&id) = self.token_to_id.get(mapped.as_str()) {
            output.push(id);
            return;
        }

        let mut symbols: Vec<String> = mapped.chars().map(|ch| ch.to_string()).collect();

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
        Some(normalizer) => Err(DiffusionError::model(format!(
            "Tekken tokenizer expects no normalizer, found {:?}",
            normalizer.get("type").and_then(Json::as_str).unwrap_or("?")
        ))),
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
                if regex != TEKKEN_SPLIT_REGEX {
                    return Err(DiffusionError::model(format!(
                        "pre_tokenizer Split regex differs from the Tekken pattern this \
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
            "pre_tokenizer must contain a Split(Tekken regex) and a ByteLevel step",
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
    let empty_or_null = |key: &str| {
        matches!(model.get(key), None | Some(Json::Null))
            || model.get(key).and_then(Json::as_str) == Some("")
    };
    let falsy = |key: &str| {
        matches!(model.get(key), None | Some(Json::Null))
            || model.get(key).and_then(Json::as_bool) == Some(false)
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
    // encode_bpe_word implements the whole-word shortcut unconditionally, so
    // a file WITHOUT ignore_merges would mis-tokenize; refuse it.
    if model.get("ignore_merges").and_then(Json::as_bool) != Some(true) {
        return Err(DiffusionError::model(
            "this port requires model.ignore_merges=true (Tekken)",
        ));
    }
    Ok(())
}

// --- Tekken pre-tokenizer splitter --------------------------------------------
// Regex-equivalent hand-rolled splitter for TEKKEN_SPLIT_REGEX. Identical to
// h3_tokenizer's Qwen2 splitter except the digit branch takes `\p{N}{1,3}`
// (greedy left-to-right runs of up to three digits). The `\p{L}` class
// approximation caveats are the same (see h3_tokenizer module docs).

fn is_letter(ch: char) -> bool {
    ch.is_alphabetic() && !ch.is_numeric()
}

fn is_crlf(ch: char) -> bool {
    matches!(ch, '\r' | '\n')
}

fn split_tekken_words(text: &str) -> Vec<&str> {
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

        // \p{N}{1,3} (Tekken: digit runs in greedy groups of up to three)
        if ch.is_numeric() {
            let mut next_pos = pos + 1;
            while next_pos < chars.len()
                && next_pos - pos < 3
                && chars[next_pos].1.is_numeric()
            {
                next_pos += 1;
            }
            out.push(slice(pos, next_pos));
            pos = next_pos;
            continue;
        }

        // ` ?[^\s\p{L}\p{N}]+[\r\n]*`
        let is_symbol = |c: char| !c.is_whitespace() && !is_letter(c) && !c.is_numeric();
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
        while pos + whitespace_count < chars.len()
            && chars[pos + whitespace_count].1.is_whitespace()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tiny_tokenizer(
        vocab: &[(&str, u32)],
        merges: &[(&str, &str)],
        added: &[(&str, u32)],
    ) -> Flux2Tokenizer {
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
            .map(|(piece, id)| (piece.to_string(), *id))
            .collect();
        Flux2Tokenizer::from_parts(token_to_id, merge_ranks, added_tokens)
    }

    #[test]
    fn tekken_splitter_groups_digit_runs() {
        assert_eq!(
            split_tekken_words("Hello world 42"),
            vec!["Hello", " world", " ", "42"]
        );
        assert_eq!(
            split_tekken_words("It costs 1234567 dollars"),
            vec!["It", " costs", " ", "123", "456", "7", " dollars"]
        );
        assert_eq!(split_tekken_words("v2.5"), vec!["v", "2", ".", "5"]);
        assert_eq!(
            split_tekken_words("1024x1024"),
            vec!["102", "4", "x", "102", "4"]
        );
    }

    #[test]
    fn tekken_splitter_matches_qwen2_on_non_digits() {
        assert_eq!(split_tekken_words("don't stop"), vec!["don", "'t", " stop"]);
        assert_eq!(split_tekken_words("crema,\" it"), vec!["crema", ",\"", " it"]);
        assert_eq!(split_tekken_words("a  \n b"), vec!["a", "  \n", " b"]);
        assert_eq!(split_tekken_words("x\ttabs"), vec!["x", "\ttabs"]);
        assert_eq!(
            split_tekken_words("dots...   three   sp"),
            vec!["dots", "...", "  ", " three", "  ", " sp"]
        );
        assert_eq!(split_tekken_words("東京の夜"), vec!["東京の夜"]);
        assert_eq!(split_tekken_words(" 🐉🔥"), vec![" 🐉🔥"]);
    }

    #[test]
    fn ignore_merges_prefers_whole_word_vocab_hit() {
        // "ab" is in the vocab with NO merge rule producing it: only the
        // ignore_merges shortcut can emit id 5.
        let tokenizer = tiny_tokenizer(&[("a", 0), ("b", 1), ("ab", 5)], &[], &[]);
        assert_eq!(tokenizer.encode("ab"), vec![5]);
        // Without a whole-word hit the merge loop still applies.
        let tokenizer = tiny_tokenizer(&[("a", 0), ("b", 1), ("c", 2)], &[], &[]);
        assert_eq!(tokenizer.encode("cba"), vec![2, 1, 0]);
    }

    #[test]
    fn added_tokens_split_before_pretokenization() {
        let tokenizer = tiny_tokenizer(
            &[("h", 0), ("i", 1), ("hi", 2), ("<", 3)],
            &[("h", "i")],
            &[("<|x|>", 9), ("<|xx|>", 10)],
        );
        assert_eq!(tokenizer.encode("hi<|x|>hi"), vec![2, 9, 2]);
        assert_eq!(tokenizer.encode("<|xx|>"), vec![10]);
        assert_eq!(tokenizer.token_id("<|x|>"), Some(9));
    }

    #[test]
    fn t2i_render_shape() {
        let rendered = render_flux2_t2i_prompt("sys.", "a cat");
        assert_eq!(
            rendered,
            "<s>[SYSTEM_PROMPT]sys.[/SYSTEM_PROMPT][INST]a cat[/INST]"
        );
    }

    /// Locate the downloaded FLUX.2 tokenizer dir; None (skip) when absent so
    /// the suite stays green on machines without the model download.
    /// Override with FLUX2_TOK_DIR.
    fn local_tokenizer_dir() -> Option<PathBuf> {
        if let Ok(dir) = std::env::var("FLUX2_TOK_DIR") {
            return Some(PathBuf::from(dir));
        }
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../local/models/flux2/tokenizer");
        dir.join("tokenizer.json").is_file().then_some(dir)
    }

    /// HF-parity fixtures generated with python `tokenizers` 0.23.1 against
    /// the real FLUX.2 tokenizer.json (`encode(text,
    /// add_special_tokens=False).ids`); see flux2-port.md.
    const PARITY_FIXTURES: &[(&str, &[u32])] = &[
        ("Hello world", &[22177, 4304]),
        (
            "It costs 1234567 dollars and 42 cents",
            &[
                2757, 12889, 1032, 1049, 1050, 1051, 1052, 1053, 1054, 1055, 22446, 1321, 1032,
                1052, 1050, 50487,
            ],
        ),
        (
            "don't stop, it's fine",
            &[21797, 2405, 5304, 1044, 1494, 1681, 7771],
        ),
        (
            "crema,\" it said... (really?)",
            &[1860, 1831, 4225, 1494, 2639, 2880, 1319, 113495, 28197],
        ),
        (
            "東京の夜にネオンが光る",
            &[18629, 2439, 21721, 2650, 25824, 50292, 3322, 36001, 3670],
        ),
        (
            "a dragon breathing fire 🐉🔥 over a castle",
            &[
                1097, 40426, 31311, 7482, 119685, 1144, 1137, 1240, 1159, 1148, 1165, 2136, 1261,
                36144,
            ],
        ),
        (
            "a  \n b\ttabs   three   sp",
            &[1097, 1256, 1010, 1289, 14133, 8217, 1256, 3300, 1256, 1942],
        ),
        (
            "hello [INST] not a chat [/INST] done",
            &[29706, 1032, 3, 1605, 1261, 21666, 1032, 4, 5595],
        ),
        (
            "<s>[SYSTEM_PROMPT]You are a helpful, harmless, and honest \
             assistant.[/SYSTEM_PROMPT][INST]A photo of a cat[/INST]",
            &[
                1, 17, 4568, 1584, 1261, 20351, 1044, 113175, 1044, 1321, 24529, 27089, 1046, 18,
                3, 1065, 16649, 1307, 1261, 7990, 4,
            ],
        ),
        (
            "A photorealistic photo of a red fox standing on a mossy rock in a \
             misty forest at dawn, volumetric light, 85mm lens",
            &[
                1065, 102244, 1279, 5744, 16649, 1307, 1261, 4804, 94137, 15866, 1408, 1261,
                119766, 1121, 9091, 1294, 1261, 11692, 1121, 17144, 1513, 54507, 1044, 100989,
                4391, 1044, 1032, 1056, 1053, 8383, 20993,
            ],
        ),
        (
            "v2.5-turbo x 1024x1024 in 2026!",
            &[
                1118, 1050, 1046, 1053, 2848, 125167, 2460, 1032, 1049, 1048, 1050, 1052, 1120,
                1049, 1048, 1050, 1052, 1294, 1032, 1050, 1048, 1050, 1054, 1033,
            ],
        ),
        (
            "café naïve fiancée São Paulo",
            &[3173, 1102, 1337, 98355, 49935, 38625, 19288, 21746],
        ),
    ];

    #[test]
    fn hf_parity_on_real_tokenizer_when_downloaded() {
        let Some(dir) = local_tokenizer_dir() else {
            eprintln!("flux2 tokenizer parity: local tokenizer dir missing, skipping");
            return;
        };
        let tokenizer = Flux2Tokenizer::load(&dir).expect("load flux2 tokenizer");
        for (text, expected) in PARITY_FIXTURES {
            let ids = tokenizer.encode(text);
            assert_eq!(&ids, expected, "parity mismatch for {:?}", text);
        }
    }

    #[test]
    fn t2i_window_on_real_tokenizer_when_downloaded() {
        let Some(dir) = local_tokenizer_dir() else {
            eprintln!("flux2 tokenizer t2i: local tokenizer dir missing, skipping");
            return;
        };
        let tokenizer = Flux2Tokenizer::load(&dir).expect("load flux2 tokenizer");
        // Oracle-prompt window; expected ids come from the frozen reference
        // path on .169 (diffusers 0.39.0 format_input + SYSTEM_MESSAGE +
        // PixtralProcessor.apply_chat_template, padding="max_length"), where
        // processor and inner-tokenizer paths were verified identical.
        let window = tokenizer.encode_t2i(
            crate::flux2::FLUX2_SYSTEM_MESSAGE,
            "A photorealistic photo of a red fox standing on a mossy rock in a \
             misty forest at dawn, volumetric light, 85mm lens",
        );
        assert_eq!(window.token_ids.len(), FLUX2_MAX_SEQUENCE_LENGTH);
        let expected_prefix: &[u32] = &[
            1, 17, 4568, 1584, 1420, 26554, 1455, 12738, 2314, 3937, 38340, 1046, 3213, 5628,
            37253, 11688, 30557, 1408, 3481, 14608, 1044, 3481, 1010, 2452, 5604, 1321, 10636,
            3816, 67813, 1046, 18, 3, 1065, 102244, 1279, 5744, 16649, 1307, 1261, 4804, 94137,
            15866, 1408, 1261, 119766, 1121, 9091, 1294, 1261, 11692, 1121, 17144, 1513, 54507,
            1044, 100989, 4391, 1044, 1032, 1056, 1053, 8383, 20993, 4,
        ];
        assert_eq!(window.real_len, expected_prefix.len());
        assert_eq!(&window.token_ids[..window.real_len], expected_prefix);
        assert!(window.token_ids[window.real_len..]
            .iter()
            .all(|&id| id == FLUX2_TOKEN_PAD));
    }
}
