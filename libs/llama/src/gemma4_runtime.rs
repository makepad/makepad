use std::collections::BTreeMap;

use makepad_ggml::{TensorType, UnaryOp, GGML_ROPE_TYPE_NEOX};

use crate::error::{LlamaError, Result};
use crate::gemma4::{Gemma4LayerTensors, Gemma4Tensors};
use crate::model::LlamaModel;
use crate::plan::{
    ModelExecutionPlan, ModelLayerInventory, ModelLayerRole, ModelTailProbePlan,
    ModelTensorInventory,
};
use crate::runtime::{
    AttentionBlockSpec, AttentionDecodeSpec, AttentionKvCacheSpec, AttentionQueryLayout,
    AttentionRopeSpec, DenseGatedFfnSpec, DenseLayerFfnSpec, HybridDecodeSpec,
    HybridLayerFfnSpec, HybridLayerSpec, HybridPerLayerInputLayerSpec,
    HybridPerLayerInputProjectSpec, LogitsProbeSpec, ProbeInputKind, RmsNormSpec,
};
use crate::weights::GgufWeightLayout;

#[derive(Clone, Debug)]
struct Gemma4Dims {
    vocab_size: u32,
    embedding_length: u32,
}

#[derive(Clone, Debug)]
struct Gemma4AttentionDims {
    q_head_dim: u32,
    q_head_count: u32,
    k_head_dim: u32,
    kv_head_count: u32,
    v_head_dim: u32,
}

impl Gemma4Dims {
    fn from_model(model: &LlamaModel, tensors: &Gemma4Tensors) -> Result<Self> {
        let cfg = model.require_gemma4()?;
        let vocab_size = tensors
            .globals
            .token_embd
            .dimensions
            .get(1)
            .copied()
            .ok_or_else(|| LlamaError::format("token_embd.weight is missing vocab dimension"))?;
        Ok(Self {
            vocab_size: u32::try_from(vocab_size)
                .map_err(|_| LlamaError::format("gemma4 vocab size does not fit in u32"))?,
            embedding_length: cfg.embedding_length,
        })
    }
}

impl Gemma4AttentionDims {
    fn from_layer(layer: &Gemma4LayerTensors) -> Result<Self> {
        let q_proj_width = tensor_dim(&layer.attn_q, 1, "attn_q")?;
        let q_head_dim = tensor_dim(&layer.attn_q_norm, 0, "attn_q_norm")?;
        let q_head_count = exact_div(q_proj_width, q_head_dim, "gemma4 q_head_count")?;

        let k_proj_width = tensor_dim(&layer.attn_k, 1, "attn_k")?;
        let k_head_dim = tensor_dim(&layer.attn_k_norm, 0, "attn_k_norm")?;
        let kv_head_count = exact_div(k_proj_width, k_head_dim, "gemma4 kv_head_count")?;

        let v_head_dim = if let Some(attn_v) = &layer.attn_v {
            let v_proj_width = tensor_dim(attn_v, 1, "attn_v")?;
            exact_div(v_proj_width, kv_head_count, "gemma4 v_head_dim")?
        } else {
            k_head_dim
        };

        Ok(Self {
            q_head_dim,
            q_head_count,
            k_head_dim,
            kv_head_count,
            v_head_dim,
        })
    }
}

pub fn gemma4_embedding_logits_probe_spec(model: &LlamaModel) -> Result<LogitsProbeSpec> {
    let tensors = model.gemma4_tensors()?;
    let dims = Gemma4Dims::from_model(model, &tensors)?;
    let cfg = model.require_gemma4()?;
    Ok(LogitsProbeSpec {
        input: ProbeInputKind::Embeddings {
            hidden_size: dims.embedding_length,
            input_type: TensorType::F32,
        },
        output_norm_name: tensors.globals.output_norm.name.clone(),
        output_name: tensors.globals.output.name.clone(),
        rms_epsilon: cfg.attention_layer_norm_rms_epsilon,
        final_logit_softcap: cfg.final_logit_softcapping.filter(|softcap| *softcap > 0.0),
    })
}

pub fn gemma4_dense_ffn_spec(model: &LlamaModel, layer_index: u32) -> Result<DenseLayerFfnSpec> {
    let tensors = model.gemma4_tensors()?;
    let cfg = model.require_gemma4()?;
    let layer = tensors
        .layers
        .iter()
        .find(|layer| layer.index == layer_index)
        .ok_or_else(|| LlamaError::format(format!("missing gemma4 layer {}", layer_index)))?;

    Ok(DenseLayerFfnSpec {
        input_norm: Some(RmsNormSpec {
            weight_name: layer.ffn_norm.name.clone(),
            epsilon: cfg.attention_layer_norm_rms_epsilon,
        }),
        ffn: DenseGatedFfnSpec {
            gate_proj_name: layer.ffn_gate.name.clone(),
            up_proj_name: layer.ffn_up.name.clone(),
            down_proj_name: layer.ffn_down.name.clone(),
            gate_proj_scale_name: None,
            up_proj_scale_name: None,
            down_proj_scale_name: None,
            gate_activation: UnaryOp::Gelu,
        },
    })
}

