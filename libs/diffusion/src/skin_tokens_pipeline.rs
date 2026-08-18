//! Persistent, production SkinTokens mesh-to-rig pipeline.
//!
//! This is the native boundary consumed by ai-content. It deliberately owns
//! the complete order of operations so a service cannot accidentally insert
//! row normalization before IDW transfer, feed only 256 Michelangelo tokens,
//! or reproduce the released Python processor's final-FSQ off-by-one bug.

use crate::skin_tokens::SkinTokensWeights;
use crate::skin_tokens_decode::{
    decode_skin_tokens_weights, unload_skin_tokens_decode_weights,
};
use crate::skin_tokens_mesh::SkinTokensMesh;
use crate::skin_tokens_neural::{
    encode_mesh_prefix_controlled, encode_vae_condition_controlled,
    unload_skin_tokens_neural_weights,
};
use crate::skin_tokens_output::skin_tokens_rig_glb_with_progress;
use crate::skin_tokens_qwen::{
    skin_tokens_qwen_generate, unload_skin_tokens_qwen_weights, SkinTokensGeneration,
    SkinTokensGenerationGrammar, SkinTokensGenerationParams, SkinTokensGenerationProgress,
    SkinTokensQwenPrepared,
};
use crate::skin_tokens_tokenizer::{
    skin_tokens_detokenize_skeleton, SkinTokensGenerationPhase, SkinTokensSkeleton,
};
use crate::{DiffusionError, ProgressHook, Result};
use std::path::Path;

/// Stable production defaults. The request seed controls mesh sampling, VAE
/// conditioning selection and autoregressive sampling. Michelangelo query
/// selection intentionally remains the released eval-mode seed zero.
#[derive(Clone, Debug)]
pub struct SkinTokensPipelineParams {
    pub seed: u64,
    pub generation: SkinTokensGenerationParams,
}

impl Default for SkinTokensPipelineParams {
    fn default() -> Self {
        Self {
            seed: 0,
            generation: SkinTokensGenerationParams::default(),
        }
    }
}

/// Final native result plus the semantic data needed by service validation.
/// Dense 54k sample weights are intentionally not retained after GLB export.
pub struct SkinTokensPipelineOutput {
    pub glb: Vec<u8>,
    pub skeleton: SkinTokensSkeleton,
    pub generation: SkinTokensGeneration,
    pub source_vertices: usize,
    pub sampled_points: usize,
}

/// Header/little-vector state retained across jobs. Large neural matrices are
/// streamed into the shared CUDA cache on first use and remain resident until
/// [`unload_skin_tokens_runtime_weights`] runs on this same worker thread.
pub struct SkinTokensPipeline {
    weights: SkinTokensWeights,
    qwen: SkinTokensQwenPrepared,
}

impl SkinTokensPipeline {
    pub fn load(checkpoint: impl AsRef<Path>) -> Result<Self> {
        let weights = SkinTokensWeights::load(checkpoint)?;
        let qwen = SkinTokensQwenPrepared::prepare(&weights)?;
        Ok(Self { weights, qwen })
    }

    pub fn weights(&self) -> &SkinTokensWeights {
        &self.weights
    }

