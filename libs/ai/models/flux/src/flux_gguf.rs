//! City96 / ComfyUI-GGUF Flux DiT loader.
//!
//! Same container as Qwen/H3 (`GgufFile`). Tensor names are already the
//! canonical `double_blocks.*` / `img_in.*` set our transformer walks.
//! GGUF dims are ggml order (`[k, n]`), matching `flux_target_extents`.
//!
//! Prefers a no-copy mmap of the file (16GB Mac) and falls back to an
//! owned arena if mapping fails.

use crate::flux::{
    canonicalize_flux_diffusion_tensor_name, FluxTensorNameStyle, FluxTransformerConfig,
    FluxTransformerInspection,
};
use crate::flux_transformer::LoadedFluxTransformerWeights;
use crate::{emit_byte_progress, DiffusionError, ProgressHook, Result};
use makepad_ggml::{
    ggml_pad, BufferUsage, Context, MappedRegion, GGML_MEM_ALIGN,
};
use makepad_ai_llm::{GgufFile, GgufTensorInfo};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

const DIRTY_ARENA: usize = 64 * 1024 * 1024;

pub fn is_gguf_path(path: impl AsRef<Path>) -> bool {
    path.as_ref()
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("gguf"))
}

pub fn inspect(path: impl AsRef<Path>) -> Result<FluxGgufInspection> {
    let path = path.as_ref();
    let file = open_gguf(path)?;
    let transformer = inspection_from_gguf(&file)?;
    let mut quantized = 0usize;
    let mut raw = 0usize;
    let mut bytes = 0u64;
    for tensor in &file.tensors {
        bytes = bytes.saturating_add(tensor.size_bytes);
        if tensor.tensor_type.scalar_size_bytes().is_some() {
            raw += 1;
        } else {
            quantized += 1;
        }
    }
    Ok(FluxGgufInspection {
        path: path.display().to_string(),
        architecture: file
            .get_value("general.architecture")
            .and_then(|v| v.as_string())
            .and_then(|s| s.try_utf8().ok())
            .unwrap_or("")
            .to_string(),
        file_type: file
            .get_value("general.file_type")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        tensor_count: file.tensors.len(),
        quantized_tensors: quantized,
        raw_tensors: raw,
        tensor_bytes: bytes,
        transformer,
    })
}

pub fn load_weights(
    path: impl AsRef<Path>,
    extra_bytes: usize,
    mut progress: Option<ProgressHook>,
) -> Result<LoadedFluxTransformerWeights> {
    let path = path.as_ref();
    let file = open_gguf(path)?;
    let inspect = inspection_from_gguf(&file)?;
    let extras: Vec<&GgufTensorInfo> = file
        .tensors
        .iter()
        .filter(|tensor| keep_flux_tensor(&tensor.name))
        .collect();
    if extras.is_empty() {
        return Err(DiffusionError::model(format!(
            "flux GGUF {} has no recognized DiT tensors",
            path.display()
        )));
    }

    emit_byte_progress(&mut progress, "load unet gguf", 0, extras.len())?;

    // Compiled-graph extras used to land in this CPU dirty arena and grow
    // to 32GiB. Intermediates are now planned into the Metal dirty buffer
    // (no_alloc); keep a small host prefix for inputs / rope / ones.
    let _ = extra_bytes;
    let dirty = DIRTY_ARENA;
    let (ctx, tensor_ids, quantized) = match try_map_weights(&file, &extras, dirty) {
        Some(mapped) => mapped,
        None => load_owned_weights(&file, &extras, dirty, &mut progress)?,
    };
    emit_byte_progress(&mut progress, "load unet gguf", extras.len(), extras.len())?;

    Ok(LoadedFluxTransformerWeights::from_loaded(
        ctx,
        tensor_ids,
        inspect.config,
        path.to_path_buf(),
        false,
        quantized,
        extra_bytes,
    ))
}

#[derive(Clone, Debug)]
pub struct FluxGgufInspection {
    pub path: String,
    pub architecture: String,
    pub file_type: u64,
    pub tensor_count: usize,
    pub quantized_tensors: usize,
    pub raw_tensors: usize,
    pub tensor_bytes: u64,
    pub transformer: FluxTransformerInspection,
}

fn open_gguf(path: &Path) -> Result<GgufFile> {
    let file = GgufFile::open(path).map_err(|err| {
        DiffusionError::model(format!("flux GGUF {}: {err}", path.display()))
    })?;
    if file.version != 3 {
        return Err(DiffusionError::model(format!(
            "flux GGUF {} uses version {}, expected v3",
            path.display(),
            file.version
        )));
    }
    Ok(file)
}

fn keep_flux_tensor(name: &str) -> bool {
    recognized_canonical(&canonicalize_flux_diffusion_tensor_name(name))
}

