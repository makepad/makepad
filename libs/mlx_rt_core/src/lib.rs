use makepad_micro_serde::{DeJson, DeJsonErr, DeJsonState, JsonValue};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub type Result<T> = std::result::Result<T, MlxRtError>;

#[derive(Clone, Debug)]
pub struct MlxRouterTopKOutput {
    pub router_scaled: Vec<f32>,
    pub expert_scores: Vec<f32>,
    pub router_probs: Vec<f32>,
    pub top_k_indices: Vec<u32>,
    pub top_k_weights: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct MlxGemmaMoeExpertOutput {
    pub pre_feedforward_norm2: Vec<f32>,
    pub gate_proj: Vec<f32>,
    pub up_proj: Vec<f32>,
    pub geglu: Vec<f32>,
    pub down_proj: Vec<f32>,
    pub expert_out: Vec<f32>,
}

#[derive(Debug)]
pub enum MlxRtError {
    Io { path: PathBuf, message: String },
    Json { path: PathBuf, message: String },
    MissingFile { path: PathBuf },
    InvalidModelDir { path: PathBuf, message: String },
    InvalidSafetensors { path: PathBuf, message: String },
}

impl std::fmt::Display for MlxRtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, message } => {
                write!(f, "I/O error at {}: {}", path.display(), message)
            }
            Self::Json { path, message } => {
                write!(f, "JSON decode error at {}: {}", path.display(), message)
            }
            Self::MissingFile { path } => write!(f, "missing required file {}", path.display()),
            Self::InvalidModelDir { path, message } => {
                write!(f, "invalid model dir {}: {}", path.display(), message)
            }
            Self::InvalidSafetensors { path, message } => {
                write!(
                    f,
                    "invalid safetensors file {}: {}",
                    path.display(),
                    message
                )
            }
        }
    }
}

impl std::error::Error for MlxRtError {}

#[derive(Clone, Debug)]
pub struct MlxModelPaths {
    pub root_dir: PathBuf,
    pub config_json: PathBuf,
    pub generation_config_json: PathBuf,
    pub processor_config_json: PathBuf,
    pub tokenizer_json: PathBuf,
    pub tokenizer_config_json: PathBuf,
    pub model_safetensors_index_json: PathBuf,
}

impl MlxModelPaths {
    pub fn from_dir(root_dir: impl AsRef<Path>) -> Result<Self> {
        let root_dir = root_dir.as_ref().to_path_buf();
        if !root_dir.is_dir() {
            return Err(MlxRtError::InvalidModelDir {
                path: root_dir,
                message: "directory does not exist".to_string(),
            });
        }

        let paths = Self {
            config_json: root_dir.join("config.json"),
            generation_config_json: root_dir.join("generation_config.json"),
            processor_config_json: root_dir.join("processor_config.json"),
            tokenizer_json: root_dir.join("tokenizer.json"),
            tokenizer_config_json: root_dir.join("tokenizer_config.json"),
            model_safetensors_index_json: root_dir.join("model.safetensors.index.json"),
            root_dir,
        };
        paths.verify_required_files()?;
        Ok(paths)
    }

