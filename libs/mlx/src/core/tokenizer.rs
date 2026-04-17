use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MlxBpePreTokenizer {
    Gpt2,
    Qwen35,
}

#[derive(Clone, Debug)]
enum MlxTokenizerKind {
    SentencePiece {
        normalized_space: String,
    },
    Gpt2Bpe {
        pre_tokenizer: MlxBpePreTokenizer,
        byte_decoder: HashMap<char, u8>,
    },
}

#[derive(Clone, Debug)]
pub struct MlxTokenizer {
    kind: MlxTokenizerKind,
    vocab: HashMap<String, u32>,
    tokens_by_id: Vec<String>,
    merge_ranks: HashMap<(String, String), usize>,
    special_tokens: Vec<(String, u32)>,
    special_token_ids: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MlxGreedyToken {
    pub token_id: u32,
    pub logit: f32,
}

pub struct MlxStreamingDetokenizer<'a> {
    tokenizer: &'a MlxTokenizer,
    trim_space: bool,
    text_started: bool,
    unflushed: String,
    byte_buffer: Vec<u8>,
}

impl<'a> MlxStreamingDetokenizer<'a> {
    fn new(tokenizer: &'a MlxTokenizer, trim_space: bool) -> Self {
        Self {
            tokenizer,
            trim_space,
            text_started: false,
            unflushed: String::new(),
            byte_buffer: Vec::new(),
        }
    }

    pub fn add_token(&mut self, token_id: u32, skip_special_token_ids: &[u32]) -> String {
        if skip_special_token_ids.contains(&token_id) {
            return String::new();
        }

        let Some(token) = self.tokenizer.id_to_token(token_id) else {
            self.flush_bytes_into_unflushed_lossy();
            let mut delta = self.take_unflushed();
            delta.push_str(&self.render_text(&format!("<unused{token_id}>")));
            return delta;
        };

        match &self.tokenizer.kind {
            MlxTokenizerKind::SentencePiece { normalized_space } => {
                if let Some(byte) = parse_byte_fallback_token(token) {
                    self.byte_buffer.push(byte);
                    return String::new();
                }

                self.flush_bytes_into_unflushed_lossy();
                if token.starts_with(normalized_space) {
                    let delta = self.take_unflushed();
                    self.unflushed.clear();
                    self.unflushed.push_str(token);
                    return delta;
                }

                self.unflushed.push_str(token);
                String::new()
            }
            MlxTokenizerKind::Gpt2Bpe { byte_decoder, .. } => {
                push_gpt2_piece_bytes(token, byte_decoder, &mut self.byte_buffer);
                self.unflushed
                    .push_str(&drain_utf8_prefix(&mut self.byte_buffer));
                self.take_unflushed()
            }
        }
    }

    pub fn finalize(&mut self) -> String {
        self.flush_bytes_into_unflushed_lossy();
        self.take_unflushed()
    }

    fn flush_bytes_into_unflushed_lossy(&mut self) {
        if self.byte_buffer.is_empty() {
            return;
        }
        self.unflushed
            .push_str(&String::from_utf8_lossy(&self.byte_buffer));
        self.byte_buffer.clear();
    }

    fn take_unflushed(&mut self) -> String {
        if self.unflushed.is_empty() {
            return String::new();
        }
        let unflushed = std::mem::take(&mut self.unflushed);
        self.render_text(&unflushed)
    }

    fn render_text(&mut self, text: &str) -> String {
        let replaced = match &self.tokenizer.kind {
            MlxTokenizerKind::SentencePiece { normalized_space } => {
                text.replace(normalized_space, " ")
            }
            MlxTokenizerKind::Gpt2Bpe { .. } => text.to_owned(),
        };
        if self.text_started || !self.trim_space {
            if !replaced.is_empty() {
                self.text_started = true;
            }
            return replaced;
        }
        let trimmed = replaced.trim_start_matches(' ').to_owned();
        if !trimmed.is_empty() {
            self.text_started = true;
        }
        trimmed
    }
}

impl MlxTokenizer {
    pub fn load(root_dir: impl AsRef<Path>) -> Result<Self> {
        let manifest = MlxModelManifest::load(root_dir)?;
        Self::from_paths_and_config(&manifest.paths, &manifest.tokenizer_config)
    }

    pub fn from_snapshot(snapshot: &MlxModelSnapshot) -> Result<Self> {
        Self::from_paths_and_config(&snapshot.paths, &snapshot.tokenizer_config)
    }

