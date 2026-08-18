//! Native CLIP sentence conditioner for HY-Motion 1.0.
//!
//! The released asset is a complete HuggingFace `CLIPModel`, but HY-Motion
//! consumes only `CLIPTextModel.pooler_output`. Loading therefore uses the
//! text-only scope and executes the exact padded 77-token causal shape.

use std::path::{Path, PathBuf};

use crate::backend::gpu_weight_cache_evict_prefix;
use makepad_ai_flux::clip::{ClipTokenChunk, ClipTokenizer};
use makepad_ai_flux::clip_l::{
    clip_cache_namespace, ClipLExecutionMode, CompiledClipL, LoadedClipLWeights,
};
use crate::hy_motion::HY_MOTION_VECTOR_DIM;
use crate::{DiffusionError, Result};

pub const HY_MOTION_CLIP_TOKENS: usize = 77;

#[derive(Clone, Debug)]
pub struct HyMotionClipRun {
    /// `CLIPTextModel.pooler_output`, exactly the 768-wide DiT vector input.
    pub vector: Vec<f32>,
    pub input_ids: Vec<i32>,
    pub eos_index: usize,
}

pub struct HyMotionClipConditioner {
    tokenizer: ClipTokenizer,
    weights: LoadedClipLWeights,
}

impl HyMotionClipConditioner {
    /// Load the text tower from either the released model directory or its
    /// `model.safetensors` path. Vision weights and non-text buffers are not
    /// allocated.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let model_path = clip_model_path(path.as_ref());
        let tokenizer = ClipTokenizer::new()?;
        let weights = LoadedClipLWeights::load_text_model(&model_path)?;
        if weights.config.hidden_size as usize != HY_MOTION_VECTOR_DIM
            || weights.config.max_position_embeddings as usize != HY_MOTION_CLIP_TOKENS
        {
            return Err(DiffusionError::model(format!(
                "HY-Motion CLIP contract mismatch: hidden={} positions={}",
                weights.config.hidden_size, weights.config.max_position_embeddings
            )));
        }
        Ok(Self { tokenizer, weights })
    }

    pub fn tokenize(&self, prompt: &str) -> Result<ClipTokenChunk> {
        // HuggingFace's released call uses max_length=77, max-length
        // padding and truncation=True. `tokenize_chunks` is the right Flux
        // behavior but would reject/segment long HY prompts instead.
        let token_ids = self
            .tokenizer
            .tokenize(prompt, HY_MOTION_CLIP_TOKENS, true)?;
        let eos_index = token_ids
            .iter()
            .position(|&token| token == ClipTokenizer::EOS_TOKEN_ID)
            .ok_or_else(|| DiffusionError::model("HY-Motion CLIP tokens have no EOS"))?;
        Ok(ClipTokenChunk {
            token_ids,
            eos_index,
        })
    }

    pub fn encode(&mut self, prompt: &str) -> Result<HyMotionClipRun> {
        let tokens = self.tokenize(prompt)?;
        // The lazy executor is the portable native f32 path and avoids a
        // backend-specific graph selection controlled by process globals.
        let encoder = CompiledClipL::compile_for_mode(
            ClipLExecutionMode::Lazy,
            None,
            &mut self.weights,
            &tokens,
        )?;
        let encoded = encoder.execute(&self.weights, &tokens.token_ids)?;
        if encoded.pooled.len() != HY_MOTION_VECTOR_DIM {
            return Err(DiffusionError::model(format!(
                "HY-Motion CLIP pooler width mismatch: {}",
                encoded.pooled.len()
            )));
        }
        Ok(HyMotionClipRun {
            vector: encoded.pooled,
            input_ids: tokens.token_ids,
            eos_index: tokens.eos_index,
        })
    }

    pub fn loaded_tensor_count(&self) -> usize {
        self.weights.tensor_ids.len()
    }

    /// Release only this CLIP tower's cached CUDA weight buffers. The host
    /// checkpoint remains loaded, so a later encode can repopulate them.
    pub fn evict_device_weights(&self) -> Result<usize> {
        gpu_weight_cache_evict_prefix(&format!(
            "{}::",
            clip_cache_namespace(&self.weights)
        ))
        .map_err(DiffusionError::model)
    }
}

fn clip_model_path(path: &Path) -> PathBuf {
    if path.is_dir() {
        path.join("model.safetensors")
    } else {
        path.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROMPT: &str = "A person walks forward naturally at a steady pace.";
    const EXPECTED_REAL_IDS: &[i32] = &[
        49_406, 320, 2_533, 8_192, 2_342, 12_995, 536, 320, 12_937, 9_450, 269, 49_407,
    ];

    #[test]
    fn fixed_prompt_clip_tokenization_matches_reference() {
        let tokenizer = ClipTokenizer::new().unwrap();
        let token_ids = tokenizer
            .tokenize(PROMPT, HY_MOTION_CLIP_TOKENS, true)
            .unwrap();
        let eos_index = token_ids
            .iter()
            .position(|&token| token == ClipTokenizer::EOS_TOKEN_ID)
            .unwrap();
        assert_eq!(token_ids.len(), HY_MOTION_CLIP_TOKENS);
        assert_eq!(eos_index, EXPECTED_REAL_IDS.len() - 1);
        assert_eq!(
            &token_ids[..EXPECTED_REAL_IDS.len()],
            EXPECTED_REAL_IDS
        );
        assert!(token_ids[EXPECTED_REAL_IDS.len()..]
            .iter()
            .all(|&token| token == ClipTokenizer::PAD_TOKEN_ID));
    }

    #[test]
    fn long_prompt_matches_huggingface_single_sequence_truncation() {
        let tokenizer = ClipTokenizer::new().unwrap();
        let prompt = "walking forward ".repeat(200);
        let token_ids = tokenizer
            .tokenize(&prompt, HY_MOTION_CLIP_TOKENS, true)
            .unwrap();
        assert_eq!(token_ids.len(), HY_MOTION_CLIP_TOKENS);
        assert_eq!(token_ids[0], ClipTokenizer::BOS_TOKEN_ID);
        assert_eq!(token_ids[HY_MOTION_CLIP_TOKENS - 1], ClipTokenizer::EOS_TOKEN_ID);
        assert_eq!(
            token_ids
                .iter()
                .position(|&token| token == ClipTokenizer::EOS_TOKEN_ID),
            Some(HY_MOTION_CLIP_TOKENS - 1)
        );
    }
}
