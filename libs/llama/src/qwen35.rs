use std::collections::BTreeMap;

use crate::error::{LlamaError, Result};
use crate::gguf::GgufTensorInfo;
use crate::model::LlamaModel;
use crate::weights::GgufWeightLayout;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Qwen35LayerKind {
    Attention,
    Recurrent,
}

impl Qwen35LayerKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Attention => "attention",
            Self::Recurrent => "recurrent",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Qwen35GlobalTensors {
    pub token_embd: GgufTensorInfo,
    pub output_norm: GgufTensorInfo,
    pub output: GgufTensorInfo,
}

#[derive(Clone, Debug, Default)]
pub struct Qwen35AttentionScales {
    pub wq: Option<GgufTensorInfo>,
    pub wk: Option<GgufTensorInfo>,
    pub wv: Option<GgufTensorInfo>,
    pub wo: Option<GgufTensorInfo>,
}

#[derive(Clone, Debug)]
pub struct Qwen35AttentionTensors {
    pub wq: GgufTensorInfo,
    pub wk: GgufTensorInfo,
    pub wv: GgufTensorInfo,
    pub wo: GgufTensorInfo,
    pub attn_q_norm: GgufTensorInfo,
    pub attn_k_norm: GgufTensorInfo,
    pub scales: Qwen35AttentionScales,
}

#[derive(Clone, Debug, Default)]
pub struct Qwen35RecurrentScales {
    pub wqkv: Option<GgufTensorInfo>,
    pub wqkv_gate: Option<GgufTensorInfo>,
    pub ssm_out: Option<GgufTensorInfo>,
    pub ssm_alpha: Option<GgufTensorInfo>,
    pub ssm_beta: Option<GgufTensorInfo>,
}

#[derive(Clone, Debug)]
pub struct Qwen35RecurrentTensors {
    pub wqkv: GgufTensorInfo,
    pub wqkv_gate: GgufTensorInfo,
    pub ssm_conv1d: GgufTensorInfo,
    pub ssm_dt: GgufTensorInfo,
    pub ssm_a: GgufTensorInfo,
    pub ssm_beta: GgufTensorInfo,
    pub ssm_alpha: GgufTensorInfo,
    pub ssm_norm: GgufTensorInfo,
    pub ssm_out: GgufTensorInfo,
    pub scales: Qwen35RecurrentScales,
}

#[derive(Clone, Debug, Default)]
pub struct Qwen35DenseFfnScales {
    pub gate: Option<GgufTensorInfo>,
    pub up: Option<GgufTensorInfo>,
    pub down: Option<GgufTensorInfo>,
}

#[derive(Clone, Debug)]
pub struct Qwen35DenseFfnTensors {
    pub gate: GgufTensorInfo,
    pub up: GgufTensorInfo,
    pub down: GgufTensorInfo,
    pub scales: Qwen35DenseFfnScales,
}

#[derive(Clone, Debug)]
pub struct Qwen35LayerTensors {
    pub index: u32,
    pub kind: Qwen35LayerKind,
    pub attn_norm: GgufTensorInfo,
    pub post_attention_norm: GgufTensorInfo,
    pub attention: Option<Qwen35AttentionTensors>,
    pub recurrent: Option<Qwen35RecurrentTensors>,
    pub ffn: Qwen35DenseFfnTensors,
}

#[derive(Clone, Debug)]
pub struct Qwen35Tensors {
    pub globals: Qwen35GlobalTensors,
    pub layers: Vec<Qwen35LayerTensors>,
}