    fn from_paths_and_config(
        paths: &MlxModelPaths,
        tokenizer_config: &MlxTokenizerConfig,
    ) -> Result<Self> {
        let text = fs::read_to_string(&paths.tokenizer_json).map_err(|err| MlxRtError::Io {
            path: paths.tokenizer_json.clone(),
            message: err.to_string(),
        })?;
        let root = HashMap::<String, JsonValue>::deserialize_json(&text).map_err(|err| {
            MlxRtError::Json {
                path: paths.tokenizer_json.clone(),
                message: format!("{:?}", err),
            }
        })?;

        let model = tokenizer_object(&paths.tokenizer_json, "tokenizer.model", root.get("model"))?;
        let model_type = tokenizer_string(
            &paths.tokenizer_json,
            "tokenizer.model.type",
            model.get("type"),
        )?;
        if model_type != "BPE" {
            return Err(MlxRtError::InvalidModelDir {
                path: paths.tokenizer_json.clone(),
                message: format!("unsupported tokenizer model {}", model_type),
            });
        }

        let kind = detect_tokenizer_kind(paths, &root, model, tokenizer_config)?;
        let vocab_object = tokenizer_object(
            &paths.tokenizer_json,
            "tokenizer.model.vocab",
            model.get("vocab"),
        )?;
        let mut vocab = HashMap::with_capacity(vocab_object.len());
        let mut max_token_id = 0u32;
        for (token, value) in vocab_object {
            let token_id = tokenizer_u32(
                &paths.tokenizer_json,
                &format!("tokenizer.model.vocab.{token}"),
                Some(value),
            )?;
            max_token_id = max_token_id.max(token_id);
            vocab.insert(token.clone(), token_id);
        }
        let mut tokens_by_id = vec![String::new(); max_token_id as usize + 1];
        for (token, &token_id) in &vocab {
            tokens_by_id[token_id as usize] = token.clone();
        }

        let merges = tokenizer_array(
            &paths.tokenizer_json,
            "tokenizer.model.merges",
            model.get("merges"),
        )?;
        let mut merge_ranks = HashMap::with_capacity(merges.len());
        for (rank, merge_value) in merges.iter().enumerate() {
            let merge_pair = tokenizer_merge_pair(
                &paths.tokenizer_json,
                &format!("tokenizer.model.merges[{rank}]"),
                merge_value,
            )?;
            merge_ranks.insert(merge_pair, rank);
        }

        let added_tokens = tokenizer_array(
            &paths.tokenizer_json,
            "tokenizer.added_tokens",
            root.get("added_tokens"),
        )?;
        let mut special_tokens = Vec::new();
        for (index, value) in added_tokens.iter().enumerate() {
            let token = tokenizer_object(
                &paths.tokenizer_json,
                &format!("tokenizer.added_tokens[{index}]"),
                Some(value),
            )?;
            let special = tokenizer_bool(
                &paths.tokenizer_json,
                &format!("tokenizer.added_tokens[{index}].special"),
                token.get("special"),
            )?;
            let content = tokenizer_string(
                &paths.tokenizer_json,
                &format!("tokenizer.added_tokens[{index}].content"),
                token.get("content"),
            )?;
            let token_id = tokenizer_u32(
                &paths.tokenizer_json,
                &format!("tokenizer.added_tokens[{index}].id"),
                token.get("id"),
            )?;
            if token_id as usize >= tokens_by_id.len() {
                tokens_by_id.resize(token_id as usize + 1, String::new());
            }
            let slot = &mut tokens_by_id[token_id as usize];
            if !slot.is_empty() && slot != &content {
                return Err(MlxRtError::InvalidModelDir {
                    path: paths.tokenizer_json.clone(),
                    message: format!(
                        "token id {} maps to conflicting strings {:?} vs {:?}",
                        token_id, slot, content
                    ),
                });
            }
            *slot = content.clone();
            vocab.entry(content.clone()).or_insert(token_id);
            if !special {
                continue;
            }
            special_tokens.push((content, token_id));
        }
        special_tokens.sort_by(|lhs, rhs| {
            rhs.0
                .len()
                .cmp(&lhs.0.len())
                .then_with(|| lhs.0.cmp(&rhs.0))
        });
        let mut special_token_ids = special_tokens
            .iter()
            .map(|(_, token_id)| *token_id)
            .collect::<Vec<_>>();
        special_token_ids.sort_unstable();
        special_token_ids.dedup();

        Ok(Self {
            kind,
            vocab,
            tokens_by_id,
            merge_ranks,
            special_tokens,
            special_token_ids,
        })
    }

