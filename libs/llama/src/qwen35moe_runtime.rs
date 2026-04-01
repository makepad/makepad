use std::collections::BTreeMap;

use makepad_ggml::{TensorType, UnaryOp, GGML_ROPE_TYPE_IMROPE};

use crate::error::{LlamaError, Result};
use crate::model::LlamaModel;
use crate::plan::{
    ModelExecutionPlan, ModelLayerInventory, ModelLayerRole, ModelTailProbePlan,
    ModelTensorInventory,
};
use crate::qwen35moe::{Qwen35MoeLayerKind, Qwen35MoeTensors};
use crate::runtime::{
    AttentionBlockSpec, AttentionDecodeSpec, AttentionKvCacheSpec, AttentionQueryLayout,
    AttentionRopeSpec, DeltaNetRecurrentBlockSpec, DeltaNetRecurrentDecodeSpec,
    DeltaNetRecurrentStateSpec, DenseGatedFfnSpec, ExpertGatingFunc, HybridCacheShape,
    HybridCacheSpec, HybridCacheTemplate, HybridCacheTypes, HybridDecodeSpec, HybridLayerSpec,
    LogitsProbeSpec, MoeFfnSpec, MoeSharedExpertSpec, ProbeInputKind, RmsNormSpec,
};
use crate::weights::GgufWeightLayout;

#[derive(Clone, Debug)]
pub struct Qwen35MoeDims {
    pub vocab_size: u32,
    pub block_count: u32,
    pub embedding_length: u32,
    pub attention_head_count: u32,
    pub attention_head_count_kv: u32,
    pub attention_key_length: u32,
    pub attention_value_length: u32,
    pub expert_count: u32,
    pub expert_used_count: u32,
    pub ssm_conv_kernel: u32,
    pub ssm_state_size: u32,
    pub ssm_group_count: u32,
    pub ssm_time_step_rank: u32,
    pub ssm_inner_size: u32,
    pub full_attention_interval: u32,
}

impl Qwen35MoeDims {
    pub fn from_model(model: &LlamaModel, tensors: &Qwen35MoeTensors) -> Result<Self> {
        let cfg = model.require_qwen35moe()?;
        let vocab_size = tensors
            .globals
            .token_embd
            .dimensions
            .get(1)
            .copied()
            .ok_or_else(|| LlamaError::format("token_embd.weight is missing vocab dimension"))?;
        Ok(Self {
            vocab_size: u32::try_from(vocab_size).map_err(|_| {
                LlamaError::format(format!("vocab size {} does not fit in u32", vocab_size))
            })?,
            block_count: cfg.block_count,
            embedding_length: cfg.embedding_length,
            attention_head_count: cfg.attention_head_count,
            attention_head_count_kv: cfg.attention_head_count_kv,
            attention_key_length: cfg.attention_key_length,
            attention_value_length: cfg.attention_value_length,
            expert_count: cfg.expert_count,
            expert_used_count: cfg.expert_used_count,
            ssm_conv_kernel: cfg.ssm_conv_kernel,
            ssm_state_size: cfg.ssm_state_size,
            ssm_group_count: cfg.ssm_group_count,
            ssm_time_step_rank: cfg.ssm_time_step_rank,
            ssm_inner_size: cfg.ssm_inner_size,
            full_attention_interval: cfg.full_attention_interval,
        })
    }

    pub fn recurrent_conv_width(&self) -> Result<u64> {
        let conv_prefix = u64::from(self.ssm_conv_kernel.saturating_sub(1));
        let channels = u64::from(self.ssm_inner_size)
            .checked_add(
                2_u64
                    .checked_mul(u64::from(self.ssm_group_count))
                    .and_then(|v| v.checked_mul(u64::from(self.ssm_state_size)))
                    .ok_or_else(|| {
                        LlamaError::format("overflow computing qwen35moe conv channels")
                    })?,
            )
            .ok_or_else(|| LlamaError::format("overflow computing qwen35moe conv channels"))?;
        conv_prefix
            .checked_mul(channels)
            .ok_or_else(|| LlamaError::format("overflow computing qwen35moe conv width"))
    }

    pub fn recurrent_state_width(&self) -> Result<u64> {
        u64::from(self.ssm_state_size)
            .checked_mul(u64::from(self.ssm_inner_size))
            .ok_or_else(|| LlamaError::format("overflow computing qwen35moe recurrent state width"))
    }