pub fn gemma4_attention_block_spec(
    model: &LlamaModel,
    layer_index: u32,
) -> Result<AttentionBlockSpec> {
    let tensors = model.gemma4_tensors()?;
    let cfg = model.require_gemma4()?;
    let layer = tensors
        .layers
        .iter()
        .find(|layer| layer.index == layer_index)
        .ok_or_else(|| LlamaError::format(format!("missing gemma4 layer {}", layer_index)))?;
    let dims = Gemma4AttentionDims::from_layer(layer)?;
    let rope_factors_name = if layer.is_swa {
        None
    } else {
        Some(
            tensors
                .globals
                .rope_freqs
                .as_ref()
                .ok_or_else(|| {
                    LlamaError::format(format!(
                        "gemma4 full-attention layer {} is missing rope_freqs.weight",
                        layer_index
                    ))
                })?
                .name
                .clone(),
        )
    };

    Ok(AttentionBlockSpec {
        input: ProbeInputKind::TokenIds {
            token_embedding_name: tensors.globals.token_embd.name.clone(),
            token_embedding_scale: Some(gemma4_token_embedding_scale(cfg.embedding_length)),
        },
        input_norm_name: layer.attn_norm.name.clone(),
        q_proj_name: layer.attn_q.name.clone(),
        q_proj_scale_name: None,
        q_layout: AttentionQueryLayout::Plain,
        k_proj_name: layer.attn_k.name.clone(),
        k_proj_scale_name: None,
        v_proj_name: layer.attn_v.as_ref().map(|tensor| tensor.name.clone()),
        v_proj_scale_name: None,
        output_proj_name: layer.attn_output.name.clone(),
        output_proj_scale_name: None,
        q_norm_name: Some(layer.attn_q_norm.name.clone()),
        k_norm_name: Some(layer.attn_k_norm.name.clone()),
        v_norm_epsilon: Some(cfg.attention_layer_norm_rms_epsilon),
        q_head_dim: dims.q_head_dim,
        q_head_count: dims.q_head_count,
        k_head_dim: dims.k_head_dim,
        kv_head_count: dims.kv_head_count,
        v_head_dim: dims.v_head_dim,
        rms_epsilon: cfg.attention_layer_norm_rms_epsilon,
        rope: Some(AttentionRopeSpec {
            n_dims: i32::try_from(if layer.is_swa {
                cfg.rope_dimension_count_swa
            } else {
                cfg.rope_dimension_count
            })
            .map_err(|_| LlamaError::format("gemma4 rope dimension count does not fit in i32"))?,
            sections: [0; 4],
            mode: GGML_ROPE_TYPE_NEOX,
            n_ctx_orig: i32::try_from(cfg.context_length)
                .map_err(|_| LlamaError::format("gemma4 context_length does not fit in i32"))?,
            freq_base: if layer.is_swa {
                cfg.rope_freq_base_swa
            } else {
                cfg.rope_freq_base
            },
            freq_scale: 1.0,
            ext_factor: 0.0,
            attn_factor: 1.0,
            beta_fast: 0.0,
            beta_slow: 0.0,
        }),
        rope_factors_name,
        attention_scale: 1.0,
        causal: true,
        causal_window: if layer.is_swa && cfg.attention_sliding_window > 0 {
            Some(cfg.attention_sliding_window)
        } else {
            None
        },
        residual: true,
    })
}

pub fn gemma4_first_attention_block_spec(model: &LlamaModel) -> Result<(u32, AttentionBlockSpec)> {
    let tensors = model.gemma4_tensors()?;
    let layer = tensors
        .layers
        .first()
        .ok_or_else(|| LlamaError::format("gemma4 model has no attention layers"))?;
    Ok((
        layer.index,
        gemma4_attention_block_spec(model, layer.index)?,
    ))
}

pub fn gemma4_first_full_attention_block_spec(
    model: &LlamaModel,
) -> Result<(u32, AttentionBlockSpec)> {
    let tensors = model.gemma4_tensors()?;
    let layer = tensors
        .layers
        .iter()
        .find(|layer| !layer.is_swa)
        .ok_or_else(|| LlamaError::format("gemma4 model has no full-attention layers"))?;
    Ok((
        layer.index,
        gemma4_attention_block_spec(model, layer.index)?,
    ))
}