    pub fn vocab_size(&self) -> usize {
        self.vocab.len()
    }

    pub fn merge_count(&self) -> usize {
        self.merge_ranks.len()
    }

    pub fn token_to_id(&self, token: &str) -> Option<u32> {
        self.vocab.get(token).copied()
    }

    pub fn id_to_token(&self, token_id: u32) -> Option<&str> {
        self.tokens_by_id.get(token_id as usize).and_then(|token| {
            if token.is_empty() {
                None
            } else {
                Some(token.as_str())
            }
        })
    }

    pub fn encode(&self, text: &str) -> Result<Vec<u32>> {
        let mut out = Vec::new();
        let mut plain = String::new();
        let mut byte_index = 0usize;
        while byte_index < text.len() {
            let mut matched_special = None;
            for (special, token_id) in &self.special_tokens {
                if text[byte_index..].starts_with(special) {
                    matched_special = Some((special.len(), *token_id));
                    break;
                }
            }
            if let Some((special_len, token_id)) = matched_special {
                if !plain.is_empty() {
                    out.extend(self.encode_plain_text(&plain)?);
                    plain.clear();
                }
                out.push(token_id);
                byte_index += special_len;
                continue;
            }
            let next =
                text[byte_index..]
                    .chars()
                    .next()
                    .ok_or_else(|| MlxRtError::InvalidModelDir {
                        path: PathBuf::new(),
                        message: "invalid tokenizer input slice".to_string(),
                    })?;
            plain.push(next);
            byte_index += next.len_utf8();
        }
        if !plain.is_empty() {
            out.extend(self.encode_plain_text(&plain)?);
        }
        Ok(out)
    }

    pub fn decode(&self, token_ids: &[u32]) -> Result<String> {
        match &self.kind {
            MlxTokenizerKind::SentencePiece { normalized_space } => {
                let mut out = String::new();
                let mut pending_bytes = Vec::new();
                for &token_id in token_ids {
                    let token = if let Some(token) = self.id_to_token(token_id) {
                        token
                    } else {
                        flush_pending_bytes(&mut out, &mut pending_bytes);
                        out.push_str(&format!("<unused{token_id}>"));
                        continue;
                    };
                    if let Some(byte) = parse_byte_fallback_token(token) {
                        pending_bytes.push(byte);
                        continue;
                    }
                    flush_pending_bytes(&mut out, &mut pending_bytes);
                    out.push_str(&token.replace(normalized_space, " "));
                }
                flush_pending_bytes(&mut out, &mut pending_bytes);
                Ok(out)
            }
            MlxTokenizerKind::Gpt2Bpe { byte_decoder, .. } => {
                let mut out = String::new();
                let mut pending_bytes = Vec::new();
                for &token_id in token_ids {
                    let token = if let Some(token) = self.id_to_token(token_id) {
                        token
                    } else {
                        out.push_str(&drain_utf8_prefix(&mut pending_bytes));
                        if !pending_bytes.is_empty() {
                            out.push_str(&String::from_utf8_lossy(&pending_bytes));
                            pending_bytes.clear();
                        }
                        out.push_str(&format!("<unused{token_id}>"));
                        continue;
                    };
                    push_gpt2_piece_bytes(token, byte_decoder, &mut pending_bytes);
                    out.push_str(&drain_utf8_prefix(&mut pending_bytes));
                }
                if !pending_bytes.is_empty() {
                    out.push_str(&String::from_utf8_lossy(&pending_bytes));
                }
                Ok(out)
            }
        }
    }

    pub fn special_token_ids(&self) -> &[u32] {
        &self.special_token_ids
    }