    pub fn attention_k_width(&self) -> u64 {
        u64::from(self.attention_key_length) * u64::from(self.attention_head_count_kv)
    }

    pub fn attention_v_width(&self) -> u64 {
        u64::from(self.attention_value_length) * u64::from(self.attention_head_count_kv)
    }
}

pub fn qwen35moe_token_logits_probe_spec(model: &LlamaModel) -> Result<LogitsProbeSpec> {
    let tensors = model.qwen35moe_tensors()?;
    let cfg = model.require_qwen35moe()?;
    Ok(LogitsProbeSpec {
        input: ProbeInputKind::TokenIds {
            token_embedding_name: tensors.globals.token_embd.name.clone(),
        },
        output_norm_name: tensors.globals.output_norm.name.clone(),
        output_name: tensors.globals.output.name.clone(),
        rms_epsilon: cfg.attention_layer_norm_rms_epsilon,
    })
}

pub fn qwen35moe_embedding_logits_probe_spec(model: &LlamaModel) -> Result<LogitsProbeSpec> {
    let tensors = model.qwen35moe_tensors()?;
    let dims = Qwen35MoeDims::from_model(model, &tensors)?;
    let cfg = model.require_qwen35moe()?;
    Ok(LogitsProbeSpec {
        input: ProbeInputKind::Embeddings {
            hidden_size: dims.embedding_length,
            input_type: TensorType::F32,
        },
        output_norm_name: tensors.globals.output_norm.name.clone(),
        output_name: tensors.globals.output.name.clone(),
        rms_epsilon: cfg.attention_layer_norm_rms_epsilon,
    })
}

pub fn qwen35moe_attention_block_spec(
    model: &LlamaModel,
    layer_index: u32,
) -> Result<AttentionBlockSpec> {
    let tensors = model.qwen35moe_tensors()?;
    let dims = Qwen35MoeDims::from_model(model, &tensors)?;
    let cfg = model.require_qwen35moe()?;
    let layer = tensors
        .layers
        .iter()
        .find(|layer| layer.index == layer_index)
        .ok_or_else(|| LlamaError::format(format!("missing qwen35moe layer {}", layer_index)))?;
    let attention = layer.attention.as_ref().ok_or_else(|| {
        LlamaError::format(format!(
            "qwen35moe layer {} is {} and does not expose full-attention tensors",
            layer_index,
            layer.kind.name()
        ))
    })?;

    Ok(AttentionBlockSpec {
        input: ProbeInputKind::TokenIds {
            token_embedding_name: tensors.globals.token_embd.name.clone(),
        },
        input_norm_name: layer.attn_norm.name.clone(),
        q_proj_name: attention.wq.name.clone(),
        q_layout: AttentionQueryLayout::InterleavedQueryGate {
            gate_activation: UnaryOp::Sigmoid,
        },
        k_proj_name: attention.wk.name.clone(),
        v_proj_name: attention.wv.name.clone(),
        output_proj_name: attention.wo.name.clone(),
        q_norm_name: Some(attention.attn_q_norm.name.clone()),
        k_norm_name: Some(attention.attn_k_norm.name.clone()),
        q_head_dim: dims.attention_key_length,
        q_head_count: dims.attention_head_count,
        k_head_dim: dims.attention_key_length,
        kv_head_count: dims.attention_head_count_kv,
        v_head_dim: dims.attention_value_length,
        rms_epsilon: cfg.attention_layer_norm_rms_epsilon,
        rope: Some(AttentionRopeSpec {
            n_dims: i32::try_from(cfg.rope_dimension_count).map_err(|_| {
                LlamaError::format(format!(
                    "rope_dimension_count {} does not fit in i32",
                    cfg.rope_dimension_count
                ))
            })?,
            sections: rope_sections_i32(&cfg.rope_dimension_sections)?,
            mode: GGML_ROPE_TYPE_IMROPE,
            n_ctx_orig: i32::try_from(cfg.context_length).map_err(|_| {
                LlamaError::format(format!(
                    "context_length {} does not fit in i32",
                    cfg.context_length
                ))
            })?,
            freq_base: cfg.rope_freq_base,
            freq_scale: 1.0,
            ext_factor: 0.0,
            attn_factor: 1.0,
            beta_fast: 0.0,
            beta_slow: 0.0,
        }),
        causal: true,
        residual: true,
    })
}

