#[derive(Clone, Debug)]
pub struct MlxTokenizer {
    normalized_space: String,
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
            self.flush_bytes_into_unflushed();
            let mut delta = self.take_unflushed();
            delta.push_str(&self.render_text(&format!("<unused{token_id}>")));
            return delta;
        };

        if let Some(byte) = parse_byte_fallback_token(token) {
            self.byte_buffer.push(byte);
            return String::new();
        }

        self.flush_bytes_into_unflushed();
        if token.starts_with(&self.tokenizer.normalized_space) {
            let delta = self.take_unflushed();
            self.unflushed.clear();
            self.unflushed.push_str(token);
            return delta;
        }

        self.unflushed.push_str(token);
        String::new()
    }

    pub fn finalize(&mut self) -> String {
        self.flush_bytes_into_unflushed();
        self.take_unflushed()
    }

    fn flush_bytes_into_unflushed(&mut self) {
        if self.byte_buffer.is_empty() {
            return;
        }
        let decoded = String::from_utf8_lossy(&self.byte_buffer);
        self.unflushed.push_str(&decoded);
        self.byte_buffer.clear();
    }

    fn take_unflushed(&mut self) -> String {
        if self.unflushed.is_empty() {
            return String::new();
        }
        let unflushed = std::mem::take(&mut self.unflushed);
        let rendered = self.render_text(&unflushed);
        self.unflushed.clear();
        rendered
    }

    fn render_text(&mut self, text: &str) -> String {
        let replaced = text.replace(&self.tokenizer.normalized_space, " ");
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
        let snapshot = MlxModelSnapshot::load(root_dir)?;
        Self::from_snapshot(&snapshot)
    }

    pub fn from_snapshot(snapshot: &MlxModelSnapshot) -> Result<Self> {
        let text =
            fs::read_to_string(&snapshot.paths.tokenizer_json).map_err(|err| MlxRtError::Io {
                path: snapshot.paths.tokenizer_json.clone(),
                message: err.to_string(),
            })?;
        let root = HashMap::<String, JsonValue>::deserialize_json(&text).map_err(|err| {
            MlxRtError::Json {
                path: snapshot.paths.tokenizer_json.clone(),
                message: format!("{:?}", err),
            }
        })?;

        let normalizer = tokenizer_object(
            &snapshot.paths.tokenizer_json,
            "tokenizer.normalizer",
            root.get("normalizer"),
        )?;
        let normalizer_type = tokenizer_string(
            &snapshot.paths.tokenizer_json,
            "tokenizer.normalizer.type",
            normalizer.get("type"),
        )?;
        if normalizer_type != "Replace" {
            return Err(MlxRtError::InvalidModelDir {
                path: snapshot.paths.tokenizer_json.clone(),
                message: format!("unsupported tokenizer normalizer {}", normalizer_type),
            });
        }
        let normalized_space = tokenizer_pattern_string(
            &snapshot.paths.tokenizer_json,
            "tokenizer.normalizer.pattern",
            normalizer.get("pattern"),
        )?;
        let normalizer_content = tokenizer_string(
            &snapshot.paths.tokenizer_json,
            "tokenizer.normalizer.content",
            normalizer.get("content"),
        )?;
        if normalized_space != " " || normalizer_content != "▁" {
            return Err(MlxRtError::InvalidModelDir {
                path: snapshot.paths.tokenizer_json.clone(),
                message: format!(
                    "unsupported tokenizer normalizer pattern/content {:?} -> {:?}",
                    normalized_space, normalizer_content
                ),
            });
        }

        let pre_tokenizer = tokenizer_object(
            &snapshot.paths.tokenizer_json,
            "tokenizer.pre_tokenizer",
            root.get("pre_tokenizer"),
        )?;
        let pre_tokenizer_type = tokenizer_string(
            &snapshot.paths.tokenizer_json,
            "tokenizer.pre_tokenizer.type",
            pre_tokenizer.get("type"),
        )?;
        let pre_tokenizer_pattern = tokenizer_pattern_string(
            &snapshot.paths.tokenizer_json,
            "tokenizer.pre_tokenizer.pattern",
            pre_tokenizer.get("pattern"),
        )?;
        let pre_tokenizer_behavior = tokenizer_string(
            &snapshot.paths.tokenizer_json,
            "tokenizer.pre_tokenizer.behavior",
            pre_tokenizer.get("behavior"),
        )?;
        if pre_tokenizer_type != "Split"
            || pre_tokenizer_pattern != " "
            || pre_tokenizer_behavior != "MergedWithPrevious"
        {
            return Err(MlxRtError::InvalidModelDir {
                path: snapshot.paths.tokenizer_json.clone(),
                message: format!(
                    "unsupported tokenizer pre_tokenizer {} / {:?} / {}",
                    pre_tokenizer_type, pre_tokenizer_pattern, pre_tokenizer_behavior
                ),
            });
        }

        let model = tokenizer_object(
            &snapshot.paths.tokenizer_json,
            "tokenizer.model",
            root.get("model"),
        )?;
        let model_type = tokenizer_string(
            &snapshot.paths.tokenizer_json,
            "tokenizer.model.type",
            model.get("type"),
        )?;
        if model_type != "BPE" {
            return Err(MlxRtError::InvalidModelDir {
                path: snapshot.paths.tokenizer_json.clone(),
                message: format!("unsupported tokenizer model {}", model_type),
            });
        }
        if !tokenizer_bool(
            &snapshot.paths.tokenizer_json,
            "tokenizer.model.byte_fallback",
            model.get("byte_fallback"),
        )? {
            return Err(MlxRtError::InvalidModelDir {
                path: snapshot.paths.tokenizer_json.clone(),
                message: "tokenizer must enable byte_fallback".to_string(),
            });
        }

        let vocab_object = tokenizer_object(
            &snapshot.paths.tokenizer_json,
            "tokenizer.model.vocab",
            model.get("vocab"),
        )?;
        let mut vocab = HashMap::with_capacity(vocab_object.len());
        let mut max_token_id = 0u32;
        for (token, value) in vocab_object {
            let token_id = tokenizer_u32(
                &snapshot.paths.tokenizer_json,
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
            &snapshot.paths.tokenizer_json,
            "tokenizer.model.merges",
            model.get("merges"),
        )?;
        let mut merge_ranks = HashMap::with_capacity(merges.len());
        for (rank, merge_value) in merges.iter().enumerate() {
            let merge_pair = tokenizer_string_pair(
                &snapshot.paths.tokenizer_json,
                &format!("tokenizer.model.merges[{rank}]"),
                merge_value,
            )?;
            merge_ranks.insert(merge_pair, rank);
        }

        let added_tokens = tokenizer_array(
            &snapshot.paths.tokenizer_json,
            "tokenizer.added_tokens",
            root.get("added_tokens"),
        )?;
        let mut special_tokens = Vec::new();
        for (index, value) in added_tokens.iter().enumerate() {
            let token = tokenizer_object(
                &snapshot.paths.tokenizer_json,
                &format!("tokenizer.added_tokens[{index}]"),
                Some(value),
            )?;
            let special = tokenizer_bool(
                &snapshot.paths.tokenizer_json,
                &format!("tokenizer.added_tokens[{index}].special"),
                token.get("special"),
            )?;
            if !special {
                continue;
            }
            let content = tokenizer_string(
                &snapshot.paths.tokenizer_json,
                &format!("tokenizer.added_tokens[{index}].content"),
                token.get("content"),
            )?;
            let token_id = tokenizer_u32(
                &snapshot.paths.tokenizer_json,
                &format!("tokenizer.added_tokens[{index}].id"),
                token.get("id"),
            )?;
            special_tokens.push((content, token_id));
        }
        special_tokens.sort_by(|lhs, rhs| {
            rhs.0
                .len()
                .cmp(&lhs.0.len())
                .then_with(|| lhs.0.cmp(&rhs.0))
        });
        let mut special_token_ids = special_tokens.iter().map(|(_, token_id)| *token_id).collect::<Vec<_>>();
        special_token_ids.sort_unstable();
        special_token_ids.dedup();

        Ok(Self {
            normalized_space: normalizer_content,
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
            out.push_str(&token.replace(&self.normalized_space, " "));
        }
        flush_pending_bytes(&mut out, &mut pending_bytes);
        Ok(out)
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
        let normalized = text.replace(' ', &self.normalized_space);
        let mut pieces = normalized
            .chars()
            .map(|ch| ch.to_string())
            .collect::<Vec<_>>();
        while pieces.len() >= 2 {
            let mut best_index = None;
            let mut best_rank = usize::MAX;
            for pair_index in 0..pieces.len() - 1 {
                let merge_key = (pieces[pair_index].clone(), pieces[pair_index + 1].clone());
                if let Some(&rank) = self.merge_ranks.get(&merge_key) {
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
}

const EMBED_TOKENS_WEIGHT_NAME: &str = "language_model.model.embed_tokens.weight";
const EMBED_TOKENS_SCALES_NAME: &str = "language_model.model.embed_tokens.scales";
const EMBED_TOKENS_BIASES_NAME: &str = "language_model.model.embed_tokens.biases";
const FINAL_TEXT_NORM_WEIGHT_NAME: &str = "language_model.model.norm.weight";