    pub fn streaming_detokenizer(&self, trim_space: bool) -> MlxStreamingDetokenizer<'_> {
        MlxStreamingDetokenizer::new(self, trim_space)
    }

    fn encode_plain_text(&self, text: &str) -> Result<Vec<u32>> {
        if text.is_empty() {
            return Ok(Vec::new());
        }
        match &self.kind {
            MlxTokenizerKind::SentencePiece { normalized_space } => {
                let normalized = text.replace(' ', normalized_space);
                let mut pieces = normalized
                    .chars()
                    .map(|ch| ch.to_string())
                    .collect::<Vec<_>>();
                merge_bpe_pieces(&mut pieces, &self.merge_ranks);

                let mut token_ids = Vec::new();
                for piece in pieces {
                    if let Some(&token_id) = self.vocab.get(&piece) {
                        token_ids.push(token_id);
                        continue;
                    }
                    for byte in piece.into_bytes() {
                        let byte_piece = format!("<0x{byte:02X}>");
                        let token_id = self.vocab.get(&byte_piece).copied().ok_or_else(|| {
                            MlxRtError::InvalidModelDir {
                                path: PathBuf::new(),
                                message: format!("missing byte fallback token {}", byte_piece),
                            }
                        })?;
                        token_ids.push(token_id);
                    }
                }
                Ok(token_ids)
            }
            MlxTokenizerKind::Gpt2Bpe { pre_tokenizer, .. } => {
                let mut token_ids = Vec::new();
                for word in split_bpe_words(text, *pre_tokenizer) {
                    encode_gpt2_bpe_word(&self.vocab, &self.merge_ranks, &word, &mut token_ids)?;
                }
                Ok(token_ids)
            }
        }
    }
}

fn detect_tokenizer_kind(
    paths: &MlxModelPaths,
    root: &HashMap<String, JsonValue>,
    model: &HashMap<String, JsonValue>,
    tokenizer_config: &MlxTokenizerConfig,
) -> Result<MlxTokenizerKind> {
    let byte_fallback = model
        .get("byte_fallback")
        .map(|value| {
            tokenizer_bool(
                &paths.tokenizer_json,
                "tokenizer.model.byte_fallback",
                Some(value),
            )
        })
        .transpose()?
        .unwrap_or(false);

    if let Some(normalizer) = root.get("normalizer").and_then(JsonValue::object) {
        let normalizer_type = tokenizer_string(
            &paths.tokenizer_json,
            "tokenizer.normalizer.type",
            normalizer.get("type"),
        )?;
        let pre_tokenizer = tokenizer_object(
            &paths.tokenizer_json,
            "tokenizer.pre_tokenizer",
            root.get("pre_tokenizer"),
        )?;
        let pre_tokenizer_type = tokenizer_string(
            &paths.tokenizer_json,
            "tokenizer.pre_tokenizer.type",
            pre_tokenizer.get("type"),
        )?;
        if normalizer_type == "Replace" && pre_tokenizer_type == "Split" && byte_fallback {
            let normalized_space = tokenizer_pattern_string(
                &paths.tokenizer_json,
                "tokenizer.normalizer.pattern",
                normalizer.get("pattern"),
            )?;
            let normalizer_content = tokenizer_string(
                &paths.tokenizer_json,
                "tokenizer.normalizer.content",
                normalizer.get("content"),
            )?;
            let pre_tokenizer_pattern = tokenizer_pattern_string(
                &paths.tokenizer_json,
                "tokenizer.pre_tokenizer.pattern",
                pre_tokenizer.get("pattern"),
            )?;
            let pre_tokenizer_behavior = tokenizer_string(
                &paths.tokenizer_json,
                "tokenizer.pre_tokenizer.behavior",
                pre_tokenizer.get("behavior"),
            )?;
            if normalized_space == " "
                && normalizer_content == "▁"
                && pre_tokenizer_pattern == " "
                && pre_tokenizer_behavior == "MergedWithPrevious"
            {
                return Ok(MlxTokenizerKind::SentencePiece {
                    normalized_space: normalizer_content,
                });
            }
        }
    }

    let decoder = tokenizer_object(
        &paths.tokenizer_json,
        "tokenizer.decoder",
        root.get("decoder"),
    )?;
    let decoder_type = tokenizer_string(
        &paths.tokenizer_json,
        "tokenizer.decoder.type",
        decoder.get("type"),
    )?;
    let pre_tokenizer = tokenizer_object(
        &paths.tokenizer_json,
        "tokenizer.pre_tokenizer",
        root.get("pre_tokenizer"),
    )?;
    let pre_tokenizer_type = tokenizer_string(
        &paths.tokenizer_json,
        "tokenizer.pre_tokenizer.type",
        pre_tokenizer.get("type"),
    )?;
    if decoder_type == "ByteLevel" && pre_tokenizer_type == "Sequence" && !byte_fallback {
        let pre_tokenizer = if tokenizer_config
            .tokenizer_class
            .to_ascii_lowercase()
            .contains("qwen")
        {
            MlxBpePreTokenizer::Qwen35
        } else {
            MlxBpePreTokenizer::Gpt2
        };
        return Ok(MlxTokenizerKind::Gpt2Bpe {
            pre_tokenizer,
            byte_decoder: gpt2_byte_decoder(),
        });
    }

    Err(MlxRtError::InvalidModelDir {
        path: paths.tokenizer_json.clone(),
        message: format!(
            "unsupported tokenizer layout: normalizer={:?} pre_tokenizer={} decoder={}",
            root.get("normalizer"),
            pre_tokenizer_type,
            decoder_type
        ),
    })
}