    pub fn verify_required_files(&self) -> Result<()> {
        for path in [
            &self.config_json,
            &self.generation_config_json,
            &self.processor_config_json,
            &self.tokenizer_json,
            &self.tokenizer_config_json,
            &self.model_safetensors_index_json,
        ] {
            if !path.is_file() {
                return Err(MlxRtError::MissingFile { path: path.clone() });
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct MlxModelSnapshot {
    pub paths: MlxModelPaths,
    pub config: MlxModelConfig,
    pub generation_config: MlxGenerationConfig,
    pub processor_config: MlxProcessorConfig,
    pub tokenizer_config: MlxTokenizerConfig,
    pub weight_index: MlxWeightIndex,
}

impl MlxModelSnapshot {
    pub fn load(root_dir: impl AsRef<Path>) -> Result<Self> {
        let paths = MlxModelPaths::from_dir(root_dir)?;
        let config = load_json::<MlxModelConfig>(&paths.config_json)?;
        let generation_config = load_json::<MlxGenerationConfig>(&paths.generation_config_json)?;
        let processor_config = load_json::<MlxProcessorConfig>(&paths.processor_config_json)?;
        let tokenizer_config = load_json::<MlxTokenizerConfig>(&paths.tokenizer_config_json)?;
        let weight_index = load_json::<MlxWeightIndex>(&paths.model_safetensors_index_json)?;
        let snapshot = Self {
            paths,
            config,
            generation_config,
            processor_config,
            tokenizer_config,
            weight_index,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn unique_weight_shards(&self) -> Vec<String> {
        let mut shards = BTreeSet::new();
        for shard in self.weight_index.weight_map.values() {
            shards.insert(shard.clone());
        }
        shards.into_iter().collect()
    }

    pub fn validate(&self) -> Result<()> {
        if self.config.model_type != "gemma4" {
            return Err(MlxRtError::InvalidModelDir {
                path: self.paths.root_dir.clone(),
                message: format!("unexpected model_type {}", self.config.model_type),
            });
        }
        if self.config.quantization.bits != 4
            || self.config.quantization.group_size != 64
            || self.config.quantization.mode != "affine"
        {
            return Err(MlxRtError::InvalidModelDir {
                path: self.paths.root_dir.clone(),
                message: "expected affine 4-bit group_size=64 quantization".to_string(),
            });
        }
        if self.config.text_config.num_hidden_layers != 30 {
            return Err(MlxRtError::InvalidModelDir {
                path: self.paths.root_dir.clone(),
                message: format!(
                    "expected 30 text layers, got {}",
                    self.config.text_config.num_hidden_layers
                ),
            });
        }
        if self.config.vision_config.num_hidden_layers != 27 {
            return Err(MlxRtError::InvalidModelDir {
                path: self.paths.root_dir.clone(),
                message: format!(
                    "expected 27 vision layers, got {}",
                    self.config.vision_config.num_hidden_layers
                ),
            });
        }
        for shard_name in self.unique_weight_shards() {
            let shard_path = self.paths.root_dir.join(&shard_name);
            if !shard_path.is_file() {
                return Err(MlxRtError::MissingFile { path: shard_path });
            }
        }
        for required_weight in [
            "language_model.model.embed_tokens.weight",
            "language_model.model.layers.0.self_attn.q_proj.weight",
            "language_model.model.layers.29.self_attn.o_proj.weight",
            "vision_tower.patch_embedder.input_proj.weight",
            "embed_vision.embedding_projection.weight",
        ] {
            if !self.weight_index.weight_map.contains_key(required_weight) {
                return Err(MlxRtError::InvalidModelDir {
                    path: self.paths.model_safetensors_index_json.clone(),
                    message: format!("missing required tensor key {}", required_weight),
                });
            }
        }
        Ok(())
    }
}

fn load_json<T: DeJson>(path: &Path) -> Result<T> {
    let text = fs::read_to_string(path).map_err(|err| MlxRtError::Io {
        path: path.to_path_buf(),
        message: err.to_string(),
    })?;
    T::deserialize_json(&text).map_err(|err| MlxRtError::Json {
        path: path.to_path_buf(),
        message: format!("{:?}", err),
    })
}

#[derive(Clone, Debug, DeJson)]
pub struct MlxModelConfig {
    pub architectures: Vec<String>,
    pub audio_config: Option<JsonValue>,
    pub audio_token_id: u32,
    pub boa_token_id: u32,
    pub boi_token_id: u32,
    pub dtype: String,
    pub eoa_token_id: u32,
    pub eoa_token_index: u32,
    pub eoi_token_id: u32,
    pub eos_token_id: Vec<u32>,
    pub image_token_id: u32,
    pub initializer_range: f32,
    pub model_type: String,
    pub quantization: MlxQuantizationConfig,
    pub quantization_config: MlxQuantizationConfig,
    pub text_config: MlxTextConfig,
    pub tie_word_embeddings: bool,
    pub transformers_version: String,
    pub video_token_id: u32,
    pub vision_config: MlxVisionConfig,
    pub vision_soft_tokens_per_image: u32,
}

#[derive(Clone, Debug, DeJson)]
pub struct MlxQuantizationConfig {
    pub group_size: u32,
    pub bits: u32,
    pub mode: String,
}

#[derive(Clone, Debug, DeJson)]
pub struct MlxTextConfig {
    pub attention_bias: bool,
    pub attention_dropout: f32,
    pub attention_k_eq_v: bool,
    pub bos_token_id: u32,
    pub dtype: String,
    pub enable_moe_block: bool,
    pub eos_token_id: u32,
    pub final_logit_softcapping: f32,
    pub global_head_dim: u32,
    pub head_dim: u32,
    pub hidden_activation: String,
    pub hidden_size: u32,
    pub hidden_size_per_layer_input: u32,
    pub initializer_range: f32,
    pub intermediate_size: u32,
    pub layer_types: Vec<String>,
    pub max_position_embeddings: u32,
    pub model_type: String,
    pub moe_intermediate_size: u32,
    pub num_attention_heads: u32,
    pub num_experts: u32,
    pub num_global_key_value_heads: u32,
    pub num_hidden_layers: u32,
    pub num_key_value_heads: u32,
    pub num_kv_shared_layers: u32,
    pub pad_token_id: u32,
    pub rms_norm_eps: f32,
    pub rope_parameters: MlxTextRopeParameters,
    pub sliding_window: u32,
    pub tie_word_embeddings: bool,
    pub top_k_experts: u32,
    pub use_bidirectional_attention: String,
    pub use_cache: bool,
    pub use_double_wide_mlp: bool,
    pub vocab_size: u32,
    pub vocab_size_per_layer_input: u32,
}

#[derive(Clone, Debug, DeJson)]
pub struct MlxTextRopeParameters {
    pub full_attention: MlxRopeAttentionParameters,
    pub sliding_attention: MlxRopeAttentionParameters,
}

#[derive(Clone, Debug, DeJson)]
pub struct MlxRopeAttentionParameters {
    pub partial_rotary_factor: Option<f32>,
    pub rope_theta: f32,
    pub rope_type: String,
}

#[derive(Clone, Debug, DeJson)]
pub struct MlxVisionConfig {
    #[rename(_name_or_path)]
    pub _name_or_path: String,
    pub architectures: Option<Vec<String>>,
    pub attention_bias: bool,
    pub attention_dropout: f32,
    pub chunk_size_feed_forward: u32,
    pub default_output_length: u32,
    pub dtype: String,
    pub global_head_dim: u32,
    pub head_dim: u32,
    pub hidden_activation: String,
    pub hidden_size: u32,
    pub id2label: HashMap<String, String>,
    pub initializer_range: f32,
    pub intermediate_size: u32,
    pub is_encoder_decoder: bool,
    pub label2id: HashMap<String, u32>,
    pub max_position_embeddings: u32,
    pub model_type: String,
    pub num_attention_heads: u32,
    pub num_hidden_layers: u32,
    pub num_key_value_heads: u32,
    pub output_attentions: bool,
    pub output_hidden_states: bool,
    pub patch_size: u32,
    pub pooling_kernel_size: u32,
    pub position_embedding_size: u32,
    pub problem_type: Option<String>,
    pub return_dict: bool,
    pub rms_norm_eps: f32,
    pub rope_parameters: MlxVisionRopeParameters,
    pub standardize: bool,
    pub use_clipped_linears: bool,
}

#[derive(Clone, Debug, DeJson)]
pub struct MlxVisionRopeParameters {
    pub rope_theta: f32,
    pub rope_type: String,
}

#[derive(Clone, Debug, DeJson)]
pub struct MlxGenerationConfig {
    pub bos_token_id: u32,
    pub do_sample: bool,
    pub eos_token_id: Vec<u32>,
    pub pad_token_id: u32,
    pub temperature: f32,
    pub top_k: u32,
    pub top_p: f32,
    pub transformers_version: String,
}

#[derive(Clone, Debug, DeJson)]
pub struct MlxProcessorConfig {
    pub audio_seq_length: u32,
    pub image_processor: MlxImageProcessorConfig,
    pub image_seq_length: u32,
    pub processor_class: String,
}

#[derive(Clone, Debug, DeJson)]
pub struct MlxImageProcessorConfig {
    pub do_convert_rgb: bool,
    pub do_normalize: bool,
    pub do_rescale: bool,
    pub do_resize: bool,
    pub image_mean: Vec<f32>,
    pub image_processor_type: String,
    pub image_seq_length: u32,
    pub image_std: Vec<f32>,
    pub max_soft_tokens: u32,
    pub patch_size: u32,
    pub pooling_kernel_size: u32,
    pub resample: u32,
    pub rescale_factor: f64,
    pub size: MlxImageProcessorSize,
}

#[derive(Clone, Debug, DeJson)]
pub struct MlxImageProcessorSize {
    pub height: u32,
    pub width: u32,
}

#[derive(Clone, Debug, DeJson)]
pub struct MlxTokenizerConfig {
    pub audio_token: String,
    pub backend: String,
    pub boa_token: String,
    pub boi_token: String,
    pub bos_token: String,
    pub eoa_token: String,
    pub eoc_token: String,
    pub eoi_token: String,
    pub eos_token: String,
    pub eot_token: String,
    pub escape_token: String,
    pub etc_token: String,
    pub etd_token: String,
    pub etr_token: String,
    pub extra_special_tokens: Vec<String>,
    pub image_token: String,
    pub is_local: bool,
    pub mask_token: String,
    pub model_max_length: u128,
    pub model_specific_special_tokens: HashMap<String, String>,
    pub pad_token: String,
    pub padding_side: String,
    pub processor_class: String,
    pub response_schema: JsonValue,
    pub soc_token: String,
    pub sot_token: String,
    pub stc_token: String,
    pub std_token: String,
    pub str_token: String,
    pub think_token: String,
    pub tokenizer_class: String,
    pub unk_token: String,
}

#[derive(Clone, Debug, DeJson)]
pub struct MlxWeightIndex {
    pub metadata: MlxWeightIndexMetadata,
    pub weight_map: HashMap<String, String>,
}

#[derive(Clone, Debug, DeJson)]
pub struct MlxWeightIndexMetadata {
    pub total_size: u64,
}

#[derive(Clone, Debug)]
pub struct MlxIndexedSafetensors {
    pub snapshot: MlxModelSnapshot,
    pub shard_headers: HashMap<String, MlxSafetensorsHeader>,
}

impl MlxIndexedSafetensors {
    pub fn load(root_dir: impl AsRef<Path>) -> Result<Self> {
        let snapshot = MlxModelSnapshot::load(root_dir)?;
        let mut shard_headers = HashMap::new();
        for shard_name in snapshot.unique_weight_shards() {
            let shard_path = snapshot.paths.root_dir.join(&shard_name);
            let header = MlxSafetensorsHeader::load(&shard_path)?;
            shard_headers.insert(shard_name, header);
        }
        Ok(Self {
            snapshot,
            shard_headers,
        })
    }

    pub fn shard_name_for_tensor(&self, name: &str) -> Result<&str> {
        self.snapshot
            .weight_index
            .weight_map
            .get(name)
            .map(String::as_str)
            .ok_or_else(|| MlxRtError::InvalidModelDir {
                path: self.snapshot.paths.model_safetensors_index_json.clone(),
                message: format!("tensor {} missing from weight index", name),
            })
    }

    pub fn header_for_tensor(&self, name: &str) -> Result<&MlxSafetensorsHeader> {
        let shard_name = self.shard_name_for_tensor(name)?;
        self.shard_headers
            .get(shard_name)
            .ok_or_else(|| MlxRtError::MissingFile {
                path: self.snapshot.paths.root_dir.join(shard_name),
            })
    }

    pub fn tensor(&self, name: &str) -> Result<&MlxTensorEntry> {
        let header = self.header_for_tensor(name)?;
        header
            .tensor(name)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: header.path.clone(),
                message: format!("tensor {} not found in shard header", name),
            })
    }

    pub fn read_tensor_bytes(&self, name: &str) -> Result<Vec<u8>> {
        self.header_for_tensor(name)?.read_tensor_bytes(name)
    }

    pub fn read_bf16_tensor_words(&self, name: &str) -> Result<Vec<u16>> {
        self.header_for_tensor(name)?.read_bf16_tensor_words(name)
    }

    pub fn embed_token_f32(&self, token_id: u32) -> Result<Vec<f32>> {
        let header = self.header_for_tensor(EMBED_TOKENS_WEIGHT_NAME)?;
        header.affine_dequantize_row_f32(
            EMBED_TOKENS_WEIGHT_NAME,
            EMBED_TOKENS_SCALES_NAME,
            EMBED_TOKENS_BIASES_NAME,
            token_id as u64,
            self.snapshot.config.quantization.group_size as u64,
            self.snapshot.config.quantization.bits,
        )
    }

    pub fn embed_token_bf16_words(&self, token_id: u32) -> Result<Vec<u16>> {
        Ok(self
            .embed_token_f32(token_id)?
            .into_iter()
            .map(f32_to_bf16_word)
            .collect())
    }

    pub fn final_text_norm_f32(&self, hidden_bf16_words: &[u16]) -> Result<Vec<f32>> {
        self.header_for_tensor(FINAL_TEXT_NORM_WEIGHT_NAME)?
            .rms_norm_weighted_f32(
                hidden_bf16_words,
                FINAL_TEXT_NORM_WEIGHT_NAME,
                self.snapshot.config.text_config.rms_norm_eps,
            )
    }

    pub fn final_text_norm_bf16_words(&self, hidden_bf16_words: &[u16]) -> Result<Vec<u16>> {
        Ok(self
            .final_text_norm_f32(hidden_bf16_words)?
            .into_iter()
            .map(f32_to_bf16_word)
            .collect())
    }

    pub fn tied_text_logits_top1_f32(&self, hidden_bf16_words: &[u16]) -> Result<MlxGreedyToken> {
        let header = self.header_for_tensor(EMBED_TOKENS_WEIGHT_NAME)?;
        let softcap = Some(self.snapshot.config.text_config.final_logit_softcapping)
            .filter(|softcap| *softcap > 0.0);
        header.affine_quantized_matmul_t_top1_f32(
            hidden_bf16_words,
            EMBED_TOKENS_WEIGHT_NAME,
            EMBED_TOKENS_SCALES_NAME,
            EMBED_TOKENS_BIASES_NAME,
            self.snapshot.config.quantization.group_size as u64,
            self.snapshot.config.quantization.bits,
            softcap,
        )
    }
}

#[derive(Clone, Debug)]
pub struct MlxTokenizer {
    normalized_space: String,
    vocab: HashMap<String, u32>,
    tokens_by_id: Vec<String>,
    merge_ranks: HashMap<(String, String), usize>,
    special_tokens: Vec<(String, u32)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MlxGreedyToken {
    pub token_id: u32,
    pub logit: f32,
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

        Ok(Self {
            normalized_space: normalizer_content,
            vocab,
            tokens_by_id,
            merge_ranks,
            special_tokens,
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
            let token = self
                .id_to_token(token_id)
                .ok_or_else(|| MlxRtError::InvalidModelDir {
                    path: PathBuf::new(),
                    message: format!("token id {} is out of vocabulary", token_id),
                })?;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MlxDType {
    Bool,
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    F16,
    BF16,
    F32,
    F64,
}

impl MlxDType {
    pub fn from_safetensors_str(value: &str) -> Result<Self> {
        match value {
            "BOOL" => Ok(Self::Bool),
            "U8" => Ok(Self::U8),
            "U16" => Ok(Self::U16),
            "U32" => Ok(Self::U32),
            "U64" => Ok(Self::U64),
            "I8" => Ok(Self::I8),
            "I16" => Ok(Self::I16),
            "I32" => Ok(Self::I32),
            "I64" => Ok(Self::I64),
            "F16" => Ok(Self::F16),
            "BF16" => Ok(Self::BF16),
            "F32" => Ok(Self::F32),
            "F64" => Ok(Self::F64),
            other => Err(MlxRtError::InvalidSafetensors {
                path: PathBuf::new(),
                message: format!("unsupported dtype {}", other),
            }),
        }
    }

    pub fn byte_width(self) -> u64 {
        match self {
            Self::Bool | Self::U8 | Self::I8 => 1,
            Self::U16 | Self::I16 | Self::F16 | Self::BF16 => 2,
            Self::U32 | Self::I32 | Self::F32 => 4,
            Self::U64 | Self::I64 | Self::F64 => 8,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MlxTensorEntry {
    pub dtype: MlxDType,
    pub shape: Vec<u64>,
    pub data_offsets: [u64; 2],
}

impl MlxTensorEntry {
    pub fn element_count(&self) -> u64 {
        self.shape.iter().copied().product::<u64>()
    }

    pub fn data_len_bytes(&self) -> u64 {
        self.data_offsets[1] - self.data_offsets[0]
    }

    pub fn expected_len_bytes(&self) -> u64 {
        self.element_count() * self.dtype.byte_width()
    }

    pub fn file_offsets(&self, payload_base_offset: u64) -> [u64; 2] {
        [
            payload_base_offset + self.data_offsets[0],
            payload_base_offset + self.data_offsets[1],
        ]
    }
}

#[derive(Clone, Debug)]
pub struct MlxSafetensorsHeader {
    pub path: PathBuf,
    pub file_len: u64,
    pub header_len: u64,
    pub metadata: HashMap<String, String>,
    pub tensors: HashMap<String, MlxTensorEntry>,
    file: Arc<Mutex<fs::File>>,
}

impl MlxSafetensorsHeader {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut file = fs::File::open(&path).map_err(|err| MlxRtError::Io {
            path: path.clone(),
            message: err.to_string(),
        })?;
        let file_len = file
            .metadata()
            .map_err(|err| MlxRtError::Io {
                path: path.clone(),
                message: err.to_string(),
            })?
            .len();

        let mut header_len_bytes = [0u8; 8];
        file.read_exact(&mut header_len_bytes)
            .map_err(|err| MlxRtError::Io {
                path: path.clone(),
                message: err.to_string(),
            })?;
        let header_len = u64::from_le_bytes(header_len_bytes);
        let payload_base_offset =
            8u64.checked_add(header_len)
                .ok_or_else(|| MlxRtError::InvalidSafetensors {
                    path: path.clone(),
                    message: "header length overflow".to_string(),
                })?;
        if payload_base_offset > file_len {
            return Err(MlxRtError::InvalidSafetensors {
                path: path.clone(),
                message: format!(
                    "header extends past EOF: payload base {} > file len {}",
                    payload_base_offset, file_len
                ),
            });
        }

        let mut header_bytes = vec![0u8; header_len as usize];
        file.read_exact(&mut header_bytes)
            .map_err(|err| MlxRtError::Io {
                path: path.clone(),
                message: err.to_string(),
            })?;
        let header_text =
            String::from_utf8(header_bytes).map_err(|err| MlxRtError::InvalidSafetensors {
                path: path.clone(),
                message: err.to_string(),
            })?;
        let header_map =
            HashMap::<String, JsonValue>::deserialize_json(&header_text).map_err(|err| {
                MlxRtError::Json {
                    path: path.clone(),
                    message: format!("{:?}", err),
                }
            })?;

        let mut metadata = HashMap::new();
        let mut tensors = HashMap::new();

        for (name, value) in header_map {
            if name == "__metadata__" {
                metadata = json_string_map(&path, "__metadata__", &value)?;
                continue;
            }
            let object = json_object(&path, &name, &value)?;
            let dtype = json_dtype(&path, &name, object.get("dtype"))?;
            let shape = json_u64_array(&path, &name, object.get("shape"))?;
            let data_offsets = json_two_u64s(&path, &name, object.get("data_offsets"))?;
            let entry = MlxTensorEntry {
                dtype,
                shape,
                data_offsets,
            };
            let file_offsets = entry.file_offsets(payload_base_offset);
            if file_offsets[1] > file_len {
                return Err(MlxRtError::InvalidSafetensors {
                    path: path.clone(),
                    message: format!(
                        "tensor {} ends past EOF: {} > {}",
                        name, file_offsets[1], file_len
                    ),
                });
            }
            if entry.data_len_bytes() != entry.expected_len_bytes() {
                return Err(MlxRtError::InvalidSafetensors {
                    path: path.clone(),
                    message: format!(
                        "tensor {} length mismatch: stored {} expected {}",
                        name,
                        entry.data_len_bytes(),
                        entry.expected_len_bytes()
                    ),
                });
            }
            tensors.insert(name, entry);
        }

        Ok(Self {
            path,
            file_len,
            header_len,
            metadata,
            tensors,
            file: Arc::new(Mutex::new(file)),
        })
    }

    pub fn payload_base_offset(&self) -> u64 {
        8 + self.header_len
    }

    pub fn tensor(&self, name: &str) -> Option<&MlxTensorEntry> {
        self.tensors.get(name)
    }

    fn read_file_range(&self, start: u64, len: usize) -> Result<Vec<u8>> {
        let mut file = self.file.lock().map_err(|_| MlxRtError::Io {
            path: self.path.clone(),
            message: "safetensors file mutex poisoned".to_string(),
        })?;
        file.seek(SeekFrom::Start(start))
            .map_err(|err| MlxRtError::Io {
                path: self.path.clone(),
                message: err.to_string(),
            })?;
        let mut bytes = vec![0u8; len];
        file.read_exact(&mut bytes).map_err(|err| MlxRtError::Io {
            path: self.path.clone(),
            message: err.to_string(),
        })?;
        Ok(bytes)
    }

    pub fn read_tensor_bytes(&self, name: &str) -> Result<Vec<u8>> {
        let entry = self
            .tensor(name)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} not found in header", name),
            })?;
        let file_offsets = entry.file_offsets(self.payload_base_offset());
        self.read_file_range(file_offsets[0], entry.data_len_bytes() as usize)
    }

    pub fn read_rank2_row_bytes(&self, name: &str, row: u64) -> Result<Vec<u8>> {
        let entry = self
            .tensor(name)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} not found in header", name),
            })?;
        if entry.shape.len() != 2 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} expected rank 2, got {:?}", name, entry.shape),
            });
        }
        if row >= entry.shape[0] {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} row {} out of range", name, row),
            });
        }
        let row_bytes = entry.shape[1] * entry.dtype.byte_width();
        let file_offsets = entry.file_offsets(self.payload_base_offset());
        let start = file_offsets[0] + row * row_bytes;
        let end = start + row_bytes;
        if end > file_offsets[1] {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} row {} extends past tensor payload", name, row),
            });
        }
        self.read_file_range(start, row_bytes as usize)
    }

    pub fn read_rank2_row_u32_words(&self, name: &str, row: u64) -> Result<Vec<u32>> {
        let entry = self
            .tensor(name)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} not found in header", name),
            })?;
        if entry.dtype != MlxDType::U32 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} expected U32, got {:?}", name, entry.dtype),
            });
        }
        let bytes = self.read_rank2_row_bytes(name, row)?;
        let mut out = Vec::with_capacity(bytes.len() / 4);
        for chunk in bytes.chunks_exact(4) {
            out.push(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
        Ok(out)
    }

    pub fn read_rank2_row_bf16_words(&self, name: &str, row: u64) -> Result<Vec<u16>> {
        let entry = self
            .tensor(name)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} not found in header", name),
            })?;
        if entry.dtype != MlxDType::BF16 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} expected BF16, got {:?}", name, entry.dtype),
            });
        }
        let bytes = self.read_rank2_row_bytes(name, row)?;
        let mut out = Vec::with_capacity(bytes.len() / 2);
        for chunk in bytes.chunks_exact(2) {
            out.push(u16::from_le_bytes([chunk[0], chunk[1]]));
        }
        Ok(out)
    }

    fn read_rank3_plane_bytes(&self, name: &str, plane: u64) -> Result<Vec<u8>> {
        let entry = self
            .tensor(name)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} not found in header", name),
            })?;
        if entry.shape.len() != 3 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} expected rank 3, got {:?}", name, entry.shape),
            });
        }
        if plane >= entry.shape[0] {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} plane {} out of range", name, plane),
            });
        }
        let plane_elems = entry.shape[1].checked_mul(entry.shape[2]).ok_or_else(|| {
            MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} plane element count overflow", name),
            }
        })?;
        let plane_bytes = plane_elems
            .checked_mul(entry.dtype.byte_width())
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} plane byte count overflow", name),
            })?;
        let file_offsets = entry.file_offsets(self.payload_base_offset());
        let start = file_offsets[0]
            .checked_add(plane.checked_mul(plane_bytes).ok_or_else(|| {
                MlxRtError::InvalidSafetensors {
                    path: self.path.clone(),
                    message: format!("tensor {} plane offset overflow", name),
                }
            })?)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} plane start overflow", name),
            })?;
        let end = start
            .checked_add(plane_bytes)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} plane end overflow", name),
            })?;
        if end > file_offsets[1] {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "tensor {} plane {} extends past tensor payload",
                    name, plane
                ),
            });
        }
        self.read_file_range(start, plane_bytes as usize)
    }

    pub fn read_rank3_plane_u32_words(&self, name: &str, plane: u64) -> Result<Vec<u32>> {
        let entry = self
            .tensor(name)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} not found in header", name),
            })?;
        if entry.dtype != MlxDType::U32 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} expected U32, got {:?}", name, entry.dtype),
            });
        }
        let bytes = self.read_rank3_plane_bytes(name, plane)?;
        let mut out = Vec::with_capacity(bytes.len() / 4);
        for chunk in bytes.chunks_exact(4) {
            out.push(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
        Ok(out)
    }

    pub fn read_rank3_plane_bf16_words(&self, name: &str, plane: u64) -> Result<Vec<u16>> {
        let entry = self
            .tensor(name)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} not found in header", name),
            })?;
        if entry.dtype != MlxDType::BF16 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} expected BF16, got {:?}", name, entry.dtype),
            });
        }
        let bytes = self.read_rank3_plane_bytes(name, plane)?;
        let mut out = Vec::with_capacity(bytes.len() / 2);
        for chunk in bytes.chunks_exact(2) {
            out.push(u16::from_le_bytes([chunk[0], chunk[1]]));
        }
        Ok(out)
    }

    pub fn read_u32_tensor_words(&self, name: &str) -> Result<Vec<u32>> {
        let entry = self
            .tensor(name)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} not found in header", name),
            })?;
        if entry.dtype != MlxDType::U32 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} expected U32, got {:?}", name, entry.dtype),
            });
        }
        let bytes = self.read_tensor_bytes(name)?;
        let mut out = Vec::with_capacity(bytes.len() / 4);
        for chunk in bytes.chunks_exact(4) {
            out.push(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
        Ok(out)
    }

    pub fn read_bf16_tensor_words(&self, name: &str) -> Result<Vec<u16>> {
        let entry = self
            .tensor(name)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} not found in header", name),
            })?;
        if entry.dtype != MlxDType::BF16 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} expected BF16, got {:?}", name, entry.dtype),
            });
        }
        let bytes = self.read_tensor_bytes(name)?;
        let mut out = Vec::with_capacity(bytes.len() / 2);
        for chunk in bytes.chunks_exact(2) {
            out.push(u16::from_le_bytes([chunk[0], chunk[1]]));
        }
        Ok(out)
    }

    pub fn affine_dequantize_row_f32(
        &self,
        weight_name: &str,
        scales_name: &str,
        biases_name: &str,
        row: u64,
        group_size: u64,
        bits: u32,
    ) -> Result<Vec<f32>> {
        if bits == 0 || bits > 8 || (bits & (bits - 1)) != 0 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("unsupported affine dequant bits {}", bits),
            });
        }
        let weight = self.read_rank2_row_u32_words(weight_name, row)?;
        let scales = self.read_rank2_row_bf16_words(scales_name, row)?;
        let biases = self.read_rank2_row_bf16_words(biases_name, row)?;
        if scales.len() != biases.len() {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "row {} scale/bias length mismatch: {} vs {}",
                    row,
                    scales.len(),
                    biases.len()
                ),
            });
        }
        let values_per_word = 32 / bits as u64;
        let out_size = weight.len() as u64 * values_per_word;
        if out_size != scales.len() as u64 * group_size {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "row {} packed/scales shape mismatch for group_size={} bits={}",
                    row, group_size, bits
                ),
            });
        }
        let words_per_group = group_size / values_per_word;
        if words_per_group == 0 || weight.len() as u64 != scales.len() as u64 * words_per_group {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("row {} invalid words_per_group {}", row, words_per_group),
            });
        }
        let mask = (1u32 << bits) - 1;
        let mut out = Vec::with_capacity(out_size as usize);
        for group_idx in 0..scales.len() {
            let scale = bf16_word_to_f32(scales[group_idx]);
            let bias = bf16_word_to_f32(biases[group_idx]);
            let group_start = group_idx * words_per_group as usize;
            let group_end = group_start + words_per_group as usize;
            for packed in &weight[group_start..group_end] {
                for shift in (0..32).step_by(bits as usize) {
                    let q = ((*packed >> shift) & mask) as f32;
                    out.push(bf16_round_to_f32(q * scale + bias));
                }
            }
        }
        Ok(out)
    }

    pub fn affine_quantized_matmul_t_f32(
        &self,
        x_bf16_words: &[u16],
        weight_name: &str,
        scales_name: &str,
        biases_name: &str,
        group_size: u64,
        bits: u32,
    ) -> Result<Vec<f32>> {
        if bits == 0 || bits > 8 || (bits & (bits - 1)) != 0 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("unsupported affine quantized matmul bits {}", bits),
            });
        }
        let weight_entry =
            self.tensor(weight_name)
                .ok_or_else(|| MlxRtError::InvalidSafetensors {
                    path: self.path.clone(),
                    message: format!("tensor {} not found in header", weight_name),
                })?;
        let scales_entry =
            self.tensor(scales_name)
                .ok_or_else(|| MlxRtError::InvalidSafetensors {
                    path: self.path.clone(),
                    message: format!("tensor {} not found in header", scales_name),
                })?;
        let biases_entry =
            self.tensor(biases_name)
                .ok_or_else(|| MlxRtError::InvalidSafetensors {
                    path: self.path.clone(),
                    message: format!("tensor {} not found in header", biases_name),
                })?;
        if weight_entry.dtype != MlxDType::U32 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "tensor {} expected U32, got {:?}",
                    weight_name, weight_entry.dtype
                ),
            });
        }
        if scales_entry.dtype != MlxDType::BF16 || biases_entry.dtype != MlxDType::BF16 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "tensors {} / {} expected BF16, got {:?} / {:?}",
                    scales_name, biases_name, scales_entry.dtype, biases_entry.dtype
                ),
            });
        }
        if weight_entry.shape.len() != 2
            || scales_entry.shape.len() != 2
            || biases_entry.shape.len() != 2
        {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "quantized matmul expects rank-2 tensors, got {:?} {:?} {:?}",
                    weight_entry.shape, scales_entry.shape, biases_entry.shape
                ),
            });
        }
        if scales_entry.shape != biases_entry.shape {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "scale/bias shape mismatch: {:?} vs {:?}",
                    scales_entry.shape, biases_entry.shape
                ),
            });
        }
        if weight_entry.shape[0] != scales_entry.shape[0] {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "weight/scales outer shape mismatch: {:?} vs {:?}",
                    weight_entry.shape, scales_entry.shape
                ),
            });
        }
        let values_per_word = 32 / bits as u64;
        let inner_dim = weight_entry.shape[1] * values_per_word;
        if inner_dim != scales_entry.shape[1] * group_size {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "packed/scales shape mismatch for group_size={} bits={}",
                    group_size, bits
                ),
            });
        }
        if x_bf16_words.len() as u64 != inner_dim {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "activation length mismatch: got {} expected {}",
                    x_bf16_words.len(),
                    inner_dim
                ),
            });
        }
        let words_per_group = group_size / values_per_word;
        if words_per_group == 0 || weight_entry.shape[1] != scales_entry.shape[1] * words_per_group
        {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("invalid words_per_group {}", words_per_group),
            });
        }

        let weights = self.read_u32_tensor_words(weight_name)?;
        let scales = self.read_bf16_tensor_words(scales_name)?;
        let biases = self.read_bf16_tensor_words(biases_name)?;
        let x = x_bf16_words
            .iter()
            .copied()
            .map(bf16_word_to_f32)
            .collect::<Vec<_>>();
        let rows = weight_entry.shape[0] as usize;
        let weight_stride = weight_entry.shape[1] as usize;
        let groups_per_row = scales_entry.shape[1] as usize;
        let pack_factor = values_per_word as usize;
        let mask = (1u32 << bits) - 1;
        let mut out = Vec::with_capacity(rows);

        for row in 0..rows {
            let weight_row_start = row * weight_stride;
            let qparam_row_start = row * groups_per_row;
            let mut sum = 0.0f32;
            let mut x_index = 0usize;
            for group in 0..groups_per_row {
                let scale = bf16_word_to_f32(scales[qparam_row_start + group]);
                let bias = bf16_word_to_f32(biases[qparam_row_start + group]);
                let group_start = weight_row_start + group * words_per_group as usize;
                let group_end = group_start + words_per_group as usize;
                for packed in &weights[group_start..group_end] {
                    let mut packed_word = *packed;
                    for _ in 0..pack_factor {
                        let q = (packed_word & mask) as f32;
                        let deq_mul = bf16_round_to_f32(scale * q);
                        let deq = bf16_round_to_f32(deq_mul + bias);
                        let prod = bf16_round_to_f32(x[x_index] * deq);
                        sum = bf16_round_to_f32(sum + prod);
                        x_index += 1;
                        if bits != 8 {
                            packed_word >>= bits;
                        }
                    }
                }
            }
            out.push(sum);
        }

        Ok(out)
    }

    pub fn affine_quantized_matmul_t_top1_f32(
        &self,
        x_bf16_words: &[u16],
        weight_name: &str,
        scales_name: &str,
        biases_name: &str,
        group_size: u64,
        bits: u32,
        softcap: Option<f32>,
    ) -> Result<MlxGreedyToken> {
        if bits == 0 || bits > 8 || (bits & (bits - 1)) != 0 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("unsupported affine quantized matmul bits {}", bits),
            });
        }
        let weight_entry =
            self.tensor(weight_name)
                .ok_or_else(|| MlxRtError::InvalidSafetensors {
                    path: self.path.clone(),
                    message: format!("tensor {} not found in header", weight_name),
                })?;
        let scales_entry =
            self.tensor(scales_name)
                .ok_or_else(|| MlxRtError::InvalidSafetensors {
                    path: self.path.clone(),
                    message: format!("tensor {} not found in header", scales_name),
                })?;
        let biases_entry =
            self.tensor(biases_name)
                .ok_or_else(|| MlxRtError::InvalidSafetensors {
                    path: self.path.clone(),
                    message: format!("tensor {} not found in header", biases_name),
                })?;
        if weight_entry.dtype != MlxDType::U32 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "tensor {} expected U32, got {:?}",
                    weight_name, weight_entry.dtype
                ),
            });
        }
        if scales_entry.dtype != MlxDType::BF16 || biases_entry.dtype != MlxDType::BF16 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "tensors {} / {} expected BF16, got {:?} / {:?}",
                    scales_name, biases_name, scales_entry.dtype, biases_entry.dtype
                ),
            });
        }
        if weight_entry.shape.len() != 2
            || scales_entry.shape.len() != 2
            || biases_entry.shape.len() != 2
        {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "quantized matmul expects rank-2 tensors, got {:?} {:?} {:?}",
                    weight_entry.shape, scales_entry.shape, biases_entry.shape
                ),
            });
        }
        if scales_entry.shape != biases_entry.shape {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "scale/bias shape mismatch: {:?} vs {:?}",
                    scales_entry.shape, biases_entry.shape
                ),
            });
        }
        if weight_entry.shape[0] != scales_entry.shape[0] {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "weight/scales outer shape mismatch: {:?} vs {:?}",
                    weight_entry.shape, scales_entry.shape
                ),
            });
        }
        let values_per_word = 32 / bits as u64;
        let inner_dim = weight_entry.shape[1] * values_per_word;
        if inner_dim != scales_entry.shape[1] * group_size {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "packed/scales shape mismatch for group_size={} bits={}",
                    group_size, bits
                ),
            });
        }
        if x_bf16_words.len() as u64 != inner_dim {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "activation length mismatch: got {} expected {}",
                    x_bf16_words.len(),
                    inner_dim
                ),
            });
        }
        let words_per_group = group_size / values_per_word;
        if words_per_group == 0 || weight_entry.shape[1] != scales_entry.shape[1] * words_per_group
        {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("invalid words_per_group {}", words_per_group),
            });
        }

        let weight_offsets = weight_entry.file_offsets(self.payload_base_offset());
        let scales_offsets = scales_entry.file_offsets(self.payload_base_offset());
        let biases_offsets = biases_entry.file_offsets(self.payload_base_offset());
        let weight_row_bytes = (weight_entry.shape[1] * weight_entry.dtype.byte_width()) as usize;
        let qparam_row_bytes = (scales_entry.shape[1] * scales_entry.dtype.byte_width()) as usize;
        let rows = weight_entry.shape[0] as usize;
        let groups_per_row = scales_entry.shape[1] as usize;
        let pack_factor = values_per_word as usize;
        let mask = (1u32 << bits) - 1;
        let x = x_bf16_words
            .iter()
            .copied()
            .map(bf16_word_to_f32)
            .collect::<Vec<_>>();
        let mut weight_file = fs::File::open(&self.path).map_err(|err| MlxRtError::Io {
            path: self.path.clone(),
            message: err.to_string(),
        })?;
        let mut scales_file = fs::File::open(&self.path).map_err(|err| MlxRtError::Io {
            path: self.path.clone(),
            message: err.to_string(),
        })?;
        let mut biases_file = fs::File::open(&self.path).map_err(|err| MlxRtError::Io {
            path: self.path.clone(),
            message: err.to_string(),
        })?;
        let mut weight_bytes = vec![0u8; weight_row_bytes];
        let mut scales_bytes = vec![0u8; qparam_row_bytes];
        let mut biases_bytes = vec![0u8; qparam_row_bytes];

        let mut best = MlxGreedyToken {
            token_id: 0,
            logit: f32::NEG_INFINITY,
        };
        for row in 0..rows {
            let weight_row_offset = weight_offsets[0] + row as u64 * weight_row_bytes as u64;
            let qparam_row_offset = scales_offsets[0] + row as u64 * qparam_row_bytes as u64;
            let bias_row_offset = biases_offsets[0] + row as u64 * qparam_row_bytes as u64;
            weight_file
                .seek(SeekFrom::Start(weight_row_offset))
                .map_err(|err| MlxRtError::Io {
                    path: self.path.clone(),
                    message: err.to_string(),
                })?;
            weight_file
                .read_exact(&mut weight_bytes)
                .map_err(|err| MlxRtError::Io {
                    path: self.path.clone(),
                    message: err.to_string(),
                })?;
            scales_file
                .seek(SeekFrom::Start(qparam_row_offset))
                .map_err(|err| MlxRtError::Io {
                    path: self.path.clone(),
                    message: err.to_string(),
                })?;
            scales_file
                .read_exact(&mut scales_bytes)
                .map_err(|err| MlxRtError::Io {
                    path: self.path.clone(),
                    message: err.to_string(),
                })?;
            biases_file
                .seek(SeekFrom::Start(bias_row_offset))
                .map_err(|err| MlxRtError::Io {
                    path: self.path.clone(),
                    message: err.to_string(),
                })?;
            biases_file
                .read_exact(&mut biases_bytes)
                .map_err(|err| MlxRtError::Io {
                    path: self.path.clone(),
                    message: err.to_string(),
                })?;

            let mut sum = 0.0f32;
            let mut x_index = 0usize;
            for group in 0..groups_per_row {
                let scale_byte_offset = group * 2;
                let scale = bf16_word_to_f32(u16::from_le_bytes([
                    scales_bytes[scale_byte_offset],
                    scales_bytes[scale_byte_offset + 1],
                ]));
                let bias = bf16_word_to_f32(u16::from_le_bytes([
                    biases_bytes[scale_byte_offset],
                    biases_bytes[scale_byte_offset + 1],
                ]));
                let group_start = group * words_per_group as usize * 4;
                let group_end = group_start + words_per_group as usize * 4;
                for chunk in weight_bytes[group_start..group_end].chunks_exact(4) {
                    let mut packed_word =
                        u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                    for _ in 0..pack_factor {
                        let q = (packed_word & mask) as f32;
                        let deq_mul = bf16_round_to_f32(scale * q);
                        let deq = bf16_round_to_f32(deq_mul + bias);
                        let prod = bf16_round_to_f32(x[x_index] * deq);
                        sum = bf16_round_to_f32(sum + prod);
                        x_index += 1;
                        if bits != 8 {
                            packed_word >>= bits;
                        }
                    }
                }
            }

            let logit = if let Some(softcap) = softcap.filter(|softcap| *softcap > 0.0) {
                bf16_round_to_f32((sum / softcap).tanh() * softcap)
            } else {
                sum
            };
            if logit > best.logit {
                best = MlxGreedyToken {
                    token_id: row as u32,
                    logit,
                };
            }
        }

        Ok(best)
    }

    pub fn affine_quantized_matmul_t_f32_rank3_plane(
        &self,
        x_bf16_words: &[u16],
        weight_name: &str,
        scales_name: &str,
        biases_name: &str,
        plane: u64,
        group_size: u64,
        bits: u32,
    ) -> Result<Vec<f32>> {
        if bits == 0 || bits > 8 || (bits & (bits - 1)) != 0 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("unsupported affine quantized matmul bits {}", bits),
            });
        }
        let weight_entry =
            self.tensor(weight_name)
                .ok_or_else(|| MlxRtError::InvalidSafetensors {
                    path: self.path.clone(),
                    message: format!("tensor {} not found in header", weight_name),
                })?;
        let scales_entry =
            self.tensor(scales_name)
                .ok_or_else(|| MlxRtError::InvalidSafetensors {
                    path: self.path.clone(),
                    message: format!("tensor {} not found in header", scales_name),
                })?;
        let biases_entry =
            self.tensor(biases_name)
                .ok_or_else(|| MlxRtError::InvalidSafetensors {
                    path: self.path.clone(),
                    message: format!("tensor {} not found in header", biases_name),
                })?;
        if weight_entry.dtype != MlxDType::U32 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "tensor {} expected U32, got {:?}",
                    weight_name, weight_entry.dtype
                ),
            });
        }
        if scales_entry.dtype != MlxDType::BF16 || biases_entry.dtype != MlxDType::BF16 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "tensors {} / {} expected BF16, got {:?} / {:?}",
                    scales_name, biases_name, scales_entry.dtype, biases_entry.dtype
                ),
            });
        }
        if weight_entry.shape.len() != 3
            || scales_entry.shape.len() != 3
            || biases_entry.shape.len() != 3
        {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "rank-3 affine quantized matmul expects rank-3 tensors, got {:?} {:?} {:?}",
                    weight_entry.shape, scales_entry.shape, biases_entry.shape
                ),
            });
        }
        if scales_entry.shape != biases_entry.shape {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "scale/bias shape mismatch: {:?} vs {:?}",
                    scales_entry.shape, biases_entry.shape
                ),
            });
        }
        if weight_entry.shape[0] != scales_entry.shape[0]
            || weight_entry.shape[1] != scales_entry.shape[1]
        {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "weight/scales outer shape mismatch: {:?} vs {:?}",
                    weight_entry.shape, scales_entry.shape
                ),
            });
        }
        if plane >= weight_entry.shape[0] {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "plane {} out of range for tensor {} with {} planes",
                    plane, weight_name, weight_entry.shape[0]
                ),
            });
        }
        let values_per_word = 32 / bits as u64;
        let inner_dim = weight_entry.shape[2] * values_per_word;
        if inner_dim != scales_entry.shape[2] * group_size {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "packed/scales plane shape mismatch for group_size={} bits={}",
                    group_size, bits
                ),
            });
        }
        if x_bf16_words.len() as u64 != inner_dim {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "activation length mismatch: got {} expected {}",
                    x_bf16_words.len(),
                    inner_dim
                ),
            });
        }
        let words_per_group = group_size / values_per_word;
        if words_per_group == 0 || weight_entry.shape[2] != scales_entry.shape[2] * words_per_group
        {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("invalid words_per_group {}", words_per_group),
            });
        }

        let weights = self.read_rank3_plane_u32_words(weight_name, plane)?;
        let scales = self.read_rank3_plane_bf16_words(scales_name, plane)?;
        let biases = self.read_rank3_plane_bf16_words(biases_name, plane)?;
        let x = x_bf16_words
            .iter()
            .copied()
            .map(bf16_word_to_f32)
            .collect::<Vec<_>>();
        let rows = weight_entry.shape[1] as usize;
        let weight_stride = weight_entry.shape[2] as usize;
        let groups_per_row = scales_entry.shape[2] as usize;
        let pack_factor = values_per_word as usize;
        let mask = (1u32 << bits) - 1;
        let mut out = Vec::with_capacity(rows);

        for row in 0..rows {
            let weight_row_start = row * weight_stride;
            let qparam_row_start = row * groups_per_row;
            let mut sum = 0.0f32;
            let mut x_index = 0usize;
            for group in 0..groups_per_row {
                let scale = bf16_word_to_f32(scales[qparam_row_start + group]);
                let bias = bf16_word_to_f32(biases[qparam_row_start + group]);
                let group_start = weight_row_start + group * words_per_group as usize;
                let group_end = group_start + words_per_group as usize;
                for packed in &weights[group_start..group_end] {
                    let mut packed_word = *packed;
                    for _ in 0..pack_factor {
                        let q = (packed_word & mask) as f32;
                        let deq_mul = bf16_round_to_f32(scale * q);
                        let deq = bf16_round_to_f32(deq_mul + bias);
                        let prod = bf16_round_to_f32(x[x_index] * deq);
                        sum = bf16_round_to_f32(sum + prod);
                        x_index += 1;
                        if bits != 8 {
                            packed_word >>= bits;
                        }
                    }
                }
            }
            out.push(sum);
        }

        Ok(out)
    }

    pub fn rms_norm_weighted_f32(
        &self,
        x_bf16_words: &[u16],
        weight_name: &str,
        eps: f32,
    ) -> Result<Vec<f32>> {
        let weight_entry =
            self.tensor(weight_name)
                .ok_or_else(|| MlxRtError::InvalidSafetensors {
                    path: self.path.clone(),
                    message: format!("missing tensor {}", weight_name),
                })?;
        if weight_entry.shape.len() != 1 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "rms_norm expects rank-1 weight, got {:?}",
                    weight_entry.shape
                ),
            });
        }
        let hidden = weight_entry.shape[0] as usize;
        if x_bf16_words.len() != hidden {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "rms_norm activation length mismatch: got {} expected {}",
                    x_bf16_words.len(),
                    hidden
                ),
            });
        }

        let weight_words = self.read_bf16_tensor_words(weight_name)?;
        let x = x_bf16_words
            .iter()
            .copied()
            .map(bf16_word_to_f32)
            .collect::<Vec<_>>();

        let mut mean_square = 0.0f32;
        for value in &x {
            mean_square += value * value;
        }
        mean_square /= hidden as f32;
        let inv_rms = 1.0f32 / (mean_square + eps).sqrt();

        let mut out = Vec::with_capacity(hidden);
        for (index, value) in x.iter().copied().enumerate() {
            let normalized = bf16_round_to_f32(value * inv_rms);
            let weight = bf16_word_to_f32(weight_words[index]);
            out.push(bf16_round_to_f32(normalized * weight));
        }
        Ok(out)
    }

    pub fn gemma_router_topk_from_residual_bf16(
        &self,
        residual_bf16_words: &[u16],
        router_scale_name: &str,
        per_expert_scale_name: &str,
        proj_weight_name: &str,
        proj_scales_name: &str,
        proj_biases_name: &str,
        eps: f32,
        top_k: usize,
    ) -> Result<MlxRouterTopKOutput> {
        if top_k == 0 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: "router top_k must be greater than zero".to_string(),
            });
        }

        let hidden = residual_bf16_words.len();
        let router_scale_words = self.read_bf16_tensor_words(router_scale_name)?;
        if router_scale_words.len() != hidden {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "router scale length mismatch: got {} expected {}",
                    router_scale_words.len(),
                    hidden
                ),
            });
        }
        let per_expert_scale_words = self.read_bf16_tensor_words(per_expert_scale_name)?;
        if top_k > per_expert_scale_words.len() {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "router top_k {} exceeds num_experts {}",
                    top_k,
                    per_expert_scale_words.len()
                ),
            });
        }

        let residual = residual_bf16_words
            .iter()
            .copied()
            .map(bf16_word_to_f32)
            .collect::<Vec<_>>();
        let mut mean_square = 0.0f32;
        for value in &residual {
            mean_square += value * value;
        }
        mean_square /= hidden as f32;
        let inv_rms = 1.0f32 / (mean_square + eps).sqrt();

        let root_size = bf16_round_to_f32((hidden as f32).powf(-0.5));
        let mut router_scaled = Vec::with_capacity(hidden);
        let mut router_scaled_words = Vec::with_capacity(hidden);
        for (index, value) in residual.iter().copied().enumerate() {
            let normed = bf16_round_to_f32(value * inv_rms);
            let scaled_root = bf16_round_to_f32(normed * root_size);
            let scaled =
                bf16_round_to_f32(scaled_root * bf16_word_to_f32(router_scale_words[index]));
            router_scaled.push(scaled);
            router_scaled_words.push(f32_to_bf16_word(scaled));
        }

        let expert_scores = self.affine_quantized_matmul_t_f32(
            &router_scaled_words,
            proj_weight_name,
            proj_scales_name,
            proj_biases_name,
            64,
            4,
        )?;
        if expert_scores.is_empty() {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: "router projection produced no scores".to_string(),
            });
        }
        if top_k > expert_scores.len() {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "router top_k {} exceeds expert_scores length {}",
                    top_k,
                    expert_scores.len()
                ),
            });
        }

        let max_score = expert_scores
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        let exp_scores = expert_scores
            .iter()
            .copied()
            .map(|value| (value - max_score).exp())
            .collect::<Vec<_>>();
        let exp_sum = exp_scores.iter().copied().sum::<f32>();
        let router_probs = exp_scores
            .iter()
            .copied()
            .map(|value| bf16_round_to_f32(value / exp_sum))
            .collect::<Vec<_>>();

        let mut indices = (0..expert_scores.len()).collect::<Vec<_>>();
        indices.sort_by(|&lhs, &rhs| {
            expert_scores[rhs]
                .total_cmp(&expert_scores[lhs])
                .then_with(|| lhs.cmp(&rhs))
        });
        let top_k_indices = indices
            .into_iter()
            .take(top_k)
            .map(|index| index as u32)
            .collect::<Vec<_>>();

        let mut top_k_weights = top_k_indices
            .iter()
            .copied()
            .map(|index| router_probs[index as usize])
            .collect::<Vec<_>>();
        let mut top_k_sum = 0.0f32;
        for weight in &top_k_weights {
            top_k_sum = bf16_round_to_f32(top_k_sum + *weight);
        }
        for (slot, weight) in top_k_weights.iter_mut().enumerate() {
            let normalized = bf16_round_to_f32(*weight / top_k_sum);
            let expert_scale =
                bf16_word_to_f32(per_expert_scale_words[top_k_indices[slot] as usize]);
            *weight = bf16_round_to_f32(normalized * expert_scale);
        }

        Ok(MlxRouterTopKOutput {
            router_scaled,
            expert_scores,
            router_probs,
            top_k_indices,
            top_k_weights,
        })
    }

    pub fn gemma_moe_expert_block_from_residual_bf16(
        &self,
        residual_bf16_words: &[u16],
        pre_feedforward_norm2_weight_name: &str,
        expert_gate_weight_name: &str,
        expert_gate_scales_name: &str,
        expert_gate_biases_name: &str,
        expert_up_weight_name: &str,
        expert_up_scales_name: &str,
        expert_up_biases_name: &str,
        expert_down_weight_name: &str,
        expert_down_scales_name: &str,
        expert_down_biases_name: &str,
        top_k_indices: &[u32],
        top_k_weights: &[f32],
        eps: f32,
    ) -> Result<MlxGemmaMoeExpertOutput> {
        if top_k_indices.is_empty() {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: "moe expert path needs at least one routed expert".to_string(),
            });
        }
        if top_k_indices.len() != top_k_weights.len() {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "top_k index/weight length mismatch: {} vs {}",
                    top_k_indices.len(),
                    top_k_weights.len()
                ),
            });
        }

        let pre_feedforward_norm2 = self.rms_norm_weighted_f32(
            residual_bf16_words,
            pre_feedforward_norm2_weight_name,
            eps,
        )?;
        let pre_feedforward_norm2_words = pre_feedforward_norm2
            .iter()
            .copied()
            .map(f32_to_bf16_word)
            .collect::<Vec<_>>();

        let mut gate_proj = Vec::new();
        let mut up_proj = Vec::new();
        let mut geglu = Vec::new();
        let mut down_proj = Vec::new();
        let hidden = residual_bf16_words.len();

        for &expert_index in top_k_indices {
            let gate_row = self.affine_quantized_matmul_t_f32_rank3_plane(
                &pre_feedforward_norm2_words,
                expert_gate_weight_name,
                expert_gate_scales_name,
                expert_gate_biases_name,
                expert_index as u64,
                64,
                4,
            )?;
            let up_row = self.affine_quantized_matmul_t_f32_rank3_plane(
                &pre_feedforward_norm2_words,
                expert_up_weight_name,
                expert_up_scales_name,
                expert_up_biases_name,
                expert_index as u64,
                64,
                4,
            )?;
            if gate_row.len() != up_row.len() {
                return Err(MlxRtError::InvalidSafetensors {
                    path: self.path.clone(),
                    message: format!(
                        "expert {} gate/up output length mismatch: {} vs {}",
                        expert_index,
                        gate_row.len(),
                        up_row.len()
                    ),
                });
            }
            let mut geglu_row = Vec::with_capacity(gate_row.len());
            for (&gate, &up) in gate_row.iter().zip(up_row.iter()) {
                let gate_sq = bf16_round_to_f32(gate * gate);
                let gate_cubic = bf16_round_to_f32(gate_sq * gate);
                let gate_poly =
                    bf16_round_to_f32(gate + bf16_round_to_f32(0.044_715f32 * gate_cubic));
                let gate_tanh_input = bf16_round_to_f32(0.797_884_6f32 * gate_poly);
                let gate_tanh = bf16_round_to_f32(gate_tanh_input.tanh());
                let gate_one_plus = bf16_round_to_f32(1.0f32 + gate_tanh);
                let gate_half = bf16_round_to_f32(0.5f32 * gate);
                let gate_gelu = bf16_round_to_f32(gate_half * gate_one_plus);
                geglu_row.push(bf16_round_to_f32(gate_gelu * up));
            }
            let geglu_words = geglu_row
                .iter()
                .copied()
                .map(f32_to_bf16_word)
                .collect::<Vec<_>>();
            let down_row = self.affine_quantized_matmul_t_f32_rank3_plane(
                &geglu_words,
                expert_down_weight_name,
                expert_down_scales_name,
                expert_down_biases_name,
                expert_index as u64,
                64,
                4,
            )?;
            if down_row.len() != hidden {
                return Err(MlxRtError::InvalidSafetensors {
                    path: self.path.clone(),
                    message: format!(
                        "expert {} down projection length mismatch: got {} expected {}",
                        expert_index,
                        down_row.len(),
                        hidden
                    ),
                });
            }
            gate_proj.extend_from_slice(&gate_row);
            up_proj.extend_from_slice(&up_row);
            geglu.extend_from_slice(&geglu_row);
            down_proj.extend_from_slice(&down_row);
        }

        let mut expert_out = vec![0.0f32; hidden];
        for (expert_slot, &weight) in top_k_weights.iter().enumerate() {
            for hidden_index in 0..hidden {
                let weighted =
                    bf16_round_to_f32(down_proj[expert_slot * hidden + hidden_index] * weight);
                expert_out[hidden_index] = bf16_round_to_f32(expert_out[hidden_index] + weighted);
            }
        }

        Ok(MlxGemmaMoeExpertOutput {
            pre_feedforward_norm2,
            gate_proj,
            up_proj,
            geglu,
            down_proj,
            expert_out,
        })
    }
}

