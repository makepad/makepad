pub type Result<T> = std::result::Result<T, MlxRtError>;

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
        let embed_weight_entry =
            header
                .tensor(EMBED_TOKENS_WEIGHT_NAME)
                .ok_or_else(|| MlxRtError::InvalidSafetensors {
                    path: header.path.clone(),
                    message: format!("tensor {} not found in header", EMBED_TOKENS_WEIGHT_NAME),
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