fn recognized_canonical(name: &str) -> bool {
    name.starts_with("double_blocks.")
        || name.starts_with("single_blocks.")
        || name.starts_with("img_in.")
        || name.starts_with("time_in.")
        || name.starts_with("vector_in.")
        || name.starts_with("guidance_in.")
        || name.starts_with("txt_in.")
        || name.starts_with("final_layer.")
        || name.starts_with("distilled_guidance_layer.")
        || name.starts_with("img_in_patch.")
}

fn inspection_from_gguf(file: &GgufFile) -> Result<FluxTransformerInspection> {
    let mut inferred = FluxTransformerConfig::flux1_dev();
    let mut canonical_hits = 0usize;
    let mut renamed_hits = 0usize;
    let mut max_double_block = None::<u32>;
    let mut max_single_block = None::<u32>;
    let mut hidden_size = None::<u32>;
    let mut context_in_dim = None::<u32>;
    let mut in_channels = None::<u32>;
    let mut out_channels = None::<u32>;
    let mut vec_in_dim = None::<u32>;
    let mut head_dim = None::<u32>;
    let mut guidance_embed = false;

    for tensor in &file.tensors {
        let canonical = canonicalize_flux_diffusion_tensor_name(&tensor.name);
        if recognized_canonical(&canonical) {
            canonical_hits += 1;
        }
        if canonical != tensor.name {
            renamed_hits += 1;
        }
        let dims = &tensor.dimensions;
        if canonical == "txt_in.weight" {
            hidden_size = pytorch_dim(dims, 0);
            context_in_dim = pytorch_dim(dims, 1);
        } else if canonical == "img_in.weight" {
            in_channels = pytorch_dim(dims, 1);
        } else if canonical == "vector_in.in_layer.weight" {
            vec_in_dim = pytorch_dim(dims, 1);
        } else if canonical == "guidance_in.in_layer.weight" {
            guidance_embed = true;
        } else if canonical == "single_blocks.0.norm.key_norm.scale"
            || canonical == "double_blocks.0.txt_attn.norm.key_norm.scale"
        {
            head_dim = pytorch_dim(dims, 0);
        } else if canonical == "final_layer.linear.weight" {
            out_channels = pytorch_dim(dims, 0);
        } else if let Some(index) = block_index(&canonical, "double_blocks.") {
            max_double_block = Some(max_double_block.map_or(index, |current| current.max(index)));
        } else if let Some(index) = block_index(&canonical, "single_blocks.") {
            max_single_block = Some(max_single_block.map_or(index, |current| current.max(index)));
        }
    }

    inferred.hidden_size = hidden_size.ok_or_else(|| {
        DiffusionError::model(format!(
            "could not infer FLUX hidden_size from {}",
            file.path.display()
        ))
    })?;
    inferred.context_in_dim = context_in_dim.ok_or_else(|| {
        DiffusionError::model(format!(
            "could not infer FLUX context_in_dim from {}",
            file.path.display()
        ))
    })?;
    if let Some(value) = in_channels {
        inferred.in_channels = value;
    }
    if let Some(value) = out_channels {
        inferred.out_channels = value;
    }
    if let Some(value) = vec_in_dim {
        inferred.vec_in_dim = value;
    }
    if let Some(value) = max_double_block {
        inferred.depth = value + 1;
    }
    if let Some(value) = max_single_block {
        inferred.depth_single_blocks = value + 1;
    }
    inferred.guidance_embed = guidance_embed;

    let head_dim = head_dim.ok_or_else(|| {
        DiffusionError::model(format!(
            "could not infer FLUX head_dim from {}",
            file.path.display()
        ))
    })?;
    if head_dim == 0 || inferred.hidden_size % head_dim != 0 {
        return Err(DiffusionError::model(format!(
            "invalid FLUX head_dim {} for hidden_size {} in {}",
            head_dim,
            inferred.hidden_size,
            file.path.display()
        )));
    }
    inferred.num_heads = inferred.hidden_size / head_dim;
    inferred.guidance_embed = guidance_embed;

    let tensor_name_style = match (canonical_hits > 0, renamed_hits > 0) {
        (true, false) => FluxTensorNameStyle::Canonical,
        (true, true) => FluxTensorNameStyle::Mixed,
        (false, true) => FluxTensorNameStyle::Diffusers,
        (false, false) => FluxTensorNameStyle::Unknown,
    };

    Ok(FluxTransformerInspection {
        tensor_name_style,
        canonical_tensor_count: canonical_hits,
        config: inferred,
    })
}