pub const GEMMA4_QPROJ_CASE_INNER_DIM: usize = 2_816;
pub const GEMMA4_QPROJ_CASE_OUTPUT_DIM: usize = 4_096;
pub const GEMMA4_QPROJ_CASE_OUTPUT_FNV1A64: u64 = 0x4A22_9C27_44EA_03B8;
pub const GEMMA4_QPROJ_CASE_ACTIVATION_PATTERN: [f32; 16] = [
    -1.0, -0.75, -0.5, -0.25, 0.0, 0.25, 0.5, 0.75, 1.0, 0.5, 0.0, -0.5, -1.0, 0.125, 0.375, 0.625,
];

fn bf16_word_to_f32(word: u16) -> f32 {
    f32::from_bits((word as u32) << 16)
}

fn f32_to_bf16_word(value: f32) -> u16 {
    (bf16_round_to_f32(value).to_bits() >> 16) as u16
}

fn bf16_round_to_f32(value: f32) -> f32 {
    let bits = value.to_bits();
    let lsb = (bits >> 16) & 1;
    let rounded = bits.wrapping_add(0x7FFF + lsb) & 0xFFFF0000;
    f32::from_bits(rounded)
}

pub fn gemma4_qproj_case_input_bf16_words(len: usize) -> Vec<u16> {
    gemma4_qproj_case_input_bf16_words_with_phase(len, 0)
}

