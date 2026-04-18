#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MlxQwen35MoeLayerKind {
    Attention,
    Recurrent,
}

impl MlxQwen35MoeLayerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Attention => "full_attention",
            Self::Recurrent => "linear_attention",
        }
    }
}

#[derive(Clone, Debug)]
pub struct MlxQwen35MoeConfig {
    pub architectures: Vec<String>,
    pub image_token_id: u32,
    pub model_type: String,
    pub quantization: Option<MlxQuantizationConfig>,
    pub tensor_quantization: HashMap<String, MlxQuantizationConfig>,
    pub text_config: MlxQwen35MoeTextConfig,
    pub tie_word_embeddings: bool,
    pub transformers_version: Option<String>,
    pub video_token_id: u32,
    pub vision_config: Option<MlxQwen35MoeVisionConfig>,
    pub vision_end_token_id: u32,
    pub vision_start_token_id: u32,
}

impl MlxQwen35MoeConfig {
    pub fn quantization_for_tensor(&self, actual_name: &str) -> Option<&MlxQuantizationConfig> {
        self.tensor_quantization
            .get(actual_name)
            .or_else(|| {
                [".weight", ".scales", ".biases"]
                    .iter()
                    .find_map(|suffix| actual_name.strip_suffix(suffix))
                    .and_then(|stem| self.tensor_quantization.get(stem))
            })
            .or(self.quantization.as_ref())
    }
}

#[derive(Clone, Debug, DeJson)]
struct MlxQwen35MoeConfigJson {
    pub architectures: Vec<String>,
    pub image_token_id: u32,
    pub model_type: String,
    pub quantization: Option<MlxQuantizationConfig>,
    pub text_config: MlxQwen35MoeTextConfig,
    pub tie_word_embeddings: bool,
    pub transformers_version: Option<String>,
    pub video_token_id: u32,
    pub vision_config: Option<MlxQwen35MoeVisionConfig>,
    pub vision_end_token_id: u32,
    pub vision_start_token_id: u32,
}

#[derive(Clone, Debug, DeJson)]
pub struct MlxQwen35MoeTextConfig {
    pub attention_bias: bool,
    pub attention_dropout: f32,
    pub attn_output_gate: bool,
    pub bos_token_id: u32,
    pub dtype: String,
    pub eos_token_id: u32,
    pub full_attention_interval: u32,
    pub head_dim: u32,
    pub hidden_act: String,
    pub hidden_size: u32,
    pub initializer_range: f32,
    pub layer_types: Vec<String>,
    pub linear_conv_kernel_dim: u32,
    pub linear_key_head_dim: u32,
    pub linear_num_key_heads: u32,
    pub linear_num_value_heads: u32,
    pub linear_value_head_dim: u32,
    pub mamba_ssm_dtype: String,
    pub max_position_embeddings: u32,
    pub model_type: String,
    pub moe_intermediate_size: u32,
    pub mtp_num_hidden_layers: u32,
    pub mtp_use_dedicated_embeddings: bool,
    pub num_attention_heads: u32,
    pub num_experts: u32,
    pub num_experts_per_tok: u32,
    pub num_hidden_layers: u32,
    pub num_key_value_heads: u32,
    pub output_router_logits: bool,
    pub pad_token_id: Option<u32>,
    pub partial_rotary_factor: f32,
    pub rms_norm_eps: f32,
    pub rope_parameters: MlxQwen35MoeRopeParameters,
    pub router_aux_loss_coef: f32,
    pub shared_expert_intermediate_size: u32,
    pub tie_word_embeddings: bool,
    pub use_cache: bool,
    pub vocab_size: u32,
}

#[derive(Clone, Debug, DeJson)]
pub struct MlxQwen35MoeRopeParameters {
    pub mrope_interleaved: bool,
    pub mrope_section: Vec<u32>,
    pub partial_rotary_factor: f32,
    pub rope_theta: f32,
    #[rename(type)]
    pub rope_type: String,
}

#[derive(Clone, Debug, DeJson)]
pub struct MlxQwen35MoeVisionConfig {
    pub deepstack_visual_indexes: Vec<u32>,
    pub depth: u32,
    pub hidden_act: String,
    pub hidden_size: u32,
    pub in_channels: u32,
    pub initializer_range: f32,
    pub intermediate_size: u32,
    pub model_type: String,
    pub num_heads: u32,
    pub num_position_embeddings: u32,
    pub out_hidden_size: u32,
    pub patch_size: u32,
    pub spatial_merge_size: u32,
    pub temporal_patch_size: u32,
}

#[derive(Clone, Debug, DeJson)]
pub struct MlxQwen35MoeProcessorConfig {
    pub size: MlxQwen35MoeProcessorSize,
    pub patch_size: u32,
    pub temporal_patch_size: u32,
    pub merge_size: u32,
    pub image_mean: Vec<f32>,
    pub image_std: Vec<f32>,
    pub processor_class: String,
    pub image_processor_type: String,
}

#[derive(Clone, Debug, DeJson)]
pub struct MlxQwen35MoeProcessorSize {
    pub longest_edge: u64,
    pub shortest_edge: u64,
}

#[derive(Clone, Debug)]
pub struct MlxQwen35MoeSnapshot {
    pub manifest: MlxModelManifest,
    pub config: MlxQwen35MoeConfig,
    pub generation_config: MlxGenerationConfig,
    pub processor_config: Option<MlxQwen35MoeProcessorConfig>,
}