pub fn gemma4_attention_block_layout(
    model: &LlamaModel,
    layer_index: u32,
) -> Result<GgufWeightLayout> {
    let tensors = model.gemma4_tensors()?;
    let layer = tensors
        .layers
        .iter()
        .find(|layer| layer.index == layer_index)
        .ok_or_else(|| LlamaError::format(format!("missing gemma4 layer {}", layer_index)))?;

    let mut weights = vec![
        tensors.globals.token_embd.clone(),
        layer.attn_norm.clone(),
        layer.attn_q.clone(),
        layer.attn_q_norm.clone(),
        layer.attn_k.clone(),
        layer.attn_k_norm.clone(),
        layer.attn_output.clone(),
    ];
    if let Some(attn_v) = &layer.attn_v {
        weights.push(attn_v.clone());
    }
    if !layer.is_swa {
        weights.push(
            tensors
                .globals
                .rope_freqs
                .as_ref()
                .ok_or_else(|| {
                    LlamaError::format(format!(
                        "gemma4 full-attention layer {} is missing rope_freqs.weight",
                        layer_index
                    ))
                })?
                .clone(),
        );
    }
    GgufWeightLayout::from_tensors(weights)
}

pub fn gemma4_attention_decode_spec(
    model: &LlamaModel,
    layer_index: u32,
    max_context: u32,
    max_sequences: u32,
    k_type: TensorType,
    v_type: TensorType,
) -> Result<AttentionDecodeSpec> {
    let tensors = model.gemma4_tensors()?;
    let cfg = model.require_gemma4()?;
    let layer = tensors
        .layers
        .iter()
        .find(|layer| layer.index == layer_index)
        .ok_or_else(|| LlamaError::format(format!("missing gemma4 layer {}", layer_index)))?;
    let (cache_layer_index, write_kv) =
        gemma4_cache_layer_assignment(cfg.block_count, cfg.attention_shared_kv_layers, layer)?;

    Ok(AttentionDecodeSpec {
        block: gemma4_attention_block_spec(model, layer_index)?,
        cache: AttentionKvCacheSpec {
            max_context,
            max_sequences,
            k_type,
            v_type,
        },
        cache_layer_index,
        write_kv,
    })
}