/// GGUF stores ggml order (`ne[0]` innermost = PyTorch last dim).
fn pytorch_dim(ggml_dims: &[u64], axis: usize) -> Option<u32> {
    let idx = ggml_dims.len().checked_sub(1)?.checked_sub(axis)?;
    u32::try_from(ggml_dims[idx]).ok()
}

fn block_index(name: &str, prefix: &str) -> Option<u32> {
    let rest = name.strip_prefix(prefix)?;
    let (index, _) = rest.split_once('.')?;
    index.parse().ok()
}

fn tensor_extents(tensor: &GgufTensorInfo) -> Result<Vec<i64>> {
    tensor
        .dimensions
        .iter()
        .map(|dim| {
            i64::try_from(*dim).map_err(|_| {
                DiffusionError::model(format!(
                    "flux GGUF tensor '{}' dim {dim} does not fit i64",
                    tensor.name
                ))
            })
        })
        .collect()
}

fn try_map_weights(
    file: &GgufFile,
    tensors: &[&GgufTensorInfo],
    dirty: usize,
) -> Option<(Context, BTreeMap<String, makepad_ggml::TensorId>, bool)> {
    let region = match MappedRegion::map_file(&file.path) {
        Ok(region) => Arc::new(region),
        Err(err) => {
            eprintln!("flux: mmap {} failed ({err}); using owned arena", file.path.display());
            return None;
        }
    };
    let dirty_size = ggml_pad(dirty, GGML_MEM_ALIGN).next_multiple_of(16384);
    let mut ctx = Context::new_with_ro_region(region, dirty_size);
    let mut tensor_ids = BTreeMap::new();
    let mut quantized = false;
    for tensor in tensors {
        let canonical = canonicalize_flux_diffusion_tensor_name(&tensor.name);
        let start = usize::try_from(tensor.absolute_offset(file.data_offset).ok()?).ok()?;
        let extents = tensor_extents(tensor).ok()?;
        let id = ctx
            .new_named_tensor_at_offset(
                canonical.clone(),
                tensor.tensor_type,
                extents.len(),
                &extents,
                BufferUsage::Weights,
                start,
            )
            .ok()?;
        let nbytes = ctx.tensor(id)?.nbytes();
        let size_bytes = usize::try_from(tensor.size_bytes).ok()?;
        if nbytes != size_bytes {
            eprintln!(
                "flux: mapped '{}' size {nbytes} != gguf {size_bytes}; using owned arena",
                tensor.name
            );
            return None;
        }
        if tensor_ids.insert(canonical, id).is_some() {
            return None;
        }
        quantized |= tensor.tensor_type.scalar_size_bytes().is_none();
    }
    Some((ctx, tensor_ids, quantized))
}

fn load_owned_weights(
    file: &GgufFile,
    tensors: &[&GgufTensorInfo],
    extra_bytes: usize,
    progress: &mut Option<ProgressHook>,
) -> Result<(Context, BTreeMap<String, makepad_ggml::TensorId>, bool)> {
    let mut total = extra_bytes;
    for tensor in tensors {
        let size = usize::try_from(tensor.size_bytes).map_err(|_| {
            DiffusionError::model(format!(
                "flux GGUF tensor '{}' size does not fit usize",
                tensor.name
            ))
        })?;
        total = ggml_pad(total, GGML_MEM_ALIGN)
            .checked_add(size)
            .ok_or_else(|| DiffusionError::model("flux GGUF arena overflow"))?;
    }
    let mut ctx = Context::new(makepad_ggml::InitParams {
        mem_size: total,
        mem_buffer: None,
        no_alloc: false,
    });
    let mut tensor_ids = BTreeMap::new();
    let mut quantized = false;
    let mut done = 0usize;
    for tensor in tensors {
        let canonical = canonicalize_flux_diffusion_tensor_name(&tensor.name);
        let extents = tensor_extents(tensor)?;
        let id = ctx
            .new_named_tensor(
                canonical.clone(),
                tensor.tensor_type,
                extents.len(),
                &extents,
                BufferUsage::Weights,
            )
            .map_err(DiffusionError::model)?;
        let dst = ctx
            .tensor_data_mut(id)
            .map_err(DiffusionError::model)?;
        file.read_tensor_into(&tensor.name, dst).map_err(|err| {
            DiffusionError::model(format!("flux GGUF read '{}': {err}", tensor.name))
        })?;
        if tensor_ids.insert(canonical, id).is_some() {
            return Err(DiffusionError::model(format!(
                "duplicate canonical flux GGUF tensor '{}'",
                tensor.name
            )));
        }
        quantized |= tensor.tensor_type.scalar_size_bytes().is_none();
        done += 1;
        if done % 32 == 0 {
            emit_byte_progress(progress, "load unet gguf", done, tensors.len())?;
        }
    }
    Ok((ctx, tensor_ids, quantized))
}