pub fn gemma4_qproj_case_input_bf16_words_with_phase(len: usize, phase: usize) -> Vec<u16> {
    (0..len)
        .map(|index| {
            f32_to_bf16_word(
                GEMMA4_QPROJ_CASE_ACTIVATION_PATTERN
                    [(index + phase) % GEMMA4_QPROJ_CASE_ACTIVATION_PATTERN.len()],
            )
        })
        .collect()
}

pub fn gemma4_qproj_case_input_f32_values_with_phase(len: usize, phase: usize) -> Vec<f32> {
    (0..len)
        .map(|index| {
            GEMMA4_QPROJ_CASE_ACTIVATION_PATTERN
                [(index + phase) % GEMMA4_QPROJ_CASE_ACTIVATION_PATTERN.len()]
        })
        .collect()
}

pub fn fnv1a64_u32_words(words: &[u32]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for word in words {
        for byte in word.to_le_bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

fn json_object<'a>(
    path: &Path,
    context: &str,
    value: &'a JsonValue,
) -> Result<&'a HashMap<String, JsonValue>> {
    match value {
        JsonValue::Object(object) => Ok(object),
        other => Err(MlxRtError::InvalidSafetensors {
            path: path.to_path_buf(),
            message: format!("{} expected object, got {:?}", context, other),
        }),
    }
}

