use std::collections::BTreeMap;

use crate::gguf::GgufTensorInfo;
use crate::model::LlamaArchitecture;
use crate::runtime::{HybridCacheTemplate, LogitsProbeSpec};
use crate::weights::GgufWeightLayout;
use crate::Result;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelLayerRole {
    Attention,
    Recurrent,
    Unknown,
}

impl ModelLayerRole {
    pub fn name(self) -> &'static str {
        match self {
            Self::Attention => "attention",
            Self::Recurrent => "recurrent",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ModelLayerInventory {
    pub index: u32,
    pub role: ModelLayerRole,
    pub tensors: BTreeMap<String, GgufTensorInfo>,
}

#[derive(Clone, Debug, Default)]
pub struct ModelTensorInventory {
    pub globals: BTreeMap<String, GgufTensorInfo>,
    pub layers: Vec<ModelLayerInventory>,
}

impl ModelTensorInventory {
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

    pub fn count_layers_with_role(&self, role: ModelLayerRole) -> usize {
        self.layers
            .iter()
            .filter(|layer| layer.role == role)
            .count()
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
        for tensor in self.globals.values() {
            visit(tensor);
        }
        for layer in &self.layers {
            for tensor in layer.tensors.values() {
                visit(tensor);
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct ModelTailProbePlan {
    pub spec: LogitsProbeSpec,
    pub weights: GgufWeightLayout,
    pub extra_activation_bytes: usize,
}

#[derive(Clone, Debug)]
pub struct ModelExecutionPlan {
    pub architecture: LlamaArchitecture,
    pub embedding_length: u32,
    pub vocab_size: Option<u32>,
    pub inventory: ModelTensorInventory,
    pub full_weights: GgufWeightLayout,
    pub tail_probe: ModelTailProbePlan,
    pub hybrid_cache: Option<HybridCacheTemplate>,
}

impl ModelExecutionPlan {
    pub fn layer_count(&self) -> usize {
        self.inventory.layers.len()
    }
}

#[cfg(test)]
mod tests {
    use super::{ModelLayerInventory, ModelLayerRole, ModelTensorInventory};
    use crate::gguf::GgufTensorInfo;
    use makepad_ggml::TensorType;
    use std::collections::BTreeMap;

    #[test]
    fn inventory_deduplicates_by_tensor_name() {
        let shared = GgufTensorInfo {
            name: "shared.weight".to_owned(),
            dimensions: vec![4, 4],
            tensor_type: TensorType::F32,
            offset: 0,
            size_bytes: 64,
        };

        let mut globals = BTreeMap::new();
        globals.insert("shared".to_owned(), shared.clone());

        let mut layer_tensors = BTreeMap::new();
        layer_tensors.insert("alias".to_owned(), shared);

        let inventory = ModelTensorInventory {
            globals,
            layers: vec![ModelLayerInventory {
                index: 0,
                role: ModelLayerRole::Attention,
                tensors: layer_tensors,
            }],
        };

        assert_eq!(inventory.unique_tensor_count(), 1);
        assert_eq!(inventory.total_tensor_bytes(), 64);
    }
}
