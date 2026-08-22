//! Exact checkpoint contract for the full HY-Motion 1.0 model.
//!
//! The official `latest.ckpt` stores these tensors below
//! `model_state_dict`.  Opening the archive is metadata-only: PyTorch
//! storages remain on disk until [`HyMotionCheckpoint::f32`] is called.

use std::path::Path;

use crate::hy_motion::{
    HY_MOTION_CONTEXT_DIM, HY_MOTION_DOUBLE_LAYERS, HY_MOTION_HEAD_DIM,
    HY_MOTION_HIDDEN, HY_MOTION_INPUT_DIM, HY_MOTION_MLP, HY_MOTION_OUTPUT_DIM,
    HY_MOTION_SINGLE_LAYERS, HY_MOTION_VECTOR_DIM,
};
use crate::torch_pth::PthStateDict;
use crate::{DiffusionError, Result};

pub const HY_MOTION_TEXT_REFINER_LAYERS: usize = 2;
pub const HY_MOTION_REQUIRED_TENSORS: usize = 426;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HyMotionTensorSpec {
    /// Name relative to the official `model_state_dict` object.
    pub name: String,
    pub shape: Vec<usize>,
}

impl HyMotionTensorSpec {
    pub fn parameter_count(&self) -> usize {
        self.shape.iter().product()
    }
}

fn push(specs: &mut Vec<HyMotionTensorSpec>, name: impl Into<String>, shape: &[usize]) {
    specs.push(HyMotionTensorSpec {
        name: name.into(),
        shape: shape.to_vec(),
    });
}

fn push_linear(
    specs: &mut Vec<HyMotionTensorSpec>,
    prefix: &str,
    output: usize,
    input: usize,
) {
    push(specs, format!("{prefix}.weight"), &[output, input]);
    push(specs, format!("{prefix}.bias"), &[output]);
}