fn json_string(path: &Path, context: &str, value: Option<&JsonValue>) -> Result<String> {
    match value {
        Some(JsonValue::String(text)) => Ok(text.clone()),
        Some(other) => Err(MlxRtError::InvalidSafetensors {
            path: path.to_path_buf(),
            message: format!("{} expected string, got {:?}", context, other),
        }),
        None => Err(MlxRtError::InvalidSafetensors {
            path: path.to_path_buf(),
            message: format!("{} missing string field", context),
        }),
    }
}

fn json_u64(path: &Path, context: &str, value: &JsonValue) -> Result<u64> {
    match value {
        JsonValue::U64(number) => Ok(*number),
        JsonValue::U128(number) => {
            u64::try_from(*number).map_err(|_| MlxRtError::InvalidSafetensors {
                path: path.to_path_buf(),
                message: format!("{} value {} does not fit in u64", context, number),
            })
        }
        JsonValue::I64(number) => {
            u64::try_from(*number).map_err(|_| MlxRtError::InvalidSafetensors {
                path: path.to_path_buf(),
                message: format!("{} value {} is negative", context, number),
            })
        }
        JsonValue::I128(number) => {
            u64::try_from(*number).map_err(|_| MlxRtError::InvalidSafetensors {
                path: path.to_path_buf(),
                message: format!("{} value {} is negative or too large", context, number),
            })
        }
        other => Err(MlxRtError::InvalidSafetensors {
            path: path.to_path_buf(),
            message: format!("{} expected integer, got {:?}", context, other),
        }),
    }
}