pub fn gemma4_hybrid_decode_spec(
    model: &LlamaModel,
    max_context: u32,
    max_sequences: u32,
    attention_k_type: TensorType,
    attention_v_type: TensorType,
    recurrent_r_type: TensorType,
    recurrent_s_type: TensorType,
) -> Result<HybridDecodeSpec> {
    let _ = (recurrent_r_type, recurrent_s_type);
    let tensors = model.gemma4_tensors()?;
    let cfg = model.require_gemma4()?;
    let per_layer_input = if cfg.embedding_length_per_layer_input != 0 {
        Some(HybridPerLayerInputProjectSpec {
            token_embedding_name: tensors
                .globals
                .per_layer_token_embd
                .as_ref()
                .ok_or_else(|| {
                    LlamaError::format(
                        "gemma4 model is missing per_layer_token_embd.weight".to_string(),
                    )
                })?
                .name
                .clone(),
            token_embedding_scale: Some(gemma4_per_layer_token_embedding_scale(
                cfg.embedding_length_per_layer_input,
            )),
            model_proj_name: tensors
                .globals
                .per_layer_model_proj
                .as_ref()
                .ok_or_else(|| {
                    LlamaError::format(
                        "gemma4 model is missing per_layer_model_proj.weight".to_string(),
                    )
                })?
                .name
                .clone(),
            model_proj_scale: Some(gemma4_per_layer_model_projection_scale(
                cfg.embedding_length,
            )),
            proj_norm: RmsNormSpec {
                weight_name: tensors
                    .globals
                    .per_layer_proj_norm
                    .as_ref()
                    .ok_or_else(|| {
                        LlamaError::format(
                            "gemma4 model is missing per_layer_proj_norm.weight".to_string(),
                        )
                    })?
                    .name
                    .clone(),
                epsilon: cfg.attention_layer_norm_rms_epsilon,
            },
            hidden_size: cfg.embedding_length_per_layer_input,
            layer_count: cfg.block_count,
            combine_scale: Some(gemma4_per_layer_input_combine_scale()),
        })
    } else {
        None
    };

    let mut layers = Vec::with_capacity(tensors.layers.len());
    for layer in &tensors.layers {
        layers.push(HybridLayerSpec::Attention {
            layer_index: layer.index,
            decode: gemma4_attention_decode_spec(
                model,
                layer.index,
                max_context,
                max_sequences,
                attention_k_type,
                attention_v_type,
            )?,
            post_attention_norm: Some(RmsNormSpec {
                weight_name: layer.post_attention_norm.name.clone(),
                epsilon: cfg.attention_layer_norm_rms_epsilon,
            }),
            ffn: HybridLayerFfnSpec::Dense(gemma4_dense_ffn_spec(model, layer.index)?),
            post_ffn_norm: Some(RmsNormSpec {
                weight_name: layer.post_ffw_norm.name.clone(),
                epsilon: cfg.attention_layer_norm_rms_epsilon,
            }),
            per_layer_input: if cfg.embedding_length_per_layer_input != 0 {
                Some(HybridPerLayerInputLayerSpec {
                    input_gate_name: layer
                        .per_layer_inp_gate
                        .as_ref()
                        .ok_or_else(|| {
                            LlamaError::format(format!(
                                "gemma4 layer {} is missing inp_gate.weight",
                                layer.index
                            ))
                        })?
                        .name
                        .clone(),
                    proj_name: layer
                        .per_layer_proj
                        .as_ref()
                        .ok_or_else(|| {
                            LlamaError::format(format!(
                                "gemma4 layer {} is missing proj.weight",
                                layer.index
                            ))
                        })?
                        .name
                        .clone(),
                    post_norm: RmsNormSpec {
                        weight_name: layer
                            .per_layer_post_norm
                            .as_ref()
                            .ok_or_else(|| {
                                LlamaError::format(format!(
                                    "gemma4 layer {} is missing post_norm.weight",
                                    layer.index
                                ))
                            })?
                            .name
                            .clone(),
                        epsilon: cfg.attention_layer_norm_rms_epsilon,
                    },
                    activation: UnaryOp::Gelu,
                })
            } else {
                None
            },
            output_scale_name: layer
                .layer_output_scale
                .as_ref()
                .map(|tensor| tensor.name.clone()),
        });
    }

    Ok(HybridDecodeSpec {
        input: ProbeInputKind::TokenIds {
            token_embedding_name: tensors.globals.token_embd.name.clone(),
            token_embedding_scale: Some(gemma4_token_embedding_scale(cfg.embedding_length)),
        },
        output_norm_name: tensors.globals.output_norm.name.clone(),
        output_name: tensors.globals.output.name.clone(),
        rms_epsilon: cfg.attention_layer_norm_rms_epsilon,
        final_logit_softcap: cfg.final_logit_softcapping.filter(|softcap| *softcap > 0.0),
        per_layer_input,
        layers,
    })
}

pub fn gemma4_execution_plan(model: &LlamaModel) -> Result<ModelExecutionPlan> {
    let tensors = model.gemma4_tensors()?;
    let dims = Gemma4Dims::from_model(model, &tensors)?;
    let inventory = gemma4_inventory(&tensors);

    Ok(ModelExecutionPlan {
        architecture: model.architecture.clone(),
        embedding_length: dims.embedding_length,
        vocab_size: Some(dims.vocab_size),
        full_weights: inventory.weight_layout()?,
        tail_probe: ModelTailProbePlan {
            spec: gemma4_embedding_logits_probe_spec(model)?,
            weights: GgufWeightLayout::from_tensors(gemma4_tail_probe_tensors(&tensors))?,
            extra_activation_bytes: 8 << 20,
        },
        hybrid_cache: None,
        inventory,
    })
}

fn gemma4_tail_probe_tensors(tensors: &Gemma4Tensors) -> Vec<crate::gguf::GgufTensorInfo> {
    vec![
        tensors.globals.output_norm.clone(),
        tensors.globals.output.clone(),
    ]
}

