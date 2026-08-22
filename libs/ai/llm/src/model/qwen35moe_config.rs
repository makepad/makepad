use crate::error::Result;
use crate::gguf::GgufFile;

use super::gguf_meta::{required_f32, required_u32, required_u32_array};

#[derive(Clone, Debug)]
pub struct Qwen35MoeConfig {
    pub block_count: u32,
    pub context_length: u32,
    pub embedding_length: u32,
    pub attention_head_count: u32,
    pub attention_head_count_kv: u32,
    pub attention_key_length: u32,
    pub attention_value_length: u32,
    pub rope_dimension_count: u32,
    pub rope_dimension_sections: Vec<u32>,
    pub rope_freq_base: f32,
    pub attention_layer_norm_rms_epsilon: f32,
    pub expert_count: u32,
    pub expert_used_count: u32,
    pub expert_feed_forward_length: u32,
    pub expert_shared_feed_forward_length: u32,
    pub ssm_conv_kernel: u32,
    pub ssm_state_size: u32,
    pub ssm_group_count: u32,
    pub ssm_time_step_rank: u32,
    pub ssm_inner_size: u32,
    pub full_attention_interval: u32,
}

impl Qwen35MoeConfig {
    pub fn from_gguf(gguf: &GgufFile) -> Result<Self> {
        Ok(Self {
            block_count: required_u32(gguf, "qwen35moe.block_count")?,
            context_length: required_u32(gguf, "qwen35moe.context_length")?,
            embedding_length: required_u32(gguf, "qwen35moe.embedding_length")?,
            attention_head_count: required_u32(gguf, "qwen35moe.attention.head_count")?,
            attention_head_count_kv: required_u32(gguf, "qwen35moe.attention.head_count_kv")?,
            attention_key_length: required_u32(gguf, "qwen35moe.attention.key_length")?,
            attention_value_length: required_u32(gguf, "qwen35moe.attention.value_length")?,
            rope_dimension_count: required_u32(gguf, "qwen35moe.rope.dimension_count")?,
            rope_dimension_sections: required_u32_array(gguf, "qwen35moe.rope.dimension_sections")?,
            rope_freq_base: required_f32(gguf, "qwen35moe.rope.freq_base")?,
            attention_layer_norm_rms_epsilon: required_f32(
                gguf,
                "qwen35moe.attention.layer_norm_rms_epsilon",
            )?,
            expert_count: required_u32(gguf, "qwen35moe.expert_count")?,
            expert_used_count: required_u32(gguf, "qwen35moe.expert_used_count")?,
            expert_feed_forward_length: required_u32(gguf, "qwen35moe.expert_feed_forward_length")?,
            expert_shared_feed_forward_length: required_u32(
                gguf,
                "qwen35moe.expert_shared_feed_forward_length",
            )?,
            ssm_conv_kernel: required_u32(gguf, "qwen35moe.ssm.conv_kernel")?,
            ssm_state_size: required_u32(gguf, "qwen35moe.ssm.state_size")?,
            ssm_group_count: required_u32(gguf, "qwen35moe.ssm.group_count")?,
            ssm_time_step_rank: required_u32(gguf, "qwen35moe.ssm.time_step_rank")?,
            ssm_inner_size: required_u32(gguf, "qwen35moe.ssm.inner_size")?,
            full_attention_interval: required_u32(gguf, "qwen35moe.full_attention_interval")?,
        })
    }
}