pub fn qwen35moe_first_attention_block_spec(
    model: &LlamaModel,
) -> Result<(u32, AttentionBlockSpec)> {
    let tensors = model.qwen35moe_tensors()?;
    let layer = tensors
        .layers
        .iter()
        .find(|layer| layer.kind == Qwen35MoeLayerKind::Attention)
        .ok_or_else(|| LlamaError::format("qwen35moe model has no attention layers"))?;
    Ok((
        layer.index,
        qwen35moe_attention_block_spec(model, layer.index)?,
    ))
}

pub fn qwen35moe_attention_decode_spec(
    model: &LlamaModel,
    layer_index: u32,
    max_context: u32,
    max_sequences: u32,
    k_type: TensorType,
    v_type: TensorType,
) -> Result<AttentionDecodeSpec> {
    Ok(AttentionDecodeSpec {
        block: qwen35moe_attention_block_spec(model, layer_index)?,
        cache: AttentionKvCacheSpec {
            max_context,
            max_sequences,
            k_type,
            v_type,
        },
    })
}

pub fn qwen35moe_recurrent_block_spec(
    model: &LlamaModel,
    layer_index: u32,
) -> Result<DeltaNetRecurrentBlockSpec> {
    let tensors = model.qwen35moe_tensors()?;
    let dims = Qwen35MoeDims::from_model(model, &tensors)?;
    let cfg = model.require_qwen35moe()?;
    let layer = tensors
        .layers
        .iter()
        .find(|layer| layer.index == layer_index)
        .ok_or_else(|| LlamaError::format(format!("missing qwen35moe layer {}", layer_index)))?;
    let recurrent = layer.recurrent.as_ref().ok_or_else(|| {
        LlamaError::format(format!(
            "qwen35moe layer {} is {} and does not expose recurrent tensors",
            layer_index,
            layer.kind.name()
        ))
    })?;

    let value_head_dim = dims
        .ssm_inner_size
        .checked_div(dims.ssm_time_step_rank)
        .ok_or_else(|| LlamaError::format("qwen35moe ssm_time_step_rank must be non-zero"))?;
    if value_head_dim * dims.ssm_time_step_rank != dims.ssm_inner_size {
        return Err(LlamaError::format(format!(
            "qwen35moe ssm_inner_size {} is not divisible by ssm_time_step_rank {}",
            dims.ssm_inner_size, dims.ssm_time_step_rank
        )));
    }

    Ok(DeltaNetRecurrentBlockSpec {
        input: ProbeInputKind::TokenIds {
            token_embedding_name: tensors.globals.token_embd.name.clone(),
        },
        input_norm_name: layer.attn_norm.name.clone(),
        qkv_proj_name: recurrent.wqkv.name.clone(),
        z_proj_name: recurrent.wqkv_gate.name.clone(),
        beta_proj_name: recurrent.ssm_beta.name.clone(),
        alpha_proj_name: recurrent.ssm_alpha.name.clone(),
        dt_bias_name: recurrent.ssm_dt.name.clone(),
        a_name: recurrent.ssm_a.name.clone(),
        conv_kernel_name: recurrent.ssm_conv1d.name.clone(),
        norm_name: recurrent.ssm_norm.name.clone(),
        output_proj_name: recurrent.ssm_out.name.clone(),
        key_head_dim: dims.ssm_state_size,
        key_head_count: dims.ssm_group_count,
        value_head_dim,
        value_head_count: dims.ssm_time_step_rank,
        rms_epsilon: cfg.attention_layer_norm_rms_epsilon,
        residual: true,
    })
}

pub fn qwen35moe_first_recurrent_block_spec(
    model: &LlamaModel,
) -> Result<(u32, DeltaNetRecurrentBlockSpec)> {
    let tensors = model.qwen35moe_tensors()?;
    let layer = tensors
        .layers
        .iter()
        .find(|layer| layer.kind == Qwen35MoeLayerKind::Recurrent)
        .ok_or_else(|| LlamaError::format("qwen35moe model has no recurrent layers"))?;
    Ok((
        layer.index,
        qwen35moe_recurrent_block_spec(model, layer.index)?,
    ))
}

pub fn qwen35moe_delta_net_recurrent_decode_spec(
    model: &LlamaModel,
    layer_index: u32,
    max_sequences: u32,
    r_type: TensorType,
    s_type: TensorType,
) -> Result<DeltaNetRecurrentDecodeSpec> {
    Ok(DeltaNetRecurrentDecodeSpec {
        block: qwen35moe_recurrent_block_spec(model, layer_index)?,
        cache: DeltaNetRecurrentStateSpec {
            max_sequences,
            r_type,
            s_type,
        },
    })
}

