pub type Result<T> = std::result::Result<T, MlxRtError>;

use makepad_ggml::backend::{
    try_get_rows_ggml_bytes_cached, try_matmul_nt_ggml_bytes_cached_bf16_words,
};
use makepad_ggml::quant::{
    bf16_to_f32, get_rows_ggml_bytes_cpu, vec_dot_nvfp4_f32, GGML_TYPE_NVFP4,
};

pub struct MlxRouterTopKOutput {
    pub router_scaled: Vec<f32>,
    pub expert_scores: Vec<f32>,
    pub router_probs: Vec<f32>,
    pub top_k_indices: Vec<u32>,
    pub top_k_weights: Vec<f32>,
}

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
    pub processor_config_json: Option<PathBuf>,
    pub tokenizer_json: PathBuf,
    pub tokenizer_config_json: PathBuf,
    pub model_safetensors_index_json: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MlxModelFamily {
    Gemma4,
    Qwen35Moe,
}

impl MlxModelFamily {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gemma4 => "gemma4",
            Self::Qwen35Moe => "qwen3_5_moe",
        }
    }

    fn from_model_type(model_type: &str) -> Option<Self> {
        match model_type {
            "gemma4" => Some(Self::Gemma4),
            "qwen3_5_moe" => Some(Self::Qwen35Moe),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct MlxModelManifest {
    pub paths: MlxModelPaths,
    pub family: MlxModelFamily,
    pub tokenizer_config: MlxTokenizerConfig,
    pub weight_index: MlxWeightIndex,
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

        let processor_config_json = {
            let processor_config_json = root_dir.join("processor_config.json");
            if processor_config_json.is_file() {
                Some(processor_config_json)
            } else {
                let preprocessor_config_json = root_dir.join("preprocessor_config.json");
                preprocessor_config_json
                    .is_file()
                    .then_some(preprocessor_config_json)
            }
        };

        let paths = Self {
            config_json: root_dir.join("config.json"),
            generation_config_json: root_dir.join("generation_config.json"),
            processor_config_json,
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
            &self.tokenizer_json,
            &self.tokenizer_config_json,
            &self.model_safetensors_index_json,
        ] {
            if !path.is_file() {
                return Err(MlxRtError::MissingFile { path: path.clone() });
            }
        }
        if let Some(path) = &self.processor_config_json {
            if !path.is_file() {
                return Err(MlxRtError::MissingFile { path: path.clone() });
            }
        }
        Ok(())
    }
}

impl MlxModelManifest {
    pub fn load(root_dir: impl AsRef<Path>) -> Result<Self> {
        let paths = MlxModelPaths::from_dir(root_dir)?;
        let family = detect_model_family(&paths.config_json)?;
        let tokenizer_config = MlxTokenizerConfig::load(&paths.tokenizer_config_json)?;
        let weight_index = load_json::<MlxWeightIndex>(&paths.model_safetensors_index_json)?;
        Ok(Self {
            paths,
            family,
            tokenizer_config,
            weight_index,
        })
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
        let manifest = MlxModelManifest::load(root_dir)?;
        if manifest.family != MlxModelFamily::Gemma4 {
            return Err(MlxRtError::InvalidModelDir {
                path: manifest.paths.root_dir.clone(),
                message: format!(
                    "model family {} is not supported by the Gemma runtime; load it through the model-family front door instead",
                    manifest.family.as_str()
                ),
            });
        }
        let paths = manifest.paths;
        let config = load_json::<MlxModelConfig>(&paths.config_json)?;
        let generation_config = load_json::<MlxGenerationConfig>(&paths.generation_config_json)?;
        let processor_config_path =
            paths
                .processor_config_json
                .clone()
                .ok_or_else(|| MlxRtError::MissingFile {
                    path: paths.root_dir.join("processor_config.json"),
                })?;
        let processor_config = load_json::<MlxProcessorConfig>(&processor_config_path)?;
        let snapshot = Self {
            paths,
            config,
            generation_config,
            processor_config,
            tokenizer_config: manifest.tokenizer_config,
            weight_index: manifest.weight_index,
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
        let bits = self.config.quantization.bits;
        match self.config.quantization.mode.as_str() {
            "affine"
                if bits != 0
                    && bits <= 8
                    && (bits & (bits - 1)) == 0
                    && self.config.quantization.group_size == 64 => {}
            "nvfp4" if bits == 4 && self.config.quantization.group_size == 16 => {}
            _ => {
                return Err(MlxRtError::InvalidModelDir {
                    path: self.paths.root_dir.clone(),
                    message: format!(
                        "expected affine <=8-bit group_size=64 or nvfp4 4-bit group_size=16, got bits={} group_size={} mode={}",
                        self.config.quantization.bits,
                        self.config.quantization.group_size,
                        self.config.quantization.mode,
                    ),
                });
            }
        }
        if self.config.text_config.num_hidden_layers == 0 {
            return Err(MlxRtError::InvalidModelDir {
                path: self.paths.root_dir.clone(),
                message: "expected at least one text layer".to_string(),
            });
        }
        if self.config.text_config.layer_types.len()
            != self.config.text_config.num_hidden_layers as usize
        {
            return Err(MlxRtError::InvalidModelDir {
                path: self.paths.root_dir.clone(),
                message: format!(
                    "layer_types length {} does not match num_hidden_layers {}",
                    self.config.text_config.layer_types.len(),
                    self.config.text_config.num_hidden_layers,
                ),
            });
        }
        if self.config.vision_config.num_hidden_layers == 0 {
            return Err(MlxRtError::InvalidModelDir {
                path: self.paths.root_dir.clone(),
                message: "expected at least one vision layer".to_string(),
            });
        }
        for shard_name in self.unique_weight_shards() {
            let shard_path = self.paths.root_dir.join(&shard_name);
            if !shard_path.is_file() {
                return Err(MlxRtError::MissingFile { path: shard_path });
            }
        }
        let last_text_layer_idx = self
            .config
            .text_config
            .num_hidden_layers
            .checked_sub(1)
            .ok_or_else(|| MlxRtError::InvalidModelDir {
                path: self.paths.root_dir.clone(),
                message: "expected at least one text layer".to_string(),
            })?;
        for required_weight in [
            "language_model.model.embed_tokens.weight",
            "language_model.model.layers.0.self_attn.q_proj.weight",
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
        let last_layer_o_proj =
            format!("language_model.model.layers.{last_text_layer_idx}.self_attn.o_proj.weight");
        if !self
            .weight_index
            .weight_map
            .contains_key(last_layer_o_proj.as_str())
        {
            return Err(MlxRtError::InvalidModelDir {
                path: self.paths.model_safetensors_index_json.clone(),
                message: format!("missing required tensor key {}", last_layer_o_proj),
            });
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

fn detect_model_family(config_json: &Path) -> Result<MlxModelFamily> {
    let text = fs::read_to_string(config_json).map_err(|err| MlxRtError::Io {
        path: config_json.to_path_buf(),
        message: err.to_string(),
    })?;
    let root =
        HashMap::<String, JsonValue>::deserialize_json(&text).map_err(|err| MlxRtError::Json {
            path: config_json.to_path_buf(),
            message: format!("{:?}", err),
        })?;
    let model_type = root
        .get("model_type")
        .and_then(tokenizer_json_string_opt)
        .ok_or_else(|| MlxRtError::InvalidModelDir {
            path: config_json.to_path_buf(),
            message: "config.json is missing string field model_type".to_string(),
        })?;
    MlxModelFamily::from_model_type(model_type).ok_or_else(|| MlxRtError::InvalidModelDir {
        path: config_json.to_path_buf(),
        message: format!("unsupported model_type {}", model_type),
    })
}

fn tokenizer_json_string_opt(value: &JsonValue) -> Option<&str> {
    match value {
        JsonValue::String(text) => Some(text.as_str()),
        _ => None,
    }
}

fn tokenizer_json_token_string_opt(value: &JsonValue) -> Option<String> {
    match value {
        JsonValue::String(text) => Some(text.clone()),
        JsonValue::Object(object) => object
            .get("content")
            .and_then(tokenizer_json_string_opt)
            .map(str::to_owned),
        JsonValue::Null | JsonValue::Undefined => None,
        _ => None,
    }
}

fn tokenizer_string_array_opt(value: &JsonValue) -> Option<Vec<String>> {
    let array = match value {
        JsonValue::Array(array) => array,
        _ => return None,
    };
    let mut out = Vec::with_capacity(array.len());
    for item in array {
        out.push(tokenizer_json_string_opt(item)?.to_owned());
    }
    Some(out)
}

fn tokenizer_extra_special_token_map(value: &JsonValue) -> Option<HashMap<String, String>> {
    let object = match value {
        JsonValue::Object(object) => object,
        _ => return None,
    };
    let mut out = HashMap::with_capacity(object.len());
    for (key, value) in object {
        out.insert(key.clone(), tokenizer_json_token_string_opt(value)?);
    }
    Some(out)
}

fn root_string_or(
    root: &HashMap<String, JsonValue>,
    key: &str,
    fallback: Option<String>,
) -> String {
    root.get(key)
        .and_then(tokenizer_json_string_opt)
        .map(str::to_owned)
        .or(fallback)
        .unwrap_or_default()
}

fn root_token_string_or(
    root: &HashMap<String, JsonValue>,
    key: &str,
    fallback: Option<String>,
) -> String {
    root.get(key)
        .and_then(tokenizer_json_token_string_opt)
        .or(fallback)
        .unwrap_or_default()
}

fn root_bool_or(root: &HashMap<String, JsonValue>, key: &str, default: bool) -> bool {
    root.get(key)
        .and_then(|value| match value {
            JsonValue::Bool(flag) => Some(*flag),
            _ => None,
        })
        .unwrap_or(default)
}

fn root_u128_or(root: &HashMap<String, JsonValue>, key: &str, default: u128) -> u128 {
    root.get(key)
        .and_then(|value| match value {
            JsonValue::U64(number) => Some(*number as u128),
            JsonValue::U128(number) => Some(*number),
            JsonValue::I64(number) => u128::try_from(*number).ok(),
            JsonValue::I128(number) => u128::try_from(*number).ok(),
            _ => None,
        })
        .unwrap_or(default)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MlxTokenIdList(pub Vec<u32>);

impl MlxTokenIdList {
    pub fn iter(&self) -> std::slice::Iter<'_, u32> {
        self.0.iter()
    }
}

impl DeJson for MlxTokenIdList {
    fn de_json(
        s: &mut DeJsonState,
        i: &mut std::str::Chars,
    ) -> std::result::Result<Self, DeJsonErr> {
        match s.tok {
            makepad_micro_serde::DeJsonTok::U64(value) => {
                s.next_tok(i)?;
                Ok(Self(vec![
                    u32::try_from(value).map_err(|_| s.err_msg("token id does not fit in u32"))?
                ]))
            }
            makepad_micro_serde::DeJsonTok::U128(value) => {
                s.next_tok(i)?;
                Ok(Self(vec![
                    u32::try_from(value).map_err(|_| s.err_msg("token id does not fit in u32"))?
                ]))
            }
            makepad_micro_serde::DeJsonTok::I64(value) => {
                s.next_tok(i)?;
                Ok(Self(vec![u32::try_from(value).map_err(|_| {
                    s.err_msg("token id must be a non-negative u32")
                })?]))
            }
            makepad_micro_serde::DeJsonTok::I128(value) => {
                s.next_tok(i)?;
                Ok(Self(vec![u32::try_from(value).map_err(|_| {
                    s.err_msg("token id must be a non-negative u32")
                })?]))
            }
            makepad_micro_serde::DeJsonTok::BlockOpen => Ok(Self(Vec::<u32>::de_json(s, i)?)),
            _ => Err(s.err_msg("expected token id or token id array")),
        }
    }
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
    pub eos_token_id: MlxTokenIdList,
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

#[derive(Clone, Debug, DeJson, PartialEq, Eq)]
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
    pub moe_intermediate_size: Option<u32>,
    pub expert_intermediate_size: Option<u32>,
    pub num_attention_heads: u32,
    pub num_experts: Option<u32>,
    pub num_global_key_value_heads: Option<u32>,
    pub num_hidden_layers: u32,
    pub num_key_value_heads: u32,
    pub num_kv_shared_layers: u32,
    pub pad_token_id: u32,
    pub rms_norm_eps: f32,
    pub rope_parameters: MlxTextRopeParameters,
    pub sliding_window: u32,
    pub tie_word_embeddings: bool,
    pub top_k_experts: Option<u32>,
    pub use_bidirectional_attention: Option<String>,
    pub use_cache: bool,
    pub use_double_wide_mlp: bool,
    pub vocab_size: u32,
    pub vocab_size_per_layer_input: u32,
}

impl MlxTextConfig {
    pub fn top_k_experts_or_zero(&self) -> u32 {
        self.top_k_experts.unwrap_or(0)
    }

    pub fn num_global_key_value_heads_or_default(&self) -> u32 {
        self.num_global_key_value_heads
            .unwrap_or(self.num_key_value_heads)
    }
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
    pub _name_or_path: Option<String>,
    pub architectures: Option<Vec<String>>,
    pub attention_bias: bool,
    pub attention_dropout: f32,
    pub chunk_size_feed_forward: Option<u32>,
    pub default_output_length: u32,
    pub dtype: String,
    pub global_head_dim: u32,
    pub head_dim: u32,
    pub hidden_activation: String,
    pub hidden_size: u32,
    pub id2label: Option<HashMap<String, String>>,
    pub initializer_range: Option<f32>,
    pub intermediate_size: u32,
    pub is_encoder_decoder: Option<bool>,
    pub label2id: Option<HashMap<String, u32>>,
    pub max_position_embeddings: u32,
    pub model_type: String,
    pub num_attention_heads: u32,
    pub num_hidden_layers: u32,
    pub num_key_value_heads: u32,
    pub output_attentions: Option<bool>,
    pub output_hidden_states: Option<bool>,
    pub patch_size: u32,
    pub pooling_kernel_size: u32,
    pub position_embedding_size: u32,
    pub problem_type: Option<String>,
    pub return_dict: Option<bool>,
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
    pub eos_token_id: MlxTokenIdList,
    pub pad_token_id: u32,
    pub temperature: f32,
    pub top_k: u32,
    pub top_p: f32,
    pub transformers_version: Option<String>,
}

#[derive(Clone, Debug, DeJson)]
pub struct MlxProcessorConfig {
    pub audio_seq_length: u32,
    pub image_processor: MlxImageProcessorConfig,
    pub image_seq_length: u32,
    pub processor_class: String,
    pub feature_extractor: Option<JsonValue>,
    pub audio_ms_per_token: Option<u32>,
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

#[derive(Clone, Debug, Default)]
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
    pub response_schema: Option<JsonValue>,
    pub soc_token: String,
    pub sot_token: String,
    pub stc_token: String,
    pub std_token: String,
    pub str_token: String,
    pub think_token: String,
    pub tokenizer_class: String,
    pub unk_token: String,
    pub chat_template: String,
    pub pretokenize_regex: String,
}

impl MlxTokenizerConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path).map_err(|err| MlxRtError::Io {
            path: path.to_path_buf(),
            message: err.to_string(),
        })?;
        let root = HashMap::<String, JsonValue>::deserialize_json(&text).map_err(|err| {
            MlxRtError::Json {
                path: path.to_path_buf(),
                message: format!("{:?}", err),
            }
        })?;

        let extra_special_tokens = root
            .get("extra_special_tokens")
            .and_then(tokenizer_extra_special_token_map)
            .unwrap_or_default();

        let mut extra_special_token_values =
            extra_special_tokens.values().cloned().collect::<Vec<_>>();
        if let Some(additional_special) = root
            .get("additional_special_tokens")
            .and_then(tokenizer_string_array_opt)
        {
            extra_special_token_values.extend(additional_special);
        }
        extra_special_token_values.sort();
        extra_special_token_values.dedup();

        Ok(Self {
            audio_token: root_string_or(
                &root,
                "audio_token",
                extra_special_tokens.get("audio_token").cloned(),
            ),
            backend: root_string_or(&root, "backend", None),
            boa_token: root_string_or(
                &root,
                "boa_token",
                extra_special_tokens.get("audio_bos_token").cloned(),
            ),
            boi_token: root_string_or(
                &root,
                "boi_token",
                extra_special_tokens.get("vision_bos_token").cloned(),
            ),
            bos_token: root_token_string_or(&root, "bos_token", None),
            eoa_token: root_string_or(
                &root,
                "eoa_token",
                extra_special_tokens.get("audio_eos_token").cloned(),
            ),
            eoc_token: root_string_or(&root, "eoc_token", None),
            eoi_token: root_string_or(
                &root,
                "eoi_token",
                extra_special_tokens.get("vision_eos_token").cloned(),
            ),
            eos_token: root_token_string_or(&root, "eos_token", None),
            eot_token: root_string_or(&root, "eot_token", None),
            escape_token: root_string_or(&root, "escape_token", None),
            etc_token: root_string_or(&root, "etc_token", None),
            etd_token: root_string_or(&root, "etd_token", None),
            etr_token: root_string_or(&root, "etr_token", None),
            extra_special_tokens: extra_special_token_values,
            image_token: root_string_or(
                &root,
                "image_token",
                extra_special_tokens.get("image_token").cloned(),
            ),
            is_local: root_bool_or(&root, "is_local", false),
            mask_token: root_token_string_or(&root, "mask_token", None),
            model_max_length: root_u128_or(&root, "model_max_length", 0),
            model_specific_special_tokens: root
                .get("model_specific_special_tokens")
                .and_then(tokenizer_extra_special_token_map)
                .unwrap_or_default(),
            pad_token: root_token_string_or(&root, "pad_token", None),
            padding_side: root_string_or(&root, "padding_side", Some("right".to_string())),
            processor_class: root_string_or(&root, "processor_class", None),
            response_schema: root.get("response_schema").cloned(),
            soc_token: root_string_or(&root, "soc_token", None),
            sot_token: root_string_or(&root, "sot_token", None),
            stc_token: root_string_or(&root, "stc_token", None),
            std_token: root_string_or(&root, "std_token", None),
            str_token: root_string_or(&root, "str_token", None),
            think_token: root_string_or(&root, "think_token", None),
            tokenizer_class: root_string_or(&root, "tokenizer_class", None),
            unk_token: root_token_string_or(&root, "unk_token", None),
            chat_template: root_string_or(&root, "chat_template", None),
            pretokenize_regex: root_string_or(&root, "pretokenize_regex", None),
        })
    }
}

#[derive(Clone, Debug, DeJson)]
pub struct MlxWeightIndex {
    pub metadata: MlxWeightIndexMetadata,
    pub weight_map: HashMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct MlxWeightIndexMetadata {
    pub total_size: u64,
}

impl DeJson for MlxWeightIndexMetadata {
    fn de_json(
        s: &mut DeJsonState,
        i: &mut std::str::Chars,
    ) -> std::result::Result<Self, DeJsonErr> {
        let root = HashMap::<String, JsonValue>::de_json(s, i)?;
        let total_size = match root.get("total_size") {
            Some(JsonValue::U64(number)) => *number,
            Some(JsonValue::U128(number)) => {
                u64::try_from(*number).map_err(|_| s.err_msg("total_size does not fit in u64"))?
            }
            Some(JsonValue::I64(number)) => u64::try_from(*number)
                .map_err(|_| s.err_msg("total_size must be a non-negative integer"))?,
            Some(JsonValue::I128(number)) => u64::try_from(*number)
                .map_err(|_| s.err_msg("total_size must be a non-negative integer"))?,
            Some(JsonValue::F64(number))
                if *number >= 0.0 && number.fract() == 0.0 && *number <= u64::MAX as f64 =>
            {
                *number as u64
            }
            Some(other) => {
                return Err(s.err_msg(&format!(
                    "total_size expected integer-compatible value, got {:?}",
                    other
                )))
            }
            None => return Err(s.err_msg("total_size missing from metadata")),
        };
        Ok(Self { total_size })
    }
}

#[derive(Clone, Debug)]
pub struct MlxIndexedSafetensors {
    pub snapshot: MlxModelSnapshot,
    pub shard_headers: HashMap<String, MlxSafetensorsHeader>,
    bf16_tensor_cache: Arc<Mutex<HashMap<String, Arc<Vec<u16>>>>>,
}

impl MlxIndexedSafetensors {
    fn invalid_model_error(&self, message: impl Into<String>) -> MlxRtError {
        MlxRtError::InvalidModelDir {
            path: self.snapshot.paths.root_dir.clone(),
            message: message.into(),
        }
    }

    fn invalid_safetensors_error(&self, path: PathBuf, message: impl Into<String>) -> MlxRtError {
        MlxRtError::InvalidSafetensors {
            path,
            message: message.into(),
        }
    }

    pub fn quantization_mode(&self) -> &str {
        self.snapshot.config.quantization.mode.as_str()
    }

    pub(crate) fn nvfp4_rank2_layout(
        &self,
        weight_name: &str,
        scales_name: &str,
    ) -> Result<(usize, usize, usize)> {
        let weight_entry = self.tensor(weight_name)?;
        let scales_entry = self.tensor(scales_name)?;
        if weight_entry.dtype != MlxDType::U32 {
            return Err(self.invalid_safetensors_error(
                self.header_for_tensor(weight_name)?.path.clone(),
                format!(
                    "tensor {weight_name} expected U32, got {:?}",
                    weight_entry.dtype
                ),
            ));
        }
        if scales_entry.dtype != MlxDType::U8 {
            return Err(self.invalid_safetensors_error(
                self.header_for_tensor(scales_name)?.path.clone(),
                format!(
                    "tensor {scales_name} expected U8, got {:?}",
                    scales_entry.dtype
                ),
            ));
        }
        if weight_entry.shape.len() != 2 || scales_entry.shape.len() != 2 {
            return Err(self.invalid_safetensors_error(
                self.header_for_tensor(weight_name)?.path.clone(),
                format!(
                    "NVFP4 rank-2 tensors expected for {weight_name}/{scales_name}, got {:?}/{:?}",
                    weight_entry.shape, scales_entry.shape
                ),
            ));
        }
        if weight_entry.shape[0] != scales_entry.shape[0] {
            return Err(self.invalid_safetensors_error(
                self.header_for_tensor(weight_name)?.path.clone(),
                format!(
                    "NVFP4 row count mismatch for {weight_name}/{scales_name}: {:?} vs {:?}",
                    weight_entry.shape, scales_entry.shape
                ),
            ));
        }
        let rows = weight_entry.shape[0] as usize;
        let blocks_per_row = scales_entry.shape[1] as usize;
        if blocks_per_row == 0 || blocks_per_row % 4 != 0 {
            return Err(self.invalid_safetensors_error(
                self.header_for_tensor(scales_name)?.path.clone(),
                format!(
                    "NVFP4 scales for {scales_name} must be non-zero and divisible by 4, got {}",
                    blocks_per_row
                ),
            ));
        }
        let weight_row_bytes = usize::try_from(
            weight_entry.shape[1]
                .checked_mul(weight_entry.dtype.byte_width())
                .ok_or_else(|| {
                    self.invalid_safetensors_error(
                        self.header_for_tensor(weight_name)
                            .map(|header| header.path.clone())
                            .unwrap_or_default(),
                        format!("NVFP4 weight row byte count overflow for {weight_name}"),
                    )
                })?,
        )
        .map_err(|_| {
            self.invalid_safetensors_error(
                self.header_for_tensor(weight_name)
                    .map(|header| header.path.clone())
                    .unwrap_or_default(),
                format!("NVFP4 weight row byte count does not fit usize for {weight_name}"),
            )
        })?;
        if weight_row_bytes != blocks_per_row * 8 {
            return Err(self.invalid_safetensors_error(
                self.header_for_tensor(weight_name)?.path.clone(),
                format!(
                    "NVFP4 packed weight row size mismatch for {weight_name}: got {} bytes expected {}",
                    weight_row_bytes,
                    blocks_per_row * 8
                ),
            ));
        }
        Ok((rows, blocks_per_row, blocks_per_row * 16))
    }

    fn repack_nvfp4_row_bytes(weight_row_bytes: &[u8], scale_row_bytes: &[u8]) -> Vec<u8> {
        let blocks_per_row = scale_row_bytes.len();
        let super_blocks = blocks_per_row / 4;
        let mut out = vec![0u8; super_blocks * 36];
        for super_block in 0..super_blocks {
            let out_base = super_block * 36;
            for sub in 0..4 {
                out[out_base + sub] = scale_row_bytes[super_block * 4 + sub] & 0x7f;
            }
            for sub in 0..4 {
                let src =
                    &weight_row_bytes[(super_block * 4 + sub) * 8..(super_block * 4 + sub + 1) * 8];
                let dst = &mut out[out_base + 4 + sub * 8..out_base + 4 + (sub + 1) * 8];
                for j in 0..4 {
                    let lo0 = src[j] & 0x0f;
                    let hi0 = src[j] >> 4;
                    let lo1 = src[j + 4] & 0x0f;
                    let hi1 = src[j + 4] >> 4;
                    dst[2 * j] = lo0 | (lo1 << 4);
                    dst[2 * j + 1] = hi0 | (hi1 << 4);
                }
            }
        }
        out
    }

    pub fn repack_nvfp4_row_to_ggml_bytes(
        &self,
        weight_name: &str,
        scales_name: &str,
        row: u64,
    ) -> Result<Vec<u8>> {
        let (rows, _, _) = self.nvfp4_rank2_layout(weight_name, scales_name)?;
        if row as usize >= rows {
            return Err(self.invalid_safetensors_error(
                self.header_for_tensor(weight_name)?.path.clone(),
                format!(
                    "NVFP4 row {} out of range for tensor {} with {} rows",
                    row, weight_name, rows
                ),
            ));
        }
        let weight_row_bytes = self
            .header_for_tensor(weight_name)?
            .read_rank2_row_bytes(weight_name, row)?;
        let scale_row_bytes = self
            .header_for_tensor(scales_name)?
            .read_rank2_row_bytes(scales_name, row)?;
        Ok(Self::repack_nvfp4_row_bytes(
            &weight_row_bytes,
            &scale_row_bytes,
        ))
    }

    pub fn repack_nvfp4_tensor_to_ggml_bytes(
        &self,
        weight_name: &str,
        scales_name: &str,
    ) -> Result<Vec<u8>> {
        let (rows, blocks_per_row, _) = self.nvfp4_rank2_layout(weight_name, scales_name)?;
        let row_bytes = (blocks_per_row / 4) * 36;
        let total_bytes = rows.checked_mul(row_bytes).ok_or_else(|| {
            self.invalid_model_error(format!("NVFP4 byte count overflow for {weight_name}"))
        })?;
        let mut out = Vec::with_capacity(total_bytes);
        let weight_header = self.header_for_tensor(weight_name)?;
        let scales_header = self.header_for_tensor(scales_name)?;
        for row in 0..rows {
            let weight_row_bytes = weight_header.read_rank2_row_bytes(weight_name, row as u64)?;
            let scale_row_bytes = scales_header.read_rank2_row_bytes(scales_name, row as u64)?;
            out.extend_from_slice(&Self::repack_nvfp4_row_bytes(
                &weight_row_bytes,
                &scale_row_bytes,
            ));
        }
        Ok(out)
    }

    pub fn repack_nvfp4_tensors_to_ggml_bytes(&self, tensors: &[(&str, &str)]) -> Result<Vec<u8>> {
        if tensors.is_empty() {
            return Ok(Vec::new());
        }

        let mut expected_inner_dim = None::<usize>;
        let mut total_bytes = 0usize;
        let mut layouts = Vec::with_capacity(tensors.len());
        for &(weight_name, scales_name) in tensors {
            let (rows, blocks_per_row, inner_dim) =
                self.nvfp4_rank2_layout(weight_name, scales_name)?;
            if let Some(expected) = expected_inner_dim {
                if inner_dim != expected {
                    return Err(self.invalid_model_error(format!(
                        "NVFP4 concatenation expects shared inner dim, got {inner_dim} for {weight_name} vs {expected}"
                    )));
                }
            } else {
                expected_inner_dim = Some(inner_dim);
            }
            let row_bytes = (blocks_per_row / 4) * 36;
            total_bytes = total_bytes
                .checked_add(rows.checked_mul(row_bytes).ok_or_else(|| {
                    self.invalid_model_error(format!(
                        "NVFP4 byte count overflow for concatenated tensor {weight_name}"
                    ))
                })?)
                .ok_or_else(|| {
                    self.invalid_model_error(format!(
                        "NVFP4 total byte count overflow while concatenating {weight_name}"
                    ))
                })?;
            layouts.push((weight_name, scales_name, rows));
        }

        let mut out = Vec::with_capacity(total_bytes);
        for (weight_name, scales_name, rows) in layouts {
            let weight_header = self.header_for_tensor(weight_name)?;
            let scales_header = self.header_for_tensor(scales_name)?;
            for row in 0..rows {
                let weight_row_bytes =
                    weight_header.read_rank2_row_bytes(weight_name, row as u64)?;
                let scale_row_bytes =
                    scales_header.read_rank2_row_bytes(scales_name, row as u64)?;
                out.extend_from_slice(&Self::repack_nvfp4_row_bytes(
                    &weight_row_bytes,
                    &scale_row_bytes,
                ));
            }
        }
        Ok(out)
    }

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
            bf16_tensor_cache: Arc::new(Mutex::new(HashMap::new())),
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

    pub fn read_bf16_tensor_words_cached(&self, name: &str) -> Result<Arc<Vec<u16>>> {
        {
            let cache = self
                .bf16_tensor_cache
                .lock()
                .map_err(|_| MlxRtError::InvalidModelDir {
                    path: self.snapshot.paths.root_dir.clone(),
                    message: "bf16 tensor cache mutex poisoned".to_string(),
                })?;
            if let Some(words) = cache.get(name) {
                return Ok(words.clone());
            }
        }

        let words = Arc::new(self.read_bf16_tensor_words(name)?);
        let mut cache = self
            .bf16_tensor_cache
            .lock()
            .map_err(|_| MlxRtError::InvalidModelDir {
                path: self.snapshot.paths.root_dir.clone(),
                message: "bf16 tensor cache mutex poisoned".to_string(),
            })?;
        Ok(cache
            .entry(name.to_owned())
            .or_insert_with(|| words.clone())
            .clone())
    }

    pub fn embed_token_f32(&self, token_id: u32) -> Result<Vec<f32>> {
        if self.quantization_mode() == "nvfp4" {
            let (rows, _, hidden) =
                self.nvfp4_rank2_layout(EMBED_TOKENS_WEIGHT_NAME, EMBED_TOKENS_SCALES_NAME)?;
            if token_id as usize >= rows {
                return Err(self.invalid_safetensors_error(
                    self.header_for_tensor(EMBED_TOKENS_WEIGHT_NAME)?
                        .path
                        .clone(),
                    format!("token id {} out of range for {} rows", token_id, rows),
                ));
            }
            let root = self.snapshot.paths.root_dir.to_string_lossy();
            let weight_key = format!("{root}:{EMBED_TOKENS_WEIGHT_NAME}");
            let row_indices = [token_id as i32];
            let mut embed = if let Some(result) = try_get_rows_ggml_bytes_cached(
                GGML_TYPE_NVFP4,
                hidden,
                rows,
                &row_indices,
                root.as_ref(),
                &weight_key,
                || {
                    self.repack_nvfp4_tensor_to_ggml_bytes(
                        EMBED_TOKENS_WEIGHT_NAME,
                        EMBED_TOKENS_SCALES_NAME,
                    )
                    .map_err(|err| err.to_string())
                },
            ) {
                result.map_err(|err| self.invalid_model_error(err))?
            } else {
                let row_bytes = self.repack_nvfp4_row_to_ggml_bytes(
                    EMBED_TOKENS_WEIGHT_NAME,
                    EMBED_TOKENS_SCALES_NAME,
                    token_id as u64,
                )?;
                get_rows_ggml_bytes_cpu(&row_bytes, GGML_TYPE_NVFP4, hidden, 1, &[0])
                    .ok_or_else(|| self.invalid_model_error("CPU NVFP4 get_rows fallback failed"))?
            };
            let embed_scale = bf16_round_to_f32((hidden as f32).sqrt());
            for value in &mut embed {
                *value = bf16_round_to_f32(*value * embed_scale);
            }
            return Ok(embed);
        }

        let header = self.header_for_tensor(EMBED_TOKENS_WEIGHT_NAME)?;
        let embed_weight_entry = header.tensor(EMBED_TOKENS_WEIGHT_NAME).ok_or_else(|| {
            MlxRtError::InvalidSafetensors {
                path: header.path.clone(),
                message: format!("tensor {} not found in header", EMBED_TOKENS_WEIGHT_NAME),
            }
        })?;
        let mut embed = header.affine_dequantize_row_f32(
            EMBED_TOKENS_WEIGHT_NAME,
            EMBED_TOKENS_SCALES_NAME,
            EMBED_TOKENS_BIASES_NAME,
            token_id as u64,
            self.snapshot.config.quantization.group_size as u64,
            self.snapshot.config.quantization.bits,
        )?;
        let embed_scale = bf16_round_to_f32((embed_weight_entry.shape[1] as f32).sqrt());
        for value in &mut embed {
            *value = bf16_round_to_f32(*value * embed_scale);
        }
        Ok(embed)
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
        if self.quantization_mode() == "nvfp4" {
            let logits = self.tied_text_logits_f32(hidden_bf16_words)?;
            let mut best = MlxGreedyToken {
                token_id: 0,
                logit: f32::NEG_INFINITY,
            };
            for (token_id, &logit) in logits.iter().enumerate() {
                let token_id = token_id as u32;
                if logit > best.logit || (logit == best.logit && token_id < best.token_id) {
                    best = MlxGreedyToken { token_id, logit };
                }
            }
            return Ok(best);
        }

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

    pub fn tied_text_logits_f32(&self, hidden_bf16_words: &[u16]) -> Result<Vec<f32>> {
        if self.quantization_mode() == "nvfp4" {
            let (rows, _, hidden) =
                self.nvfp4_rank2_layout(EMBED_TOKENS_WEIGHT_NAME, EMBED_TOKENS_SCALES_NAME)?;
            if hidden_bf16_words.len() != hidden {
                return Err(self.invalid_safetensors_error(
                    self.header_for_tensor(EMBED_TOKENS_WEIGHT_NAME)?
                        .path
                        .clone(),
                    format!(
                        "NVFP4 logits activation length mismatch: got {} expected {}",
                        hidden_bf16_words.len(),
                        hidden
                    ),
                ));
            }
            let root = self.snapshot.paths.root_dir.to_string_lossy();
            let weight_key = format!("{root}:{EMBED_TOKENS_WEIGHT_NAME}");
            let mut logits = if let Some(result) = try_matmul_nt_ggml_bytes_cached_bf16_words(
                hidden_bf16_words,
                GGML_TYPE_NVFP4,
                1,
                hidden_bf16_words.len(),
                rows,
                root.as_ref(),
                &weight_key,
                || {
                    self.repack_nvfp4_tensor_to_ggml_bytes(
                        EMBED_TOKENS_WEIGHT_NAME,
                        EMBED_TOKENS_SCALES_NAME,
                    )
                    .map_err(|err| err.to_string())
                },
            ) {
                result.map_err(|err| self.invalid_model_error(err))?
            } else {
                let hidden = hidden_bf16_words
                    .iter()
                    .copied()
                    .map(bf16_to_f32)
                    .collect::<Vec<_>>();
                let mut logits = Vec::with_capacity(rows);
                for row in 0..rows {
                    let row_bytes = self.repack_nvfp4_row_to_ggml_bytes(
                        EMBED_TOKENS_WEIGHT_NAME,
                        EMBED_TOKENS_SCALES_NAME,
                        row as u64,
                    )?;
                    let mut sum = 0.0f32;
                    for (block, input_block) in
                        row_bytes.chunks_exact(36).zip(hidden.chunks_exact(64))
                    {
                        sum += vec_dot_nvfp4_f32(block, input_block);
                    }
                    logits.push(sum);
                }
                logits
            };
            if let Some(softcap) = Some(self.snapshot.config.text_config.final_logit_softcapping)
                .filter(|softcap| *softcap > 0.0)
            {
                for logit in &mut logits {
                    *logit = bf16_round_to_f32((*logit / softcap).tanh() * softcap);
                }
            }
            return Ok(logits);
        }

        let header = self.header_for_tensor(EMBED_TOKENS_WEIGHT_NAME)?;
        let mut logits = header.affine_quantized_matmul_t_f32(
            hidden_bf16_words,
            EMBED_TOKENS_WEIGHT_NAME,
            EMBED_TOKENS_SCALES_NAME,
            EMBED_TOKENS_BIASES_NAME,
            self.snapshot.config.quantization.group_size as u64,
            self.snapshot.config.quantization.bits,
        )?;
        if let Some(softcap) = Some(self.snapshot.config.text_config.final_logit_softcapping)
            .filter(|softcap| *softcap > 0.0)
        {
            for logit in &mut logits {
                *logit = bf16_round_to_f32((*logit / softcap).tanh() * softcap);
            }
        }
        Ok(logits)
    }
}
