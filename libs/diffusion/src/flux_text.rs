use crate::clip::ClipTokenChunk;
use crate::clip_l::{CompiledClipLMetal, LoadedClipLWeights};
use crate::comfy::FluxPrompts;
use crate::flux::{
    tokenize_flux_clip_l_prompt, tokenize_flux_t5xxl_prompt, FluxPromptToImagePlan, FluxResolvedBundle,
};
use crate::t5::T5TokenizedPrompt;
use crate::t5_encoder::{LazyT5xxlMetal, LoadedT5xxlWeights};
use crate::{DiffusionError, Result};
use makepad_ggml::backend::metal::MetalRuntime;

#[derive(Clone, Debug)]
pub struct FluxTokenizedPrompts {
    pub clip_l: ClipTokenChunk,
    pub t5xxl: T5TokenizedPrompt,
}

#[derive(Clone, Debug)]
pub struct FluxConditioning {
    pub clip_pooled: Vec<f32>,
    pub clip_hidden_size: usize,
    pub t5_hidden_states: Vec<f32>,
    pub t5_token_count: usize,
    pub t5_hidden_size: usize,
    pub t5_attention_mask: Vec<i32>,
    pub t5_eos_index: usize,
}

#[derive(Debug)]
pub struct FluxLoadedTextEncoders {
    pub clip_l: LoadedClipLWeights,
    pub t5xxl: LoadedT5xxlWeights,
}

pub struct FluxCompiledTextEncodersMetal {
    clip_l: CompiledClipLMetal,
    t5xxl: LazyT5xxlMetal,
}

impl FluxTokenizedPrompts {
    pub fn from_prompts(prompts: &FluxPrompts) -> Result<Self> {
        let clip_l = tokenize_flux_clip_l_prompt(&prompts.clip_l)?;
        if clip_l.chunks.len() != 1 {
            return Err(DiffusionError::workflow(format!(
                "flux text conditioning currently supports one clip_l chunk, got {}",
                clip_l.chunks.len()
            )));
        }

        Ok(Self {
            clip_l: clip_l.chunks.into_iter().next().unwrap(),
            t5xxl: tokenize_flux_t5xxl_prompt(&prompts.t5xxl)?,
        })
    }
}

impl FluxLoadedTextEncoders {
    pub fn load(bundle: &FluxResolvedBundle) -> Result<Self> {
        let clip_l_path = bundle
            .clip_l_path
            .as_ref()
            .ok_or_else(|| DiffusionError::workflow("workflow bundle does not include clip_l"))?;
        let t5xxl_path = bundle
            .t5xxl_path
            .as_ref()
            .ok_or_else(|| DiffusionError::workflow("workflow bundle does not include t5xxl"))?;

        Ok(Self {
            clip_l: LoadedClipLWeights::load(clip_l_path)?,
            t5xxl: LoadedT5xxlWeights::load(t5xxl_path)?,
        })
    }

    pub fn load_from_plan(plan: &FluxPromptToImagePlan) -> Result<Self> {
        Self::load(&plan.bundle)
    }
}

impl FluxCompiledTextEncodersMetal {
    pub fn compile(
        weights: &mut FluxLoadedTextEncoders,
        prompts: &FluxTokenizedPrompts,
    ) -> Result<Self> {
        let runtime = MetalRuntime::new().map_err(DiffusionError::model)?;
        Self::compile_with_runtime(runtime, weights, prompts)
    }

    pub fn compile_with_runtime(
        runtime: MetalRuntime,
        weights: &mut FluxLoadedTextEncoders,
        prompts: &FluxTokenizedPrompts,
    ) -> Result<Self> {
        Ok(Self {
            clip_l: CompiledClipLMetal::compile_with_runtime(
                runtime.clone(),
                &mut weights.clip_l,
                &prompts.clip_l,
            )?,
            t5xxl: LazyT5xxlMetal::compile_with_runtime(
                runtime,
                &mut weights.t5xxl,
                &prompts.t5xxl,
            )?,
        })
    }

    pub fn execute(
        &self,
        weights: &FluxLoadedTextEncoders,
        prompts: &FluxTokenizedPrompts,
    ) -> Result<FluxConditioning> {
        let clip = self
            .clip_l
            .execute(&weights.clip_l, &prompts.clip_l.token_ids)?;
        let t5 = self
            .t5xxl
            .execute(&weights.t5xxl, &prompts.t5xxl.token_ids)?;

        Ok(FluxConditioning {
            clip_pooled: clip.pooled,
            clip_hidden_size: clip.hidden_size,
            t5_hidden_states: t5.hidden_states,
            t5_token_count: t5.token_count,
            t5_hidden_size: t5.hidden_size,
            t5_attention_mask: vec![1; t5.token_count],
            t5_eos_index: t5.eos_index,
        })
    }
}