    /// Rig one source GLB without Python, Torch, SciPy or Blender.
    ///
    /// `cancel` is checked at every host stage, between native transformer
    /// blocks, per generated token and per decoded joint. An in-flight CUDA
    /// kernel remains the smallest non-preemptible unit.
    pub fn rig_glb(
        &self,
        input_glb: &[u8],
        params: &SkinTokensPipelineParams,
        cancel: Option<&dyn Fn() -> bool>,
        mut progress: Option<ProgressHook<'_>>,
    ) -> Result<SkinTokensPipelineOutput> {
        if params.generation.grammar != SkinTokensGenerationGrammar::Strict {
            return Err(DiffusionError::workflow(
                "production SkinTokens pipeline requires strict four-FSQ-per-joint grammar",
            ));
        }
        check_cancel(cancel)?;
        emit(&mut progress, "rig: parse mesh", 0.0)?;
        let mesh = SkinTokensMesh::from_glb(input_glb)?;
        check_cancel(cancel)?;

        emit(&mut progress, "rig: sample surface", 0.025)?;
        // NumPy's legacy RandomState contract is a 32-bit seed. Truncation is
        // explicit and deterministic for the service's u64 request seed.
        let samples = mesh.sample(params.seed as u32)?;
        let condition = samples.condition_f32()?;
        check_cancel(cancel)?;

        emit(&mut progress, "rig: encode skin condition", 0.06)?;
        let vae = encode_vae_condition_controlled(
            &self.weights,
            &condition,
            params.seed,
            cancel,
        )?;
        check_cancel(cancel)?;

        emit(&mut progress, "rig: encode mesh prefix", 0.14)?;
        let mesh_prefix =
            encode_mesh_prefix_controlled(&self.weights, &condition, cancel)?;
        check_cancel(cancel)?;

        let mut generation_params = params.generation.clone();
        generation_params.seed = params.seed;
        emit(&mut progress, "rig: generate skeleton", 0.25)?;
        let mut generation_progress = |state: &SkinTokensGenerationProgress| -> Result<()> {
            check_cancel(cancel)?;
            let (label, phase_fraction) = generation_fraction(state);
            emit(
                &mut progress,
                label,
                0.25 + 0.43 * phase_fraction.clamp(0.0, 1.0),
            )
        };
        let generation = skin_tokens_qwen_generate(
            &self.weights,
            &self.qwen,
            &mesh_prefix.prefix,
            &generation_params,
            Some(&mut generation_progress),
        )?;
        check_cancel(cancel)?;

        let skeleton = skin_tokens_detokenize_skeleton(&generation.skeleton_ids)?;
        if skeleton.joints.len() != generation.fsq_indices.len() {
            return Err(DiffusionError::workflow(format!(
                "SkinTokens generated {} joints but {} FSQ groups",
                skeleton.joints.len(),
                generation.fsq_indices.len(),
            )));
        }
        let fsq_indices = flatten_fsq_indices(&generation)?;

        emit(&mut progress, "rig: decode skin weights", 0.68)?;
        let mut decode_progress = |label: &str, fraction: f64| -> Result<()> {
            check_cancel(cancel)?;
            emit(
                &mut progress,
                label,
                0.68 + 0.17 * fraction.clamp(0.0, 1.0),
            )
        };
        let sample_weights = decode_skin_tokens_weights(
            &self.weights,
            &fsq_indices,
            &condition,
            &vae.latents,
            Some(&mut decode_progress),
        )?;
        check_cancel(cancel)?;

        emit(&mut progress, "rig: transfer skin weights", 0.85)?;
        let mut output_progress = |label: &str, fraction: f64| -> Result<()> {
            check_cancel(cancel)?;
            emit(
                &mut progress,
                label,
                0.85 + 0.15 * fraction.clamp(0.0, 1.0),
            )
        };
        let glb = skin_tokens_rig_glb_with_progress(
            input_glb,
            &mesh,
            &samples,
            &skeleton,
            &sample_weights,
            Some(&mut output_progress),
        )?;
        check_cancel(cancel)?;
        emit(&mut progress, "rig: done", 1.0)?;
        Ok(SkinTokensPipelineOutput {
            glb,
            skeleton,
            generation,
            source_vertices: mesh.source_positions.len(),
            sampled_points: samples.positions.len(),
        })
    }
}

fn flatten_fsq_indices(generation: &SkinTokensGeneration) -> Result<Vec<usize>> {
    let mut flat = Vec::with_capacity(generation.fsq_indices.len() * 4);
    for group in &generation.fsq_indices {
        for &index in group {
            let index = index as usize;
            if index >= 32_768 {
                return Err(DiffusionError::workflow(format!(
                    "SkinTokens generated out-of-range FSQ index {index}",
                )));
            }
            flat.push(index);
        }
    }
    Ok(flat)
}

fn generation_fraction(state: &SkinTokensGenerationProgress) -> (&'static str, f64) {
    match state.phase {
        SkinTokensGenerationPhase::Skeleton => (
            "rig: generate skeleton",
            0.45 * (state.generated as f64 / 160.0).min(1.0),
        ),
        SkinTokensGenerationPhase::Skin { bones, generated } => {
            let required = (bones * 4).max(1);
            (
                "rig: generate skin codes",
                0.45 + 0.55 * (generated as f64 / required as f64).min(1.0),
            )
        }
        SkinTokensGenerationPhase::Complete { .. } => ("rig: generate complete", 1.0),
    }
}

fn check_cancel(cancel: Option<&dyn Fn() -> bool>) -> Result<()> {
    if cancel.is_some_and(|is_cancelled| is_cancelled()) {
        Err(DiffusionError::Cancelled)
    } else {
        Ok(())
    }
}

fn emit(progress: &mut Option<ProgressHook<'_>>, label: &str, fraction: f64) -> Result<()> {
    match progress {
        Some(progress) => progress(label, fraction),
        None => Ok(()),
    }
}

/// Evict all three TokenRig matrix namespaces. Call this only after dropping
/// every activation/pipeline value, and on the CUDA worker thread which ran
/// inference so thread-local caches are released deterministically.
pub fn unload_skin_tokens_runtime_weights() -> Result<()> {
    let _ = unload_skin_tokens_qwen_weights()?;
    unload_skin_tokens_decode_weights()?;
    unload_skin_tokens_neural_weights()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_defaults_are_strict_and_seed_is_forwarded() {
        let params = SkinTokensPipelineParams::default();
        assert_eq!(params.generation.grammar, SkinTokensGenerationGrammar::Strict);
        assert_eq!(params.seed, 0);
    }

    #[test]
    fn progress_bands_are_monotonic_at_phase_boundary() {
        let skeleton = SkinTokensGenerationProgress {
            generated: 160,
            max_length: 2_048,
            active_beams: 10,
            phase: SkinTokensGenerationPhase::Skeleton,
        };
        let skin = SkinTokensGenerationProgress {
            generated: 161,
            max_length: 2_048,
            active_beams: 10,
            phase: SkinTokensGenerationPhase::Skin {
                bones: 34,
                generated: 0,
            },
        };
        assert!(generation_fraction(&skeleton).1 <= generation_fraction(&skin).1);
    }
}
