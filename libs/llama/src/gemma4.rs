use std::collections::BTreeMap;

use crate::error::{LlamaError, Result};
use crate::gguf::GgufTensorInfo;
use crate::model::LlamaModel;
use crate::weights::GgufWeightLayout;

#[derive(Clone, Debug)]
pub struct Gemma4GlobalTensors {
    pub token_embd: GgufTensorInfo,
    pub per_layer_token_embd: Option<GgufTensorInfo>,
    pub per_layer_model_proj: Option<GgufTensorInfo>,
    pub per_layer_proj_norm: Option<GgufTensorInfo>,
    pub output_norm: GgufTensorInfo,
    pub output: GgufTensorInfo,
    pub rope_freqs: Option<GgufTensorInfo>,
}

#[derive(Clone, Debug)]
pub struct Gemma4LayerTensors {
    pub index: u32,
    pub is_swa: bool,
    pub attn_norm: GgufTensorInfo,
    pub attn_q: GgufTensorInfo,
    pub attn_q_norm: GgufTensorInfo,
    pub attn_k: GgufTensorInfo,
    pub attn_k_norm: GgufTensorInfo,
    pub attn_v: Option<GgufTensorInfo>,
    pub attn_output: GgufTensorInfo,
    pub post_attention_norm: GgufTensorInfo,
    pub ffn_norm: GgufTensorInfo,
    pub ffn_gate: GgufTensorInfo,
    pub ffn_up: GgufTensorInfo,
    pub ffn_down: GgufTensorInfo,
    pub post_ffw_norm: GgufTensorInfo,
    pub per_layer_inp_gate: Option<GgufTensorInfo>,
    pub per_layer_proj: Option<GgufTensorInfo>,
    pub per_layer_post_norm: Option<GgufTensorInfo>,
    pub layer_output_scale: Option<GgufTensorInfo>,
}

#[derive(Clone, Debug)]
pub struct Gemma4Tensors {
    pub globals: Gemma4GlobalTensors,
    pub layers: Vec<Gemma4LayerTensors>,
}

impl Gemma4Tensors {
    pub fn from_model(model: &LlamaModel) -> Result<Self> {
        let cfg = model.require_gemma4()?;
        if cfg.expert_count != 0 || cfg.expert_used_count != 0 {
            return Err(LlamaError::unsupported(
                "gemma4 MoE layers are not implemented yet".to_string(),
            ));
        }

        let token_embd = required_tensor(model, "token_embd.weight")?;
        let per_layer_token_embd = if cfg.embedding_length_per_layer_input != 0 {
            Some(required_tensor(model, "per_layer_token_embd.weight")?)
        } else {
            None
        };
        let per_layer_model_proj = if cfg.embedding_length_per_layer_input != 0 {
            Some(required_tensor(model, "per_layer_model_proj.weight")?)
        } else {
            None
        };
        let per_layer_proj_norm = if cfg.embedding_length_per_layer_input != 0 {
            Some(required_tensor(model, "per_layer_proj_norm.weight")?)
        } else {
            None
        };
        let output_norm = required_tensor(model, "output_norm.weight")?;
        let output = optional_tensor(model, "output.weight").unwrap_or_else(|| token_embd.clone());
        let rope_freqs = optional_tensor(model, "rope_freqs.weight");

        let mut layers = Vec::with_capacity(cfg.block_count as usize);
        for index in 0..cfg.block_count {
            let is_swa = cfg
                .attention_sliding_window_pattern
                .get(index as usize)
                .copied()
                .unwrap_or(0)
                != 0;
            layers.push(Gemma4LayerTensors {
                index,
                is_swa,
                attn_norm: required_tensor(model, &layer_name(index, "attn_norm", "weight"))?,
                attn_q: required_tensor(model, &layer_name(index, "attn_q", "weight"))?,
                attn_q_norm: required_tensor(model, &layer_name(index, "attn_q_norm", "weight"))?,
                attn_k: required_tensor(model, &layer_name(index, "attn_k", "weight"))?,
                attn_k_norm: required_tensor(model, &layer_name(index, "attn_k_norm", "weight"))?,
                attn_v: optional_tensor(model, &layer_name(index, "attn_v", "weight")),
                attn_output: required_tensor(model, &layer_name(index, "attn_output", "weight"))?,
                post_attention_norm: required_tensor(
                    model,
                    &layer_name(index, "post_attention_norm", "weight"),
                )?,
                ffn_norm: required_tensor(model, &layer_name(index, "ffn_norm", "weight"))?,
                ffn_gate: required_tensor(model, &layer_name(index, "ffn_gate", "weight"))?,
                ffn_up: required_tensor(model, &layer_name(index, "ffn_up", "weight"))?,
                ffn_down: required_tensor(model, &layer_name(index, "ffn_down", "weight"))?,
                post_ffw_norm: required_tensor(
                    model,
                    &layer_name(index, "post_ffw_norm", "weight"),
                )?,
                per_layer_inp_gate: if cfg.embedding_length_per_layer_input != 0 {
                    Some(required_tensor(model, &layer_name(index, "inp_gate", "weight"))?)
                } else {
                    None
                },
                per_layer_proj: if cfg.embedding_length_per_layer_input != 0 {
                    Some(required_tensor(model, &layer_name(index, "proj", "weight"))?)
                } else {
                    None
                },
                per_layer_post_norm: if cfg.embedding_length_per_layer_input != 0 {
                    Some(required_tensor(model, &layer_name(index, "post_norm", "weight"))?)
                } else {
                    None
                },
                layer_output_scale: optional_tensor(
                    model,
                    &layer_name(index, "layer_output_scale", "weight"),
                ),
            });
        }

        if layers.iter().any(|layer| !layer.is_swa) && rope_freqs.is_none() {
            return Err(LlamaError::format(
                "gemma4 model uses full-attention layers but is missing rope_freqs.weight",
            ));
        }

        Ok(Self {
            globals: Gemma4GlobalTensors {
                token_embd,
                per_layer_token_embd,
                per_layer_model_proj,
                per_layer_proj_norm,
                output_norm,
                output,
                rope_freqs,
            },
            layers,
        })
    }