fn json_u64_array(path: &Path, context: &str, value: Option<&JsonValue>) -> Result<Vec<u64>> {
    let array = match value {
        Some(JsonValue::Array(array)) => array,
        Some(other) => {
            return Err(MlxRtError::InvalidSafetensors {
                path: path.to_path_buf(),
                message: format!("{} expected integer array, got {:?}", context, other),
            });
        }
        None => {
            return Err(MlxRtError::InvalidSafetensors {
                path: path.to_path_buf(),
                message: format!("{} missing integer array", context),
            });
        }
    };
    let mut out = Vec::with_capacity(array.len());
    for (index, item) in array.iter().enumerate() {
        out.push(json_u64(path, &format!("{}[{}]", context, index), item)?);
    }
    Ok(out)
}

fn json_two_u64s(path: &Path, context: &str, value: Option<&JsonValue>) -> Result<[u64; 2]> {
    let values = json_u64_array(path, context, value)?;
    if values.len() != 2 {
        return Err(MlxRtError::InvalidSafetensors {
            path: path.to_path_buf(),
            message: format!("{} expected two integers, got {}", context, values.len()),
        });
    }
    Ok([values[0], values[1]])
}

fn json_string_map(
    path: &Path,
    context: &str,
    value: &JsonValue,
) -> Result<HashMap<String, String>> {
    let object = json_object(path, context, value)?;
    let mut out = HashMap::with_capacity(object.len());
    for (key, value) in object {
        out.insert(
            key.clone(),
            json_string(path, &format!("{}.{}", context, key), Some(value))?,
        );
    }
    Ok(out)
}

