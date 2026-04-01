use std::collections::BTreeMap;

use crate::error::{LlamaError, Result};
use crate::gguf::GgufTensorInfo;
use crate::model::LlamaModel;
use crate::weights::GgufWeightLayout;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Qwen35MoeLayerKind {
    Attention,
    Recurrent,
}

impl Qwen35MoeLayerKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Attention => "attention",
            Self::Recurrent => "recurrent",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Qwen35MoeGlobalTensors {
    pub token_embd: GgufTensorInfo,
    pub output_norm: GgufTensorInfo,
    pub output: GgufTensorInfo,
}

#[derive(Clone, Debug, Default)]
pub struct Qwen35MoeAttentionScales {
    pub wq: Option<GgufTensorInfo>,
    pub wk: Option<GgufTensorInfo>,
    pub wv: Option<GgufTensorInfo>,
    pub wo: Option<GgufTensorInfo>,
}

#[derive(Clone, Debug)]
pub struct Qwen35MoeAttentionTensors {
    pub wq: GgufTensorInfo,
    pub wk: GgufTensorInfo,
    pub wv: GgufTensorInfo,
    pub wo: GgufTensorInfo,
    pub attn_q_norm: GgufTensorInfo,
    pub attn_k_norm: GgufTensorInfo,
    pub scales: Qwen35MoeAttentionScales,
}

#[derive(Clone, Debug, Default)]
pub struct Qwen35MoeRecurrentScales {
    pub wqkv: Option<GgufTensorInfo>,
    pub wqkv_gate: Option<GgufTensorInfo>,
    pub ssm_out: Option<GgufTensorInfo>,
    pub ssm_alpha: Option<GgufTensorInfo>,
    pub ssm_beta: Option<GgufTensorInfo>,
}

#[derive(Clone, Debug)]
pub struct Qwen35MoeRecurrentTensors {
    pub wqkv: GgufTensorInfo,
    pub wqkv_gate: GgufTensorInfo,
    pub ssm_conv1d: GgufTensorInfo,
    pub ssm_dt: GgufTensorInfo,
    pub ssm_a: GgufTensorInfo,
    pub ssm_beta: GgufTensorInfo,
    pub ssm_alpha: GgufTensorInfo,
    pub ssm_norm: GgufTensorInfo,
    pub ssm_out: GgufTensorInfo,
    pub scales: Qwen35MoeRecurrentScales,
}

#[derive(Clone, Debug, Default)]
pub struct Qwen35MoeMoeScales {
    pub ffn_gate_exps: Option<GgufTensorInfo>,
    pub ffn_up_exps: Option<GgufTensorInfo>,
    pub ffn_down_exps: Option<GgufTensorInfo>,
    pub ffn_gate_shexp: Option<GgufTensorInfo>,
    pub ffn_up_shexp: Option<GgufTensorInfo>,
    pub ffn_down_shexp: Option<GgufTensorInfo>,
}

#[derive(Clone, Debug)]
pub struct Qwen35MoeMoeTensors {
    pub ffn_gate_inp: GgufTensorInfo,
    pub ffn_gate_up_exps: Option<GgufTensorInfo>,
    pub ffn_gate_exps: Option<GgufTensorInfo>,
    pub ffn_up_exps: Option<GgufTensorInfo>,
    pub ffn_down_exps: GgufTensorInfo,
    pub ffn_gate_inp_shexp: GgufTensorInfo,
    pub ffn_gate_shexp: GgufTensorInfo,
    pub ffn_up_shexp: GgufTensorInfo,
    pub ffn_down_shexp: GgufTensorInfo,
    pub scales: Qwen35MoeMoeScales,
}

impl Qwen35MoeMoeTensors {
    pub fn uses_merged_gate_up(&self) -> bool {
        self.ffn_gate_up_exps.is_some()
    }
}

#[derive(Clone, Debug)]
pub struct Qwen35MoeLayerTensors {
    pub index: u32,
    pub kind: Qwen35MoeLayerKind,
    pub attn_norm: GgufTensorInfo,
    pub post_attention_norm: GgufTensorInfo,
    pub attention: Option<Qwen35MoeAttentionTensors>,
    pub recurrent: Option<Qwen35MoeRecurrentTensors>,
    pub moe: Qwen35MoeMoeTensors,
}