/// Every tensor needed for full-model prompt-to-latent inference.
///
/// The body-model buffers and the optional "special game" prompt constants
/// are intentionally excluded.  They are neither read by the normal text
/// path nor by the released 22-joint decoder used for generated motion.
pub fn hy_motion_tensor_specs() -> Vec<HyMotionTensorSpec> {
    let mut specs = Vec::with_capacity(HY_MOTION_REQUIRED_TENSORS);

    push_linear(
        &mut specs,
        "motion_transformer.input_encoder",
        HY_MOTION_HIDDEN,
        HY_MOTION_INPUT_DIM,
    );
    push_linear(
        &mut specs,
        "motion_transformer.ctxt_encoder",
        HY_MOTION_HIDDEN,
        HY_MOTION_CONTEXT_DIM,
    );
    push_linear(
        &mut specs,
        "motion_transformer.vtxt_encoder.linears.0",
        HY_MOTION_HIDDEN,
        HY_MOTION_VECTOR_DIM,
    );
    push_linear(
        &mut specs,
        "motion_transformer.vtxt_encoder.linears.2",
        HY_MOTION_HIDDEN,
        HY_MOTION_HIDDEN,
    );
    push_linear(
        &mut specs,
        "motion_transformer.timestep_encoder.blocks.0",
        HY_MOTION_HIDDEN,
        HY_MOTION_HIDDEN,
    );
    push_linear(
        &mut specs,
        "motion_transformer.timestep_encoder.blocks.2",
        HY_MOTION_HIDDEN,
        HY_MOTION_HIDDEN,
    );

    push_linear(
        &mut specs,
        "motion_transformer.text_refiner.input_embedder",
        HY_MOTION_HIDDEN,
        HY_MOTION_HIDDEN,
    );
    push_linear(
        &mut specs,
        "motion_transformer.text_refiner.context_encoder.linears.0",
        HY_MOTION_HIDDEN,
        HY_MOTION_HIDDEN,
    );
    push_linear(
        &mut specs,
        "motion_transformer.text_refiner.context_encoder.linears.2",
        HY_MOTION_HIDDEN,
        HY_MOTION_HIDDEN,
    );
    push_linear(
        &mut specs,
        "motion_transformer.text_refiner.timestep_encoder.blocks.0",
        HY_MOTION_HIDDEN,
        HY_MOTION_HIDDEN,
    );
    push_linear(
        &mut specs,
        "motion_transformer.text_refiner.timestep_encoder.blocks.2",
        HY_MOTION_HIDDEN,
        HY_MOTION_HIDDEN,
    );
    for block in 0..HY_MOTION_TEXT_REFINER_LAYERS {
        let prefix = format!(
            "motion_transformer.text_refiner.individual_token_refiner.blocks.{block}"
        );
        for norm in ["norm1", "norm2"] {
            push(
                &mut specs,
                format!("{prefix}.{norm}.weight"),
                &[HY_MOTION_HIDDEN],
            );
            push(
                &mut specs,
                format!("{prefix}.{norm}.bias"),
                &[HY_MOTION_HIDDEN],
            );
        }
        push_linear(
            &mut specs,
            &format!("{prefix}.self_attn_qkv"),
            3 * HY_MOTION_HIDDEN,
            HY_MOTION_HIDDEN,
        );
        for qk in ["q", "k"] {
            push(
                &mut specs,
                format!("{prefix}.self_attn_{qk}_norm.weight"),
                &[HY_MOTION_HEAD_DIM],
            );
            push(
                &mut specs,
                format!("{prefix}.self_attn_{qk}_norm.bias"),
                &[HY_MOTION_HEAD_DIM],
            );
        }
        push_linear(
            &mut specs,
            &format!("{prefix}.self_attn_proj"),
            HY_MOTION_HIDDEN,
            HY_MOTION_HIDDEN,
        );
        push_linear(
            &mut specs,
            &format!("{prefix}.mlp.fc1"),
            HY_MOTION_MLP,
            HY_MOTION_HIDDEN,
        );
        push_linear(
            &mut specs,
            &format!("{prefix}.mlp.fc2"),
            HY_MOTION_HIDDEN,
            HY_MOTION_MLP,
        );
        push_linear(
            &mut specs,
            &format!("{prefix}.adaLN_modulation.linear"),
            2 * HY_MOTION_HIDDEN,
            HY_MOTION_HIDDEN,
        );
    }

    for block in 0..HY_MOTION_DOUBLE_LAYERS {
        let prefix = format!("motion_transformer.double_blocks.{block}");
        for stream in ["motion", "text"] {
            push_linear(
                &mut specs,
                &format!("{prefix}.{stream}_mod.linear"),
                6 * HY_MOTION_HIDDEN,
                HY_MOTION_HIDDEN,
            );
            push_linear(
                &mut specs,
                &format!("{prefix}.{stream}_qkv"),
                3 * HY_MOTION_HIDDEN,
                HY_MOTION_HIDDEN,
            );
            // The main MMDiT uses affine-free, per-head RMSNorm for Q/K.
            for qk in ["q", "k"] {
                push(
                    &mut specs,
                    format!("{prefix}.{stream}_{qk}_norm.weight"),
                    &[HY_MOTION_HEAD_DIM],
                );
            }
            push_linear(
                &mut specs,
                &format!("{prefix}.{stream}_out_proj"),
                HY_MOTION_HIDDEN,
                HY_MOTION_HIDDEN,
            );
            push_linear(
                &mut specs,
                &format!("{prefix}.{stream}_mlp.fc1"),
                HY_MOTION_MLP,
                HY_MOTION_HIDDEN,
            );
            push_linear(
                &mut specs,
                &format!("{prefix}.{stream}_mlp.fc2"),
                HY_MOTION_HIDDEN,
                HY_MOTION_MLP,
            );
        }
    }

    for block in 0..HY_MOTION_SINGLE_LAYERS {
        let prefix = format!("motion_transformer.single_blocks.{block}");
        push_linear(
            &mut specs,
            &format!("{prefix}.modulation.linear"),
            3 * HY_MOTION_HIDDEN,
            HY_MOTION_HIDDEN,
        );
        // Q, K, V and the MLP's expanded branch share one projection.
        push_linear(
            &mut specs,
            &format!("{prefix}.linear1"),
            3 * HY_MOTION_HIDDEN + HY_MOTION_MLP,
            HY_MOTION_HIDDEN,
        );
        push_linear(
            &mut specs,
            &format!("{prefix}.linear2"),
            HY_MOTION_HIDDEN,
            HY_MOTION_HIDDEN + HY_MOTION_MLP,
        );
        for qk in ["q", "k"] {
            push(
                &mut specs,
                format!("{prefix}.{qk}_norm.weight"),
                &[HY_MOTION_HEAD_DIM],
            );
        }
    }

    push_linear(
        &mut specs,
        "motion_transformer.final_layer.adaLN_modulation.linear",
        2 * HY_MOTION_HIDDEN,
        HY_MOTION_HIDDEN,
    );
    push_linear(
        &mut specs,
        "motion_transformer.final_layer.linear",
        HY_MOTION_OUTPUT_DIM,
        HY_MOTION_HIDDEN,
    );

    // Classifier-free null conditioning and latent normalization buffers are
    // pipeline state, not children of `motion_transformer`.
    push(
        &mut specs,
        "null_vtxt_feat",
        &[1, 1, HY_MOTION_VECTOR_DIM],
    );
    push(
        &mut specs,
        "null_ctxt_input",
        &[1, 1, HY_MOTION_CONTEXT_DIM],
    );
    push(&mut specs, "mean", &[HY_MOTION_INPUT_DIM]);
    push(&mut specs, "std", &[HY_MOTION_INPUT_DIM]);

    debug_assert_eq!(specs.len(), HY_MOTION_REQUIRED_TENSORS);
    specs
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HyMotionCheckpointReport {
    pub archive_tensor_count: usize,
    pub required_tensor_count: usize,
    pub required_parameter_count: usize,
    pub checkpoint_prefix: String,
}

/// Lazy, shape-validated view of the official full-model checkpoint.
pub struct HyMotionCheckpoint {
    state: PthStateDict,
    prefix: String,
}

impl HyMotionCheckpoint {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let state = PthStateDict::load_nested(path)?;
        let sentinel = "motion_transformer.input_encoder.weight";
        let prefix = ["model_state_dict.", "state_dict.", ""]
            .into_iter()
            .find(|prefix| state.has(&format!("{prefix}{sentinel}")))
            .ok_or_else(|| {
                DiffusionError::model(
                    "HY-Motion checkpoint does not contain the full motion transformer",
                )
            })?
            .to_string();
        let checkpoint = Self { state, prefix };
        checkpoint.validate()?;
        Ok(checkpoint)
    }

    pub fn validate(&self) -> Result<HyMotionCheckpointReport> {
        let specs = hy_motion_tensor_specs();
        for spec in &specs {
            let archive_name = self.archive_name(&spec.name);
            let actual = self.state.shape(&archive_name).map_err(|_| {
                DiffusionError::model(format!(
                    "HY-Motion full checkpoint tensor missing: {}",
                    spec.name
                ))
            })?;
            if actual != spec.shape {
                return Err(DiffusionError::model(format!(
                    "HY-Motion checkpoint tensor {} has shape {:?}, expected {:?}",
                    spec.name, actual, spec.shape
                )));
            }
        }
        Ok(HyMotionCheckpointReport {
            archive_tensor_count: self.state.names().count(),
            required_tensor_count: specs.len(),
            required_parameter_count: specs.iter().map(|spec| spec.parameter_count()).sum(),
            checkpoint_prefix: self.prefix.clone(),
        })
    }

    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    pub fn f32(&mut self, relative_name: &str) -> Result<Vec<f32>> {
        let spec = hy_motion_tensor_specs()
            .into_iter()
            .find(|spec| spec.name == relative_name)
            .ok_or_else(|| {
                DiffusionError::model(format!(
                    "not a required HY-Motion tensor: {relative_name}"
                ))
            })?;
        let archive_name = self.archive_name(relative_name);
        self.state.f32_shaped(&archive_name, &spec.shape)
    }

    fn archive_name(&self, relative_name: &str) -> String {
        format!("{}{relative_name}", self.prefix)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn full_checkpoint_contract_has_expected_size_and_unique_names() {
        let specs = hy_motion_tensor_specs();
        assert_eq!(specs.len(), HY_MOTION_REQUIRED_TENSORS);
        assert_eq!(
            specs
                .iter()
                .map(|spec| spec.name.as_str())
                .collect::<HashSet<_>>()
                .len(),
            specs.len()
        );
        assert_eq!(
            specs.iter().map(|spec| spec.parameter_count()).sum::<usize>(),
            1_042_873_947
        );
    }

    #[test]
    fn full_checkpoint_contract_rejects_lite_shapes_by_construction() {
        let specs = hy_motion_tensor_specs();
        let input = specs
            .iter()
            .find(|spec| spec.name == "motion_transformer.input_encoder.weight")
            .unwrap();
        assert_eq!(input.shape, [1280, 201]);
        let last = specs
            .iter()
            .find(|spec| spec.name == "motion_transformer.single_blocks.17.linear1.weight")
            .unwrap();
        assert_eq!(last.shape, [8960, 1280]);
    }
}