pub fn qwen35moe_moe_ffn_spec(model: &LlamaModel, layer_index: u32) -> Result<MoeFfnSpec> {
    let tensors = model.qwen35moe_tensors()?;
    let dims = Qwen35MoeDims::from_model(model, &tensors)?;
    let cfg = model.require_qwen35moe()?;
    let layer = tensors
        .layers
        .iter()
        .find(|layer| layer.index == layer_index)
        .ok_or_else(|| LlamaError::format(format!("missing qwen35moe layer {}", layer_index)))?;

    Ok(MoeFfnSpec {
        input: ProbeInputKind::Embeddings {
            hidden_size: dims.embedding_length,
            input_type: TensorType::F32,
        },
        input_norm: Some(RmsNormSpec {
            weight_name: layer.post_attention_norm.name.clone(),
            epsilon: cfg.attention_layer_norm_rms_epsilon,
        }),
        router_proj_name: layer.moe.ffn_gate_inp.name.clone(),
        expert_count: dims.expert_count,
        expert_used_count: dims.expert_used_count,
        gating_func: ExpertGatingFunc::SoftMax,
        normalize_selected_weights: true,
        weight_scale: 1.0,
        merged_gate_up_proj_name: layer.moe.ffn_gate_up_exps.as_ref().map(|t| t.name.clone()),
        gate_proj_name: layer.moe.ffn_gate_exps.as_ref().map(|t| t.name.clone()),
        up_proj_name: layer
            .moe
            .ffn_up_exps
            .as_ref()
            .map(|t| t.name.clone())
            .unwrap_or_default(),
        down_proj_name: layer.moe.ffn_down_exps.name.clone(),
        activation: UnaryOp::Silu,
        shared_expert: Some(MoeSharedExpertSpec {
            ffn: DenseGatedFfnSpec {
                gate_proj_name: layer.moe.ffn_gate_shexp.name.clone(),
                up_proj_name: layer.moe.ffn_up_shexp.name.clone(),
                down_proj_name: layer.moe.ffn_down_shexp.name.clone(),
                gate_activation: UnaryOp::Silu,
            },
            output_gate_name: Some(layer.moe.ffn_gate_inp_shexp.name.clone()),
            output_gate_activation: UnaryOp::Sigmoid,
        }),
    })
}

pub fn qwen35moe_first_moe_ffn_spec(model: &LlamaModel) -> Result<(u32, MoeFfnSpec)> {
    let tensors = model.qwen35moe_tensors()?;
    let layer = tensors
        .layers
        .first()
        .ok_or_else(|| LlamaError::format("qwen35moe model has no layers"))?;
    Ok((layer.index, qwen35moe_moe_ffn_spec(model, layer.index)?))
}

pub fn qwen35moe_attention_block_layout(
    model: &LlamaModel,
    layer_index: u32,
) -> Result<GgufWeightLayout> {
    let tensors = model.qwen35moe_tensors()?;
    let layer = tensors
        .layers
        .iter()
        .find(|layer| layer.index == layer_index)
        .ok_or_else(|| LlamaError::format(format!("missing qwen35moe layer {}", layer_index)))?;
    let attention = layer.attention.as_ref().ok_or_else(|| {
        LlamaError::format(format!(
            "qwen35moe layer {} is {} and does not expose full-attention tensors",
            layer_index,
            layer.kind.name()
        ))
    })?;

    GgufWeightLayout::from_tensors(vec![
        tensors.globals.token_embd.clone(),
        layer.attn_norm.clone(),
        attention.wq.clone(),
        attention.wk.clone(),
        attention.wv.clone(),
        attention.wo.clone(),
        attention.attn_q_norm.clone(),
        attention.attn_k_norm.clone(),
    ])
}

