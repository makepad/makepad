use std::collections::BTreeMap;
use std::sync::Arc;

use crate::{
    ggml_pad, BufferUsage, Context, InitParams, MappedRegion, TensorId, GGML_MEM_ALIGN,
};

use makepad_ai_loader::{read_placed, Placement};

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
    pub fn from_tensors(tensors: impl IntoIterator<Item = GgufTensorInfo>) -> Result<Self> {
        let mut dedup = BTreeMap::new();
        for tensor in tensors {
            dedup.entry(tensor.name.clone()).or_insert(tensor);
        }

        let tensors = dedup.into_values().collect::<Vec<_>>();
        let total_bytes = padded_total_bytes(&tensors)?;
        Ok(Self {
            tensors,
            total_bytes,
        })
    }

    pub fn allocate_context(&self) -> Result<LoadedGgufWeights> {
        self.allocate_context_with_extra(0)
    }

    pub fn allocate_context_with_extra(&self, extra_bytes: usize) -> Result<LoadedGgufWeights> {
        // Page-multiple arena: lets the Metal backend wrap it zero-copy
        // (newBufferWithBytesNoCopy needs page-aligned base + page-multiple
        // length; large mallocs are page-aligned on macOS). 16384 covers
        // both 4K and 16K page targets.
        let mem_size = ggml_pad(
            self.total_bytes
                .checked_add(extra_bytes)
                .ok_or_else(|| LlamaError::format("overflow computing gguf context size"))?,
            GGML_MEM_ALIGN,
        )
        .next_multiple_of(16384);
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

        // One bulk read for the whole arena instead of a seek+read per
        // tensor: a per-tensor request stream keeps only one I/O in flight
        // and leaves most of an NVMe's bandwidth unused (measured 2.0 GB/s
        // vs 5.3 GB/s for the same 16 GB gguf).
        let mut placed = Vec::with_capacity(self.tensors.len());
        for tensor in &self.tensors {
            let tensor_id = loaded.tensor_id(&tensor.name).ok_or_else(|| {
                LlamaError::format(format!("missing loaded tensor '{}'", tensor.name))
            })?;
            let entry = loaded
                .ctx
                .tensor(tensor_id)
                .ok_or_else(|| LlamaError::format("loaded tensor id is invalid"))?;
            let dst_offset = entry.data_offset.ok_or_else(|| {
                LlamaError::format(format!("tensor '{}' has no arena offset", tensor.name))
            })?;
            let nbytes = entry.nbytes();
            let size_bytes = usize::try_from(tensor.size_bytes).map_err(|_| {
                LlamaError::format(format!(
                    "tensor '{}' size {} does not fit in usize",
                    tensor.name, tensor.size_bytes
                ))
            })?;
            if nbytes != size_bytes {
                return Err(LlamaError::format(format!(
                    "tensor '{}' size {} does not match gguf size {}",
                    tensor.name, nbytes, size_bytes
                )));
            }
            placed.push(Placement {
                file_offset: tensor.absolute_offset(gguf.data_offset)?,
                dst_offset: dst_offset as u64,
                len: tensor.size_bytes,
            });
        }

        // Single-arena contexts place tensors at plain `mem_buffer` indices;
        // the mapped two-region context never reaches this path.
        debug_assert_eq!(loaded.ctx.ro_split(), 0);
        read_placed(&gguf.path, loaded.ctx.mem_buffer_mut(), &placed)
            .map_err(LlamaError::format)?;
        Ok(loaded)
    }

    /// Mapped path: weights served straight from a read-only mmap of the
    /// gguf file (clean file-backed pages, lazy page-in) at their file
    /// offsets; caches/activations get a separate owned dirty region of
    /// `extra_bytes`. Returns None when mapping is unavailable so callers
    /// fall back EXACTLY to the owned-arena path.
    pub fn map_and_load(&self, gguf: &GgufFile, extra_bytes: usize) -> Option<LoadedGgufWeights> {
        let region = match MappedRegion::map_file(&gguf.path) {
            Ok(region) => Arc::new(region),
            Err(err) => {
                eprintln!("llama: {} — using the owned-arena weight path", err);
                return None;
            }
        };

        // Lazy page-in makes the load look instant and hands the bill to
        // whoever touches the weights first — the first forward pass —
        // where it is paid at demand-fault speed. Measured on an M3 Max:
        // faulting a cold mapping runs ~0.8 GB/s, while prefaulting the
        // same file runs ~6.3 GB/s, so the bytes cost ~7.7x more when they
        // arrive one fault at a time. `prefault` pulls that work forward
        // into the load, where it is both faster and visible.
        //
        // Opt-in until it has been A/B'd against a real generate run:
        // `MAKEPAD_LLAMA_PREFAULT=1`.
        if matches!(
            std::env::var("MAKEPAD_LLAMA_PREFAULT").as_deref(),
            Ok("1") | Ok("on")
        ) {
            let started = std::time::Instant::now();
            region.prefault();
            let seconds = started.elapsed().as_secs_f64();
            let gb = region.len() as f64 / (1u64 << 30) as f64;
            let (resident, pages) = region.residency();
            eprintln!(
                "llama: prefaulted {:.2} GB in {:.2} s ({:.2} GB/s, {:.1}% resident)",
                gb,
                seconds,
                gb / seconds,
                100.0 * resident as f64 / pages.max(1) as f64
            );
        }

        // Same padding rule as allocate_context_with_extra: page-multiple
        // dirty arena so the Metal no-copy buffer wraps it directly.
        let dirty_size = ggml_pad(extra_bytes, GGML_MEM_ALIGN).next_multiple_of(16384);
        let mut ctx = Context::new_with_ro_region(region, dirty_size);
        let mut tensor_ids = BTreeMap::new();
        for tensor in &self.tensors {
            match map_tensor_at_file_offset(&mut ctx, gguf, tensor) {
                Ok(id) => {
                    tensor_ids.insert(tensor.name.clone(), id);
                }
                Err(err) => {
                    eprintln!(
                        "llama: cannot map tensor '{}' ({}) — using the owned-arena weight path",
                        tensor.name, err
                    );
                    return None;
                }
            }
        }
        Some(LoadedGgufWeights { ctx, tensor_ids })
    }
}

