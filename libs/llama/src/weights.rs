use std::collections::BTreeMap;

use makepad_ggml::{ggml_pad, BufferUsage, Context, InitParams, TensorId, GGML_MEM_ALIGN};

use crate::error::{LlamaError, Result};
use crate::gguf::{GgufFile, GgufTensorInfo};

#[derive(Clone, Debug)]
pub struct GgufWeightLayout {
    pub tensors: Vec<GgufTensorInfo>,
    pub total_bytes: usize,
}

#[derive(Clone, Debug)]
pub struct LoadedGgufWeights {
    pub ctx: Context,
    pub tensor_ids: BTreeMap<String, TensorId>,
}

impl LoadedGgufWeights {
    pub fn tensor_id(&self, name: &str) -> Option<TensorId> {
        self.tensor_ids.get(name).copied()
    }

    pub fn require_tensor_id(&self, name: &str) -> Result<TensorId> {
        self.tensor_id(name)
            .ok_or_else(|| LlamaError::format(format!("missing resident tensor '{}'", name)))
    }
}

impl GgufWeightLayout {
    pub fn from_tensors(
        tensors: impl IntoIterator<Item = GgufTensorInfo>,
    ) -> Result<Self> {
        let mut dedup = BTreeMap::new();
        for tensor in tensors {
            dedup.entry(tensor.name.clone()).or_insert(tensor);
        }

        let tensors = dedup.into_values().collect::<Vec<_>>();
        let total_bytes = padded_total_bytes(&tensors)?;
        Ok(Self { tensors, total_bytes })
    }

    pub fn allocate_context(&self) -> Result<LoadedGgufWeights> {
        self.allocate_context_with_extra(0)
    }

    pub fn allocate_context_with_extra(&self, extra_bytes: usize) -> Result<LoadedGgufWeights> {
        let mem_size = ggml_pad(
            self.total_bytes
                .checked_add(extra_bytes)
                .ok_or_else(|| LlamaError::format("overflow computing gguf context size"))?,
            GGML_MEM_ALIGN,
        );
        let mut ctx = Context::new(InitParams {
            mem_size,
            mem_buffer: None,
            no_alloc: false,
        });
        let tensor_ids = allocate_tensor_ids(&mut ctx, &self.tensors)?;
        Ok(LoadedGgufWeights { ctx, tensor_ids })
    }

    pub fn allocate_and_load(&self, gguf: &GgufFile) -> Result<LoadedGgufWeights> {
        self.allocate_and_load_with_extra(gguf, 0)
    }

    pub fn allocate_and_load_with_extra(
        &self,
        gguf: &GgufFile,
        extra_bytes: usize,
    ) -> Result<LoadedGgufWeights> {
        let mut loaded = self.allocate_context_with_extra(extra_bytes)?;
        for tensor in &self.tensors {
            let tensor_id = loaded
                .tensor_id(&tensor.name)
                .ok_or_else(|| LlamaError::format(format!("missing loaded tensor '{}'", tensor.name)))?;
            let dst = loaded
                .ctx
                .tensor_data_mut(tensor_id)
                .map_err(LlamaError::format)?;
            gguf.read_tensor_into(&tensor.name, dst)?;
        }
        Ok(loaded)
    }
}

fn allocate_tensor_ids(
    ctx: &mut Context,
    tensors: &[GgufTensorInfo],
) -> Result<BTreeMap<String, TensorId>> {
    let mut tensor_ids = BTreeMap::new();
    for tensor in tensors {
        let dims = tensor_extents_i64(tensor)?;
        let id = ctx
            .new_named_tensor(
                tensor.name.clone(),
                tensor.tensor_type,
                dims.len(),
                &dims,
                BufferUsage::Weights,
            )
            .map_err(LlamaError::format)?;
        tensor_ids.insert(tensor.name.clone(), id);
    }
    Ok(tensor_ids)
}

fn padded_total_bytes(tensors: &[GgufTensorInfo]) -> Result<usize> {
    let mut total = 0usize;
    for tensor in tensors {
        total = ggml_pad(total, GGML_MEM_ALIGN);
        let size_bytes = usize::try_from(tensor.size_bytes).map_err(|_| {
            LlamaError::format(format!(
                "tensor '{}' size {} does not fit in usize",
                tensor.name, tensor.size_bytes
            ))
        })?;
        total = total.checked_add(size_bytes).ok_or_else(|| {
            LlamaError::format(format!(
                "overflow computing total bytes for tensor '{}'",
                tensor.name
            ))
        })?;
    }
    Ok(ggml_pad(total, GGML_MEM_ALIGN))
}

fn tensor_extents_i64(tensor: &GgufTensorInfo) -> Result<Vec<i64>> {
    tensor
        .dimensions
        .iter()
        .map(|&dim| {
            i64::try_from(dim).map_err(|_| {
                LlamaError::format(format!(
                    "tensor '{}' dimension {} does not fit in i64",
                    tensor.name, dim
                ))
            })
        })
        .collect()
}
