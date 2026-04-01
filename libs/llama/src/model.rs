use crate::error::{LlamaError, Result};
use crate::gguf::{GgufArray, GgufFile, GgufString, GgufTensorInfo, GgufValue};
use crate::plan::ModelExecutionPlan;
use crate::qwen35moe::Qwen35MoeTensors;
use crate::qwen35moe_runtime::qwen35moe_execution_plan;
use crate::weights::GgufWeightLayout;
use std::path::Path;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LlamaArchitecture {
    Qwen35,
    Qwen35Moe,
    Unknown(String),
}

impl LlamaArchitecture {
    pub fn from_str(value: &str) -> Self {
        match value {
            "qwen35" => Self::Qwen35,
            "qwen35moe" => Self::Qwen35Moe,
            other => Self::Unknown(other.to_owned()),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Qwen35 => "qwen35",
            Self::Qwen35Moe => "qwen35moe",
            Self::Unknown(name) => name.as_str(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ModelGeneral {
    pub architecture: String,
    pub model_type: Option<String>,
    pub name: Option<String>,
    pub file_type: Option<u32>,
    pub quantization_version: Option<u32>,
}

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

#[derive(Clone, Debug)]
pub struct LlamaModel {
    pub gguf: GgufFile,
    pub architecture: LlamaArchitecture,
    pub general: ModelGeneral,
    pub qwen35moe: Option<Qwen35MoeConfig>,
}

impl LlamaModel {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let gguf = GgufFile::open(path)?;
        let architecture = required_utf8_string(&gguf, "general.architecture")?;
        let architecture_kind = LlamaArchitecture::from_str(&architecture);

        let general = ModelGeneral {
            architecture,
            model_type: optional_utf8_string(&gguf, "general.type")?,
            name: optional_utf8_string(&gguf, "general.name")?,
            file_type: optional_u32(&gguf, "general.file_type"),
            quantization_version: optional_u32(&gguf, "general.quantization_version"),
        };

        let qwen35moe = match architecture_kind {
            LlamaArchitecture::Qwen35Moe => Some(Qwen35MoeConfig::from_gguf(&gguf)?),
            _ => None,
        };

        Ok(Self {
            gguf,
            architecture: architecture_kind,
            general,
            qwen35moe,
        })
    }

    pub fn tensor(&self, name: &str) -> Option<&GgufTensorInfo> {
        self.gguf.get_tensor(name)
    }

    pub fn require_qwen35moe(&self) -> Result<&Qwen35MoeConfig> {
        self.qwen35moe.as_ref().ok_or_else(|| {
            LlamaError::unsupported(format!(
                "model architecture '{}' is not qwen35moe",
                self.architecture.name()
            ))
        })
    }

    pub fn qwen35moe_tensors(&self) -> Result<Qwen35MoeTensors> {
        Qwen35MoeTensors::from_model(self)
    }

    pub fn qwen35moe_weight_layout(&self) -> Result<GgufWeightLayout> {
        self.qwen35moe_tensors()?.weight_layout()
    }

    pub fn execution_plan(&self) -> Result<ModelExecutionPlan> {
        match self.architecture {
            LlamaArchitecture::Qwen35Moe => qwen35moe_execution_plan(self),
            _ => Err(LlamaError::unsupported(format!(
                "execution plan builder is not implemented for architecture '{}'",
                self.architecture.name()
            ))),
        }
    }

    pub fn validate_layout(&self) -> Result<()> {
        let plan = self.execution_plan()?;
        if plan.inventory.layers.is_empty() {
            return Err(LlamaError::format(format!(
                "model '{}' produced an empty layer inventory",
                self.architecture.name()
            )));
        }
        Ok(())
    }

    pub fn validate_qwen35moe_layout(&self) -> Result<()> {
        self.validate_layout()
    }
}

fn required_u32(gguf: &GgufFile, key: &str) -> Result<u32> {
    let value = gguf.require_value(key)?;
    value_to_u32(value).ok_or_else(|| {
        LlamaError::format(format!(
            "gguf key '{}' has type {}, expected integral scalar",
            key,
            value.value_type().name()
        ))
    })
}

fn required_f32(gguf: &GgufFile, key: &str) -> Result<f32> {
    gguf.require_value(key)?.as_f32().ok_or_else(|| {
        LlamaError::format(format!(
            "gguf key '{}' has type {}, expected f32",
            key,
            gguf.require_value(key).unwrap().value_type().name()
        ))
    })
}

fn required_u32_array(gguf: &GgufFile, key: &str) -> Result<Vec<u32>> {
    let value = gguf.require_value(key)?;
    match value {
        GgufValue::Array(values) => array_to_u32_vec(values).ok_or_else(|| {
            LlamaError::format(format!(
                "gguf key '{}' has type {}, expected integral array",
                key,
                value.value_type().name()
            ))
        }),
        other => Err(LlamaError::format(format!(
            "gguf key '{}' has type {}, expected u32 array",
            key,
            other.value_type().name()
        ))),
    }
}

fn required_utf8_string(gguf: &GgufFile, key: &str) -> Result<String> {
    match gguf.require_value(key)? {
        GgufValue::String(value) => value.try_utf8().map(|s| s.to_owned()),
        other => Err(LlamaError::format(format!(
            "gguf key '{}' has type {}, expected string",
            key,
            other.value_type().name()
        ))),
    }
}

fn optional_utf8_string(gguf: &GgufFile, key: &str) -> Result<Option<String>> {
    match gguf.get_value(key) {
        None => Ok(None),
        Some(GgufValue::String(value)) => value.try_utf8().map(|s| Some(s.to_owned())),
        Some(other) => Err(LlamaError::format(format!(
            "gguf key '{}' has type {}, expected string",
            key,
            other.value_type().name()
        ))),
    }
}

fn optional_u32(gguf: &GgufFile, key: &str) -> Option<u32> {
    gguf.get_value(key).and_then(value_to_u32)
}

#[allow(dead_code)]
fn optional_string<'a>(gguf: &'a GgufFile, key: &str) -> Option<&'a GgufString> {
    gguf.get_value(key).and_then(GgufValue::as_string)
}

fn value_to_u32(value: &GgufValue) -> Option<u32> {
    match value {
        GgufValue::Uint32(v) => Some(*v),
        GgufValue::Uint64(v) => u32::try_from(*v).ok(),
        GgufValue::Int32(v) => u32::try_from(*v).ok(),
        GgufValue::Int64(v) => u32::try_from(*v).ok(),
        _ => None,
    }
}

fn array_to_u32_vec(value: &GgufArray) -> Option<Vec<u32>> {
    match value {
        GgufArray::Uint32(values) => Some(values.clone()),
        GgufArray::Int32(values) => values
            .iter()
            .copied()
            .map(u32::try_from)
            .collect::<std::result::Result<Vec<_>, _>>()
            .ok(),
        GgufArray::Uint64(values) => values
            .iter()
            .copied()
            .map(u32::try_from)
            .collect::<std::result::Result<Vec<_>, _>>()
            .ok(),
        GgufArray::Int64(values) => values
            .iter()
            .copied()
            .map(u32::try_from)
            .collect::<std::result::Result<Vec<_>, _>>()
            .ok(),
        _ => None,
    }
}