pub fn qwen35moe_recurrent_block_layout(
    model: &LlamaModel,
    layer_index: u32,
) -> Result<GgufWeightLayout> {
    let tensors = model.qwen35moe_tensors()?;
    let layer = tensors
        .layers
        .iter()
        .find(|layer| layer.index == layer_index)
        .ok_or_else(|| LlamaError::format(format!("missing qwen35moe layer {}", layer_index)))?;
    let recurrent = layer.recurrent.as_ref().ok_or_else(|| {
        LlamaError::format(format!(
            "qwen35moe layer {} is {} and does not expose recurrent tensors",
            layer_index,
            layer.kind.name()
        ))
    })?;

    GgufWeightLayout::from_tensors(vec![
        tensors.globals.token_embd.clone(),
        layer.attn_norm.clone(),
        recurrent.wqkv.clone(),
        recurrent.wqkv_gate.clone(),
        recurrent.ssm_conv1d.clone(),
        recurrent.ssm_dt.clone(),
        recurrent.ssm_a.clone(),
        recurrent.ssm_beta.clone(),
        recurrent.ssm_alpha.clone(),
        recurrent.ssm_norm.clone(),
        recurrent.ssm_out.clone(),
    ])
}

pub fn qwen35moe_moe_ffn_layout(model: &LlamaModel, layer_index: u32) -> Result<GgufWeightLayout> {
    let tensors = model.qwen35moe_tensors()?;
    let layer = tensors
        .layers
        .iter()
        .find(|layer| layer.index == layer_index)
        .ok_or_else(|| LlamaError::format(format!("missing qwen35moe layer {}", layer_index)))?;

    let mut weights = vec![
        layer.post_attention_norm.clone(),
        layer.moe.ffn_gate_inp.clone(),
        layer.moe.ffn_down_exps.clone(),
        layer.moe.ffn_gate_inp_shexp.clone(),
        layer.moe.ffn_gate_shexp.clone(),
        layer.moe.ffn_up_shexp.clone(),
        layer.moe.ffn_down_shexp.clone(),
    ];
    if let Some(tensor) = &layer.moe.ffn_gate_up_exps {
        weights.push(tensor.clone());
    }
    if let Some(tensor) = &layer.moe.ffn_gate_exps {
        weights.push(tensor.clone());
    }
    if let Some(tensor) = &layer.moe.ffn_up_exps {
        weights.push(tensor.clone());
    }

    GgufWeightLayout::from_tensors(weights)
}

pub fn qwen35moe_hybrid_cache_spec(
    model: &LlamaModel,
    n_ctx_seq: u32,
    n_seq_max: u32,
    attention_k_type: TensorType,
    attention_v_type: TensorType,
    recurrent_r_type: TensorType,
    recurrent_s_type: TensorType,
) -> Result<HybridCacheSpec> {
    Ok(qwen35moe_hybrid_cache_template(model)?.materialize(
        HybridCacheShape {
            n_ctx_seq,
            n_seq_max,
        },
        HybridCacheTypes {
            attention_k_type,
            attention_v_type,
            recurrent_r_type,
            recurrent_s_type,
        },
    ))
}

pub fn qwen35moe_hybrid_cache_template(model: &LlamaModel) -> Result<HybridCacheTemplate> {
    let tensors = model.qwen35moe_tensors()?;
    let dims = Qwen35MoeDims::from_model(model, &tensors)?;

    let attention_layers = tensors
        .layers
        .iter()
        .filter(|layer| layer.kind == Qwen35MoeLayerKind::Attention)
        .map(|layer| layer.index)
        .collect();
    let recurrent_layers = tensors
        .layers
        .iter()
        .filter(|layer| layer.kind == Qwen35MoeLayerKind::Recurrent)
        .map(|layer| layer.index)
        .collect();

    Ok(HybridCacheTemplate {
        attention_layers,
        recurrent_layers,
        attention_k_width: dims.attention_k_width(),
        attention_v_width: dims.attention_v_width(),
        recurrent_r_width: dims.recurrent_conv_width()?,
        recurrent_s_width: dims.recurrent_state_width()?,
    })
}