fn tokenizer_merge_pair(path: &Path, context: &str, value: &JsonValue) -> Result<(String, String)> {
    match value {
        JsonValue::String(text) => {
            let Some(split_at) = text.find(' ').filter(|split_at| *split_at > 0) else {
                return Err(MlxRtError::Json {
                    path: path.to_path_buf(),
                    message: format!("{} expected merge pair, got {:?}", context, text),
                });
            };
            Ok((text[..split_at].to_owned(), text[split_at + 1..].to_owned()))
        }
        JsonValue::Array(_) => tokenizer_string_pair(path, context, value),
        other => Err(MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{} expected merge pair, got {:?}", context, other),
        }),
    }
}

fn merge_bpe_pieces(pieces: &mut Vec<String>, merge_ranks: &HashMap<(String, String), usize>) {
    while pieces.len() >= 2 {
        let mut best_index = None;
        let mut best_rank = usize::MAX;
        for pair_index in 0..pieces.len() - 1 {
            let merge_key = (pieces[pair_index].clone(), pieces[pair_index + 1].clone());
            if let Some(&rank) = merge_ranks.get(&merge_key) {
                if rank < best_rank {
                    best_rank = rank;
                    best_index = Some(pair_index);
                }
            }
        }
        let Some(pair_index) = best_index else {
            break;
        };
        let merged = format!("{}{}", pieces[pair_index], pieces[pair_index + 1]);
        pieces.splice(pair_index..pair_index + 2, [merged]);
    }
}

fn encode_gpt2_bpe_word(
    vocab: &HashMap<String, u32>,
    merge_ranks: &HashMap<(String, String), usize>,
    word: &str,
    out: &mut Vec<u32>,
) -> Result<()> {
    let encoded = encode_bpe_word_bytes(word);
    let mut pieces = encoded.chars().map(|ch| ch.to_string()).collect::<Vec<_>>();
    merge_bpe_pieces(&mut pieces, merge_ranks);
    for piece in pieces {
        let token_id = vocab
            .get(&piece)
            .copied()
            .ok_or_else(|| MlxRtError::InvalidModelDir {
                path: PathBuf::new(),
                message: format!("missing tokenizer piece {:?}", piece),
            })?;
        out.push(token_id);
    }
    Ok(())
}

fn push_gpt2_piece_bytes(
    piece: &str,
    byte_decoder: &HashMap<char, u8>,
    pending_bytes: &mut Vec<u8>,
) {
    for ch in piece.chars() {
        if let Some(byte) = byte_decoder.get(&ch) {
            pending_bytes.push(*byte);
        } else {
            let mut utf8 = [0u8; 4];
            pending_bytes.extend_from_slice(ch.encode_utf8(&mut utf8).as_bytes());
        }
    }
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
                    Some(len) => {
                        out.push_str(&String::from_utf8_lossy(&pending_bytes[..len]));
                        pending_bytes.drain(..len);
                    }
                }
            }
        }
    }
}

fn split_bpe_words(text: &str, pre: MlxBpePreTokenizer) -> Vec<String> {
    match pre {
        MlxBpePreTokenizer::Gpt2 => split_gpt2_words(text),
        MlxBpePreTokenizer::Qwen35 => split_qwen_words(text, true),
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct CharFlags {
    is_present: bool,
    is_whitespace: bool,
    is_letter: bool,
    is_number: bool,
    is_mark: bool,
}

#[derive(Clone, Copy, Debug)]
struct TextChar {
    ch: char,
    start: usize,
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
        encoded.push_str(gpt2_byte_encoder()[usize::from(byte)].as_str());
    }
    encoded
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

const EMBED_TOKENS_WEIGHT_NAME: &str = "language_model.model.embed_tokens.weight";
const EMBED_TOKENS_SCALES_NAME: &str = "language_model.model.embed_tokens.scales";
const EMBED_TOKENS_BIASES_NAME: &str = "language_model.model.embed_tokens.biases";
const FINAL_TEXT_NORM_WEIGHT_NAME: &str = "language_model.model.norm.weight";