fn map_tensor_at_file_offset(
    ctx: &mut Context,
    gguf: &GgufFile,
    tensor: &GgufTensorInfo,
) -> Result<TensorId> {
    let start = usize::try_from(tensor.absolute_offset(gguf.data_offset)?)
        .map_err(|_| LlamaError::format("tensor offset does not fit in usize"))?;
    let size_bytes = usize::try_from(tensor.size_bytes)
        .map_err(|_| LlamaError::format("tensor size does not fit in usize"))?;
    let end = start
        .checked_add(size_bytes)
        .ok_or_else(|| LlamaError::format("tensor byte range overflow"))?;
    let file_size = usize::try_from(gguf.file_size)
        .map_err(|_| LlamaError::format("file size does not fit in usize"))?;
    if end > file_size {
        return Err(LlamaError::format(format!(
            "tensor range [{}..{}) exceeds file size {}",
            start, end, file_size
        )));
    }
    let dims = tensor_extents_i64(tensor)?;
    let id = ctx
        .new_named_tensor_at_offset(
            tensor.name.clone(),
            tensor.tensor_type,
            dims.len(),
            &dims,
            BufferUsage::Weights,
            start,
        )
        .map_err(LlamaError::format)?;
    let nbytes = ctx
        .tensor(id)
        .ok_or_else(|| LlamaError::format("mapped tensor id is invalid"))?
        .nbytes();
    if nbytes != size_bytes {
        return Err(LlamaError::format(format!(
            "computed tensor size {} does not match gguf size {}",
            nbytes, size_bytes
        )));
    }
    Ok(id)
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