pub fn qwen35moe_hybrid_decode_spec(
    model: &LlamaModel,
    max_context: u32,
    max_sequences: u32,
    attention_k_type: TensorType,
    attention_v_type: TensorType,
    recurrent_r_type: TensorType,
    recurrent_s_type: TensorType,
) -> Result<HybridDecodeSpec> {
    let tensors = model.qwen35moe_tensors()?;
    let cfg = model.require_qwen35moe()?;

    let mut layers = Vec::with_capacity(tensors.layers.len());
    for layer in &tensors.layers {
        let ffn = qwen35moe_moe_ffn_spec(model, layer.index)?;
        match layer.kind {
            Qwen35MoeLayerKind::Attention => layers.push(HybridLayerSpec::Attention {
                layer_index: layer.index,
                decode: qwen35moe_attention_decode_spec(
                    model,
                    layer.index,
                    max_context,
                    max_sequences,
                    attention_k_type,
                    attention_v_type,
                )?,
                ffn,
            }),
            Qwen35MoeLayerKind::Recurrent => layers.push(HybridLayerSpec::Recurrent {
                layer_index: layer.index,
                decode: qwen35moe_delta_net_recurrent_decode_spec(
                    model,
                    layer.index,
                    max_sequences,
                    recurrent_r_type,
                    recurrent_s_type,
                )?,
                ffn,
            }),
        }
    }

    Ok(HybridDecodeSpec {
        input: ProbeInputKind::TokenIds {
            token_embedding_name: tensors.globals.token_embd.name.clone(),
        },
        output_norm_name: tensors.globals.output_norm.name.clone(),
        output_name: tensors.globals.output.name.clone(),
        rms_epsilon: cfg.attention_layer_norm_rms_epsilon,
        layers,
    })
}

pub fn qwen35moe_execution_plan(model: &LlamaModel) -> Result<ModelExecutionPlan> {
    let tensors = model.qwen35moe_tensors()?;
    let dims = Qwen35MoeDims::from_model(model, &tensors)?;
    let inventory = qwen35moe_inventory(&tensors);

    Ok(ModelExecutionPlan {
        architecture: model.architecture.clone(),
        embedding_length: dims.embedding_length,
        vocab_size: Some(dims.vocab_size),
        full_weights: inventory.weight_layout()?,
        tail_probe: ModelTailProbePlan {
            spec: qwen35moe_embedding_logits_probe_spec(model)?,
            weights: GgufWeightLayout::from_tensors(qwen35moe_tail_probe_tensors(&tensors))?,
            extra_activation_bytes: 8 << 20,
        },
        hybrid_cache: Some(qwen35moe_hybrid_cache_template(model)?),
        inventory,
    })
}