#[derive(Clone, Debug)]
pub struct Qwen35MoeTensors {
    pub globals: Qwen35MoeGlobalTensors,
    pub layers: Vec<Qwen35MoeLayerTensors>,
}

impl Qwen35MoeTensors {
    pub fn from_model(model: &LlamaModel) -> Result<Self> {
        let cfg = model.require_qwen35moe()?;
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
                Qwen35MoeLayerKind::Attention => Some(Qwen35MoeAttentionTensors {
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
                    scales: Qwen35MoeAttentionScales {
                        wq: optional_tensor(model, &layer_name(index, "attn_q", "scale")),
                        wk: optional_tensor(model, &layer_name(index, "attn_k", "scale")),
                        wv: optional_tensor(model, &layer_name(index, "attn_v", "scale")),
                        wo: optional_tensor(model, &layer_name(index, "attn_output", "scale")),
                    },
                }),
                Qwen35MoeLayerKind::Recurrent => None,
            };

            let recurrent = match kind {
                Qwen35MoeLayerKind::Attention => None,
                Qwen35MoeLayerKind::Recurrent => Some(Qwen35MoeRecurrentTensors {
                    wqkv: required_tensor(model, &layer_name(index, "attn_qkv", "weight"))?,
                    wqkv_gate: required_tensor(model, &layer_name(index, "attn_gate", "weight"))?,
                    ssm_conv1d: required_tensor(model, &layer_name(index, "ssm_conv1d", "weight"))?,
                    ssm_dt: required_tensor(model, &layer_name(index, "ssm_dt", "bias"))?,
                    ssm_a: required_tensor(model, &layer_scalar_name(index, "ssm_a"))?,
                    ssm_beta: required_tensor(model, &layer_name(index, "ssm_beta", "weight"))?,
                    ssm_alpha: required_tensor(model, &layer_name(index, "ssm_alpha", "weight"))?,
                    ssm_norm: required_tensor(model, &layer_name(index, "ssm_norm", "weight"))?,
                    ssm_out: required_tensor(model, &layer_name(index, "ssm_out", "weight"))?,
                    scales: Qwen35MoeRecurrentScales {
                        wqkv: optional_tensor(model, &layer_name(index, "attn_qkv", "scale")),
                        wqkv_gate: optional_tensor(model, &layer_name(index, "attn_gate", "scale")),
                        ssm_out: optional_tensor(model, &layer_name(index, "ssm_out", "scale")),
                        ssm_alpha: optional_tensor(model, &layer_name(index, "ssm_alpha", "scale")),
                        ssm_beta: optional_tensor(model, &layer_name(index, "ssm_beta", "scale")),
                    },
                }),
            };

            let ffn_gate_up_exps = optional_tensor(model, &layer_name(index, "ffn_gate_up_exps", "weight"));
            let ffn_gate_exps = optional_tensor(model, &layer_name(index, "ffn_gate_exps", "weight"));
            let ffn_up_exps = optional_tensor(model, &layer_name(index, "ffn_up_exps", "weight"));
            if ffn_gate_up_exps.is_none() && (ffn_gate_exps.is_none() || ffn_up_exps.is_none()) {
                return Err(LlamaError::format(format!(
                    "layer {} is missing expert gate/up weights: expected either blk.{}.ffn_gate_up_exps.weight or both blk.{}.ffn_gate_exps.weight and blk.{}.ffn_up_exps.weight",
                    index, index, index, index
                )));
            }