fn json_dtype(path: &Path, context: &str, value: Option<&JsonValue>) -> Result<MlxDType> {
    let dtype_str = json_string(path, &format!("{}.dtype", context), value)?;
    MlxDType::from_safetensors_str(&dtype_str).map_err(|_| MlxRtError::InvalidSafetensors {
        path: path.to_path_buf(),
        message: format!("{} unsupported dtype {}", context, dtype_str),
    })
}

fn tokenizer_object<'a>(
    path: &Path,
    context: &str,
    value: Option<&'a JsonValue>,
) -> Result<&'a HashMap<String, JsonValue>> {
    match value {
        Some(JsonValue::Object(object)) => Ok(object),
        Some(other) => Err(MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{} expected object, got {:?}", context, other),
        }),
        None => Err(MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{} missing object", context),
        }),
    }
}

fn tokenizer_array<'a>(
    path: &Path,
    context: &str,
    value: Option<&'a JsonValue>,
) -> Result<&'a Vec<JsonValue>> {
    match value {
        Some(JsonValue::Array(array)) => Ok(array),
        Some(other) => Err(MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{} expected array, got {:?}", context, other),
        }),
        None => Err(MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{} missing array", context),
        }),
    }
}

fn tokenizer_string(path: &Path, context: &str, value: Option<&JsonValue>) -> Result<String> {
    match value {
        Some(JsonValue::String(text)) => Ok(text.clone()),
        Some(other) => Err(MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{} expected string, got {:?}", context, other),
        }),
        None => Err(MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{} missing string", context),
        }),
    }
}

fn tokenizer_bool(path: &Path, context: &str, value: Option<&JsonValue>) -> Result<bool> {
    match value {
        Some(JsonValue::Bool(flag)) => Ok(*flag),
        Some(other) => Err(MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{} expected bool, got {:?}", context, other),
        }),
        None => Err(MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{} missing bool", context),
        }),
    }
}

fn tokenizer_u32(path: &Path, context: &str, value: Option<&JsonValue>) -> Result<u32> {
    match value {
        Some(JsonValue::U64(number)) => u32::try_from(*number).map_err(|_| MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{} value {} does not fit in u32", context, number),
        }),
        Some(JsonValue::U128(number)) => u32::try_from(*number).map_err(|_| MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{} value {} does not fit in u32", context, number),
        }),
        Some(JsonValue::I64(number)) => u32::try_from(*number).map_err(|_| MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{} value {} is negative or too large", context, number),
        }),
        Some(JsonValue::I128(number)) => u32::try_from(*number).map_err(|_| MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{} value {} is negative or too large", context, number),
        }),
        Some(other) => Err(MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{} expected integer, got {:?}", context, other),
        }),
        None => Err(MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{} missing integer", context),
        }),
    }
}

fn tokenizer_pattern_string(
    path: &Path,
    context: &str,
    value: Option<&JsonValue>,
) -> Result<String> {
    let object = tokenizer_object(path, context, value)?;
    tokenizer_string(path, &format!("{}.String", context), object.get("String"))
}

fn tokenizer_string_pair(
    path: &Path,
    context: &str,
    value: &JsonValue,
) -> Result<(String, String)> {
    let array = match value {
        JsonValue::Array(array) => array,
        other => {
            return Err(MlxRtError::Json {
                path: path.to_path_buf(),
                message: format!("{} expected [string, string], got {:?}", context, other),
            });
        }
    };
    if array.len() != 2 {
        return Err(MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{} expected two strings, got {}", context, array.len()),
        });
    }
    Ok((
        tokenizer_string(path, &format!("{}[0]", context), array.first())?,
        tokenizer_string(path, &format!("{}[1]", context), array.get(1))?,
    ))
}

fn parse_byte_fallback_token(token: &str) -> Option<u8> {
    if !token.starts_with("<0x") || !token.ends_with('>') || token.len() != 6 {
        return None;
    }
    u8::from_str_radix(&token[3..5], 16).ok()
}

fn flush_pending_bytes(out: &mut String, pending_bytes: &mut Vec<u8>) {
    if pending_bytes.is_empty() {
        return;
    }
    out.push_str(&String::from_utf8_lossy(pending_bytes));
    pending_bytes.clear();
}

#[cfg(test)]
mod tests {
    use super::{
        bf16_word_to_f32, fnv1a64_u32_words, gemma4_qproj_case_input_bf16_words, MlxDType,
        MlxIndexedSafetensors, MlxModelSnapshot, MlxSafetensorsHeader, MlxTokenizer,
        GEMMA4_QPROJ_CASE_INNER_DIM, GEMMA4_QPROJ_CASE_OUTPUT_DIM,
        GEMMA4_QPROJ_CASE_OUTPUT_FNV1A64,
    };
    use std::path::PathBuf;