fn qwen35moe_inventory(tensors: &Qwen35MoeTensors) -> ModelTensorInventory {
    let mut globals = BTreeMap::new();
    insert_tensor(&mut globals, "token_embd", &tensors.globals.token_embd);
    insert_tensor(&mut globals, "output_norm", &tensors.globals.output_norm);
    insert_tensor(&mut globals, "output", &tensors.globals.output);

    let layers = tensors
        .layers
        .iter()
        .map(|layer| {
            let mut entries = BTreeMap::new();
            insert_tensor(&mut entries, "attn_norm", &layer.attn_norm);
            insert_tensor(
                &mut entries,
                "post_attention_norm",
                &layer.post_attention_norm,
            );

            if let Some(attention) = &layer.attention {
                insert_tensor(&mut entries, "attn_q", &attention.wq);
                insert_tensor(&mut entries, "attn_k", &attention.wk);
                insert_tensor(&mut entries, "attn_v", &attention.wv);
                insert_tensor(&mut entries, "attn_output", &attention.wo);
                insert_tensor(&mut entries, "attn_q_norm", &attention.attn_q_norm);
                insert_tensor(&mut entries, "attn_k_norm", &attention.attn_k_norm);
                insert_optional_tensor(&mut entries, "attn_q.scale", &attention.scales.wq);
                insert_optional_tensor(&mut entries, "attn_k.scale", &attention.scales.wk);
                insert_optional_tensor(&mut entries, "attn_v.scale", &attention.scales.wv);
                insert_optional_tensor(&mut entries, "attn_output.scale", &attention.scales.wo);
            }

            if let Some(recurrent) = &layer.recurrent {
                insert_tensor(&mut entries, "attn_qkv", &recurrent.wqkv);
                insert_tensor(&mut entries, "attn_gate", &recurrent.wqkv_gate);
                insert_tensor(&mut entries, "ssm_conv1d", &recurrent.ssm_conv1d);
                insert_tensor(&mut entries, "ssm_dt", &recurrent.ssm_dt);
                insert_tensor(&mut entries, "ssm_a", &recurrent.ssm_a);
                insert_tensor(&mut entries, "ssm_beta", &recurrent.ssm_beta);
                insert_tensor(&mut entries, "ssm_alpha", &recurrent.ssm_alpha);
                insert_tensor(&mut entries, "ssm_norm", &recurrent.ssm_norm);
                insert_tensor(&mut entries, "ssm_out", &recurrent.ssm_out);
                insert_optional_tensor(&mut entries, "attn_qkv.scale", &recurrent.scales.wqkv);
                insert_optional_tensor(
                    &mut entries,
                    "attn_gate.scale",
                    &recurrent.scales.wqkv_gate,
                );
                insert_optional_tensor(&mut entries, "ssm_out.scale", &recurrent.scales.ssm_out);
                insert_optional_tensor(
                    &mut entries,
                    "ssm_alpha.scale",
                    &recurrent.scales.ssm_alpha,
                );
                insert_optional_tensor(&mut entries, "ssm_beta.scale", &recurrent.scales.ssm_beta);
            }

            insert_tensor(&mut entries, "ffn_gate_inp", &layer.moe.ffn_gate_inp);
            insert_optional_tensor(
                &mut entries,
                "ffn_gate_up_exps",
                &layer.moe.ffn_gate_up_exps,
            );
            insert_optional_tensor(&mut entries, "ffn_gate_exps", &layer.moe.ffn_gate_exps);
            insert_optional_tensor(&mut entries, "ffn_up_exps", &layer.moe.ffn_up_exps);
            insert_tensor(&mut entries, "ffn_down_exps", &layer.moe.ffn_down_exps);
            insert_tensor(
                &mut entries,
                "ffn_gate_inp_shexp",
                &layer.moe.ffn_gate_inp_shexp,
            );
            insert_tensor(&mut entries, "ffn_gate_shexp", &layer.moe.ffn_gate_shexp);
            insert_tensor(&mut entries, "ffn_up_shexp", &layer.moe.ffn_up_shexp);
            insert_tensor(&mut entries, "ffn_down_shexp", &layer.moe.ffn_down_shexp);
            insert_optional_tensor(
                &mut entries,
                "ffn_gate_exps.scale",
                &layer.moe.scales.ffn_gate_exps,
            );
            insert_optional_tensor(
                &mut entries,
                "ffn_up_exps.scale",
                &layer.moe.scales.ffn_up_exps,
            );
            insert_optional_tensor(
                &mut entries,
                "ffn_down_exps.scale",
                &layer.moe.scales.ffn_down_exps,
            );
            insert_optional_tensor(
                &mut entries,
                "ffn_gate_shexp.scale",
                &layer.moe.scales.ffn_gate_shexp,
            );
            insert_optional_tensor(
                &mut entries,
                "ffn_up_shexp.scale",
                &layer.moe.scales.ffn_up_shexp,
            );
            insert_optional_tensor(
                &mut entries,
                "ffn_down_shexp.scale",
                &layer.moe.scales.ffn_down_shexp,
            );

            ModelLayerInventory {
                index: layer.index,
                role: layer_role(layer.kind),
                tensors: entries,
            }
        })
        .collect();

    ModelTensorInventory { globals, layers }
}

fn qwen35moe_tail_probe_tensors(tensors: &Qwen35MoeTensors) -> Vec<crate::GgufTensorInfo> {
    vec![
        tensors.globals.output_norm.clone(),
        tensors.globals.output.clone(),
    ]
}

fn layer_role(kind: Qwen35MoeLayerKind) -> ModelLayerRole {
    match kind {
        Qwen35MoeLayerKind::Attention => ModelLayerRole::Attention,
        Qwen35MoeLayerKind::Recurrent => ModelLayerRole::Recurrent,
    }
}

fn insert_tensor(
    tensors: &mut BTreeMap<String, crate::GgufTensorInfo>,
    key: &str,
    tensor: &crate::GgufTensorInfo,
) {
    tensors.insert(key.to_owned(), tensor.clone());
}

fn insert_optional_tensor(
    tensors: &mut BTreeMap<String, crate::GgufTensorInfo>,
    key: &str,
    tensor: &Option<crate::GgufTensorInfo>,
) {
    if let Some(tensor) = tensor {
        insert_tensor(tensors, key, tensor);
    }
}

fn rope_sections_i32(sections: &[u32]) -> Result<[i32; 4]> {
    let mut out = [0_i32; 4];
    for (index, slot) in out.iter_mut().enumerate() {
        *slot = i32::try_from(*sections.get(index).unwrap_or(&0)).map_err(|_| {
            LlamaError::format(format!(
                "rope section {} does not fit in i32",
                sections.get(index).copied().unwrap_or_default()
            ))
        })?;
    }
    Ok(out)
}