            layers.push(Qwen35MoeLayerTensors {
                index,
                kind,
                attn_norm,
                post_attention_norm,
                attention,
                recurrent,
                moe: Qwen35MoeMoeTensors {
                    ffn_gate_inp: required_tensor(model, &layer_name(index, "ffn_gate_inp", "weight"))?,
                    ffn_gate_up_exps,
                    ffn_gate_exps,
                    ffn_up_exps,
                    ffn_down_exps: required_tensor(
                        model,
                        &layer_name(index, "ffn_down_exps", "weight"),
                    )?,
                    ffn_gate_inp_shexp: required_tensor(
                        model,
                        &layer_name(index, "ffn_gate_inp_shexp", "weight"),
                    )?,
                    ffn_gate_shexp: required_tensor(
                        model,
                        &layer_name(index, "ffn_gate_shexp", "weight"),
                    )?,
                    ffn_up_shexp: required_tensor(
                        model,
                        &layer_name(index, "ffn_up_shexp", "weight"),
                    )?,
                    ffn_down_shexp: required_tensor(
                        model,
                        &layer_name(index, "ffn_down_shexp", "weight"),
                    )?,
                    scales: Qwen35MoeMoeScales {
                        ffn_gate_exps: optional_tensor(
                            model,
                            &layer_name(index, "ffn_gate_exps", "scale"),
                        ),
                        ffn_up_exps: optional_tensor(
                            model,
                            &layer_name(index, "ffn_up_exps", "scale"),
                        ),
                        ffn_down_exps: optional_tensor(
                            model,
                            &layer_name(index, "ffn_down_exps", "scale"),
                        ),
                        ffn_gate_shexp: optional_tensor(
                            model,
                            &layer_name(index, "ffn_gate_shexp", "scale"),
                        ),
                        ffn_up_shexp: optional_tensor(
                            model,
                            &layer_name(index, "ffn_up_shexp", "scale"),
                        ),
                        ffn_down_shexp: optional_tensor(
                            model,
                            &layer_name(index, "ffn_down_shexp", "scale"),
                        ),
                    },
                },
            });
        }

        Ok(Self {
            globals: Qwen35MoeGlobalTensors {
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

            visit(&layer.moe.ffn_gate_inp);
            visit_optional(&layer.moe.ffn_gate_up_exps, &mut visit);
            visit_optional(&layer.moe.ffn_gate_exps, &mut visit);
            visit_optional(&layer.moe.ffn_up_exps, &mut visit);
            visit(&layer.moe.ffn_down_exps);
            visit(&layer.moe.ffn_gate_inp_shexp);
            visit(&layer.moe.ffn_gate_shexp);
            visit(&layer.moe.ffn_up_shexp);
            visit(&layer.moe.ffn_down_shexp);
            visit_optional(&layer.moe.scales.ffn_gate_exps, &mut visit);
            visit_optional(&layer.moe.scales.ffn_up_exps, &mut visit);
            visit_optional(&layer.moe.scales.ffn_down_exps, &mut visit);
            visit_optional(&layer.moe.scales.ffn_gate_shexp, &mut visit);
            visit_optional(&layer.moe.scales.ffn_up_shexp, &mut visit);
            visit_optional(&layer.moe.scales.ffn_down_shexp, &mut visit);
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

fn layer_kind(index: u32, full_attention_interval: u32) -> Result<Qwen35MoeLayerKind> {
    if full_attention_interval == 0 {
        return Err(LlamaError::format(
            "qwen35moe.full_attention_interval must be greater than zero",
        ));
    }
    if (index + 1) % full_attention_interval == 0 {
        Ok(Qwen35MoeLayerKind::Attention)
    } else {
        Ok(Qwen35MoeLayerKind::Recurrent)
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
    use super::{layer_kind, Qwen35MoeLayerKind};

    #[test]
    fn layer_kind_matches_llama_cpp_interval_rule() {
        let kinds = (0..8)
            .map(|index| layer_kind(index, 4).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![
                Qwen35MoeLayerKind::Recurrent,
                Qwen35MoeLayerKind::Recurrent,
                Qwen35MoeLayerKind::Recurrent,
                Qwen35MoeLayerKind::Attention,
                Qwen35MoeLayerKind::Recurrent,
                Qwen35MoeLayerKind::Recurrent,
                Qwen35MoeLayerKind::Recurrent,
                Qwen35MoeLayerKind::Attention,
            ]
        );
    }
}