    pub fn unique_tensor_count(&self) -> usize {
        self.unique_tensors().len()
    }

    pub fn total_tensor_bytes(&self) -> u64 {
        self.unique_tensors()
            .into_iter()
            .map(|tensor| tensor.size_bytes)
            .sum()
    }

    pub fn weight_layout(&self) -> Result<GgufWeightLayout> {
        GgufWeightLayout::from_tensors(self.unique_tensors())
    }

    pub fn unique_tensors(&self) -> Vec<GgufTensorInfo> {
        let mut tensors = BTreeMap::new();
        self.visit_tensors(|tensor| {
            tensors
                .entry(tensor.name.clone())
                .or_insert_with(|| tensor.clone());
        });
        tensors.into_values().collect()
    }

    fn visit_tensors(&self, mut visit: impl FnMut(&GgufTensorInfo)) {
        visit(&self.globals.token_embd);
        visit_optional(&self.globals.per_layer_token_embd, &mut visit);
        visit_optional(&self.globals.per_layer_model_proj, &mut visit);
        visit_optional(&self.globals.per_layer_proj_norm, &mut visit);
        visit(&self.globals.output_norm);
        visit(&self.globals.output);
        visit_optional(&self.globals.rope_freqs, &mut visit);

        for layer in &self.layers {
            visit(&layer.attn_norm);
            visit(&layer.attn_q);
            visit(&layer.attn_q_norm);
            visit(&layer.attn_k);
            visit(&layer.attn_k_norm);
            visit_optional(&layer.attn_v, &mut visit);
            visit(&layer.attn_output);
            visit(&layer.post_attention_norm);
            visit(&layer.ffn_norm);
            visit(&layer.ffn_gate);
            visit(&layer.ffn_up);
            visit(&layer.ffn_down);
            visit(&layer.post_ffw_norm);
            visit_optional(&layer.per_layer_inp_gate, &mut visit);
            visit_optional(&layer.per_layer_proj, &mut visit);
            visit_optional(&layer.per_layer_post_norm, &mut visit);
            visit_optional(&layer.layer_output_scale, &mut visit);
        }
    }
}

fn visit_optional<'a>(
    tensor: &'a Option<GgufTensorInfo>,
    visit: &mut impl FnMut(&'a GgufTensorInfo),
) {
    if let Some(tensor) = tensor.as_ref() {
        visit(tensor);
    }
}

fn required_tensor(model: &LlamaModel, name: &str) -> Result<GgufTensorInfo> {
    model.gguf.require_tensor(name).cloned()
}

fn optional_tensor(model: &LlamaModel, name: &str) -> Option<GgufTensorInfo> {
    model.gguf.get_tensor(name).cloned()
}

fn layer_name(index: u32, stem: &str, suffix: &str) -> String {
    format!("blk.{}.{}.{}", index, stem, suffix)
}