    fn local_model_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../local/models/gemma-4-26b-mlx")
    }

    fn local_model_shard_1() -> PathBuf {
        local_model_dir().join("model-00001-of-00003.safetensors")
    }

    #[test]
    fn loads_local_gemma4_mlx_snapshot() {
        let snapshot = MlxModelSnapshot::load(local_model_dir()).unwrap();

        assert_eq!(
            snapshot.config.architectures,
            vec!["Gemma4ForConditionalGeneration".to_string()]
        );
        assert_eq!(snapshot.config.model_type, "gemma4");
        assert_eq!(snapshot.config.text_config.num_hidden_layers, 30);
        assert_eq!(snapshot.config.vision_config.num_hidden_layers, 27);
        assert_eq!(snapshot.config.quantization.bits, 4);
        assert_eq!(snapshot.config.quantization.group_size, 64);
        assert_eq!(snapshot.config.quantization.mode, "affine");
        assert_eq!(snapshot.processor_config.image_processor.size.height, 224);
        assert_eq!(snapshot.processor_config.image_processor.size.width, 224);
        assert_eq!(
            snapshot.tokenizer_config.model_max_length,
            1000000000000000019884624838656u128
        );
        assert_eq!(snapshot.weight_index.metadata.total_size, 15_335_574_684);
        assert_eq!(snapshot.unique_weight_shards().len(), 3);
        assert_eq!(
            snapshot
                .weight_index
                .weight_map
                .get("language_model.model.layers.0.self_attn.q_proj.weight")
                .map(String::as_str),
            Some("model-00001-of-00003.safetensors")
        );
        assert_eq!(
            snapshot
                .weight_index
                .weight_map
                .get("embed_vision.embedding_projection.weight")
                .map(String::as_str),
            Some("model-00003-of-00003.safetensors")
        );
    }

    #[test]
    fn reads_local_safetensors_header_without_touching_payload() {
        let header = MlxSafetensorsHeader::load(local_model_shard_1()).unwrap();

        assert_eq!(header.header_len, 64_065);
        assert_eq!(
            header.metadata.get("format").map(String::as_str),
            Some("mlx")
        );
        assert_eq!(header.tensors.len(), 488);

        let embed_weight = header
            .tensor("language_model.model.embed_tokens.weight")
            .unwrap();
        assert_eq!(embed_weight.dtype, MlxDType::U32);
        assert_eq!(embed_weight.shape, vec![262_144, 352]);
        assert_eq!(embed_weight.data_offsets, [3_116_339_724, 3_485_438_476]);
        assert_eq!(
            embed_weight.data_len_bytes(),
            embed_weight.expected_len_bytes()
        );

        let q_proj = header
            .tensor("language_model.model.layers.0.self_attn.q_proj.weight")
            .unwrap();
        assert_eq!(q_proj.dtype, MlxDType::U32);
        assert_eq!(q_proj.shape, vec![4_096, 352]);
        assert_eq!(q_proj.data_offsets, [3_612_518_924, 3_618_286_092]);
        assert_eq!(q_proj.data_len_bytes(), q_proj.expected_len_bytes());

        let q_proj_scales = header
            .tensor("language_model.model.layers.0.self_attn.q_proj.scales")
            .unwrap();
        assert_eq!(q_proj_scales.dtype, MlxDType::BF16);
        assert_eq!(q_proj_scales.shape, vec![4_096, 44]);
        assert_eq!(q_proj_scales.data_offsets, [2_536_973_576, 2_537_334_024]);
        assert_eq!(
            q_proj_scales.file_offsets(header.payload_base_offset())[0],
            2_537_037_649
        );
    }

    #[test]
    fn indexed_safetensors_resolves_late_layer_to_correct_shard() {
        let indexed = MlxIndexedSafetensors::load(local_model_dir()).unwrap();

        assert_eq!(
            indexed
                .shard_name_for_tensor("language_model.model.layers.29.self_attn.q_proj.weight")
                .unwrap(),
            "model-00003-of-00003.safetensors"
        );

        let header = indexed
            .header_for_tensor("language_model.model.layers.29.self_attn.q_proj.weight")
            .unwrap();
        assert!(header.path.ends_with("model-00003-of-00003.safetensors"));

        let entry = indexed
            .tensor("language_model.model.layers.29.self_attn.q_proj.weight")
            .unwrap();
        assert_eq!(entry.dtype, MlxDType::U32);
        assert_eq!(entry.shape, vec![8_192, 352]);
    }

    #[test]
    fn loads_local_tokenizer_metadata() {
        let tokenizer = MlxTokenizer::load(local_model_dir()).unwrap();

        assert!(tokenizer.vocab_size() > 260_000);
        assert!(tokenizer.merge_count() > 500_000);
        assert_eq!(tokenizer.token_to_id("<bos>"), Some(2));
        assert_eq!(tokenizer.token_to_id("<eos>"), Some(1));
        assert_eq!(tokenizer.token_to_id("<|video|>"), Some(258_884));
        assert_eq!(tokenizer.token_to_id("say"), Some(30_468));
        assert_eq!(tokenizer.token_to_id("▁hi"), Some(5_631));
        assert_eq!(tokenizer.id_to_token(2), Some("<bos>"));
    }

    #[test]
    fn tokenizer_encodes_and_decodes_simple_phrase() {
        let tokenizer = MlxTokenizer::load(local_model_dir()).unwrap();

        assert_eq!(tokenizer.encode("say hi").unwrap(), vec![30_468, 5_631]);
        assert_eq!(tokenizer.encode(" hi").unwrap(), vec![5_631]);
        assert_eq!(tokenizer.decode(&[30_468, 5_631]).unwrap(), "say hi");
        assert_eq!(tokenizer.decode(&[1_879, 5_631]).unwrap(), " say hi");
    }

    #[test]
    fn embeds_and_norms_local_text_token_rows() {
        let weights = MlxIndexedSafetensors::load(local_model_dir()).unwrap();

        let embed = weights.embed_token_bf16_words(30_468).unwrap();
        assert_eq!(embed.len(), 2_816);

        let final_norm = weights.final_text_norm_bf16_words(&embed).unwrap();
        assert_eq!(final_norm.len(), 2_816);
    }

    #[test]
    #[ignore]
    fn embed_rows_report_hashes_for_two_token_prompt() {
        let weights = MlxIndexedSafetensors::load(local_model_dir()).unwrap();
        for token_id in [30_468u32, 5_631u32] {
            let bits = weights
                .embed_token_bf16_words(token_id)
                .unwrap()
                .into_iter()
                .map(|word| (word as u32) << 16)
                .collect::<Vec<_>>();
            println!(
                "token_id={} embed_fnv1a64=0x{:016X}",
                token_id,
                fnv1a64_u32_words(&bits)
            );
            println!(
                "token_id={} embed_first16_f32_bits={}",
                token_id,
                bits.iter()
                    .take(16)
                    .map(|bits| format!("0x{bits:08X}"))
                    .collect::<Vec<_>>()
                    .join(",")
            );
        }
    }

    #[test]
    fn reads_local_tensor_payload_words() {
        let header = MlxSafetensorsHeader::load(local_model_shard_1()).unwrap();

        let q_proj_weight = header
            .read_u32_tensor_words("language_model.model.layers.0.self_attn.q_proj.weight")
            .unwrap();
        assert_eq!(
            &q_proj_weight[..8],
            &[
                2_259_126_473,
                1_283_001_501,
                2_291_701_430,
                1_970_953_151,
                1_283_929_482,
                2_027_333_543,
                934_918_473,
                3_033_893_010,
            ]
        );

        let q_proj_scales = header
            .read_bf16_tensor_words("language_model.model.layers.0.self_attn.q_proj.scales")
            .unwrap();
        assert_eq!(
            &q_proj_scales[..16],
            &[
                15_321, 48_110, 48_135, 48_112, 15_290, 48_057, 15_308, 15_307, 15_254, 15_260,
                15_397, 15_275, 15_300, 15_297, 48_099, 15_299,
            ]
        );
    }

    #[test]
    fn dequantizes_one_local_q_proj_row_matches_mlx_oracle() {
        let header = MlxSafetensorsHeader::load(local_model_shard_1()).unwrap();
        let row = header
            .affine_dequantize_row_f32(
                "language_model.model.layers.0.self_attn.q_proj.weight",
                "language_model.model.layers.0.self_attn.q_proj.scales",
                "language_model.model.layers.0.self_attn.q_proj.biases",
                0,
                64,
                4,
            )
            .unwrap();
        assert_eq!(row.len(), 2_816);
        assert_eq!(
            fnv1a64_u32_words(&row.iter().map(|value| value.to_bits()).collect::<Vec<_>>()),
            0x2D44_4223_7EE7_C10F
        );
        assert_eq!(
            &row[..16]
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>(),
            &[
                0x3BD9_0000,
                0x3CD9_0000,
                0x0000_0000,
                0x0000_0000,
                0xBBD9_0000,
                0x3C59_0000,
                0xBC59_0000,
                0x0000_0000,
                0x3D08_0000,
                0x3BD9_0000,
                0x3CD9_0000,
                0xBD59_0000,
                0x3BD9_0000,
                0xBBD9_0000,
                0x3CD9_0000,
                0xBCD9_0000,
            ]
        );
    }

    #[test]
    fn quantized_matmul_one_local_q_proj_case_matches_mlx_oracle() {
        let header = MlxSafetensorsHeader::load(local_model_shard_1()).unwrap();
        let x = gemma4_qproj_case_input_bf16_words(GEMMA4_QPROJ_CASE_INNER_DIM);
        assert_eq!(
            x[..16]
                .iter()
                .copied()
                .map(bf16_word_to_f32)
                .map(f32::to_bits)
                .collect::<Vec<_>>(),
            vec![
                0xBF80_0000,
                0xBF40_0000,
                0xBF00_0000,
                0xBE80_0000,
                0x0000_0000,
                0x3E80_0000,
                0x3F00_0000,
                0x3F40_0000,
                0x3F80_0000,
                0x3F00_0000,
                0x0000_0000,
                0xBF00_0000,
                0xBF80_0000,
                0x3E00_0000,
                0x3EC0_0000,
                0x3F20_0000,
            ]
        );

        let out = header
            .affine_quantized_matmul_t_f32(
                &x,
                "language_model.model.layers.0.self_attn.q_proj.weight",
                "language_model.model.layers.0.self_attn.q_proj.scales",
                "language_model.model.layers.0.self_attn.q_proj.biases",
                64,
                4,
            )
            .unwrap();
        assert_eq!(out.len(), GEMMA4_QPROJ_CASE_OUTPUT_DIM);
        let out_bits = out.iter().map(|value| value.to_bits()).collect::<Vec<_>>();
        assert_eq!(
            &out_bits[..16],
            &[
                0xBF59_0000,
                0x4029_0000,
                0x3F3B_0000,
                0xBF63_0000,
                0x3DAF_0000,
                0xBF51_0000,
                0xBF49_0000,
                0x3FCE_0000,
                0x3D8B_0000,
                0xBEB9_0000,
                0x3F0F_0000,
                0x3E8E_0000,
                0x3DF2_0000,
                0x3E80_0000,
                0x3E89_0000,
                0xC022_0000,
            ]
        );
        assert_eq!(
            fnv1a64_u32_words(&out_bits),
            GEMMA4_QPROJ_CASE_OUTPUT_FNV1A64
        );
    }

    #[test]
    fn rms_norm_one_local_layer0_input_case_matches_mlx_gpu_oracle() {
        let header = MlxSafetensorsHeader::load(local_model_shard_1()).unwrap();
        let x = gemma4_qproj_case_input_bf16_words(2_816);
        let out = header
            .rms_norm_weighted_f32(
                &x,
                "language_model.model.layers.0.input_layernorm.weight",
                1e-6,
            )
            .unwrap();
        assert_eq!(out.len(), 2_816);
        let out_bits = out.iter().map(|value| value.to_bits()).collect::<Vec<_>>();
        assert_eq!(
            &out_bits[..16],
            &[
                0xC0A2_0000,
                0xC080_0000,
                0xC033_0000,
                0xBFBC_0000,
                0x0000_0000,
                0x3FB6_0000,
                0x402F_0000,
                0x40E3_0000,
                0x4126_0000,
                0x4041_0000,
                0x0000_0000,
                0xC048_0000,
                0xC11C_0000,
                0x3F6C_0000,
                0x4081_0000,
                0x40A4_0000,
            ]
        );
        assert_eq!(fnv1a64_u32_words(&out_bits), 0xBF5E_A05B_53DF_E923);
    }
}
