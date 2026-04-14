use crate::error::{LlamaError, Result};
use crate::gguf::GgufFile;

use super::gguf_meta::{
    optional_f32, optional_u32, required_f32, required_u32, required_u32_or_first_array,
    required_u32_or_repeat_array,
};

#[derive(Clone, Debug)]
pub struct Gemma4Config {
    pub block_count: u32,
    pub context_length: u32,
    pub embedding_length: u32,
    pub embedding_length_per_layer_input: u32,
    pub feed_forward_length: u32,
    pub expert_feed_forward_length: u32,
    pub expert_count: u32,
    pub expert_used_count: u32,
    pub attention_head_count: u32,
    pub attention_head_count_kv: u32,
    pub attention_key_length: u32,
    pub attention_value_length: u32,
    pub attention_key_length_swa: u32,
    pub attention_value_length_swa: u32,
    pub attention_sliding_window: u32,
    pub attention_sliding_window_pattern: Vec<u32>,
    pub attention_shared_kv_layers: u32,
    pub rope_dimension_count: u32,
    pub rope_dimension_count_swa: u32,
    pub rope_freq_base: f32,
    pub rope_freq_base_swa: f32,
    pub attention_layer_norm_rms_epsilon: f32,
    pub final_logit_softcapping: Option<f32>,
}

impl Gemma4Config {
    pub fn from_gguf(gguf: &GgufFile) -> Result<Self> {
        let block_count = required_u32(gguf, "gemma4.block_count")?;
        let repeat_len = usize::try_from(block_count)
            .map_err(|_| LlamaError::format("gemma4.block_count does not fit in usize"))?;
        let attention_key_length =
            required_u32_or_first_array(gguf, "gemma4.attention.key_length")?;
        let attention_value_length =
            required_u32_or_first_array(gguf, "gemma4.attention.value_length")?;
        let rope_dimension_count =
            required_u32_or_first_array(gguf, "gemma4.rope.dimension_count")?;
        let rope_freq_base = required_f32(gguf, "gemma4.rope.freq_base")?;
        Ok(Self {
            block_count,
            context_length: required_u32(gguf, "gemma4.context_length")?,
            embedding_length: required_u32(gguf, "gemma4.embedding_length")?,
            embedding_length_per_layer_input: optional_u32(
                gguf,
                "gemma4.embedding_length_per_layer_input",
            )
            .unwrap_or(0),
            feed_forward_length: required_u32_or_first_array(gguf, "gemma4.feed_forward_length")?,
            expert_feed_forward_length: optional_u32(gguf, "gemma4.expert_feed_forward_length")
                .unwrap_or(0),
            expert_count: optional_u32(gguf, "gemma4.expert_count").unwrap_or(0),
            expert_used_count: optional_u32(gguf, "gemma4.expert_used_count").unwrap_or(0),
            attention_head_count: required_u32_or_first_array(gguf, "gemma4.attention.head_count")?,
            attention_head_count_kv: required_u32_or_first_array(
                gguf,
                "gemma4.attention.head_count_kv",
            )?,
            attention_key_length,
            attention_value_length,
            attention_key_length_swa: optional_u32(gguf, "gemma4.attention.key_length_swa")
                .unwrap_or(attention_key_length),
            attention_value_length_swa: optional_u32(gguf, "gemma4.attention.value_length_swa")
                .unwrap_or(attention_value_length),
            attention_sliding_window: optional_u32(gguf, "gemma4.attention.sliding_window")
                .unwrap_or(0),
            attention_sliding_window_pattern: required_u32_or_repeat_array(
                gguf,
                "gemma4.attention.sliding_window_pattern",
                repeat_len,
            )?,
            attention_shared_kv_layers: optional_u32(gguf, "gemma4.attention.shared_kv_layers")
                .unwrap_or(0),
            rope_dimension_count,
            rope_dimension_count_swa: optional_u32(gguf, "gemma4.rope.dimension_count_swa")
                .unwrap_or(rope_dimension_count),
            rope_freq_base,
            rope_freq_base_swa: optional_f32(gguf, "gemma4.rope.freq_base_swa")
                .unwrap_or(rope_freq_base),
            attention_layer_norm_rms_epsilon: required_f32(
                gguf,
                "gemma4.attention.layer_norm_rms_epsilon",
            )?,
            final_logit_softcapping: optional_f32(gguf, "gemma4.final_logit_softcapping"),
        })
    }
}