fn gemma4_inventory(tensors: &Gemma4Tensors) -> ModelTensorInventory {
    let mut globals = BTreeMap::new();
    insert_tensor(&mut globals, "token_embd", &tensors.globals.token_embd);
    insert_optional_tensor(
        &mut globals,
        "per_layer_token_embd",
        &tensors.globals.per_layer_token_embd,
    );
    insert_optional_tensor(
        &mut globals,
        "per_layer_model_proj",
        &tensors.globals.per_layer_model_proj,
    );
    insert_optional_tensor(
        &mut globals,
        "per_layer_proj_norm",
        &tensors.globals.per_layer_proj_norm,
    );
    insert_tensor(&mut globals, "output_norm", &tensors.globals.output_norm);
    insert_tensor(&mut globals, "output", &tensors.globals.output);
    insert_optional_tensor(&mut globals, "rope_freqs", &tensors.globals.rope_freqs);

    let layers = tensors
        .layers
        .iter()
        .map(|layer| {
            let mut entries = BTreeMap::new();
            insert_tensor(&mut entries, "attn_norm", &layer.attn_norm);
            insert_tensor(&mut entries, "attn_q", &layer.attn_q);
            insert_tensor(&mut entries, "attn_q_norm", &layer.attn_q_norm);
            insert_tensor(&mut entries, "attn_k", &layer.attn_k);
            insert_tensor(&mut entries, "attn_k_norm", &layer.attn_k_norm);
            insert_optional_tensor(&mut entries, "attn_v", &layer.attn_v);
            insert_tensor(&mut entries, "attn_output", &layer.attn_output);
            insert_tensor(
                &mut entries,
                "post_attention_norm",
                &layer.post_attention_norm,
            );
            insert_tensor(&mut entries, "ffn_norm", &layer.ffn_norm);
            insert_tensor(&mut entries, "ffn_gate", &layer.ffn_gate);
            insert_tensor(&mut entries, "ffn_up", &layer.ffn_up);
            insert_tensor(&mut entries, "ffn_down", &layer.ffn_down);
            insert_tensor(&mut entries, "post_ffw_norm", &layer.post_ffw_norm);
            insert_optional_tensor(&mut entries, "per_layer_inp_gate", &layer.per_layer_inp_gate);
            insert_optional_tensor(&mut entries, "per_layer_proj", &layer.per_layer_proj);
            insert_optional_tensor(
                &mut entries,
                "per_layer_post_norm",
                &layer.per_layer_post_norm,
            );
            insert_optional_tensor(
                &mut entries,
                "layer_output_scale",
                &layer.layer_output_scale,
            );

            ModelLayerInventory {
                index: layer.index,
                role: ModelLayerRole::Attention,
                tensors: entries,
            }
        })
        .collect();

    ModelTensorInventory { globals, layers }
}

fn gemma4_cache_layer_assignment(
    block_count: u32,
    shared_kv_layers: u32,
    layer: &Gemma4LayerTensors,
) -> Result<(u32, bool)> {
    let kv_from_start = block_count
        .checked_sub(shared_kv_layers)
        .ok_or_else(|| LlamaError::format("gemma4 shared_kv_layers exceeds block_count"))?;
    if layer.index < kv_from_start {
        return Ok((layer.index, true));
    }
    let reuse_distance = if layer.is_swa { 2 } else { 1 };
    let cache_layer_index = kv_from_start.checked_sub(reuse_distance).ok_or_else(|| {
        LlamaError::format("gemma4 shared_kv_layers leaves no valid cache source layer")
    })?;
    Ok((cache_layer_index, false))
}

fn gemma4_token_embedding_scale(embedding_length: u32) -> f32 {
    (embedding_length as f32).sqrt()
}

fn gemma4_per_layer_token_embedding_scale(embedding_length: u32) -> f32 {
    (embedding_length as f32).sqrt()
}

fn gemma4_per_layer_model_projection_scale(embedding_length: u32) -> f32 {
    1.0 / (embedding_length as f32).sqrt()
}

fn gemma4_per_layer_input_combine_scale() -> f32 {
    1.0 / 2.0f32.sqrt()
}

fn insert_tensor(
    map: &mut BTreeMap<String, crate::gguf::GgufTensorInfo>,
    key: &str,
    tensor: &crate::gguf::GgufTensorInfo,
) {
    map.insert(key.to_owned(), tensor.clone());
}

fn insert_optional_tensor(
    map: &mut BTreeMap<String, crate::gguf::GgufTensorInfo>,
    key: &str,
    tensor: &Option<crate::gguf::GgufTensorInfo>,
) {
    if let Some(tensor) = tensor {
        insert_tensor(map, key, tensor);
    }
}

fn tensor_dim(tensor: &crate::gguf::GgufTensorInfo, index: usize, name: &str) -> Result<u32> {
    let dim = tensor.dimensions.get(index).copied().ok_or_else(|| {
        LlamaError::format(format!("tensor '{}' is missing dimension {}", name, index))
    })?;
    u32::try_from(dim).map_err(|_| {
        LlamaError::format(format!(
            "tensor '{}' dim {} does not fit in u32",
            name, index
        ))
    })
}

fn exact_div(value: u32, divisor: u32, what: &str) -> Result<u32> {
    if divisor == 0 || value % divisor != 0 {
        return Err(LlamaError::format(format!(
            "{} requires exact division, got {}/{}",
            what, value, divisor
        )));
    }
    Ok(value / divisor)
}