impl Qwen35Tensors {
    pub fn from_model(model: &LlamaModel) -> Result<Self> {
        let cfg = model.require_qwen35()?;
        let token_embd = required_tensor(model, "token_embd.weight")?;
        let output_norm = required_tensor(model, "output_norm.weight")?;
        let output = optional_tensor(model, "output.weight").unwrap_or_else(|| token_embd.clone());

        let mut layers = Vec::with_capacity(cfg.block_count as usize);
        for index in 0..cfg.block_count {
            let kind = layer_kind(index, cfg.full_attention_interval)?;
            let attn_norm = required_tensor(model, &layer_name(index, "attn_norm", "weight"))?;
            let post_attention_norm =
                required_tensor(model, &layer_name(index, "post_attention_norm", "weight"))?;

            let attention = match kind {
                Qwen35LayerKind::Attention => Some(Qwen35AttentionTensors {
                    wq: required_tensor(model, &layer_name(index, "attn_q", "weight"))?,
                    wk: required_tensor(model, &layer_name(index, "attn_k", "weight"))?,
                    wv: required_tensor(model, &layer_name(index, "attn_v", "weight"))?,
                    wo: required_tensor(model, &layer_name(index, "attn_output", "weight"))?,
                    attn_q_norm: required_tensor(
                        model,
                        &layer_name(index, "attn_q_norm", "weight"),
                    )?,
                    attn_k_norm: required_tensor(
                        model,
                        &layer_name(index, "attn_k_norm", "weight"),
                    )?,
                    scales: Qwen35AttentionScales {
                        wq: optional_tensor(model, &layer_name(index, "attn_q", "scale")),
                        wk: optional_tensor(model, &layer_name(index, "attn_k", "scale")),
                        wv: optional_tensor(model, &layer_name(index, "attn_v", "scale")),
                        wo: optional_tensor(model, &layer_name(index, "attn_output", "scale")),
                    },
                }),
                Qwen35LayerKind::Recurrent => None,
            };

            let recurrent = match kind {
                Qwen35LayerKind::Attention => None,
                Qwen35LayerKind::Recurrent => Some(Qwen35RecurrentTensors {
                    wqkv: required_tensor(model, &layer_name(index, "attn_qkv", "weight"))?,
                    wqkv_gate: required_tensor(model, &layer_name(index, "attn_gate", "weight"))?,
                    ssm_conv1d: required_tensor(model, &layer_name(index, "ssm_conv1d", "weight"))?,
                    ssm_dt: required_tensor(model, &layer_name(index, "ssm_dt", "bias"))?,
                    ssm_a: required_tensor(model, &layer_scalar_name(index, "ssm_a"))?,
                    ssm_beta: required_tensor(model, &layer_name(index, "ssm_beta", "weight"))?,
                    ssm_alpha: required_tensor(model, &layer_name(index, "ssm_alpha", "weight"))?,
                    ssm_norm: required_tensor(model, &layer_name(index, "ssm_norm", "weight"))?,
                    ssm_out: required_tensor(model, &layer_name(index, "ssm_out", "weight"))?,
                    scales: Qwen35RecurrentScales {
                        wqkv: optional_tensor(model, &layer_name(index, "attn_qkv", "scale")),
                        wqkv_gate: optional_tensor(model, &layer_name(index, "attn_gate", "scale")),
                        ssm_out: optional_tensor(model, &layer_name(index, "ssm_out", "scale")),
                        ssm_alpha: optional_tensor(model, &layer_name(index, "ssm_alpha", "scale")),
                        ssm_beta: optional_tensor(model, &layer_name(index, "ssm_beta", "scale")),
                    },
                }),
            };

            layers.push(Qwen35LayerTensors {
                index,
                kind,
                attn_norm,
                post_attention_norm,
                attention,
                recurrent,
                ffn: Qwen35DenseFfnTensors {
                    gate: required_tensor(model, &layer_name(index, "ffn_gate", "weight"))?,
                    up: required_tensor(model, &layer_name(index, "ffn_up", "weight"))?,
                    down: required_tensor(model, &layer_name(index, "ffn_down", "weight"))?,
                    scales: Qwen35DenseFfnScales {
                        gate: optional_tensor(model, &layer_name(index, "ffn_gate", "scale")),
                        up: optional_tensor(model, &layer_name(index, "ffn_up", "scale")),
                        down: optional_tensor(model, &layer_name(index, "ffn_down", "scale")),
                    },
                },
            });
        }

        Ok(Self {
            globals: Qwen35GlobalTensors {
                token_embd,
                output_norm,
                output,
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
        visit(&self.globals.output_norm);
        visit(&self.globals.output);

        for layer in &self.layers {
            visit(&layer.attn_norm);
            visit(&layer.post_attention_norm);

            if let Some(attention) = &layer.attention {
                visit(&attention.wq);
                visit(&attention.wk);
                visit(&attention.wv);
                visit(&attention.wo);
                visit(&attention.attn_q_norm);
                visit(&attention.attn_k_norm);
                visit_optional(&attention.scales.wq, &mut visit);
                visit_optional(&attention.scales.wk, &mut visit);
                visit_optional(&attention.scales.wv, &mut visit);
                visit_optional(&attention.scales.wo, &mut visit);
            }

            if let Some(recurrent) = &layer.recurrent {
                visit(&recurrent.wqkv);
                visit(&recurrent.wqkv_gate);
                visit(&recurrent.ssm_conv1d);
                visit(&recurrent.ssm_dt);
                visit(&recurrent.ssm_a);
                visit(&recurrent.ssm_beta);
                visit(&recurrent.ssm_alpha);
                visit(&recurrent.ssm_norm);
                visit(&recurrent.ssm_out);
                visit_optional(&recurrent.scales.wqkv, &mut visit);
                visit_optional(&recurrent.scales.wqkv_gate, &mut visit);
                visit_optional(&recurrent.scales.ssm_out, &mut visit);
                visit_optional(&recurrent.scales.ssm_alpha, &mut visit);
                visit_optional(&recurrent.scales.ssm_beta, &mut visit);
            }

            visit(&layer.ffn.gate);
            visit(&layer.ffn.up);
            visit(&layer.ffn.down);
            visit_optional(&layer.ffn.scales.gate, &mut visit);
            visit_optional(&layer.ffn.scales.up, &mut visit);
            visit_optional(&layer.ffn.scales.down, &mut visit);
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

fn layer_kind(index: u32, full_attention_interval: u32) -> Result<Qwen35LayerKind> {
    if full_attention_interval == 0 {
        return Err(LlamaError::format(
            "qwen35.full_attention_interval must be greater than zero",
        ));
    }
    if (index + 1) % full_attention_interval == 0 {
        Ok(Qwen35LayerKind::Attention)
    } else {
        Ok(Qwen35LayerKind::Recurrent)
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

fn layer_scalar_name(index: u32, stem: &str) -> String {
    format!("blk.{}.{}", index, stem)
}

#[cfg(test)]
mod tests {
    use super::{layer_kind, Qwen35LayerKind};

    #[test]
    fn layer_kind_matches_llama_cpp_interval_rule() {
        let kinds = (0..8)
            .map(|index| layer_kind(index, 4).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![
                Qwen35LayerKind::Recurrent,
                Qwen35LayerKind::Recurrent,
                Qwen35LayerKind::Recurrent,
                Qwen35LayerKind::Attention,
                Qwen35LayerKind::Recurrent,
                Qwen35LayerKind::Recurrent,
                Qwen35LayerKind::Recurrent,
                Qwen35LayerKind::Attention,
            ]
        );
    }
}
