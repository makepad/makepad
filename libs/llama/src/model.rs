mod gemma4_config;
mod gguf_meta;
mod qwen35_config;
mod qwen35moe_config;

use makepad_ggml::TensorType;

use crate::error::{LlamaError, Result};
use crate::gemma4::Gemma4Tensors;
use crate::gemma4_runtime::{gemma4_execution_plan, gemma4_hybrid_decode_spec};
use crate::gguf::{GgufFile, GgufTensorInfo};
use crate::plan::ModelExecutionPlan;
use crate::qwen35::Qwen35Tensors;
use crate::qwen35_runtime::{qwen35_execution_plan, qwen35_hybrid_decode_spec};
use crate::qwen35moe::Qwen35MoeTensors;
use crate::qwen35moe_runtime::{qwen35moe_execution_plan, qwen35moe_hybrid_decode_spec};
use crate::runtime::HybridDecodeSpec;
use crate::weights::GgufWeightLayout;
use std::path::Path;

pub use gemma4_config::Gemma4Config;
use gguf_meta::{optional_u32, optional_utf8_string, required_utf8_string};
pub use qwen35_config::Qwen35Config;
pub use qwen35moe_config::Qwen35MoeConfig;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LlamaArchitecture {
    Qwen35,
    Qwen35Moe,
    Gemma4,
    Unknown(String),
}

impl LlamaArchitecture {
    pub fn from_str(value: &str) -> Self {
        match value {
            "qwen35" => Self::Qwen35,
            "qwen35moe" => Self::Qwen35Moe,
            "gemma4" => Self::Gemma4,
            other => Self::Unknown(other.to_owned()),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Qwen35 => "qwen35",
            Self::Qwen35Moe => "qwen35moe",
            Self::Gemma4 => "gemma4",
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
pub struct LlamaModel {
    pub gguf: GgufFile,
    pub architecture: LlamaArchitecture,
    pub general: ModelGeneral,
    pub qwen35: Option<Qwen35Config>,
    pub qwen35moe: Option<Qwen35MoeConfig>,
    pub gemma4: Option<Gemma4Config>,
}

impl LlamaModel {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let gguf = GgufFile::open(path)?;
        let architecture = required_utf8_string(&gguf, "general.architecture")?;
        let architecture_kind = LlamaArchitecture::from_str(&architecture);
        let is_gemma4 = architecture == "gemma4";

        let general = ModelGeneral {
            architecture: architecture.clone(),
            model_type: optional_utf8_string(&gguf, "general.type")?,
            name: optional_utf8_string(&gguf, "general.name")?,
            file_type: optional_u32(&gguf, "general.file_type"),
            quantization_version: optional_u32(&gguf, "general.quantization_version"),
        };

        let qwen35 = match architecture_kind {
            LlamaArchitecture::Qwen35 => Some(Qwen35Config::from_gguf(&gguf)?),
            _ => None,
        };
        let qwen35moe = match architecture_kind {
            LlamaArchitecture::Qwen35Moe => Some(Qwen35MoeConfig::from_gguf(&gguf)?),
            _ => None,
        };
        let gemma4 = if is_gemma4 {
            Some(Gemma4Config::from_gguf(&gguf)?)
        } else {
            None
        };

        Ok(Self {
            gguf,
            architecture: architecture_kind,
            general,
            qwen35,
            qwen35moe,
            gemma4,
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

    pub fn require_qwen35(&self) -> Result<&Qwen35Config> {
        self.qwen35.as_ref().ok_or_else(|| {
            LlamaError::unsupported(format!(
                "model architecture '{}' is not qwen35",
                self.architecture.name()
            ))
        })
    }

    pub fn qwen35_tensors(&self) -> Result<Qwen35Tensors> {
        Qwen35Tensors::from_model(self)
    }

    pub fn qwen35_weight_layout(&self) -> Result<GgufWeightLayout> {
        self.qwen35_tensors()?.weight_layout()
    }

    pub fn require_gemma4(&self) -> Result<&Gemma4Config> {
        self.gemma4.as_ref().ok_or_else(|| {
            LlamaError::unsupported(format!(
                "model architecture '{}' is not gemma4",
                self.architecture.name()
            ))
        })
    }

    pub fn gemma4_tensors(&self) -> Result<Gemma4Tensors> {
        Gemma4Tensors::from_model(self)
    }

    pub fn gemma4_weight_layout(&self) -> Result<GgufWeightLayout> {
        self.gemma4_tensors()?.weight_layout()
    }

    pub fn context_length(&self) -> Result<u32> {
        if let Some(cfg) = &self.qwen35 {
            return Ok(cfg.context_length);
        }
        if let Some(cfg) = &self.qwen35moe {
            return Ok(cfg.context_length);
        }
        if let Some(cfg) = &self.gemma4 {
            return Ok(cfg.context_length);
        }
        Err(LlamaError::unsupported(format!(
            "context length is not implemented for architecture '{}'",
            self.architecture.name()
        )))
    }

    pub fn hybrid_decode_spec(
        &self,
        max_context: u32,
        max_sequences: u32,
        attention_k_type: TensorType,
        attention_v_type: TensorType,
        recurrent_r_type: TensorType,
        recurrent_s_type: TensorType,
    ) -> Result<HybridDecodeSpec> {
        match self.architecture {
            LlamaArchitecture::Qwen35 => qwen35_hybrid_decode_spec(
                self,
                max_context,
                max_sequences,
                attention_k_type,
                attention_v_type,
                recurrent_r_type,
                recurrent_s_type,
            ),
            LlamaArchitecture::Qwen35Moe => qwen35moe_hybrid_decode_spec(
                self,
                max_context,
                max_sequences,
                attention_k_type,
                attention_v_type,
                recurrent_r_type,
                recurrent_s_type,
            ),
            LlamaArchitecture::Gemma4 => gemma4_hybrid_decode_spec(
                self,
                max_context,
                max_sequences,
                attention_k_type,
                attention_v_type,
                recurrent_r_type,
                recurrent_s_type,
            ),
            _ => Err(LlamaError::unsupported(format!(
                "hybrid decode spec is not implemented for architecture '{}'",
                self.architecture.name()
            ))),
        }
    }

    pub fn execution_plan(&self) -> Result<ModelExecutionPlan> {
        match self.architecture {
            LlamaArchitecture::Qwen35 => qwen35_execution_plan(self),
            LlamaArchitecture::Qwen35Moe => qwen35moe_execution_plan(self),
            LlamaArchitecture::Gemma4 => gemma4_execution_plan(self),
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