impl MlxQwen35MoeSnapshot {
    pub fn load(root_dir: impl AsRef<Path>) -> Result<Self> {
        let manifest = MlxModelManifest::load(root_dir)?;
        if manifest.family != MlxModelFamily::Qwen35Moe {
            return Err(MlxRtError::InvalidModelDir {
                path: manifest.paths.root_dir.clone(),
                message: format!(
                    "model family {} is not supported by the Qwen3.5-MoE runtime",
                    manifest.family.as_str()
                ),
            });
        }
        let config = load_qwen35moe_config(&manifest.paths.config_json)?;
        let generation_config =
            load_json::<MlxGenerationConfig>(&manifest.paths.generation_config_json)?;
        let processor_config = manifest
            .paths
            .processor_config_json
            .as_ref()
            .map(|path| load_json::<MlxQwen35MoeProcessorConfig>(path.as_path()))
            .transpose()?;
        let snapshot = Self {
            manifest,
            config,
            generation_config,
            processor_config,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn paths(&self) -> &MlxModelPaths {
        &self.manifest.paths
    }

    pub fn tokenizer_config(&self) -> &MlxTokenizerConfig {
        &self.manifest.tokenizer_config
    }

    pub fn weight_index(&self) -> &MlxWeightIndex {
        &self.manifest.weight_index
    }

    pub fn quantization_for_tensor(&self, actual_name: &str) -> Option<&MlxQuantizationConfig> {
        self.config.quantization_for_tensor(actual_name)
    }

    pub fn unique_weight_shards(&self) -> Vec<String> {
        let mut shards = BTreeSet::new();
        for shard in self.weight_index().weight_map.values() {
            shards.insert(shard.clone());
        }
        shards.into_iter().collect()
    }

    pub fn layer_kind(&self, index: u32) -> Result<MlxQwen35MoeLayerKind> {
        qwen35moe_layer_kind(index, self.config.text_config.full_attention_interval)
    }

    pub fn canonical_weight_map(&self) -> Result<HashMap<String, String>> {
        let weight_map = &self.weight_index().weight_map;
        let index_path = &self.paths().model_safetensors_index_json;
        let mut aliases = HashMap::new();

        let add_alias = |aliases: &mut HashMap<String, String>, alias: String, actual: &str| {
            if let Some(previous) = aliases.insert(alias.clone(), actual.to_owned()) {
                if previous != actual {
                    return Err(MlxRtError::InvalidModelDir {
                        path: index_path.clone(),
                        message: format!(
                            "canonical tensor alias {} conflicts: {} vs {}",
                            alias, previous, actual
                        ),
                    });
                }
            }
            Ok(())
        };

        let add_required_alias_candidates = |aliases: &mut HashMap<String, String>,
                                             alias: String,
                                             actuals: &[String]|
         -> Result<()> {
            for actual in actuals {
                if weight_map.contains_key(actual) {
                    return add_alias(aliases, alias, actual);
                }
            }
            Err(MlxRtError::InvalidModelDir {
                path: index_path.clone(),
                message: format!(
                    "missing required tensor alias {} (tried {})",
                    alias,
                    actuals.join(", ")
                ),
            })
        };

        let add_optional_alias_candidates = |aliases: &mut HashMap<String, String>,
                                             alias: String,
                                             actuals: &[String]|
         -> Result<()> {
            for actual in actuals {
                if weight_map.contains_key(actual) {
                    return add_alias(aliases, alias, actual);
                }
            }
            Ok(())
        };

        add_required_alias_candidates(
            &mut aliases,
            "token_embd.weight".to_string(),
            &[
                "language_model.model.embed_tokens.weight".to_string(),
                "model.language_model.embed_tokens.weight".to_string(),
            ],
        )?;
        add_required_alias_candidates(
            &mut aliases,
            "output_norm.weight".to_string(),
            &[
                "language_model.model.norm.weight".to_string(),
                "model.language_model.norm.weight".to_string(),
            ],
        )?;
        if weight_map.contains_key("language_model.lm_head.weight")
            || weight_map.contains_key("lm_head.weight")
        {
            add_required_alias_candidates(
                &mut aliases,
                "output.weight".to_string(),
                &[
                    "language_model.lm_head.weight".to_string(),
                    "lm_head.weight".to_string(),
                ],
            )?;
        } else {
            add_required_alias_candidates(
                &mut aliases,
                "output.weight".to_string(),
                &[
                    "language_model.model.embed_tokens.weight".to_string(),
                    "model.language_model.embed_tokens.weight".to_string(),
                ],
            )?;
        }

        for layer_index in 0..self.config.text_config.num_hidden_layers {
            let prefixes = [
                format!("language_model.model.layers.{layer_index}"),
                format!("model.language_model.layers.{layer_index}"),
            ];
            add_required_alias_candidates(
                &mut aliases,
                format!("blk.{layer_index}.attn_norm.weight"),
                &prefixes
                    .iter()
                    .map(|prefix| format!("{prefix}.input_layernorm.weight"))
                    .collect::<Vec<_>>(),
            )?;
            add_required_alias_candidates(
                &mut aliases,
                format!("blk.{layer_index}.post_attention_norm.weight"),
                &prefixes
                    .iter()
                    .map(|prefix| format!("{prefix}.post_attention_layernorm.weight"))
                    .collect::<Vec<_>>(),
            )?;
            add_required_alias_candidates(
                &mut aliases,
                format!("blk.{layer_index}.ffn_gate_inp.weight"),
                &prefixes
                    .iter()
                    .map(|prefix| format!("{prefix}.mlp.gate.weight"))
                    .collect::<Vec<_>>(),
            )?;
            add_optional_alias_candidates(
                &mut aliases,
                format!("blk.{layer_index}.ffn_gate_up_exps.weight"),
                &prefixes
                    .iter()
                    .map(|prefix| format!("{prefix}.mlp.experts.gate_up_proj"))
                    .collect::<Vec<_>>(),
            )?;
            add_optional_alias_candidates(
                &mut aliases,
                format!("blk.{layer_index}.ffn_gate_exps.weight"),
                &[
                    format!(
                        "language_model.model.layers.{layer_index}.mlp.switch_mlp.gate_proj.weight"
                    ),
                    format!("model.language_model.layers.{layer_index}.mlp.experts.gate_proj"),
                ],
            )?;
            add_optional_alias_candidates(
                &mut aliases,
                format!("blk.{layer_index}.ffn_up_exps.weight"),
                &[
                    format!(
                        "language_model.model.layers.{layer_index}.mlp.switch_mlp.up_proj.weight"
                    ),
                    format!("model.language_model.layers.{layer_index}.mlp.experts.up_proj"),
                ],
            )?;
            add_required_alias_candidates(
                &mut aliases,
                format!("blk.{layer_index}.ffn_down_exps.weight"),
                &[
                    format!(
                        "language_model.model.layers.{layer_index}.mlp.switch_mlp.down_proj.weight"
                    ),
                    format!("model.language_model.layers.{layer_index}.mlp.experts.down_proj"),
                ],
            )?;
            add_required_alias_candidates(
                &mut aliases,
                format!("blk.{layer_index}.ffn_gate_inp_shexp.weight"),
                &prefixes
                    .iter()
                    .map(|prefix| format!("{prefix}.mlp.shared_expert_gate.weight"))
                    .collect::<Vec<_>>(),
            )?;
            add_required_alias_candidates(
                &mut aliases,
                format!("blk.{layer_index}.ffn_gate_shexp.weight"),
                &prefixes
                    .iter()
                    .map(|prefix| format!("{prefix}.mlp.shared_expert.gate_proj.weight"))
                    .collect::<Vec<_>>(),
            )?;
            add_required_alias_candidates(
                &mut aliases,
                format!("blk.{layer_index}.ffn_up_shexp.weight"),
                &prefixes
                    .iter()
                    .map(|prefix| format!("{prefix}.mlp.shared_expert.up_proj.weight"))
                    .collect::<Vec<_>>(),
            )?;
            add_required_alias_candidates(
                &mut aliases,
                format!("blk.{layer_index}.ffn_down_shexp.weight"),
                &prefixes
                    .iter()
                    .map(|prefix| format!("{prefix}.mlp.shared_expert.down_proj.weight"))
                    .collect::<Vec<_>>(),
            )?;

            match self.layer_kind(layer_index)? {
                MlxQwen35MoeLayerKind::Attention => {
                    add_required_alias_candidates(
                        &mut aliases,
                        format!("blk.{layer_index}.attn_q.weight"),
                        &prefixes
                            .iter()
                            .map(|prefix| format!("{prefix}.self_attn.q_proj.weight"))
                            .collect::<Vec<_>>(),
                    )?;
                    add_required_alias_candidates(
                        &mut aliases,
                        format!("blk.{layer_index}.attn_k.weight"),
                        &prefixes
                            .iter()
                            .map(|prefix| format!("{prefix}.self_attn.k_proj.weight"))
                            .collect::<Vec<_>>(),
                    )?;
                    add_required_alias_candidates(
                        &mut aliases,
                        format!("blk.{layer_index}.attn_v.weight"),
                        &prefixes
                            .iter()
                            .map(|prefix| format!("{prefix}.self_attn.v_proj.weight"))
                            .collect::<Vec<_>>(),
                    )?;
                    add_required_alias_candidates(
                        &mut aliases,
                        format!("blk.{layer_index}.attn_output.weight"),
                        &prefixes
                            .iter()
                            .map(|prefix| format!("{prefix}.self_attn.o_proj.weight"))
                            .collect::<Vec<_>>(),
                    )?;
                    add_required_alias_candidates(
                        &mut aliases,
                        format!("blk.{layer_index}.attn_q_norm.weight"),
                        &prefixes
                            .iter()
                            .map(|prefix| format!("{prefix}.self_attn.q_norm.weight"))
                            .collect::<Vec<_>>(),
                    )?;
                    add_required_alias_candidates(
                        &mut aliases,
                        format!("blk.{layer_index}.attn_k_norm.weight"),
                        &prefixes
                            .iter()
                            .map(|prefix| format!("{prefix}.self_attn.k_norm.weight"))
                            .collect::<Vec<_>>(),
                    )?;
                }
                MlxQwen35MoeLayerKind::Recurrent => {
                    add_required_alias_candidates(
                        &mut aliases,
                        format!("blk.{layer_index}.attn_qkv.weight"),
                        &prefixes
                            .iter()
                            .map(|prefix| format!("{prefix}.linear_attn.in_proj_qkv.weight"))
                            .collect::<Vec<_>>(),
                    )?;
                    add_required_alias_candidates(
                        &mut aliases,
                        format!("blk.{layer_index}.attn_gate.weight"),
                        &prefixes
                            .iter()
                            .map(|prefix| format!("{prefix}.linear_attn.in_proj_z.weight"))
                            .collect::<Vec<_>>(),
                    )?;
                    add_required_alias_candidates(
                        &mut aliases,
                        format!("blk.{layer_index}.ssm_conv1d.weight"),
                        &prefixes
                            .iter()
                            .map(|prefix| format!("{prefix}.linear_attn.conv1d.weight"))
                            .collect::<Vec<_>>(),
                    )?;
                    add_required_alias_candidates(
                        &mut aliases,
                        format!("blk.{layer_index}.ssm_dt.bias"),
                        &prefixes
                            .iter()
                            .map(|prefix| format!("{prefix}.linear_attn.dt_bias"))
                            .collect::<Vec<_>>(),
                    )?;
                    add_required_alias_candidates(
                        &mut aliases,
                        format!("blk.{layer_index}.ssm_a"),
                        &prefixes
                            .iter()
                            .map(|prefix| format!("{prefix}.linear_attn.A_log"))
                            .collect::<Vec<_>>(),
                    )?;
                    add_required_alias_candidates(
                        &mut aliases,
                        format!("blk.{layer_index}.ssm_beta.weight"),
                        &prefixes
                            .iter()
                            .map(|prefix| format!("{prefix}.linear_attn.in_proj_b.weight"))
                            .collect::<Vec<_>>(),
                    )?;
                    add_required_alias_candidates(
                        &mut aliases,
                        format!("blk.{layer_index}.ssm_alpha.weight"),
                        &prefixes
                            .iter()
                            .map(|prefix| format!("{prefix}.linear_attn.in_proj_a.weight"))
                            .collect::<Vec<_>>(),
                    )?;
                    add_required_alias_candidates(
                        &mut aliases,
                        format!("blk.{layer_index}.ssm_norm.weight"),
                        &prefixes
                            .iter()
                            .map(|prefix| format!("{prefix}.linear_attn.norm.weight"))
                            .collect::<Vec<_>>(),
                    )?;
                    add_required_alias_candidates(
                        &mut aliases,
                        format!("blk.{layer_index}.ssm_out.weight"),
                        &prefixes
                            .iter()
                            .map(|prefix| format!("{prefix}.linear_attn.out_proj.weight"))
                            .collect::<Vec<_>>(),
                    )?;
                }
            }
        }

        if let Some(vision_config) = &self.config.vision_config {
            add_optional_alias_candidates(
                &mut aliases,
                "visual.patch_embed.weight".to_string(),
                &[
                    "vision_tower.patch_embed.proj.weight".to_string(),
                    "model.visual.patch_embed.proj.weight".to_string(),
                ],
            )?;
            add_optional_alias_candidates(
                &mut aliases,
                "visual.patch_embed.bias".to_string(),
                &[
                    "vision_tower.patch_embed.proj.bias".to_string(),
                    "model.visual.patch_embed.proj.bias".to_string(),
                ],
            )?;
            add_optional_alias_candidates(
                &mut aliases,
                "visual.pos_embed.weight".to_string(),
                &[
                    "vision_tower.pos_embed.weight".to_string(),
                    "model.visual.pos_embed.weight".to_string(),
                ],
            )?;
            for block_index in 0..vision_config.depth {
                let prefixes = [
                    format!("vision_tower.blocks.{block_index}"),
                    format!("model.visual.blocks.{block_index}"),
                ];
                add_optional_alias_candidates(
                    &mut aliases,
                    format!("visual.blk.{block_index}.attn_norm.weight"),
                    &prefixes
                        .iter()
                        .map(|prefix| format!("{prefix}.norm1.weight"))
                        .collect::<Vec<_>>(),
                )?;
                add_optional_alias_candidates(
                    &mut aliases,
                    format!("visual.blk.{block_index}.attn_norm.bias"),
                    &prefixes
                        .iter()
                        .map(|prefix| format!("{prefix}.norm1.bias"))
                        .collect::<Vec<_>>(),
                )?;
                add_optional_alias_candidates(
                    &mut aliases,
                    format!("visual.blk.{block_index}.attn_qkv.weight"),
                    &prefixes
                        .iter()
                        .map(|prefix| format!("{prefix}.attn.qkv.weight"))
                        .collect::<Vec<_>>(),
                )?;
                add_optional_alias_candidates(
                    &mut aliases,
                    format!("visual.blk.{block_index}.attn_qkv.bias"),
                    &prefixes
                        .iter()
                        .map(|prefix| format!("{prefix}.attn.qkv.bias"))
                        .collect::<Vec<_>>(),
                )?;
                add_optional_alias_candidates(
                    &mut aliases,
                    format!("visual.blk.{block_index}.attn_output.weight"),
                    &prefixes
                        .iter()
                        .map(|prefix| format!("{prefix}.attn.proj.weight"))
                        .collect::<Vec<_>>(),
                )?;
                add_optional_alias_candidates(
                    &mut aliases,
                    format!("visual.blk.{block_index}.attn_output.bias"),
                    &prefixes
                        .iter()
                        .map(|prefix| format!("{prefix}.attn.proj.bias"))
                        .collect::<Vec<_>>(),
                )?;
                add_optional_alias_candidates(
                    &mut aliases,
                    format!("visual.blk.{block_index}.post_attention_norm.weight"),
                    &prefixes
                        .iter()
                        .map(|prefix| format!("{prefix}.norm2.weight"))
                        .collect::<Vec<_>>(),
                )?;
                add_optional_alias_candidates(
                    &mut aliases,
                    format!("visual.blk.{block_index}.post_attention_norm.bias"),
                    &prefixes
                        .iter()
                        .map(|prefix| format!("{prefix}.norm2.bias"))
                        .collect::<Vec<_>>(),
                )?;
                add_optional_alias_candidates(
                    &mut aliases,
                    format!("visual.blk.{block_index}.mlp_up.weight"),
                    &prefixes
                        .iter()
                        .map(|prefix| format!("{prefix}.mlp.linear_fc1.weight"))
                        .collect::<Vec<_>>(),
                )?;
                add_optional_alias_candidates(
                    &mut aliases,
                    format!("visual.blk.{block_index}.mlp_up.bias"),
                    &prefixes
                        .iter()
                        .map(|prefix| format!("{prefix}.mlp.linear_fc1.bias"))
                        .collect::<Vec<_>>(),
                )?;
                add_optional_alias_candidates(
                    &mut aliases,
                    format!("visual.blk.{block_index}.mlp_down.weight"),
                    &prefixes
                        .iter()
                        .map(|prefix| format!("{prefix}.mlp.linear_fc2.weight"))
                        .collect::<Vec<_>>(),
                )?;
                add_optional_alias_candidates(
                    &mut aliases,
                    format!("visual.blk.{block_index}.mlp_down.bias"),
                    &prefixes
                        .iter()
                        .map(|prefix| format!("{prefix}.mlp.linear_fc2.bias"))
                        .collect::<Vec<_>>(),
                )?;
            }
            add_optional_alias_candidates(
                &mut aliases,
                "visual.merger.norm.weight".to_string(),
                &[
                    "vision_tower.merger.norm.weight".to_string(),
                    "model.visual.merger.norm.weight".to_string(),
                ],
            )?;
            add_optional_alias_candidates(
                &mut aliases,
                "visual.merger.norm.bias".to_string(),
                &[
                    "vision_tower.merger.norm.bias".to_string(),
                    "model.visual.merger.norm.bias".to_string(),
                ],
            )?;
            add_optional_alias_candidates(
                &mut aliases,
                "visual.merger.fc1.weight".to_string(),
                &[
                    "vision_tower.merger.linear_fc1.weight".to_string(),
                    "model.visual.merger.linear_fc1.weight".to_string(),
                ],
            )?;
            add_optional_alias_candidates(
                &mut aliases,
                "visual.merger.fc1.bias".to_string(),
                &[
                    "vision_tower.merger.linear_fc1.bias".to_string(),
                    "model.visual.merger.linear_fc1.bias".to_string(),
                ],
            )?;
            add_optional_alias_candidates(
                &mut aliases,
                "visual.merger.fc2.weight".to_string(),
                &[
                    "vision_tower.merger.linear_fc2.weight".to_string(),
                    "model.visual.merger.linear_fc2.weight".to_string(),
                ],
            )?;
            add_optional_alias_candidates(
                &mut aliases,
                "visual.merger.fc2.bias".to_string(),
                &[
                    "vision_tower.merger.linear_fc2.bias".to_string(),
                    "model.visual.merger.linear_fc2.bias".to_string(),
                ],
            )?;
        }
        Ok(aliases)
    }

    fn validate(&self) -> Result<()> {
        if self.config.model_type != "qwen3_5_moe" {
            return Err(MlxRtError::InvalidModelDir {
                path: self.paths().root_dir.clone(),
                message: format!("unexpected model_type {}", self.config.model_type),
            });
        }
        if self.config.text_config.model_type != "qwen3_5_moe_text" {
            return Err(MlxRtError::InvalidModelDir {
                path: self.paths().config_json.clone(),
                message: format!(
                    "unexpected text_config.model_type {}",
                    self.config.text_config.model_type
                ),
            });
        }
        if self.config.text_config.num_hidden_layers == 0 {
            return Err(MlxRtError::InvalidModelDir {
                path: self.paths().root_dir.clone(),
                message: "expected at least one text layer".to_string(),
            });
        }
        if self.config.text_config.layer_types.len()
            != self.config.text_config.num_hidden_layers as usize
        {
            return Err(MlxRtError::InvalidModelDir {
                path: self.paths().config_json.clone(),
                message: format!(
                    "layer_types length {} does not match num_hidden_layers {}",
                    self.config.text_config.layer_types.len(),
                    self.config.text_config.num_hidden_layers,
                ),
            });
        }
        for layer_index in 0..self.config.text_config.num_hidden_layers {
            let expected = self.layer_kind(layer_index)?.as_str();
            let actual = self.config.text_config.layer_types[layer_index as usize].as_str();
            if actual != expected {
                return Err(MlxRtError::InvalidModelDir {
                    path: self.paths().config_json.clone(),
                    message: format!(
                        "layer_types[{}] expected {} from full_attention_interval {}, got {}",
                        layer_index,
                        expected,
                        self.config.text_config.full_attention_interval,
                        actual,
                    ),
                });
            }
        }
        for shard_name in self.unique_weight_shards() {
            let shard_path = self.paths().root_dir.join(&shard_name);
            if !shard_path.is_file() {
                return Err(MlxRtError::MissingFile { path: shard_path });
            }
        }

        let weight_map = &self.weight_index().weight_map;
        let index_path = &self.paths().model_safetensors_index_json;
        require_weight_key_candidates(
            weight_map,
            index_path,
            &[
                "language_model.model.embed_tokens.weight",
                "model.language_model.embed_tokens.weight",
            ],
        )?;
        require_weight_key_candidates(
            weight_map,
            index_path,
            &[
                "language_model.model.norm.weight",
                "model.language_model.norm.weight",
            ],
        )?;
        if !self.config.tie_word_embeddings {
            require_weight_key_candidates(
                weight_map,
                index_path,
                &["language_model.lm_head.weight", "lm_head.weight"],
            )?;
        }

        let first_attention = (0..self.config.text_config.num_hidden_layers)
            .find(|&index| self.layer_kind(index).ok() == Some(MlxQwen35MoeLayerKind::Attention))
            .ok_or_else(|| MlxRtError::InvalidModelDir {
                path: self.paths().config_json.clone(),
                message: "expected at least one full attention layer".to_string(),
            })?;
        let first_recurrent = (0..self.config.text_config.num_hidden_layers)
            .find(|&index| self.layer_kind(index).ok() == Some(MlxQwen35MoeLayerKind::Recurrent))
            .ok_or_else(|| MlxRtError::InvalidModelDir {
                path: self.paths().config_json.clone(),
                message: "expected at least one recurrent layer".to_string(),
            })?;

        require_weight_key_candidates(
            weight_map,
            index_path,
            &[
                &format!("language_model.model.layers.{first_recurrent}.input_layernorm.weight"),
                &format!("model.language_model.layers.{first_recurrent}.input_layernorm.weight"),
            ],
        )?;
        for suffix in [
            "linear_attn.in_proj_qkv.weight",
            "linear_attn.in_proj_z.weight",
            "linear_attn.conv1d.weight",
            "linear_attn.dt_bias",
            "linear_attn.A_log",
            "linear_attn.in_proj_a.weight",
            "linear_attn.in_proj_b.weight",
            "linear_attn.norm.weight",
            "linear_attn.out_proj.weight",
        ] {
            require_weight_key_candidates(
                weight_map,
                index_path,
                &[
                    &format!("language_model.model.layers.{first_recurrent}.{suffix}"),
                    &format!("model.language_model.layers.{first_recurrent}.{suffix}"),
                ],
            )?;
        }
        for suffix in [
            "self_attn.q_proj.weight",
            "self_attn.k_proj.weight",
            "self_attn.v_proj.weight",
            "self_attn.o_proj.weight",
            "self_attn.q_norm.weight",
            "self_attn.k_norm.weight",
        ] {
            require_weight_key_candidates(
                weight_map,
                index_path,
                &[
                    &format!("language_model.model.layers.{first_attention}.{suffix}"),
                    &format!("model.language_model.layers.{first_attention}.{suffix}"),
                ],
            )?;
        }

        if self.config.vision_config.is_some() {
            require_weight_key_candidates(
                weight_map,
                index_path,
                &[
                    "vision_tower.patch_embed.proj.weight",
                    "model.visual.patch_embed.proj.weight",
                ],
            )?;
            require_weight_key_candidates(
                weight_map,
                index_path,
                &[
                    "vision_tower.merger.linear_fc1.weight",
                    "model.visual.merger.linear_fc1.weight",
                ],
            )?;
        }

        let _ = self.canonical_weight_map()?;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub enum MlxFamilySnapshot {
    Gemma4(MlxModelSnapshot),
    Qwen35Moe(MlxQwen35MoeSnapshot),
}

impl MlxFamilySnapshot {
    pub fn load(root_dir: impl AsRef<Path>) -> Result<Self> {
        let root_dir = root_dir.as_ref();
        match MlxModelManifest::load(root_dir)?.family {
            MlxModelFamily::Gemma4 => Ok(Self::Gemma4(MlxModelSnapshot::load(root_dir)?)),
            MlxModelFamily::Qwen35Moe => Ok(Self::Qwen35Moe(MlxQwen35MoeSnapshot::load(root_dir)?)),
        }
    }

    pub fn family(&self) -> MlxModelFamily {
        match self {
            Self::Gemma4(_) => MlxModelFamily::Gemma4,
            Self::Qwen35Moe(_) => MlxModelFamily::Qwen35Moe,
        }
    }
}

#[derive(Clone, Debug)]
pub struct MlxQwen35MoeIndexedSafetensors {
    pub snapshot: MlxQwen35MoeSnapshot,
    pub shard_headers: HashMap<String, MlxSafetensorsHeader>,
    canonical_weight_map: HashMap<String, String>,
    bf16_tensor_cache: Arc<Mutex<HashMap<String, Arc<Vec<u16>>>>>,
    u32_tensor_cache: Arc<Mutex<HashMap<String, Arc<Vec<u32>>>>>,
}

impl MlxQwen35MoeIndexedSafetensors {
    fn invalid_model_error(&self, message: impl Into<String>) -> MlxRtError {
        MlxRtError::InvalidModelDir {
            path: self.snapshot.paths().root_dir.clone(),
            message: message.into(),
        }
    }

    pub fn load(root_dir: impl AsRef<Path>) -> Result<Self> {
        let snapshot = MlxQwen35MoeSnapshot::load(root_dir)?;
        let canonical_weight_map = snapshot.canonical_weight_map()?;
        let mut shard_headers = HashMap::new();
        for shard_name in snapshot.unique_weight_shards() {
            let shard_path = snapshot.paths().root_dir.join(&shard_name);
            let header = MlxSafetensorsHeader::load(&shard_path)?;
            shard_headers.insert(shard_name, header);
        }
        Ok(Self {
            snapshot,
            shard_headers,
            canonical_weight_map,
            bf16_tensor_cache: Arc::new(Mutex::new(HashMap::new())),
            u32_tensor_cache: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn actual_tensor_name<'a>(&'a self, name: &'a str) -> Result<&'a str> {
        if self.snapshot.weight_index().weight_map.contains_key(name) {
            return Ok(name);
        }
        self.canonical_weight_map
            .get(name)
            .map(String::as_str)
            .ok_or_else(|| MlxRtError::InvalidModelDir {
                path: self.snapshot.paths().model_safetensors_index_json.clone(),
                message: format!("tensor {} missing from Qwen3.5-MoE weight index", name),
            })
    }

    pub fn shard_name_for_tensor<'a>(&'a self, name: &'a str) -> Result<&'a str> {
        let actual_name = self.actual_tensor_name(name)?;
        self.snapshot
            .weight_index()
            .weight_map
            .get(actual_name)
            .map(String::as_str)
            .ok_or_else(|| MlxRtError::InvalidModelDir {
                path: self.snapshot.paths().model_safetensors_index_json.clone(),
                message: format!("tensor {} missing from weight index", actual_name),
            })
    }

    pub fn header_for_tensor<'a>(&'a self, name: &'a str) -> Result<&'a MlxSafetensorsHeader> {
        let shard_name = self.shard_name_for_tensor(name)?;
        self.shard_headers
            .get(shard_name)
            .ok_or_else(|| MlxRtError::MissingFile {
                path: self.snapshot.paths().root_dir.join(shard_name),
            })
    }

    pub fn tensor<'a>(&'a self, name: &'a str) -> Result<&'a MlxTensorEntry> {
        let actual_name = self.actual_tensor_name(name)?;
        let header = self.header_for_tensor(name)?;
        header
            .tensor(actual_name)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: header.path.clone(),
                message: format!("tensor {} not found in shard header", actual_name),
            })
    }

    pub fn read_tensor_bytes(&self, name: &str) -> Result<Vec<u8>> {
        let actual_name = self.actual_tensor_name(name)?;
        self.header_for_tensor(name)?.read_tensor_bytes(actual_name)
    }

    pub fn read_bf16_tensor_words(&self, name: &str) -> Result<Vec<u16>> {
        let actual_name = self.actual_tensor_name(name)?;
        self.header_for_tensor(name)?
            .read_bf16_tensor_words(actual_name)
    }

    pub fn read_bf16_tensor_words_cached(&self, name: &str) -> Result<Arc<Vec<u16>>> {
        let actual_name = self.actual_tensor_name(name)?.to_owned();
        {
            let cache = self
                .bf16_tensor_cache
                .lock()
                .map_err(|_| self.invalid_model_error("bf16 tensor cache mutex poisoned"))?;
            if let Some(words) = cache.get(&actual_name) {
                return Ok(words.clone());
            }
        }

        let words = Arc::new(self.read_bf16_tensor_words(&actual_name)?);
        let mut cache = self
            .bf16_tensor_cache
            .lock()
            .map_err(|_| self.invalid_model_error("bf16 tensor cache mutex poisoned"))?;
        Ok(cache
            .entry(actual_name)
            .or_insert_with(|| words.clone())
            .clone())
    }

    pub fn read_u32_tensor_words_cached(&self, name: &str) -> Result<Arc<Vec<u32>>> {
        let actual_name = self.actual_tensor_name(name)?.to_owned();
        {
            let cache = self
                .u32_tensor_cache
                .lock()
                .map_err(|_| self.invalid_model_error("u32 tensor cache mutex poisoned"))?;
            if let Some(words) = cache.get(&actual_name) {
                return Ok(words.clone());
            }
        }

        let words = Arc::new(
            self.header_for_tensor(&actual_name)?
                .read_u32_tensor_words(&actual_name)?,
        );
        let mut cache = self
            .u32_tensor_cache
            .lock()
            .map_err(|_| self.invalid_model_error("u32 tensor cache mutex poisoned"))?;
        Ok(cache
            .entry(actual_name)
            .or_insert_with(|| words.clone())
            .clone())
    }

    pub fn quantization_for_tensor(&self, name: &str) -> Result<Option<&MlxQuantizationConfig>> {
        let actual_name = self.actual_tensor_name(name)?;
        Ok(self.snapshot.quantization_for_tensor(actual_name))
    }
}

#[derive(Clone, Debug)]
pub struct MlxQwen35MoeGlobalTensors {
    pub token_embd: String,
    pub output_norm: String,
    pub output: String,
}

#[derive(Clone, Debug)]
pub struct MlxQwen35MoeAttentionTensors {
    pub wq: String,
    pub wk: String,
    pub wv: String,
    pub wo: String,
    pub attn_q_norm: String,
    pub attn_k_norm: String,
}

#[derive(Clone, Debug)]
pub struct MlxQwen35MoeRecurrentTensors {
    pub wqkv: String,
    pub wqkv_gate: String,
    pub ssm_conv1d: String,
    pub ssm_dt: String,
    pub ssm_a: String,
    pub ssm_beta: String,
    pub ssm_alpha: String,
    pub ssm_norm: String,
    pub ssm_out: String,
}

#[derive(Clone, Debug)]
pub struct MlxQwen35MoeMoeTensors {
    pub ffn_gate_inp: String,
    pub ffn_gate_up_exps: Option<String>,
    pub ffn_gate_exps: Option<String>,
    pub ffn_up_exps: Option<String>,
    pub ffn_down_exps: String,
    pub ffn_gate_inp_shexp: String,
    pub ffn_gate_shexp: String,
    pub ffn_up_shexp: String,
    pub ffn_down_shexp: String,
}

impl MlxQwen35MoeMoeTensors {
    pub fn uses_merged_gate_up(&self) -> bool {
        self.ffn_gate_up_exps.is_some()
    }
}

#[derive(Clone, Debug)]
pub struct MlxQwen35MoeLayerTensors {
    pub index: u32,
    pub kind: MlxQwen35MoeLayerKind,
    pub attn_norm: String,
    pub post_attention_norm: String,
    pub attention: Option<MlxQwen35MoeAttentionTensors>,
    pub recurrent: Option<MlxQwen35MoeRecurrentTensors>,
    pub moe: MlxQwen35MoeMoeTensors,
}

#[derive(Clone, Debug)]
pub struct MlxQwen35MoeTensors {
    pub globals: MlxQwen35MoeGlobalTensors,
    pub layers: Vec<MlxQwen35MoeLayerTensors>,
}

impl MlxQwen35MoeTensors {
    pub fn from_indexed(indexed: &MlxQwen35MoeIndexedSafetensors) -> Result<Self> {
        let snapshot = &indexed.snapshot;
        let token_embd = qwen35moe_required_tensor(indexed, "token_embd.weight")?;
        let output_norm = qwen35moe_required_tensor(indexed, "output_norm.weight")?;
        let output = qwen35moe_required_tensor(indexed, "output.weight")?;

        let mut layers = Vec::with_capacity(snapshot.config.text_config.num_hidden_layers as usize);
        for index in 0..snapshot.config.text_config.num_hidden_layers {
            let kind = snapshot.layer_kind(index)?;
            let attn_norm = qwen35moe_required_tensor(
                indexed,
                &qwen35moe_layer_name(index, "attn_norm", "weight"),
            )?;
            let post_attention_norm = qwen35moe_required_tensor(
                indexed,
                &qwen35moe_layer_name(index, "post_attention_norm", "weight"),
            )?;

            let attention = match kind {
                MlxQwen35MoeLayerKind::Attention => Some(MlxQwen35MoeAttentionTensors {
                    wq: qwen35moe_required_tensor(
                        indexed,
                        &qwen35moe_layer_name(index, "attn_q", "weight"),
                    )?,
                    wk: qwen35moe_required_tensor(
                        indexed,
                        &qwen35moe_layer_name(index, "attn_k", "weight"),
                    )?,
                    wv: qwen35moe_required_tensor(
                        indexed,
                        &qwen35moe_layer_name(index, "attn_v", "weight"),
                    )?,
                    wo: qwen35moe_required_tensor(
                        indexed,
                        &qwen35moe_layer_name(index, "attn_output", "weight"),
                    )?,
                    attn_q_norm: qwen35moe_required_tensor(
                        indexed,
                        &qwen35moe_layer_name(index, "attn_q_norm", "weight"),
                    )?,
                    attn_k_norm: qwen35moe_required_tensor(
                        indexed,
                        &qwen35moe_layer_name(index, "attn_k_norm", "weight"),
                    )?,
                }),
                MlxQwen35MoeLayerKind::Recurrent => None,
            };

            let recurrent = match kind {
                MlxQwen35MoeLayerKind::Attention => None,
                MlxQwen35MoeLayerKind::Recurrent => Some(MlxQwen35MoeRecurrentTensors {
                    wqkv: qwen35moe_required_tensor(
                        indexed,
                        &qwen35moe_layer_name(index, "attn_qkv", "weight"),
                    )?,
                    wqkv_gate: qwen35moe_required_tensor(
                        indexed,
                        &qwen35moe_layer_name(index, "attn_gate", "weight"),
                    )?,
                    ssm_conv1d: qwen35moe_required_tensor(
                        indexed,
                        &qwen35moe_layer_name(index, "ssm_conv1d", "weight"),
                    )?,
                    ssm_dt: qwen35moe_required_tensor(
                        indexed,
                        &qwen35moe_layer_name(index, "ssm_dt", "bias"),
                    )?,
                    ssm_a: qwen35moe_required_tensor(
                        indexed,
                        &qwen35moe_layer_scalar_name(index, "ssm_a"),
                    )?,
                    ssm_beta: qwen35moe_required_tensor(
                        indexed,
                        &qwen35moe_layer_name(index, "ssm_beta", "weight"),
                    )?,
                    ssm_alpha: qwen35moe_required_tensor(
                        indexed,
                        &qwen35moe_layer_name(index, "ssm_alpha", "weight"),
                    )?,
                    ssm_norm: qwen35moe_required_tensor(
                        indexed,
                        &qwen35moe_layer_name(index, "ssm_norm", "weight"),
                    )?,
                    ssm_out: qwen35moe_required_tensor(
                        indexed,
                        &qwen35moe_layer_name(index, "ssm_out", "weight"),
                    )?,
                }),
            };

            let ffn_gate_up_exps = qwen35moe_optional_tensor(
                indexed,
                &qwen35moe_layer_name(index, "ffn_gate_up_exps", "weight"),
            );
            let ffn_gate_exps = qwen35moe_optional_tensor(
                indexed,
                &qwen35moe_layer_name(index, "ffn_gate_exps", "weight"),
            );
            let ffn_up_exps = qwen35moe_optional_tensor(
                indexed,
                &qwen35moe_layer_name(index, "ffn_up_exps", "weight"),
            );
            if ffn_gate_up_exps.is_none() && (ffn_gate_exps.is_none() || ffn_up_exps.is_none()) {
                return Err(MlxRtError::InvalidModelDir {
                    path: snapshot.paths().model_safetensors_index_json.clone(),
                    message: format!(
                        "layer {} is missing expert gate/up weights: expected either blk.{}.ffn_gate_up_exps.weight or both blk.{}.ffn_gate_exps.weight and blk.{}.ffn_up_exps.weight",
                        index, index, index, index
                    ),
                });
            }

            layers.push(MlxQwen35MoeLayerTensors {
                index,
                kind,
                attn_norm,
                post_attention_norm,
                attention,
                recurrent,
                moe: MlxQwen35MoeMoeTensors {
                    ffn_gate_inp: qwen35moe_required_tensor(
                        indexed,
                        &qwen35moe_layer_name(index, "ffn_gate_inp", "weight"),
                    )?,
                    ffn_gate_up_exps,
                    ffn_gate_exps,
                    ffn_up_exps,
                    ffn_down_exps: qwen35moe_required_tensor(
                        indexed,
                        &qwen35moe_layer_name(index, "ffn_down_exps", "weight"),
                    )?,
                    ffn_gate_inp_shexp: qwen35moe_required_tensor(
                        indexed,
                        &qwen35moe_layer_name(index, "ffn_gate_inp_shexp", "weight"),
                    )?,
                    ffn_gate_shexp: qwen35moe_required_tensor(
                        indexed,
                        &qwen35moe_layer_name(index, "ffn_gate_shexp", "weight"),
                    )?,
                    ffn_up_shexp: qwen35moe_required_tensor(
                        indexed,
                        &qwen35moe_layer_name(index, "ffn_up_shexp", "weight"),
                    )?,
                    ffn_down_shexp: qwen35moe_required_tensor(
                        indexed,
                        &qwen35moe_layer_name(index, "ffn_down_shexp", "weight"),
                    )?,
                },
            });
        }

        Ok(Self {
            globals: MlxQwen35MoeGlobalTensors {
                token_embd,
                output_norm,
                output,
            },
            layers,
        })
    }
}

fn qwen35moe_layer_kind(index: u32, full_attention_interval: u32) -> Result<MlxQwen35MoeLayerKind> {
    if full_attention_interval == 0 {
        return Err(MlxRtError::InvalidModelDir {
            path: PathBuf::new(),
            message: "qwen3_5_moe full_attention_interval must be greater than zero".to_string(),
        });
    }
    if (index + 1) % full_attention_interval == 0 {
        Ok(MlxQwen35MoeLayerKind::Attention)
    } else {
        Ok(MlxQwen35MoeLayerKind::Recurrent)
    }
}

fn require_weight_key_candidates(
    weight_map: &HashMap<String, String>,
    index_path: &Path,
    keys: &[&str],
) -> Result<()> {
    for key in keys {
        if weight_map.contains_key(*key) {
            return Ok(());
        }
    }
    Err(MlxRtError::InvalidModelDir {
        path: index_path.to_path_buf(),
        message: format!("missing required tensor key (tried {})", keys.join(", ")),
    })
}

fn load_qwen35moe_config(path: &Path) -> Result<MlxQwen35MoeConfig> {
    let text = fs::read_to_string(path).map_err(|err| MlxRtError::Io {
        path: path.to_path_buf(),
        message: err.to_string(),
    })?;
    let mut root =
        HashMap::<String, JsonValue>::deserialize_json(&text).map_err(|err| MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{:?}", err),
        })?;
    root.remove("quantization_config");
    let tensor_quantization =
        extract_qwen35moe_tensor_quantization(path, root.get("quantization"))?;

    if let Some(JsonValue::Object(text_config)) = root.get_mut("text_config") {
        if let Some(JsonValue::Object(rope_parameters)) = text_config.get_mut("rope_parameters") {
            if !rope_parameters.contains_key("type") {
                if let Some(rope_type) = rope_parameters.remove("rope_type") {
                    rope_parameters.insert("type".to_string(), rope_type);
                }
            }
        }
    }

    if let Some(JsonValue::Object(quantization)) = root.get_mut("quantization") {
        let bits = quantization.get("bits").cloned();
        let group_size = quantization.get("group_size").cloned();
        let mode = quantization.get("mode").cloned();
        quantization.clear();
        if let Some(bits) = bits {
            quantization.insert("bits".to_string(), bits);
        }
        if let Some(group_size) = group_size {
            quantization.insert("group_size".to_string(), group_size);
        }
        if let Some(mode) = mode {
            quantization.insert("mode".to_string(), mode);
        }
    }

    let parsed =
        MlxQwen35MoeConfigJson::deserialize_json(&root.serialize_json()).map_err(|err| {
            MlxRtError::Json {
                path: path.to_path_buf(),
                message: format!("{:?}", err),
            }
        })?;
    Ok(MlxQwen35MoeConfig {
        architectures: parsed.architectures,
        image_token_id: parsed.image_token_id,
        model_type: parsed.model_type,
        quantization: parsed.quantization,
        tensor_quantization,
        text_config: parsed.text_config,
        tie_word_embeddings: parsed.tie_word_embeddings,
        transformers_version: parsed.transformers_version,
        video_token_id: parsed.video_token_id,
        vision_config: parsed.vision_config,
        vision_end_token_id: parsed.vision_end_token_id,
        vision_start_token_id: parsed.vision_start_token_id,
    })
}

fn extract_qwen35moe_tensor_quantization(
    path: &Path,
    value: Option<&JsonValue>,
) -> Result<HashMap<String, MlxQuantizationConfig>> {
    let Some(JsonValue::Object(quantization)) = value else {
        return Ok(HashMap::new());
    };
    let mut out = HashMap::new();
    for (key, entry) in quantization {
        if matches!(key.as_str(), "bits" | "group_size" | "mode") {
            continue;
        }
        let JsonValue::Object(object) = entry else {
            continue;
        };
        let bits = object
            .get("bits")
            .cloned()
            .ok_or_else(|| MlxRtError::Json {
                path: path.to_path_buf(),
                message: format!("quantization override {} missing bits", key),
            })?;
        let group_size = object
            .get("group_size")
            .cloned()
            .ok_or_else(|| MlxRtError::Json {
                path: path.to_path_buf(),
                message: format!("quantization override {} missing group_size", key),
            })?;
        let mode = object
            .get("mode")
            .cloned()
            .ok_or_else(|| MlxRtError::Json {
                path: path.to_path_buf(),
                message: format!("quantization override {} missing mode", key),
            })?;
        let mut normalized = HashMap::new();
        normalized.insert("bits".to_string(), bits);
        normalized.insert("group_size".to_string(), group_size);
        normalized.insert("mode".to_string(), mode);
        let config = MlxQuantizationConfig::deserialize_json(&normalized.serialize_json())
            .map_err(|err| MlxRtError::Json {
                path: path.to_path_buf(),
                message: format!("invalid quantization override {}: {:?}", key, err),
            })?;
        out.insert(key.clone(), config);
    }
    Ok(out)
}

fn qwen35moe_required_tensor(
    indexed: &MlxQwen35MoeIndexedSafetensors,
    name: &str,
) -> Result<String> {
    indexed.tensor(name)?;
    Ok(name.to_owned())
}

fn qwen35moe_optional_tensor(
    indexed: &MlxQwen35MoeIndexedSafetensors,
    name: &str,
) -> Option<String> {
    indexed.tensor(name).ok()?;
    Some(name.to_owned())
}

fn qwen35moe_layer_name(index: u32, stem: &str, suffix: &str) -> String {
    format!("blk.{index}.{stem}.{suffix}")
}

fn qwen35moe_layer_scalar_name(index: u32, stem: &str) -> String {
    format!("blk.{index}.{stem}")
}
