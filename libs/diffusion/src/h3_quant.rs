//! Quantized MiniMax-H3 weight sources: GGUF (unsloth/leejet Q4_K family)
//! and ComfyUI/ModelOpt NVFP4 safetensors, exposed under the canonical
//! diffusers tensor names consumed by the H3 pipeline.
//!
//! The GGUF path is deliberately strict: it validates the complete DiT or
//! text-encoder inventory at open time, maps fused QKV rows without
//! dequantizing them, and streams only the requested row range from disk.
//! A malformed or merely look-alike GGUF therefore never reaches CUDA.

use crate::error::{DiffusionError, Result};
use crate::flux2::Flux2SafetensorsHeader;
use makepad_ggml::quant::{
    block_elements, block_size, dequantize_nvfp4_pairs_row, get_rows_ggml_bytes_cpu,
    h3_nvfp4_pairs_pack,
};
use makepad_llama::{GgufFile, GgufTensorInfo};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub use makepad_ggml::quant::{
    GGML_TYPE_BF16, GGML_TYPE_F16, GGML_TYPE_F32, GGML_TYPE_H3_NVFP4_PAIRS,
    GGML_TYPE_H3_NVFP4_PAIRS_PRESCALE, GGML_TYPE_Q4_0, GGML_TYPE_Q4_K, GGML_TYPE_Q6_K,
};

/// The pruned checkpoints' rank-`dim` timestep curve replacing the
/// `time_embedder` MLP (values are the already-silu'd temb basis).
#[derive(Clone, Debug)]
pub struct H3AdalnCurve {
    pub dim: usize,
    pub grid: usize,
    /// `grid * dim`, row-major grid rows.
    pub values: Vec<f32>,
}

impl H3AdalnCurve {
    /// Linear interpolation over the grid at timestep `t`.
    pub fn temb(&self, t: f32) -> Vec<f32> {
        // Construction is private to validated loaders, but keep this safe
        // for hand-built values in tests and downstream tooling.
        if self.dim == 0 || self.grid < 2 || self.values.len() != self.dim * self.grid {
            return Vec::new();
        }
        let pos = t.clamp(0.0, 1.0) * (self.grid - 1) as f32;
        let lower = (pos.floor() as usize).min(self.grid - 2);
        let frac = pos - lower as f32;
        let lo = &self.values[lower * self.dim..(lower + 1) * self.dim];
        let hi = &self.values[(lower + 1) * self.dim..(lower + 2) * self.dim];
        lo.iter()
            .zip(hi)
            .map(|(a, b)| a + (b - a) * frac)
            .collect()
    }
}

/// One mapped 2-D weight: canonical name -> device-uploadable payload.
#[derive(Clone, Debug)]
pub struct H3QuantLinear {
    /// Output rows.
    pub n: usize,
    /// Input columns.
    pub k: usize,
    /// Runtime payload type. Pinned F32 matrices are normalized once to
    /// BF16 at the loader boundary because the native H3 dense path has no
    /// F32-weight cache/GEMM contract.
    pub ggml_type: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum H3QuantComponent {
    Dit,
    TextEncoder,
}

pub struct H3GgufWeights {
    pub path: PathBuf,
    imp: GgufImp,
}

pub struct H3Nvfp4Weights {
    pub path: PathBuf,
    imp: Nvfp4Imp,
}

#[derive(Clone, Debug)]
struct GgufMappedTensor {
    source: String,
    /// Canonical ggml dimensions: `[k, n]` for linears, `[len]` for vectors.
    dims: Vec<usize>,
    ggml_type: u32,
    byte_offset: u64,
    byte_len: u64,
}

struct GgufImp {
    file: GgufFile,
    tensors: HashMap<String, GgufMappedTensor>,
    linears: HashMap<String, H3QuantLinear>,
    total_disk_bytes: u64,
    adaln_curve: Option<H3AdalnCurve>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NvDtype {
    F32,
    F16,
    BF16,
    U8,
    I8,
    F8E4M3,
}

impl NvDtype {
    fn parse(value: &str, name: &str) -> Result<Self> {
        match value {
            "F32" => Ok(Self::F32),
            "F16" => Ok(Self::F16),
            "BF16" => Ok(Self::BF16),
            "U8" => Ok(Self::U8),
            "I8" => Ok(Self::I8),
            "F8_E4M3" => Ok(Self::F8E4M3),
            other => Err(model_error(format!(
                "h3 NVFP4 tensor '{name}' has unsupported safetensors dtype '{other}'"
            ))),
        }
    }

    fn byte_width(self) -> usize {
        match self {
            Self::F32 => 4,
            Self::F16 | Self::BF16 => 2,
            Self::U8 | Self::I8 | Self::F8E4M3 => 1,
        }
    }

    fn raw_ggml_type(self, name: &str) -> Result<u32> {
        match self {
            Self::F32 => Ok(GGML_TYPE_F32),
            Self::F16 => Ok(GGML_TYPE_F16),
            Self::BF16 => Ok(GGML_TYPE_BF16),
            other => Err(model_error(format!(
                "h3 NVFP4 tensor '{name}' uses {other:?}, not a raw CUDA linear dtype"
            ))),
        }
    }
}

#[derive(Clone, Debug)]
enum NvMappedStorage {
    /// A contiguous byte slice inside one raw F32/F16/BF16 tensor. The
    /// slice is normally the whole tensor; fused QKV uses three row slices.
    Direct {
        source: String,
        dtype: NvDtype,
        byte_offset: u64,
        byte_len: u64,
    },
    /// ModelOpt/Comfy compound NVFP4 group. `row_start..row_start+rows`
    /// permits zero-copy-at-rest splits of the DiT's fused QKV tensors.
    Quant {
        weight: String,
        scale: String,
        scale2: String,
        pre_scale: Option<String>,
        row_start: usize,
        rows: usize,
        k: usize,
    },
    /// The NVFP4-AWQ Qwen bundle keeps token embeddings as symmetric I8
    /// rows plus one F32 scale per row rather than as NVFP4 pairs.
    Int8Rows {
        weight: String,
        scale: String,
        rows: usize,
        k: usize,
    },
}

#[derive(Clone, Debug)]
struct NvMappedTensor {
    /// Canonical ggml dimensions (`[k,n]` for matrices).
    dims: Vec<usize>,
    storage: NvMappedStorage,
    disk_bytes: u64,
}

struct Nvfp4Imp {
    file: Flux2SafetensorsHeader,
    data_offset: u64,
    file_size: u64,
    tensors: HashMap<String, NvMappedTensor>,
    linears: HashMap<String, H3QuantLinear>,
    total_disk_bytes: u64,
    adaln_curve: Option<H3AdalnCurve>,
}

fn model_error(message: impl Into<String>) -> DiffusionError {
    DiffusionError::model(message.into())
}

/// Convert a raw little-endian F32 matrix to the BF16 payload consumed by
/// the native dense-linear cache. This is a format normalization, not an
/// execution fallback: the runtime type advertised by `H3QuantLinear` is
/// BF16 and the original F32 bytes are never uploaded under a false type.
fn f32_payload_to_bf16(bytes: &[u8], name: &str) -> Result<Vec<u8>> {
    if bytes.len() % 4 != 0 {
        return Err(model_error(format!(
            "h3 F32 linear '{name}' payload length {} is not a multiple of 4",
            bytes.len()
        )));
    }
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for (index, chunk) in bytes.chunks_exact(4).enumerate() {
        let value = f32::from_le_bytes(chunk.try_into().unwrap());
        if !value.is_finite() {
            return Err(model_error(format!(
                "h3 F32 linear '{name}' has non-finite value {value} at {index}"
            )));
        }
        let bits = value.to_bits();
        // IEEE round-to-nearest, ties-to-even (same conversion as
        // torch `.to(torch.bfloat16)`).
        let rounded = bits.wrapping_add(0x7fff + ((bits >> 16) & 1));
        out.extend_from_slice(&((rounded >> 16) as u16).to_le_bytes());
    }
    Ok(out)
}

impl H3GgufWeights {
    pub fn load(path: &Path, component: H3QuantComponent) -> Result<Self> {
        if !path.is_file() {
            return Err(model_error(format!(
                "h3 {:?} GGUF is not a file: {}",
                component,
                path.display()
            )));
        }
        let file = GgufFile::open(path).map_err(|err| {
            model_error(format!("h3 {:?} GGUF {}: {err}", component, path.display()))
        })?;
        if file.version != 3 {
            return Err(model_error(format!(
                "h3 {:?} GGUF {} uses version {}, expected the pinned v3 layout",
                component,
                path.display(),
                file.version
            )));
        }
        let total_disk_bytes = file
            .tensors
            .iter()
            .try_fold(0u64, |sum, tensor| sum.checked_add(tensor.size_bytes))
            .ok_or_else(|| model_error("h3 GGUF tensor byte total overflow"))?;
        let mut tensors = HashMap::new();
        let mut linears = HashMap::new();

        match component {
            H3QuantComponent::Dit => map_dit_gguf(&file, &mut tensors, &mut linears)?,
            H3QuantComponent::TextEncoder => {
                map_te_gguf(&file, &mut tensors, &mut linears)?
            }
        }

        let adaln_curve = if component == H3QuantComponent::Dit {
            load_adaln_curve(&file, &tensors)?
        } else {
            None
        };
        validate_component(component, &tensors, &linears, adaln_curve.as_ref())?;

        Ok(Self {
            path: path.to_path_buf(),
            imp: GgufImp {
                file,
                tensors,
                linears,
                total_disk_bytes,
                adaln_curve,
            },
        })
    }

    pub fn has_tensor(&self, name: &str) -> bool {
        self.imp.tensors.contains_key(name)
    }

    /// Decode a small raw floating tensor. Block-quantized tensors are
    /// intentionally refused: large linears must stay packed and streamed.
    pub fn tensor_f32(&self, name: &str) -> Result<Vec<f32>> {
        let mapped = self.mapped(name)?;
        if !matches!(mapped.ggml_type, GGML_TYPE_F32 | GGML_TYPE_F16 | GGML_TYPE_BF16) {
            return Err(model_error(format!(
                "h3 GGUF tensor '{name}' is type {}, not a host-decodable raw float tensor",
                mapped.ggml_type
            )));
        }
        let (k, n) = mapped_kn(mapped, name)?;
        let payload = self.read_mapped(mapped, name)?;
        let rows: Vec<i32> = (0..n)
            .map(|row| i32::try_from(row))
            .collect::<std::result::Result<_, _>>()
            .map_err(|_| model_error(format!("h3 GGUF tensor '{name}' has too many rows")))?;
        get_rows_ggml_bytes_cpu(&payload, mapped.ggml_type, k, n, &rows).ok_or_else(|| {
            model_error(format!(
                "h3 GGUF tensor '{name}' cannot decode type {} shape [{k},{n}]",
                mapped.ggml_type
            ))
        })
    }

    pub fn tensor_row_f32(&self, name: &str, row: u64) -> Result<Vec<f32>> {
        let mapped = self.mapped(name)?;
        let (k, n) = mapped_kn(mapped, name)?;
        let row = usize::try_from(row)
            .map_err(|_| model_error(format!("h3 GGUF tensor '{name}' row does not fit usize")))?;
        if row >= n {
            return Err(model_error(format!(
                "h3 GGUF tensor '{name}' row {row} out of range 0..{n}"
            )));
        }
        let row_bytes = ggml_row_bytes(mapped.ggml_type, k, name)?;
        let relative = row
            .checked_mul(row_bytes)
            .and_then(|value| u64::try_from(value).ok())
            .ok_or_else(|| model_error(format!("h3 GGUF tensor '{name}' row offset overflow")))?;
        let mut row_map = mapped.clone();
        row_map.byte_offset = row_map
            .byte_offset
            .checked_add(relative)
            .ok_or_else(|| model_error(format!("h3 GGUF tensor '{name}' row offset overflow")))?;
        row_map.byte_len = row_bytes as u64;
        let payload = self.read_mapped(&row_map, name)?;
        get_rows_ggml_bytes_cpu(&payload, mapped.ggml_type, k, 1, &[0]).ok_or_else(|| {
            model_error(format!(
                "h3 GGUF tensor '{name}' row {row} cannot decode type {}",
                mapped.ggml_type
            ))
        })
    }

    pub fn tensor_disk_bytes(&self, name: &str) -> Result<u64> {
        Ok(self.mapped(name)?.byte_len)
    }

    pub fn total_disk_bytes(&self) -> u64 {
        self.imp.total_disk_bytes
    }

    pub fn tensor_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.imp.tensors.keys().cloned().collect();
        names.sort();
        names
    }

    pub fn linear(&self, name: &str) -> Option<&H3QuantLinear> {
        self.imp.linears.get(name)
    }

    pub fn linear_payload(&self, name: &str) -> Result<Vec<u8>> {
        if !self.imp.linears.contains_key(name) {
            return Err(model_error(format!(
                "h3 GGUF tensor '{name}' is not a mapped linear weight"
            )));
        }
        let mapped = self.mapped(name)?;
        let bytes = self.read_mapped(mapped, name)?;
        if mapped.ggml_type == GGML_TYPE_F32 {
            f32_payload_to_bf16(&bytes, name)
        } else {
            Ok(bytes)
        }
    }

    pub fn adaln_curve(&self) -> Option<&H3AdalnCurve> {
        self.imp.adaln_curve.as_ref()
    }

    fn mapped(&self, name: &str) -> Result<&GgufMappedTensor> {
        self.imp.tensors.get(name).ok_or_else(|| {
            model_error(format!(
                "h3 GGUF tensor '{name}' not found in {}",
                self.path.display()
            ))
        })
    }

    fn read_mapped(&self, mapped: &GgufMappedTensor, canonical: &str) -> Result<Vec<u8>> {
        let source = self.imp.file.get_tensor(&mapped.source).ok_or_else(|| {
            model_error(format!(
                "h3 GGUF canonical tensor '{canonical}' lost source '{}'",
                mapped.source
            ))
        })?;
        let tensor_start = source
            .absolute_offset(self.imp.file.data_offset)
            .map_err(|err| model_error(format!("h3 GGUF '{}': {err}", mapped.source)))?;
        let start = tensor_start
            .checked_add(mapped.byte_offset)
            .ok_or_else(|| model_error(format!("h3 GGUF '{canonical}' offset overflow")))?;
        let end = start
            .checked_add(mapped.byte_len)
            .ok_or_else(|| model_error(format!("h3 GGUF '{canonical}' end overflow")))?;
        let source_end = tensor_start
            .checked_add(source.size_bytes)
            .ok_or_else(|| model_error(format!("h3 GGUF '{}' end overflow", mapped.source)))?;
        if end > source_end || end > self.imp.file.file_size {
            return Err(model_error(format!(
                "h3 GGUF '{canonical}' byte range {start}..{end} exceeds source/file bounds"
            )));
        }
        let len = usize::try_from(mapped.byte_len).map_err(|_| {
            model_error(format!("h3 GGUF '{canonical}' byte length does not fit usize"))
        })?;
        let mut file = File::open(&self.path).map_err(|err| {
            model_error(format!("h3 GGUF open {}: {err}", self.path.display()))
        })?;
        file.seek(SeekFrom::Start(start)).map_err(|err| {
            model_error(format!("h3 GGUF seek '{canonical}' at {start}: {err}"))
        })?;
        let mut bytes = vec![0u8; len];
        file.read_exact(&mut bytes).map_err(|err| {
            model_error(format!("h3 GGUF read '{canonical}' ({len} bytes): {err}"))
        })?;
        Ok(bytes)
    }
}

fn mapped_kn(mapped: &GgufMappedTensor, name: &str) -> Result<(usize, usize)> {
    match mapped.dims.as_slice() {
        [k] => Ok((*k, 1)),
        [k, n] => Ok((*k, *n)),
        dims => Err(model_error(format!(
            "h3 GGUF tensor '{name}' rank {} is not row-decodable",
            dims.len()
        ))),
    }
}

fn ggml_row_bytes(ggml_type: u32, k: usize, name: &str) -> Result<usize> {
    let block = block_elements(ggml_type);
    if k == 0 || k % block != 0 {
        return Err(model_error(format!(
            "h3 GGUF tensor '{name}' row width {k} is not divisible by type-{ggml_type} block {block}"
        )));
    }
    (k / block)
        .checked_mul(block_size(ggml_type))
        .ok_or_else(|| model_error(format!("h3 GGUF tensor '{name}' row byte size overflow")))
}

fn usize_dims(tensor: &GgufTensorInfo) -> Result<Vec<usize>> {
    tensor
        .dimensions
        .iter()
        .map(|dim| {
            usize::try_from(*dim).map_err(|_| {
                model_error(format!(
                    "h3 GGUF tensor '{}' dimension {dim} does not fit usize",
                    tensor.name
                ))
            })
        })
        .collect()
}

fn insert_direct(
    tensors: &mut HashMap<String, GgufMappedTensor>,
    canonical: String,
    source: &GgufTensorInfo,
    dims_override: Option<Vec<usize>>,
) -> Result<()> {
    let dims = match dims_override {
        Some(dims) => dims,
        None => usize_dims(source)?,
    };
    let mapped = GgufMappedTensor {
        source: source.name.clone(),
        dims,
        ggml_type: source.tensor_type.ggml_type(),
        byte_offset: 0,
        byte_len: source.size_bytes,
    };
    if tensors.insert(canonical.clone(), mapped).is_some() {
        return Err(model_error(format!(
            "h3 GGUF canonical tensor name collision: '{canonical}'"
        )));
    }
    Ok(())
}

fn insert_qkv_split(
    tensors: &mut HashMap<String, GgufMappedTensor>,
    linears: &mut HashMap<String, H3QuantLinear>,
    prefix: &str,
    source: &GgufTensorInfo,
) -> Result<()> {
    let dims = usize_dims(source)?;
    let [k, fused_n] = dims.as_slice() else {
        return Err(model_error(format!(
            "h3 GGUF fused QKV '{}' must be rank 2, got {:?}",
            source.name, dims
        )));
    };
    if *fused_n % 3 != 0 {
        return Err(model_error(format!(
            "h3 GGUF fused QKV '{}' output rows {fused_n} not divisible by 3",
            source.name
        )));
    }
    let n = *fused_n / 3;
    let ty = source.tensor_type.ggml_type();
    let runtime_ty = runtime_linear_type(ty, &source.name)?;
    let row_bytes = ggml_row_bytes(ty, *k, &source.name)?;
    let total = row_bytes
        .checked_mul(*fused_n)
        .ok_or_else(|| model_error(format!("h3 GGUF '{}' byte size overflow", source.name)))?;
    if total as u64 != source.size_bytes {
        return Err(model_error(format!(
            "h3 GGUF '{}' size {} != {fused_n} rows x {row_bytes}",
            source.name, source.size_bytes
        )));
    }
    for (part, suffix) in ["to_q", "to_k", "to_v"].into_iter().enumerate() {
        let canonical = format!("{prefix}.attn.{suffix}.weight");
        let byte_offset = part
            .checked_mul(n)
            .and_then(|rows| rows.checked_mul(row_bytes))
            .ok_or_else(|| model_error(format!("h3 GGUF '{canonical}' offset overflow")))?;
        let byte_len = n
            .checked_mul(row_bytes)
            .ok_or_else(|| model_error(format!("h3 GGUF '{canonical}' size overflow")))?;
        let mapped = GgufMappedTensor {
            source: source.name.clone(),
            dims: vec![*k, n],
            ggml_type: ty,
            byte_offset: byte_offset as u64,
            byte_len: byte_len as u64,
        };
        if tensors.insert(canonical.clone(), mapped).is_some()
            || linears
                .insert(
                    canonical.clone(),
                    H3QuantLinear {
                        n,
                        k: *k,
                        ggml_type: runtime_ty,
                    },
                )
                .is_some()
        {
            return Err(model_error(format!(
                "h3 GGUF canonical QKV name collision: '{canonical}'"
            )));
        }
    }
    Ok(())
}

fn insert_linear_for_mapped(
    tensors: &HashMap<String, GgufMappedTensor>,
    linears: &mut HashMap<String, H3QuantLinear>,
    canonical: &str,
) -> Result<()> {
    let mapped = tensors.get(canonical).ok_or_else(|| {
        model_error(format!("h3 GGUF cannot mark absent tensor '{canonical}' linear"))
    })?;
    let [k, n] = mapped.dims.as_slice() else {
        return Err(model_error(format!(
            "h3 GGUF linear '{canonical}' must have canonical rank 2, got {:?}",
            mapped.dims
        )));
    };
    let runtime_type = runtime_linear_type(mapped.ggml_type, canonical)?;
    let expected = ggml_row_bytes(mapped.ggml_type, *k, canonical)?
        .checked_mul(*n)
        .ok_or_else(|| model_error(format!("h3 GGUF linear '{canonical}' size overflow")))?;
    if expected as u64 != mapped.byte_len {
        return Err(model_error(format!(
            "h3 GGUF linear '{canonical}' payload {} != [{k},{n}] type {} expected {expected}",
            mapped.byte_len, mapped.ggml_type
        )));
    }
    linears.insert(
        canonical.to_string(),
        H3QuantLinear {
            n: *n,
            k: *k,
            ggml_type: runtime_type,
        },
    );
    Ok(())
}

fn runtime_linear_type(source_type: u32, name: &str) -> Result<u32> {
    match source_type {
        // The GGUF/NVFP4 repacks retain a handful of small projection
        // matrices in F32. Normalize those to a real BF16 cache payload;
        // gpu_weight_cache_ensure is explicitly a 2-byte weight contract.
        GGML_TYPE_F32 => Ok(GGML_TYPE_BF16),
        GGML_TYPE_F16
        | GGML_TYPE_BF16
        | GGML_TYPE_Q4_0
        | GGML_TYPE_Q4_K
        | GGML_TYPE_Q6_K
        | GGML_TYPE_H3_NVFP4_PAIRS
        | GGML_TYPE_H3_NVFP4_PAIRS_PRESCALE => Ok(source_type),
        _ => Err(model_error(format!(
            "h3 quant linear '{name}' uses ggml type {source_type}, unsupported by the native H3 CUDA path"
        ))),
    }
}

fn dit_prefix_and_tail(source_name: &str) -> Option<(String, &str)> {
    if let Some(rest) = source_name.strip_prefix("blocks.") {
        let (layer, tail) = rest.split_once('.')?;
        if layer.parse::<usize>().ok()? >= crate::h3::H3_DEPTH {
            return None;
        }
        return Some((format!("transformer_blocks.{layer}"), tail));
    }
    if let Some(rest) = source_name.strip_prefix("token_refiner.blocks.") {
        let (layer, tail) = rest.split_once('.')?;
        if layer.parse::<usize>().ok()? >= crate::h3::H3_REFINER_DEPTH {
            return None;
        }
        return Some((format!("token_refiner.refiner_blocks.{layer}"), tail));
    }
    None
}

fn map_dit_tail(prefix: &str, tail: &str) -> String {
    let tail = tail
        .replace("attn.q_norm", "attn.norm_q")
        .replace("attn.k_norm", "attn.norm_k")
        .replace("attn.out_proj", "attn.to_out.0")
        .replace("mlp.fc1", "ff.net.0.proj")
        .replace("mlp.fc2", "ff.net.2");
    format!("{prefix}.{tail}")
}

fn top_level_dit_name(name: &str) -> Option<String> {
    let replacements = [
        ("video_patch_proj.", "proj_in."),
        ("audio_patch_proj.", "audio_proj_in."),
        ("condition_proj.", "context_embedder."),
        ("final_layer.norm.", "norm_out.norm."),
        ("final_layer.adaln_proj.linear.", "norm_out.linear."),
        ("final_layer.video_out.", "proj_out."),
        ("final_layer.audio_out.", "audio_proj_out."),
        ("time_embedder.proj_in.", "time_embedder.linear_1."),
        ("time_embedder.proj_out.", "time_embedder.linear_2."),
    ];
    for (from, to) in replacements {
        if let Some(rest) = name.strip_prefix(from) {
            return Some(format!("{to}{rest}"));
        }
    }
    if name == "token_refiner.final_norm.weight" || name == "adaln_t_table" {
        return Some(name.to_string());
    }
    None
}

fn is_linear_name(name: &str, dims: &[usize]) -> bool {
    dims.len() == 2
        && name.ends_with(".weight")
        && name != "model.language_model.embed_tokens.weight"
        && name != "model.visual.pos_embed.weight"
}

fn map_dit_gguf(
    file: &GgufFile,
    tensors: &mut HashMap<String, GgufMappedTensor>,
    linears: &mut HashMap<String, H3QuantLinear>,
) -> Result<()> {
    for source in &file.tensors {
        if let Some((prefix, tail)) = dit_prefix_and_tail(&source.name) {
            if tail == "attn.qkv_proj.weight" {
                insert_qkv_split(tensors, linears, &prefix, source)?;
                continue;
            }
            let canonical = map_dit_tail(&prefix, tail);
            insert_direct(tensors, canonical.clone(), source, None)?;
            if is_linear_name(&canonical, &usize_dims(source)?) {
                insert_linear_for_mapped(tensors, linears, &canonical)?;
            }
            continue;
        }
        let Some(canonical) = top_level_dit_name(&source.name) else {
            // `rope.inv_freq` is generated analytically by our pipeline.
            continue;
        };
        insert_direct(tensors, canonical.clone(), source, None)?;
        if is_linear_name(&canonical, &usize_dims(source)?) {
            insert_linear_for_mapped(tensors, linears, &canonical)?;
        }
    }
    Ok(())
}

fn map_te_gguf(
    file: &GgufFile,
    tensors: &mut HashMap<String, GgufMappedTensor>,
    linears: &mut HashMap<String, H3QuantLinear>,
) -> Result<()> {
    for source in &file.tensors {
        let canonical = if let Some(rest) = source.name.strip_prefix("model.") {
            Some(format!("model.language_model.{rest}"))
        } else if let Some(rest) = source.name.strip_prefix("visual.") {
            Some(format!("model.visual.{rest}"))
        } else {
            None
        };
        let Some(canonical) = canonical else { continue };
        let dims_override = if canonical == "model.visual.patch_embed.proj.weight" {
            // sd.cpp stores the Conv3d under ggml's 4-D conv extents. Its
            // contiguous bytes are exactly the flattened 1536 -> 1152
            // row-major matrix used by our patch-row preprocessing.
            Some(vec![crate::h3_text::H3_VIS_PATCH_DIM, crate::h3_text::H3_VIS_HIDDEN])
        } else {
            None
        };
        insert_direct(tensors, canonical.clone(), source, dims_override)?;
        let dims = &tensors[&canonical].dims;
        if is_linear_name(&canonical, dims) {
            insert_linear_for_mapped(tensors, linears, &canonical)?;
        }
    }
    Ok(())
}

fn load_adaln_curve(
    file: &GgufFile,
    tensors: &HashMap<String, GgufMappedTensor>,
) -> Result<Option<H3AdalnCurve>> {
    let Some(mapped) = tensors.get("adaln_t_table") else {
        return Ok(None);
    };
    if mapped.ggml_type != GGML_TYPE_F32 || mapped.dims != [8, 1025] {
        return Err(model_error(format!(
            "h3 pruned GGUF adaln_t_table must be F32 [8,1025], got type {} {:?}",
            mapped.ggml_type, mapped.dims
        )));
    }
    let bytes = file
        .read_tensor_bytes(&mapped.source)
        .map_err(|err| model_error(format!("h3 adaln_t_table read: {err}")))?;
    if bytes.len() != 8 * 1025 * 4 {
        return Err(model_error(format!(
            "h3 adaln_t_table byte length {} != {}",
            bytes.len(),
            8 * 1025 * 4
        )));
    }
    let values = bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect();
    Ok(Some(H3AdalnCurve {
        dim: 8,
        grid: 1025,
        values,
    }))
}

fn require_dims(
    tensors: &HashMap<String, GgufMappedTensor>,
    name: &str,
    dims: &[usize],
) -> Result<()> {
    let tensor = tensors
        .get(name)
        .ok_or_else(|| model_error(format!("h3 quant checkpoint missing required tensor '{name}'")))?;
    if tensor.dims != dims {
        return Err(model_error(format!(
            "h3 quant tensor '{name}' shape {:?}, expected {:?}",
            tensor.dims, dims
        )));
    }
    Ok(())
}

fn require_linear(
    linears: &HashMap<String, H3QuantLinear>,
    name: &str,
    n: usize,
    k: usize,
) -> Result<()> {
    let linear = linears
        .get(name)
        .ok_or_else(|| model_error(format!("h3 quant checkpoint missing required linear '{name}'")))?;
    if linear.n != n || linear.k != k {
        return Err(model_error(format!(
            "h3 quant linear '{name}' is {}x{}, expected {n}x{k}",
            linear.n, linear.k
        )));
    }
    runtime_linear_type(linear.ggml_type, name).map(|_| ())
}

fn validate_component(
    component: H3QuantComponent,
    tensors: &HashMap<String, GgufMappedTensor>,
    linears: &HashMap<String, H3QuantLinear>,
    curve: Option<&H3AdalnCurve>,
) -> Result<()> {
    match component {
        H3QuantComponent::Dit => validate_dit(tensors, linears, curve),
        H3QuantComponent::TextEncoder => validate_te(tensors, linears),
    }
}

fn validate_dit(
    tensors: &HashMap<String, GgufMappedTensor>,
    linears: &HashMap<String, H3QuantLinear>,
    curve: Option<&H3AdalnCurve>,
) -> Result<()> {
    use crate::h3::*;
    const INNER: usize = H3_HEAD_COUNT * H3_HEAD_DIM;
    const ADALN: usize = H3_MODALITY_NUM * 6 * H3_HIDDEN_SIZE;
    require_dims(tensors, "proj_in.bias", &[H3_HIDDEN_SIZE])?;
    require_linear(linears, "proj_in.weight", H3_HIDDEN_SIZE, H3_VIDEO_PATCH_DIM)?;
    require_dims(tensors, "audio_proj_in.bias", &[H3_HIDDEN_SIZE])?;
    require_linear(
        linears,
        "audio_proj_in.weight",
        H3_HIDDEN_SIZE,
        H3_AUDIO_IN_CHANNELS,
    )?;
    require_dims(tensors, "context_embedder.bias", &[H3_HIDDEN_SIZE])?;
    require_linear(
        linears,
        "context_embedder.weight",
        H3_HIDDEN_SIZE,
        H3_TEXT_DIM,
    )?;
    require_dims(tensors, "norm_out.norm.weight", &[H3_HIDDEN_SIZE])?;
    require_dims(tensors, "norm_out.linear.bias", &[2 * H3_HIDDEN_SIZE])?;
    require_linear(linears, "norm_out.linear.weight", 2 * H3_HIDDEN_SIZE, 8)?;
    require_dims(tensors, "proj_out.bias", &[H3_VIDEO_PATCH_DIM])?;
    require_linear(linears, "proj_out.weight", H3_VIDEO_PATCH_DIM, H3_HIDDEN_SIZE)?;
    require_dims(tensors, "audio_proj_out.bias", &[H3_AUDIO_IN_CHANNELS])?;
    require_linear(
        linears,
        "audio_proj_out.weight",
        H3_AUDIO_IN_CHANNELS,
        H3_HIDDEN_SIZE,
    )?;

    if let Some(curve) = curve {
        if curve.dim != 8 || curve.grid != 1025 || curve.values.len() != 8 * 1025 {
            return Err(model_error("h3 invalid AdaLN curve after decode"));
        }
    } else {
        require_dims(tensors, "time_embedder.linear_1.bias", &[H3_TIME_EMBED_HIDDEN])?;
        require_linear(
            linears,
            "time_embedder.linear_1.weight",
            H3_TIME_EMBED_HIDDEN,
            H3_FREQ_DIM,
        )?;
        require_dims(tensors, "time_embedder.linear_2.bias", &[H3_TIME_EMBED_DIM])?;
        require_linear(
            linears,
            "time_embedder.linear_2.weight",
            H3_TIME_EMBED_DIM,
            H3_TIME_EMBED_HIDDEN,
        )?;
    }

    for layer in 0..H3_DEPTH {
        validate_dit_block(
            tensors,
            linears,
            &format!("transformer_blocks.{layer}"),
            true,
            ADALN,
            INNER,
        )?;
    }
    for layer in 0..H3_REFINER_DEPTH {
        validate_dit_block(
            tensors,
            linears,
            &format!("token_refiner.refiner_blocks.{layer}"),
            false,
            ADALN,
            INNER,
        )?;
    }
    require_dims(tensors, "token_refiner.final_norm.weight", &[H3_HIDDEN_SIZE])?;
    Ok(())
}

fn validate_dit_block(
    tensors: &HashMap<String, GgufMappedTensor>,
    linears: &HashMap<String, H3QuantLinear>,
    prefix: &str,
    adaln: bool,
    adaln_cols: usize,
    inner: usize,
) -> Result<()> {
    use crate::h3::{H3_FFN_DIM, H3_HEAD_DIM, H3_HIDDEN_SIZE};
    require_dims(tensors, &format!("{prefix}.norm1.weight"), &[H3_HIDDEN_SIZE])?;
    require_dims(tensors, &format!("{prefix}.norm2.weight"), &[H3_HIDDEN_SIZE])?;
    require_dims(tensors, &format!("{prefix}.attn.norm_q.weight"), &[H3_HEAD_DIM])?;
    require_dims(tensors, &format!("{prefix}.attn.norm_k.weight"), &[H3_HEAD_DIM])?;
    if adaln {
        require_dims(tensors, &format!("{prefix}.adaln_proj.linear.bias"), &[adaln_cols])?;
        require_linear(
            linears,
            &format!("{prefix}.adaln_proj.linear.weight"),
            adaln_cols,
            8,
        )?;
    }
    for part in ["to_q", "to_k", "to_v"] {
        require_linear(
            linears,
            &format!("{prefix}.attn.{part}.weight"),
            inner,
            H3_HIDDEN_SIZE,
        )?;
    }
    require_linear(
        linears,
        &format!("{prefix}.attn.to_out.0.weight"),
        H3_HIDDEN_SIZE,
        inner,
    )?;
    require_linear(
        linears,
        &format!("{prefix}.ff.net.0.proj.weight"),
        2 * H3_FFN_DIM,
        H3_HIDDEN_SIZE,
    )?;
    require_linear(
        linears,
        &format!("{prefix}.ff.net.2.weight"),
        H3_HIDDEN_SIZE,
        H3_FFN_DIM,
    )
}

fn validate_te(
    tensors: &HashMap<String, GgufMappedTensor>,
    linears: &HashMap<String, H3QuantLinear>,
) -> Result<()> {
    use crate::h3_text::*;
    require_dims(
        tensors,
        "model.language_model.embed_tokens.weight",
        &[H3_TE_HIDDEN, 151_936],
    )?;
    for layer in 0..H3_TE_LAYERS {
        let prefix = format!("model.language_model.layers.{layer}");
        require_dims(tensors, &format!("{prefix}.input_layernorm.weight"), &[H3_TE_HIDDEN])?;
        require_dims(
            tensors,
            &format!("{prefix}.post_attention_layernorm.weight"),
            &[H3_TE_HIDDEN],
        )?;
        require_dims(tensors, &format!("{prefix}.self_attn.q_norm.weight"), &[H3_TE_HEAD_DIM])?;
        require_dims(tensors, &format!("{prefix}.self_attn.k_norm.weight"), &[H3_TE_HEAD_DIM])?;
        require_linear(
            linears,
            &format!("{prefix}.self_attn.q_proj.weight"),
            H3_TE_Q_HEADS * H3_TE_HEAD_DIM,
            H3_TE_HIDDEN,
        )?;
        for part in ["k_proj", "v_proj"] {
            require_linear(
                linears,
                &format!("{prefix}.self_attn.{part}.weight"),
                H3_TE_KV_HEADS * H3_TE_HEAD_DIM,
                H3_TE_HIDDEN,
            )?;
        }
        require_linear(
            linears,
            &format!("{prefix}.self_attn.o_proj.weight"),
            H3_TE_HIDDEN,
            H3_TE_Q_HEADS * H3_TE_HEAD_DIM,
        )?;
        for part in ["up_proj", "gate_proj"] {
            require_linear(
                linears,
                &format!("{prefix}.mlp.{part}.weight"),
                H3_TE_FFN,
                H3_TE_HIDDEN,
            )?;
        }
        require_linear(
            linears,
            &format!("{prefix}.mlp.down_proj.weight"),
            H3_TE_HIDDEN,
            H3_TE_FFN,
        )?;
    }
    validate_vision_te(tensors, linears)
}

fn validate_vision_te(
    tensors: &HashMap<String, GgufMappedTensor>,
    linears: &HashMap<String, H3QuantLinear>,
) -> Result<()> {
    use crate::h3_text::*;
    let root = "model.visual";
    require_dims(tensors, &format!("{root}.patch_embed.proj.bias"), &[H3_VIS_HIDDEN])?;
    require_linear(
        linears,
        &format!("{root}.patch_embed.proj.weight"),
        H3_VIS_HIDDEN,
        H3_VIS_PATCH_DIM,
    )?;
    require_dims(
        tensors,
        &format!("{root}.pos_embed.weight"),
        &[H3_VIS_HIDDEN, H3_VIS_POS_SIDE * H3_VIS_POS_SIDE],
    )?;
    for block in 0..H3_VIS_DEPTH {
        let prefix = format!("{root}.blocks.{block}");
        for norm in ["norm1", "norm2"] {
            for suffix in ["weight", "bias"] {
                require_dims(
                    tensors,
                    &format!("{prefix}.{norm}.{suffix}"),
                    &[H3_VIS_HIDDEN],
                )?;
            }
        }
        require_dims(
            tensors,
            &format!("{prefix}.attn.qkv.bias"),
            &[3 * H3_VIS_HIDDEN],
        )?;
        require_linear(
            linears,
            &format!("{prefix}.attn.qkv.weight"),
            3 * H3_VIS_HIDDEN,
            H3_VIS_HIDDEN,
        )?;
        require_dims(
            tensors,
            &format!("{prefix}.attn.proj.bias"),
            &[H3_VIS_HIDDEN],
        )?;
        require_linear(
            linears,
            &format!("{prefix}.attn.proj.weight"),
            H3_VIS_HIDDEN,
            H3_VIS_HIDDEN,
        )?;
        require_dims(
            tensors,
            &format!("{prefix}.mlp.linear_fc1.bias"),
            &[H3_VIS_FFN],
        )?;
        require_linear(
            linears,
            &format!("{prefix}.mlp.linear_fc1.weight"),
            H3_VIS_FFN,
            H3_VIS_HIDDEN,
        )?;
        require_dims(
            tensors,
            &format!("{prefix}.mlp.linear_fc2.bias"),
            &[H3_VIS_HIDDEN],
        )?;
        require_linear(
            linears,
            &format!("{prefix}.mlp.linear_fc2.weight"),
            H3_VIS_HIDDEN,
            H3_VIS_FFN,
        )?;
    }
    let merge = H3_VIS_HIDDEN * H3_VIS_MERGE * H3_VIS_MERGE;
    validate_merger(tensors, linears, &format!("{root}.merger"), H3_VIS_HIDDEN, merge)?;
    for index in 0..H3_VIS_DEEPSTACK_BLOCKS.len() {
        validate_merger(
            tensors,
            linears,
            &format!("{root}.deepstack_merger_list.{index}"),
            merge,
            merge,
        )?;
    }
    Ok(())
}

fn validate_merger(
    tensors: &HashMap<String, GgufMappedTensor>,
    linears: &HashMap<String, H3QuantLinear>,
    prefix: &str,
    norm: usize,
    hidden: usize,
) -> Result<()> {
    let out = crate::h3_text::H3_VIS_OUT_HIDDEN;
    for suffix in ["weight", "bias"] {
        require_dims(tensors, &format!("{prefix}.norm.{suffix}"), &[norm])?;
    }
    require_dims(tensors, &format!("{prefix}.linear_fc1.bias"), &[hidden])?;
    require_linear(
        linears,
        &format!("{prefix}.linear_fc1.weight"),
        hidden,
        hidden,
    )?;
    require_dims(tensors, &format!("{prefix}.linear_fc2.bias"), &[out])?;
    require_linear(
        linears,
        &format!("{prefix}.linear_fc2.weight"),
        out,
        hidden,
    )
}

fn checked_usize(value: u64, context: &str) -> Result<usize> {
    usize::try_from(value)
        .map_err(|_| model_error(format!("h3 NVFP4 {context} does not fit usize")))
}

fn checked_nv_shape(shape: &[u64], name: &str) -> Result<Vec<usize>> {
    shape
        .iter()
        .map(|dim| checked_usize(*dim, &format!("tensor '{name}' dimension {dim}")))
        .collect()
}

fn checked_numel(shape: &[u64], name: &str) -> Result<u64> {
    shape.iter().try_fold(1u64, |product, dim| {
        product.checked_mul(*dim).ok_or_else(|| {
            model_error(format!("h3 NVFP4 tensor '{name}' element count overflow"))
        })
    })
}

/// Open the safetensors header without asking the generic MLX reader to
/// understand F8_E4M3. Validate every declared data span, not merely the
/// tensors the current graph happens to consume. Safetensors permits a few
/// trailing bytes, so the final tensor need only fit inside the file.
fn load_nv_safetensors(
    path: &Path,
) -> Result<(Flux2SafetensorsHeader, u64, u64, u64)> {
    if !path.is_file() {
        return Err(model_error(format!(
            "h3 NVFP4 safetensors is not a file: {}",
            path.display()
        )));
    }
    let file_size = std::fs::metadata(path)
        .map_err(|err| model_error(format!("h3 NVFP4 stat {}: {err}", path.display())))?
        .len();
    let mut raw = File::open(path)
        .map_err(|err| model_error(format!("h3 NVFP4 open {}: {err}", path.display())))?;
    let mut len_bytes = [0u8; 8];
    raw.read_exact(&mut len_bytes)
        .map_err(|err| model_error(format!("h3 NVFP4 read header length: {err}")))?;
    let header_len = u64::from_le_bytes(len_bytes);
    let data_offset = 8u64
        .checked_add(header_len)
        .ok_or_else(|| model_error("h3 NVFP4 data offset overflow"))?;
    if data_offset > file_size {
        return Err(model_error(format!(
            "h3 NVFP4 header ends at {data_offset}, beyond {} byte file {}",
            file_size,
            path.display()
        )));
    }
    let file = Flux2SafetensorsHeader::load(path)?;
    if file.tensors.is_empty() {
        return Err(model_error("h3 NVFP4 safetensors has no tensors"));
    }

    let mut spans = Vec::with_capacity(file.tensors.len());
    for (name, info) in &file.tensors {
        let dtype = NvDtype::parse(&info.dtype, name)?;
        let expected = checked_numel(&info.shape, name)?
            .checked_mul(dtype.byte_width() as u64)
            .ok_or_else(|| model_error(format!("h3 NVFP4 tensor '{name}' byte size overflow")))?;
        let (start, end) = info.data_offsets;
        let actual = end.checked_sub(start).ok_or_else(|| {
            model_error(format!("h3 NVFP4 tensor '{name}' has reversed offsets"))
        })?;
        if actual != expected {
            return Err(model_error(format!(
                "h3 NVFP4 tensor '{name}' byte span {actual} != {:?} {:?} expected {expected}",
                dtype, info.shape
            )));
        }
        let absolute_end = data_offset
            .checked_add(end)
            .ok_or_else(|| model_error(format!("h3 NVFP4 tensor '{name}' end overflow")))?;
        if absolute_end > file_size {
            return Err(model_error(format!(
                "h3 NVFP4 tensor '{name}' ends at {absolute_end}, beyond {file_size} byte file"
            )));
        }
        spans.push((start, end, name.as_str()));
    }
    spans.sort_unstable_by_key(|span| span.0);
    let mut cursor = 0u64;
    for (start, end, name) in spans {
        if start != cursor {
            return Err(model_error(format!(
                "h3 NVFP4 safetensors data is not contiguous before '{name}': {cursor}..{start}"
            )));
        }
        cursor = end;
    }
    Ok((file, data_offset, file_size, cursor))
}

fn nv_auxiliary_name(name: &str) -> bool {
    name.ends_with(".weight_scale")
        || name.ends_with(".weight_scale_2")
        || name.ends_with(".pre_quant_scale")
        || name.ends_with(".comfy_quant")
}

fn nv_canonical_name(component: H3QuantComponent, source: &str) -> Option<String> {
    match component {
        H3QuantComponent::Dit => {
            if let Some((prefix, tail)) = dit_prefix_and_tail(source) {
                Some(map_dit_tail(&prefix, tail))
            } else {
                top_level_dit_name(source)
            }
        }
        H3QuantComponent::TextEncoder => {
            if let Some(rest) = source.strip_prefix("model.") {
                Some(format!("model.language_model.{rest}"))
            } else {
                source
                    .strip_prefix("visual.")
                    .map(|rest| format!("model.visual.{rest}"))
            }
        }
    }
}

fn nv_canonical_dims(
    file: &Flux2SafetensorsHeader,
    source: &str,
    canonical: &str,
) -> Result<Vec<usize>> {
    let info = file.tensor(source).ok_or_else(|| {
        model_error(format!("h3 NVFP4 source tensor '{source}' disappeared"))
    })?;
    let shape = checked_nv_shape(&info.shape, source)?;
    match shape.as_slice() {
        [] => Ok(Vec::new()),
        [len] => Ok(vec![*len]),
        // Safetensors stores torch matrices [out,in]; canonical ggml dims
        // name the same contiguous bytes [k,n].
        [n, k] => Ok(vec![*k, *n]),
        [n, c, t, h, w]
            if canonical == "model.visual.patch_embed.proj.weight" =>
        {
            let k = c
                .checked_mul(*t)
                .and_then(|value| value.checked_mul(*h))
                .and_then(|value| value.checked_mul(*w))
                .ok_or_else(|| model_error("h3 NVFP4 vision patch dimension overflow"))?;
            Ok(vec![k, *n])
        }
        _ => Err(model_error(format!(
            "h3 NVFP4 tensor '{source}' has unsupported shape {:?} for canonical '{canonical}'",
            info.shape
        ))),
    }
}

fn insert_nv_tensor(
    tensors: &mut HashMap<String, NvMappedTensor>,
    canonical: String,
    mapped: NvMappedTensor,
) -> Result<()> {
    if tensors.insert(canonical.clone(), mapped).is_some() {
        return Err(model_error(format!(
            "h3 NVFP4 canonical tensor name collision: '{canonical}'"
        )));
    }
    Ok(())
}

fn insert_nv_linear(
    linears: &mut HashMap<String, H3QuantLinear>,
    canonical: &str,
    n: usize,
    k: usize,
    ggml_type: u32,
) -> Result<()> {
    let ggml_type = runtime_linear_type(ggml_type, canonical)?;
    if linears
        .insert(
            canonical.to_string(),
            H3QuantLinear {
                n,
                k,
                ggml_type,
            },
        )
        .is_some()
    {
        return Err(model_error(format!(
            "h3 NVFP4 canonical linear name collision: '{canonical}'"
        )));
    }
    Ok(())
}

fn insert_nv_direct(
    file: &Flux2SafetensorsHeader,
    tensors: &mut HashMap<String, NvMappedTensor>,
    linears: &mut HashMap<String, H3QuantLinear>,
    canonical: String,
    source: &str,
) -> Result<()> {
    let info = file
        .tensor(source)
        .ok_or_else(|| model_error(format!("h3 NVFP4 missing source '{source}'")))?;
    let dtype = NvDtype::parse(&info.dtype, source)?;
    // Any U8/F8/I8 mapped tensor must be handled by one of the compound
    // layouts below. Never reinterpret opaque quant bytes as a float tensor.
    let ggml_type = dtype.raw_ggml_type(source)?;
    let dims = nv_canonical_dims(file, source, &canonical)?;
    let byte_len = info.data_offsets.1 - info.data_offsets.0;
    insert_nv_tensor(
        tensors,
        canonical.clone(),
        NvMappedTensor {
            dims: dims.clone(),
            storage: NvMappedStorage::Direct {
                source: source.to_string(),
                dtype,
                byte_offset: 0,
                byte_len,
            },
            disk_bytes: byte_len,
        },
    )?;
    if is_linear_name(&canonical, &dims) {
        let [k, n] = dims.as_slice() else { unreachable!() };
        insert_nv_linear(linears, &canonical, *n, *k, ggml_type)?;
    }
    Ok(())
}

fn insert_nv_direct_qkv(
    file: &Flux2SafetensorsHeader,
    tensors: &mut HashMap<String, NvMappedTensor>,
    linears: &mut HashMap<String, H3QuantLinear>,
    prefix: &str,
    source: &str,
) -> Result<()> {
    let info = file
        .tensor(source)
        .ok_or_else(|| model_error(format!("h3 NVFP4 missing source '{source}'")))?;
    let dtype = NvDtype::parse(&info.dtype, source)?;
    let ggml_type = dtype.raw_ggml_type(source)?;
    let shape = checked_nv_shape(&info.shape, source)?;
    let [fused_n, k] = shape.as_slice() else {
        return Err(model_error(format!(
            "h3 NVFP4 fused QKV '{source}' must be rank 2, got {:?}",
            info.shape
        )));
    };
    if *fused_n % 3 != 0 {
        return Err(model_error(format!(
            "h3 NVFP4 fused QKV '{source}' rows {fused_n} not divisible by 3"
        )));
    }
    let n = *fused_n / 3;
    let row_bytes = k
        .checked_mul(dtype.byte_width())
        .ok_or_else(|| model_error(format!("h3 NVFP4 '{source}' row size overflow")))?;
    let source_bytes = info.data_offsets.1 - info.data_offsets.0;
    if source_bytes != (*fused_n * row_bytes) as u64 {
        return Err(model_error(format!(
            "h3 NVFP4 fused QKV '{source}' byte size mismatch"
        )));
    }
    for (part, suffix) in ["to_q", "to_k", "to_v"].into_iter().enumerate() {
        let canonical = format!("{prefix}.attn.{suffix}.weight");
        let byte_offset = part
            .checked_mul(n)
            .and_then(|rows| rows.checked_mul(row_bytes))
            .ok_or_else(|| model_error(format!("h3 NVFP4 '{canonical}' offset overflow")))?;
        let byte_len = n
            .checked_mul(row_bytes)
            .ok_or_else(|| model_error(format!("h3 NVFP4 '{canonical}' size overflow")))?;
        insert_nv_tensor(
            tensors,
            canonical.clone(),
            NvMappedTensor {
                dims: vec![*k, n],
                storage: NvMappedStorage::Direct {
                    source: source.to_string(),
                    dtype,
                    byte_offset: byte_offset as u64,
                    byte_len: byte_len as u64,
                },
                disk_bytes: byte_len as u64,
            },
        )?;
        insert_nv_linear(linears, &canonical, n, *k, ggml_type)?;
    }
    Ok(())
}

fn nv_quant_group<'a>(source: &'a str) -> Result<&'a str> {
    source.strip_suffix(".weight").ok_or_else(|| {
        model_error(format!(
            "h3 NVFP4 quant tensor '{source}' does not end in .weight"
        ))
    })
}

fn nv_require_dtype_shape(
    file: &Flux2SafetensorsHeader,
    name: &str,
    dtype: NvDtype,
    shape: &[usize],
) -> Result<()> {
    let info = file
        .tensor(name)
        .ok_or_else(|| model_error(format!("h3 NVFP4 missing companion '{name}'")))?;
    let actual_dtype = NvDtype::parse(&info.dtype, name)?;
    let actual_shape = checked_nv_shape(&info.shape, name)?;
    if actual_dtype != dtype || actual_shape != shape {
        return Err(model_error(format!(
            "h3 NVFP4 companion '{name}' is {actual_dtype:?} {:?}, expected {dtype:?} {shape:?}",
            info.shape
        )));
    }
    Ok(())
}

fn nv_quant_layout(
    file: &Flux2SafetensorsHeader,
    source: &str,
) -> Result<(String, String, Option<String>, usize, usize)> {
    let base = nv_quant_group(source)?;
    let info = file
        .tensor(source)
        .ok_or_else(|| model_error(format!("h3 NVFP4 missing source '{source}'")))?;
    if NvDtype::parse(&info.dtype, source)? != NvDtype::U8 {
        return Err(model_error(format!(
            "h3 NVFP4 quant weight '{source}' must be U8"
        )));
    }
    let shape = checked_nv_shape(&info.shape, source)?;
    let [rows, packed_cols] = shape.as_slice() else {
        return Err(model_error(format!(
            "h3 NVFP4 quant weight '{source}' must be rank 2"
        )));
    };
    let k = packed_cols
        .checked_mul(2)
        .ok_or_else(|| model_error(format!("h3 NVFP4 '{source}' k overflow")))?;
    if k == 0 || k % 16 != 0 {
        return Err(model_error(format!(
            "h3 NVFP4 quant weight '{source}' input width {k} is not divisible by 16"
        )));
    }
    // Comfy-Kitchen serializes block scales in the cuBLAS [128 rows, 4
    // scale-columns] tiled layout. The pinned H3 matrices require no shape
    // padding; rejecting unaligned look-alikes here keeps admission strict
    // and makes every later unswizzle shape-preserving.
    if *rows == 0 || *rows % 128 != 0 || (k / 16) % 4 != 0 {
        return Err(model_error(format!(
            "h3 NVFP4 quant weight '{source}' shape [{rows}, {k}] is not aligned to 128 output rows and 64 input columns"
        )));
    }
    let scale = format!("{base}.weight_scale");
    let scale2 = format!("{base}.weight_scale_2");
    let comfy = format!("{base}.comfy_quant");
    nv_require_dtype_shape(file, &scale, NvDtype::F8E4M3, &[*rows, k / 16])?;
    nv_require_dtype_shape(file, &scale2, NvDtype::F32, &[])?;
    let comfy_info = file
        .tensor(&comfy)
        .ok_or_else(|| model_error(format!("h3 NVFP4 missing companion '{comfy}'")))?;
    let comfy_shape = checked_nv_shape(&comfy_info.shape, &comfy)?;
    if NvDtype::parse(&comfy_info.dtype, &comfy)? != NvDtype::U8
        || !matches!(comfy_shape.as_slice(), [1..=256])
    {
        return Err(model_error(format!(
            "h3 NVFP4 companion '{comfy}' must be a short U8 metadata vector"
        )));
    }
    let pre_name = format!("{base}.pre_quant_scale");
    let pre_scale = if file.tensor(&pre_name).is_some() {
        nv_require_dtype_shape(file, &pre_name, NvDtype::BF16, &[k])?;
        Some(pre_name)
    } else {
        None
    };
    Ok((scale, scale2, pre_scale, *rows, k))
}

#[allow(clippy::too_many_arguments)]
fn insert_nv_quant_rows(
    file: &Flux2SafetensorsHeader,
    tensors: &mut HashMap<String, NvMappedTensor>,
    linears: &mut HashMap<String, H3QuantLinear>,
    canonical: String,
    source: &str,
    row_start: usize,
    rows: usize,
) -> Result<()> {
    let (scale, scale2, pre_scale, source_rows, k) = nv_quant_layout(file, source)?;
    if rows == 0 || row_start.checked_add(rows).is_none_or(|end| end > source_rows) {
        return Err(model_error(format!(
            "h3 NVFP4 quant '{source}' invalid row slice {row_start}..{} of {source_rows}",
            row_start.saturating_add(rows)
        )));
    }
    let ggml_type = if pre_scale.is_some() {
        GGML_TYPE_H3_NVFP4_PAIRS_PRESCALE
    } else {
        GGML_TYPE_H3_NVFP4_PAIRS
    };
    let row_disk = k / 2 + k / 16;
    let disk_bytes = rows
        .checked_mul(row_disk)
        .and_then(|bytes| bytes.checked_add(4))
        .and_then(|bytes| {
            pre_scale
                .as_ref()
                .map_or(Some(bytes), |_| bytes.checked_add(2 * k))
        })
        .ok_or_else(|| model_error(format!("h3 NVFP4 '{canonical}' disk size overflow")))?;
    insert_nv_tensor(
        tensors,
        canonical.clone(),
        NvMappedTensor {
            dims: vec![k, rows],
            storage: NvMappedStorage::Quant {
                weight: source.to_string(),
                scale,
                scale2,
                pre_scale,
                row_start,
                rows,
                k,
            },
            disk_bytes: disk_bytes as u64,
        },
    )?;
    insert_nv_linear(linears, &canonical, rows, k, ggml_type)
}

fn insert_nv_quant(
    file: &Flux2SafetensorsHeader,
    tensors: &mut HashMap<String, NvMappedTensor>,
    linears: &mut HashMap<String, H3QuantLinear>,
    canonical: String,
    source: &str,
) -> Result<()> {
    let (_, _, _, rows, _) = nv_quant_layout(file, source)?;
    insert_nv_quant_rows(file, tensors, linears, canonical, source, 0, rows)
}

fn insert_nv_quant_qkv(
    file: &Flux2SafetensorsHeader,
    tensors: &mut HashMap<String, NvMappedTensor>,
    linears: &mut HashMap<String, H3QuantLinear>,
    prefix: &str,
    source: &str,
) -> Result<()> {
    let (_, _, _, fused_rows, _) = nv_quant_layout(file, source)?;
    if fused_rows % 3 != 0 {
        return Err(model_error(format!(
            "h3 NVFP4 fused quant QKV '{source}' rows {fused_rows} not divisible by 3"
        )));
    }
    let rows = fused_rows / 3;
    for (part, suffix) in ["to_q", "to_k", "to_v"].into_iter().enumerate() {
        insert_nv_quant_rows(
            file,
            tensors,
            linears,
            format!("{prefix}.attn.{suffix}.weight"),
            source,
            part * rows,
            rows,
        )?;
    }
    Ok(())
}

fn insert_nv_i8_embedding(
    file: &Flux2SafetensorsHeader,
    tensors: &mut HashMap<String, NvMappedTensor>,
    canonical: String,
    source: &str,
) -> Result<()> {
    let info = file
        .tensor(source)
        .ok_or_else(|| model_error(format!("h3 NVFP4 missing source '{source}'")))?;
    if NvDtype::parse(&info.dtype, source)? != NvDtype::I8 {
        return Err(model_error(format!(
            "h3 NVFP4 embedding '{source}' must be I8"
        )));
    }
    let shape = checked_nv_shape(&info.shape, source)?;
    let [rows, k] = shape.as_slice() else {
        return Err(model_error(format!(
            "h3 NVFP4 embedding '{source}' must be rank 2"
        )));
    };
    let base = source
        .strip_suffix(".weight")
        .ok_or_else(|| model_error("h3 NVFP4 embedding lacks .weight suffix"))?;
    let scale = format!("{base}.weight_scale");
    nv_require_dtype_shape(file, &scale, NvDtype::F32, &[*rows, 1])?;
    let comfy = format!("{base}.comfy_quant");
    nv_require_dtype_shape(file, &comfy, NvDtype::U8, &[29])?;
    let disk_bytes = rows
        .checked_mul(*k)
        .and_then(|bytes| bytes.checked_add(4 * rows))
        .ok_or_else(|| model_error("h3 NVFP4 embedding disk size overflow"))?;
    insert_nv_tensor(
        tensors,
        canonical,
        NvMappedTensor {
            dims: vec![*k, *rows],
            storage: NvMappedStorage::Int8Rows {
                weight: source.to_string(),
                scale,
                rows: *rows,
                k: *k,
            },
            disk_bytes: disk_bytes as u64,
        },
    )
}

fn map_nvfp4(
    file: &Flux2SafetensorsHeader,
    component: H3QuantComponent,
) -> Result<(
    HashMap<String, NvMappedTensor>,
    HashMap<String, H3QuantLinear>,
)> {
    let mut tensors = HashMap::new();
    let mut linears = HashMap::new();
    let mut sources: Vec<&str> = file.tensors.keys().map(String::as_str).collect();
    sources.sort_unstable();
    for source in sources {
        if nv_auxiliary_name(source) {
            continue;
        }
        let Some(canonical) = nv_canonical_name(component, source) else {
            // Analytic rope tables and opaque non-weight metadata are not
            // part of the graph inventory.
            continue;
        };
        if component == H3QuantComponent::TextEncoder && source == "model.embed_tokens.weight" {
            insert_nv_i8_embedding(file, &mut tensors, canonical, source)?;
            continue;
        }
        if component == H3QuantComponent::Dit && source.ends_with(".attn.qkv_proj.weight") {
            let (prefix, tail) = dit_prefix_and_tail(source).ok_or_else(|| {
                model_error(format!("h3 NVFP4 cannot parse fused QKV '{source}'"))
            })?;
            if tail != "attn.qkv_proj.weight" {
                return Err(model_error(format!(
                    "h3 NVFP4 unexpected QKV spelling '{source}'"
                )));
            }
            let dtype = NvDtype::parse(&file.tensor(source).unwrap().dtype, source)?;
            if dtype == NvDtype::U8 {
                insert_nv_quant_qkv(file, &mut tensors, &mut linears, &prefix, source)?;
            } else {
                insert_nv_direct_qkv(file, &mut tensors, &mut linears, &prefix, source)?;
            }
            continue;
        }
        let dtype = NvDtype::parse(&file.tensor(source).unwrap().dtype, source)?;
        if dtype == NvDtype::U8 && source.ends_with(".weight") {
            insert_nv_quant(file, &mut tensors, &mut linears, canonical, source)?;
        } else {
            insert_nv_direct(file, &mut tensors, &mut linears, canonical, source)?;
        }
    }
    Ok((tensors, linears))
}

fn nv_validation_map(
    tensors: &HashMap<String, NvMappedTensor>,
    linears: &HashMap<String, H3QuantLinear>,
) -> HashMap<String, GgufMappedTensor> {
    tensors
        .iter()
        .map(|(name, tensor)| {
            let ggml_type = linears
                .get(name)
                .map(|linear| linear.ggml_type)
                .unwrap_or(GGML_TYPE_F32);
            (
                name.clone(),
                GgufMappedTensor {
                    source: name.clone(),
                    dims: tensor.dims.clone(),
                    ggml_type,
                    byte_offset: 0,
                    byte_len: tensor.disk_bytes,
                },
            )
        })
        .collect()
}

fn decode_nv_float(bytes: &[u8], dtype: NvDtype, name: &str) -> Result<Vec<f32>> {
    if bytes.len() % dtype.byte_width() != 0 {
        return Err(model_error(format!(
            "h3 NVFP4 tensor '{name}' byte length {} is not aligned for {dtype:?}",
            bytes.len()
        )));
    }
    match dtype {
        NvDtype::F32 => Ok(bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect()),
        NvDtype::F16 => Ok(bytes
            .chunks_exact(2)
            .map(|chunk| crate::h3::f16_word_to_f32(u16::from_le_bytes([chunk[0], chunk[1]])))
            .collect()),
        NvDtype::BF16 => Ok(bytes
            .chunks_exact(2)
            .map(|chunk| {
                f32::from_bits((u16::from_le_bytes([chunk[0], chunk[1]]) as u32) << 16)
            })
            .collect()),
        other => Err(model_error(format!(
            "h3 NVFP4 tensor '{name}' cannot host-decode {other:?} as raw floats"
        ))),
    }
}

/// Convert Comfy-Kitchen's cuBLAS NVFP4 scale layout back to the logical
/// `[rows, cols / 16]` row-major matrix expected by our packed H3 kernels.
///
/// Comfy-Kitchen's `to_blocked` tiles rows by 128 and scale columns by 4,
/// then stores each tile as `[row % 32][row / 32][col % 4]`. Safetensors
/// retains the logical rank-2 shape, so treating these bytes as row-major
/// silently associates almost every block scale with the wrong weight row.
fn unswizzle_nvfp4_scales(blocked: &[u8], rows: usize, cols: usize) -> Result<Vec<u8>> {
    if rows == 0 || rows % 128 != 0 || cols == 0 || cols % 4 != 0 {
        return Err(model_error(format!(
            "h3 NVFP4 blocked scale matrix [{rows}, {cols}] is not aligned to [128, 4]"
        )));
    }
    let expected = rows
        .checked_mul(cols)
        .ok_or_else(|| model_error("h3 NVFP4 blocked scale matrix size overflow"))?;
    if blocked.len() != expected {
        return Err(model_error(format!(
            "h3 NVFP4 blocked scale payload has {} bytes, expected {expected} for [{rows}, {cols}]",
            blocked.len()
        )));
    }

    let column_blocks = cols / 4;
    let mut row_major = vec![0u8; expected];
    for row in 0..rows {
        let row_block = row / 128;
        let row_in_block = row % 128;
        let row_group = row_in_block / 32;
        let row_lane = row_in_block % 32;
        for col in 0..cols {
            let column_block = col / 4;
            let column_lane = col % 4;
            let blocked_index = ((((row_block * column_blocks + column_block) * 32
                + row_lane)
                * 4
                + row_group)
                * 4)
                + column_lane;
            row_major[row * cols + col] = blocked[blocked_index];
        }
    }
    Ok(row_major)
}

/// The official Comfy-Kitchen serializer uses `hi_first=true`: the even
/// input column is in the high nibble. Our internal H3 pairs ABI deliberately
/// uses low-nibble-first, so normalize once while staging a linear.
fn normalize_nvfp4_weight_nibbles(bytes: &mut [u8]) {
    for byte in bytes {
        *byte = (*byte << 4) | (*byte >> 4);
    }
}

impl H3Nvfp4Weights {
    pub fn load(path: &Path, component: H3QuantComponent) -> Result<Self> {
        let (file, data_offset, file_size, total_disk_bytes) = load_nv_safetensors(path)?;
        let (tensors, linears) = map_nvfp4(&file, component)?;
        let mut weights = Self {
            path: path.to_path_buf(),
            imp: Nvfp4Imp {
                file,
                data_offset,
                file_size,
                tensors,
                linears,
                total_disk_bytes,
                adaln_curve: None,
            },
        };
        weights.validate_comfy_metadata()?;
        if component == H3QuantComponent::Dit && weights.has_tensor("adaln_t_table") {
            let values = weights.tensor_f32("adaln_t_table")?;
            if values.len() != 1025 * 8 || values.iter().any(|value| !value.is_finite()) {
                return Err(model_error(
                    "h3 NVFP4 adaln_t_table must contain 1025x8 finite F32 values",
                ));
            }
            weights.imp.adaln_curve = Some(H3AdalnCurve {
                dim: 8,
                grid: 1025,
                values,
            });
        }
        let validation = nv_validation_map(&weights.imp.tensors, &weights.imp.linears);
        validate_component(
            component,
            &validation,
            &weights.imp.linears,
            weights.imp.adaln_curve.as_ref(),
        )?;
        Ok(weights)
    }

    pub fn has_tensor(&self, name: &str) -> bool {
        self.imp.tensors.contains_key(name)
    }

    pub fn tensor_f32(&self, name: &str) -> Result<Vec<f32>> {
        let mapped = self.mapped(name)?;
        match &mapped.storage {
            NvMappedStorage::Direct {
                source,
                dtype,
                byte_offset,
                byte_len,
            } => decode_nv_float(
                &self.read_source_range(source, *byte_offset, *byte_len, name)?,
                *dtype,
                name,
            ),
            NvMappedStorage::Quant { .. } => Err(model_error(format!(
                "h3 NVFP4 tensor '{name}' is a compound quantized linear, not a raw host tensor"
            ))),
            NvMappedStorage::Int8Rows { .. } => Err(model_error(format!(
                "h3 NVFP4 tensor '{name}' is an I8 row-scaled embedding; request individual rows"
            ))),
        }
    }

    pub fn tensor_row_f32(&self, name: &str, row: u64) -> Result<Vec<f32>> {
        let mapped = self.mapped(name)?;
        let row = checked_usize(row, &format!("tensor '{name}' row"))?;
        match &mapped.storage {
            NvMappedStorage::Direct {
                source,
                dtype,
                byte_offset,
                byte_len,
            } => {
                let (k, n) = nv_mapped_kn(mapped, name)?;
                if row >= n {
                    return Err(model_error(format!(
                        "h3 NVFP4 tensor '{name}' row {row} out of range 0..{n}"
                    )));
                }
                let row_bytes = k
                    .checked_mul(dtype.byte_width())
                    .ok_or_else(|| model_error(format!("h3 NVFP4 '{name}' row size overflow")))?;
                if *byte_len != (n * row_bytes) as u64 {
                    return Err(model_error(format!(
                        "h3 NVFP4 tensor '{name}' direct slice is not row-aligned"
                    )));
                }
                let relative = byte_offset
                    .checked_add((row * row_bytes) as u64)
                    .ok_or_else(|| model_error(format!("h3 NVFP4 '{name}' row offset overflow")))?;
                decode_nv_float(
                    &self.read_source_range(source, relative, row_bytes as u64, name)?,
                    *dtype,
                    name,
                )
            }
            NvMappedStorage::Int8Rows {
                weight,
                scale,
                rows,
                k,
            } => {
                if row >= *rows {
                    return Err(model_error(format!(
                        "h3 NVFP4 embedding row {row} out of range 0..{rows}"
                    )));
                }
                let bytes = self.read_source_range(weight, (row * k) as u64, *k as u64, name)?;
                let scale_bytes = self.read_source_range(scale, (row * 4) as u64, 4, name)?;
                let scale = f32::from_le_bytes(scale_bytes.try_into().unwrap());
                if !scale.is_finite() || scale <= 0.0 {
                    return Err(model_error(format!(
                        "h3 NVFP4 embedding row {row} has invalid scale {scale}"
                    )));
                }
                Ok(bytes
                    .into_iter()
                    .map(|byte| (byte as i8) as f32 * scale)
                    .collect())
            }
            NvMappedStorage::Quant {
                weight,
                scale,
                scale2,
                pre_scale,
                row_start,
                rows,
                k,
            } => {
                if row >= *rows {
                    return Err(model_error(format!(
                        "h3 NVFP4 tensor '{name}' row {row} out of range 0..{rows}"
                    )));
                }
                let source_row = row_start + row;
                let mut weight_row = self.read_source_range(
                    weight,
                    (source_row * (*k / 2)) as u64,
                    (*k / 2) as u64,
                    name,
                )?;
                normalize_nvfp4_weight_nibbles(&mut weight_row);
                let scale_row = self.read_quant_scale_rows(scale, source_row, 1, *k, name)?;
                validate_nvfp4_scale_bytes(&scale_row, name)?;
                let scale2 = self.read_scale2(scale2, name)?;
                let pre = match pre_scale {
                    Some(pre) => {
                        let values = decode_nv_float(
                            &self.read_source_range(pre, 0, (2 * *k) as u64, name)?,
                            NvDtype::BF16,
                            pre,
                        )?;
                        validate_pre_scale(&values, name)?;
                        Some(values)
                    }
                    None => None,
                };
                let mut out = vec![0.0f32; *k];
                dequantize_nvfp4_pairs_row(
                    &weight_row,
                    &scale_row,
                    scale2,
                    pre.as_deref(),
                    &mut out,
                );
                Ok(out)
            }
        }
    }

    pub fn tensor_disk_bytes(&self, name: &str) -> Result<u64> {
        Ok(self.mapped(name)?.disk_bytes)
    }

    pub fn total_disk_bytes(&self) -> u64 {
        self.imp.total_disk_bytes
    }

    pub fn tensor_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.imp.tensors.keys().cloned().collect();
        names.sort();
        names
    }

    pub fn linear(&self, name: &str) -> Option<&H3QuantLinear> {
        self.imp.linears.get(name)
    }

    pub fn linear_payload(&self, name: &str) -> Result<Vec<u8>> {
        if !self.imp.linears.contains_key(name) {
            return Err(model_error(format!(
                "h3 NVFP4 tensor '{name}' is not a mapped linear weight"
            )));
        }
        let mapped = self.mapped(name)?;
        match &mapped.storage {
            NvMappedStorage::Direct {
                source,
                dtype,
                byte_offset,
                byte_len,
            } => {
                let bytes = self.read_source_range(source, *byte_offset, *byte_len, name)?;
                if *dtype == NvDtype::F32 {
                    f32_payload_to_bf16(&bytes, name)
                } else {
                    Ok(bytes)
                }
            }
            NvMappedStorage::Quant {
                weight,
                scale,
                scale2,
                pre_scale,
                row_start,
                rows,
                k,
            } => {
                let weight_row_bytes = *k / 2;
                let mut weight_bytes = self.read_source_range(
                    weight,
                    (*row_start * weight_row_bytes) as u64,
                    (*rows * weight_row_bytes) as u64,
                    name,
                )?;
                normalize_nvfp4_weight_nibbles(&mut weight_bytes);
                let scale_bytes =
                    self.read_quant_scale_rows(scale, *row_start, *rows, *k, name)?;
                validate_nvfp4_scale_bytes(&scale_bytes, name)?;
                let scale2 = self.read_scale2(scale2, name)?;
                let pre_scale_bytes = match pre_scale {
                    Some(pre) => {
                        let bytes = self.read_source_range(pre, 0, (2 * *k) as u64, name)?;
                        let values = decode_nv_float(&bytes, NvDtype::BF16, pre)?;
                        validate_pre_scale(&values, name)?;
                        Some(bytes)
                    }
                    None => None,
                };
                h3_nvfp4_pairs_pack(
                    *rows,
                    *k,
                    scale2,
                    &scale_bytes,
                    &weight_bytes,
                    pre_scale_bytes.as_deref(),
                )
                .ok_or_else(|| {
                    model_error(format!(
                        "h3 NVFP4 tensor '{name}' failed validated pairs packing"
                    ))
                })
            }
            NvMappedStorage::Int8Rows { .. } => Err(model_error(format!(
                "h3 NVFP4 embedding '{name}' is not a linear payload"
            ))),
        }
    }

    pub fn adaln_curve(&self) -> Option<&H3AdalnCurve> {
        self.imp.adaln_curve.as_ref()
    }

    fn mapped(&self, name: &str) -> Result<&NvMappedTensor> {
        self.imp.tensors.get(name).ok_or_else(|| {
            model_error(format!(
                "h3 NVFP4 tensor '{name}' not found in {}",
                self.path.display()
            ))
        })
    }

    fn read_source_range(
        &self,
        source: &str,
        relative: u64,
        len: u64,
        canonical: &str,
    ) -> Result<Vec<u8>> {
        let info = self.imp.file.tensor(source).ok_or_else(|| {
            model_error(format!(
                "h3 NVFP4 canonical tensor '{canonical}' lost source '{source}'"
            ))
        })?;
        let source_len = info.data_offsets.1 - info.data_offsets.0;
        let relative_end = relative
            .checked_add(len)
            .ok_or_else(|| model_error(format!("h3 NVFP4 '{canonical}' range overflow")))?;
        if relative_end > source_len {
            return Err(model_error(format!(
                "h3 NVFP4 '{canonical}' range {relative}..{relative_end} exceeds source '{source}' length {source_len}"
            )));
        }
        let start = self
            .imp
            .data_offset
            .checked_add(info.data_offsets.0)
            .and_then(|value| value.checked_add(relative))
            .ok_or_else(|| model_error(format!("h3 NVFP4 '{canonical}' file offset overflow")))?;
        let end = start
            .checked_add(len)
            .ok_or_else(|| model_error(format!("h3 NVFP4 '{canonical}' file end overflow")))?;
        if end > self.imp.file_size {
            return Err(model_error(format!(
                "h3 NVFP4 '{canonical}' ends beyond file bounds"
            )));
        }
        let len = checked_usize(len, &format!("tensor '{canonical}' read length"))?;
        let mut file = File::open(&self.path).map_err(|err| {
            model_error(format!("h3 NVFP4 open {}: {err}", self.path.display()))
        })?;
        file.seek(SeekFrom::Start(start)).map_err(|err| {
            model_error(format!("h3 NVFP4 seek '{canonical}' at {start}: {err}"))
        })?;
        let mut bytes = vec![0u8; len];
        file.read_exact(&mut bytes).map_err(|err| {
            model_error(format!("h3 NVFP4 read '{canonical}' ({len} bytes): {err}"))
        })?;
        Ok(bytes)
    }

    fn read_scale2(&self, source: &str, canonical: &str) -> Result<f32> {
        let bytes = self.read_source_range(source, 0, 4, canonical)?;
        let value = f32::from_le_bytes(bytes.try_into().unwrap());
        if !value.is_finite() || value <= 0.0 {
            return Err(model_error(format!(
                "h3 NVFP4 tensor '{canonical}' has invalid global scale {value}"
            )));
        }
        Ok(value)
    }

    fn read_quant_scale_rows(
        &self,
        source: &str,
        row_start: usize,
        rows: usize,
        k: usize,
        canonical: &str,
    ) -> Result<Vec<u8>> {
        let info = self.imp.file.tensor(source).ok_or_else(|| {
            model_error(format!(
                "h3 NVFP4 canonical tensor '{canonical}' lost scale source '{source}'"
            ))
        })?;
        if NvDtype::parse(&info.dtype, source)? != NvDtype::F8E4M3 {
            return Err(model_error(format!(
                "h3 NVFP4 scale source '{source}' is not F8_E4M3"
            )));
        }
        let shape = checked_nv_shape(&info.shape, source)?;
        let [source_rows, scale_cols] = shape.as_slice() else {
            return Err(model_error(format!(
                "h3 NVFP4 scale source '{source}' must be rank 2"
            )));
        };
        let expected_cols = k / 16;
        if k == 0 || k % 16 != 0 || *scale_cols != expected_cols {
            return Err(model_error(format!(
                "h3 NVFP4 scale source '{source}' has {} columns, expected {expected_cols} for k={k}",
                scale_cols
            )));
        }
        let row_end = row_start
            .checked_add(rows)
            .ok_or_else(|| model_error(format!("h3 NVFP4 '{canonical}' scale row overflow")))?;
        if rows == 0 || row_end > *source_rows {
            return Err(model_error(format!(
                "h3 NVFP4 '{canonical}' scale rows {row_start}..{row_end} exceed source '{source}' rows {source_rows}"
            )));
        }
        let byte_len = source_rows
            .checked_mul(*scale_cols)
            .ok_or_else(|| model_error(format!("h3 NVFP4 '{canonical}' scale size overflow")))?;
        let blocked = self.read_source_range(source, 0, byte_len as u64, canonical)?;
        let row_major = unswizzle_nvfp4_scales(&blocked, *source_rows, *scale_cols)?;
        let start = row_start * *scale_cols;
        let end = row_end * *scale_cols;
        Ok(row_major[start..end].to_vec())
    }

    fn validate_comfy_metadata(&self) -> Result<()> {
        const NVFP4: &[u8] = br#"{"format": "nvfp4"}"#;
        const NVFP4_FULL: &[u8] =
            br#"{"format": "nvfp4", "full_precision_matrix_mult": true}"#;
        const INT8: &[u8] = br#"{"format": "int8_tensorwise"}"#;
        let mut checked = HashSet::new();
        for mapped in self.imp.tensors.values() {
            let (source, expected_int8) = match &mapped.storage {
                NvMappedStorage::Quant { weight, .. } => (weight, false),
                NvMappedStorage::Int8Rows { weight, .. } => (weight, true),
                NvMappedStorage::Direct { .. } => continue,
            };
            let base = source.strip_suffix(".weight").ok_or_else(|| {
                model_error(format!("h3 NVFP4 source '{source}' lacks .weight suffix"))
            })?;
            let metadata = format!("{base}.comfy_quant");
            if !checked.insert(metadata.clone()) {
                continue;
            }
            let info = self.imp.file.tensor(&metadata).ok_or_else(|| {
                model_error(format!("h3 NVFP4 missing metadata '{metadata}'"))
            })?;
            let bytes = self.read_source_range(
                &metadata,
                0,
                info.data_offsets.1 - info.data_offsets.0,
                &metadata,
            )?;
            let valid = if expected_int8 {
                bytes == INT8
            } else {
                bytes == NVFP4 || bytes == NVFP4_FULL
            };
            if !valid {
                return Err(model_error(format!(
                    "h3 NVFP4 metadata '{metadata}' is not the pinned {} layout: {}",
                    if expected_int8 { "int8_tensorwise" } else { "nvfp4" },
                    String::from_utf8_lossy(&bytes)
                )));
            }
        }
        Ok(())
    }
}

fn validate_nvfp4_scale_bytes(bytes: &[u8], name: &str) -> Result<()> {
    if let Some((index, byte)) = bytes
        .iter()
        .copied()
        .enumerate()
        .find(|(_, byte)| (*byte & 0x80) != 0 || *byte == 0x7f)
    {
        let reason = if byte & 0x80 != 0 {
            "negative"
        } else {
            "NaN"
        };
        return Err(model_error(format!(
            "h3 NVFP4 tensor '{name}' has an E4M3 {reason} scale byte 0x{byte:02x} at {index}"
        )));
    }
    Ok(())
}

fn validate_pre_scale(values: &[f32], name: &str) -> Result<()> {
    if let Some((index, value)) = values
        .iter()
        .copied()
        .enumerate()
        .find(|(_, value)| !value.is_finite() || *value <= 0.0)
    {
        return Err(model_error(format!(
            "h3 NVFP4 tensor '{name}' has invalid AWQ pre-scale {value} at {index}"
        )));
    }
    Ok(())
}

fn nv_mapped_kn(mapped: &NvMappedTensor, name: &str) -> Result<(usize, usize)> {
    match mapped.dims.as_slice() {
        [k] => Ok((*k, 1)),
        [k, n] => Ok((*k, *n)),
        dims => Err(model_error(format!(
            "h3 NVFP4 tensor '{name}' rank {} is not row-decodable",
            dims.len()
        ))),
    }
}

/// Canonical spelling for the fp16 video-VAE repack's LDM-style encoder
/// names. Decoder/quant-conv names already match and return `None`.
pub fn video_vae_repack_canonical_name(file_name: &str) -> Option<String> {
    let rest = file_name.strip_prefix("encoder.down.")?;
    let (level, rest) = rest.split_once('.')?;
    if level.parse::<usize>().is_err() {
        return None;
    }
    if let Some(rest) = rest.strip_prefix("block.") {
        let (block, tail) = rest.split_once('.')?;
        if block.parse::<usize>().is_err() {
            return None;
        }
        let tail = tail
            .strip_prefix("nin_shortcut.")
            .map(|suffix| format!("conv_shortcut.{suffix}"))
            .unwrap_or_else(|| tail.to_string());
        return Some(format!(
            "encoder.down_blocks.{level}.resnets.{block}.{tail}"
        ));
    }
    rest.strip_prefix("downsample.conv.").map(|suffix| {
        format!("encoder.down_blocks.{level}.downsamplers.0.conv.{suffix}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nv_info(dtype: &str, shape: &[u64]) -> crate::flux2::Flux2TensorInfo {
        let bytes_per = match dtype {
            "F32" => 4,
            "F16" | "BF16" => 2,
            _ => 1,
        };
        let len = shape.iter().product::<u64>().max(1) * bytes_per;
        crate::flux2::Flux2TensorInfo {
            dtype: dtype.to_string(),
            shape: shape.to_vec(),
            data_offsets: (0, len),
        }
    }

    #[test]
    fn adaln_curve_interpolates_and_guards_invalid_shapes() {
        let curve = H3AdalnCurve {
            dim: 2,
            grid: 3,
            values: vec![0.0, 10.0, 2.0, 12.0, 4.0, 14.0],
        };
        assert_eq!(curve.temb(0.25), vec![1.0, 11.0]);
        assert_eq!(curve.temb(1.0), vec![4.0, 14.0]);
        assert!(H3AdalnCurve {
            dim: 2,
            grid: 1,
            values: vec![0.0, 1.0]
        }
        .temb(0.5)
        .is_empty());
    }

    #[test]
    fn dit_name_map_is_canonical_and_does_not_map_rope() {
        assert_eq!(
            map_dit_tail("transformer_blocks.7", "attn.out_proj.weight"),
            "transformer_blocks.7.attn.to_out.0.weight"
        );
        assert_eq!(
            map_dit_tail("transformer_blocks.7", "mlp.fc1.weight"),
            "transformer_blocks.7.ff.net.0.proj.weight"
        );
        assert_eq!(
            top_level_dit_name("final_layer.adaln_proj.linear.weight").as_deref(),
            Some("norm_out.linear.weight")
        );
        assert!(top_level_dit_name("rope.inv_freq").is_none());
    }

    #[test]
    fn fused_qkv_maps_to_exact_row_ranges() {
        let k = crate::h3::H3_HIDDEN_SIZE;
        let n = crate::h3::H3_HEAD_COUNT * crate::h3::H3_HEAD_DIM;
        let row_bytes = ggml_row_bytes(GGML_TYPE_Q4_K, k, "qkv").unwrap();
        let source = GgufTensorInfo {
            name: "blocks.0.attn.qkv_proj.weight".to_string(),
            dimensions: vec![k as u64, (3 * n) as u64],
            tensor_type: makepad_ggml::TensorType::Q4K,
            offset: 0,
            size_bytes: (3 * n * row_bytes) as u64,
        };
        let mut tensors = HashMap::new();
        let mut linears = HashMap::new();
        insert_qkv_split(&mut tensors, &mut linears, "transformer_blocks.0", &source)
            .unwrap();
        let q = &tensors["transformer_blocks.0.attn.to_q.weight"];
        let k_map = &tensors["transformer_blocks.0.attn.to_k.weight"];
        let v = &tensors["transformer_blocks.0.attn.to_v.weight"];
        assert_eq!(q.byte_offset, 0);
        assert_eq!(q.byte_len, (n * row_bytes) as u64);
        assert_eq!(k_map.byte_offset, q.byte_len);
        assert_eq!(v.byte_offset, 2 * q.byte_len);
        assert_eq!(v.byte_offset + v.byte_len, source.size_bytes);
        assert_eq!(linears["transformer_blocks.0.attn.to_v.weight"].n, n);
        assert_eq!(linears["transformer_blocks.0.attn.to_v.weight"].k, k);
    }

    #[test]
    fn video_vae_encoder_aliases_are_exact() {
        assert_eq!(
            video_vae_repack_canonical_name("encoder.down.1.block.0.nin_shortcut.weight")
                .as_deref(),
            Some("encoder.down_blocks.1.resnets.0.conv_shortcut.weight")
        );
        assert_eq!(
            video_vae_repack_canonical_name("encoder.down.2.downsample.conv.bias").as_deref(),
            Some("encoder.down_blocks.2.downsamplers.0.conv.bias")
        );
        assert!(video_vae_repack_canonical_name("decoder.norm_out.weight").is_none());
        assert!(video_vae_repack_canonical_name("encoder.down.bad.block.0.weight").is_none());
    }

    #[test]
    fn unsupported_linear_types_fail_closed() {
        let err = runtime_linear_type(makepad_ggml::quant::GGML_TYPE_Q5_K, "bad")
            .unwrap_err();
        assert!(err.to_string().contains("unsupported"));
    }

    #[test]
    fn f32_linear_payload_is_explicitly_normalized_to_bf16() {
        assert_eq!(
            runtime_linear_type(GGML_TYPE_F32, "projection").unwrap(),
            GGML_TYPE_BF16
        );
        let values = [
            1.0f32,
            f32::from_bits(0x3f80_8000), // tie, even lower word
            f32::from_bits(0x3f81_8000), // tie, odd lower word
            -2.5,
        ];
        let bytes: Vec<u8> = values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        let converted = f32_payload_to_bf16(&bytes, "projection").unwrap();
        let words: Vec<u16> = converted
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        assert_eq!(words, vec![0x3f80, 0x3f80, 0x3f82, 0xc020]);
        assert!(f32_payload_to_bf16(&[0, 1, 2], "bad").is_err());
        assert!(f32_payload_to_bf16(&f32::NAN.to_le_bytes(), "bad").is_err());
    }

    #[test]
    fn incomplete_component_inventory_fails_before_cuda() {
        let tensors = HashMap::new();
        let linears = HashMap::new();
        let err = validate_component(H3QuantComponent::Dit, &tensors, &linears, None)
            .unwrap_err();
        assert!(err.to_string().contains("missing required tensor"));
    }

    #[test]
    fn nvfp4_fused_qkv_maps_compound_row_slices() {
        let source = "blocks.0.attn.qkv_proj.weight";
        let base = "blocks.0.attn.qkv_proj";
        let mut entries = HashMap::new();
        // 384 output rows, 64 input columns => 32 packed bytes and four
        // scale bytes per row. The three canonical parts get 128 rows each;
        // both dimensions satisfy Comfy's [128 rows, 4 scale-cols] tiles.
        entries.insert(source.to_string(), nv_info("U8", &[384, 32]));
        entries.insert(
            format!("{base}.weight_scale"),
            nv_info("F8_E4M3", &[384, 4]),
        );
        entries.insert(format!("{base}.weight_scale_2"), nv_info("F32", &[]));
        entries.insert(format!("{base}.comfy_quant"), nv_info("U8", &[19]));
        let file = Flux2SafetensorsHeader {
            path: PathBuf::new(),
            tensors: entries,
        };
        let mut tensors = HashMap::new();
        let mut linears = HashMap::new();
        insert_nv_quant_qkv(
            &file,
            &mut tensors,
            &mut linears,
            "transformer_blocks.0",
            source,
        )
        .unwrap();
        for (part, suffix) in ["to_q", "to_k", "to_v"].into_iter().enumerate() {
            let name = format!("transformer_blocks.0.attn.{suffix}.weight");
            assert_eq!(tensors[&name].dims, [64, 128]);
            assert_eq!(linears[&name].ggml_type, GGML_TYPE_H3_NVFP4_PAIRS);
            match &tensors[&name].storage {
                NvMappedStorage::Quant {
                    row_start,
                    rows,
                    k,
                    ..
                } => {
                    assert_eq!(*row_start, part * 128);
                    assert_eq!(*rows, 128);
                    assert_eq!(*k, 64);
                }
                _ => panic!("expected compound NVFP4 storage"),
            }
        }
    }

    #[test]
    fn nvfp4_companions_and_scale_bytes_fail_closed() {
        let source = "blocks.0.mlp.fc1.weight";
        let mut entries = HashMap::new();
        entries.insert(source.to_string(), nv_info("U8", &[128, 32]));
        let file = Flux2SafetensorsHeader {
            path: PathBuf::new(),
            tensors: entries,
        };
        let err = nv_quant_layout(&file, source).unwrap_err();
        assert!(err.to_string().contains("weight_scale"), "{err}");
        assert!(validate_nvfp4_scale_bytes(&[0x00, 0x7e], "ok").is_ok());
        let err = validate_nvfp4_scale_bytes(&[0x01, 0x7f], "bad").unwrap_err();
        assert!(err.to_string().contains("NaN scale byte"), "{err}");
        let err = validate_nvfp4_scale_bytes(&[0x80], "bad").unwrap_err();
        assert!(err.to_string().contains("negative scale byte"), "{err}");
        assert!(validate_pre_scale(&[1.0, 0.5], "ok").is_ok());
        assert!(validate_pre_scale(&[1.0, 0.0], "bad").is_err());
    }

    #[test]
    fn nvfp4_checkpoint_layout_is_normalized_for_internal_pairs() {
        assert_eq!({
            let mut bytes = [0x12, 0xa5, 0x00, 0xff];
            normalize_nvfp4_weight_nibbles(&mut bytes);
            bytes
        }, [0x21, 0x5a, 0x00, 0xff]);

        // Forward equivalent of Comfy-Kitchen `to_blocked`, kept in the test
        // so the production function is checked against the source format
        // rather than against its own inverse.
        let rows = 256;
        let cols = 8;
        let row_major: Vec<u8> = (0..rows * cols)
            .map(|index| ((index * 37 + 11) & 0xff) as u8)
            .collect();
        let column_blocks = cols / 4;
        let mut blocked = vec![0u8; row_major.len()];
        for row in 0..rows {
            let row_block = row / 128;
            let row_in_block = row % 128;
            let row_group = row_in_block / 32;
            let row_lane = row_in_block % 32;
            for col in 0..cols {
                let column_block = col / 4;
                let column_lane = col % 4;
                let blocked_index = ((((row_block * column_blocks + column_block) * 32
                    + row_lane)
                    * 4
                    + row_group)
                    * 4)
                    + column_lane;
                blocked[blocked_index] = row_major[row * cols + col];
            }
        }
        assert_ne!(blocked, row_major);
        assert_eq!(
            unswizzle_nvfp4_scales(&blocked, rows, cols).unwrap(),
            row_major
        );
        assert!(unswizzle_nvfp4_scales(&blocked, 255, cols).is_err());
        assert!(unswizzle_nvfp4_scales(&blocked, rows, 6).is_err());
        assert!(unswizzle_nvfp4_scales(&blocked[..blocked.len() - 1], rows, cols).is_err());
    }

    #[test]
    fn nvfp4_safetensors_gaps_are_rejected_at_admission() {
        let header = br#"{"x":{"dtype":"F32","shape":[],"data_offsets":[1,5]}}"#;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(header.len() as u64).to_le_bytes());
        bytes.extend_from_slice(header);
        bytes.extend_from_slice(&[0u8; 5]);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "makepad-h3-nvfp4-gap-{}-{nonce}.safetensors",
            std::process::id()
        ));
        std::fs::write(&path, bytes).unwrap();
        let err = match load_nv_safetensors(&path) {
            Ok(_) => panic!("gapped safetensors unexpectedly passed admission"),
            Err(err) => err,
        };
        let _ = std::fs::remove_file(&path);
        assert!(err.to_string().contains("not contiguous"), "{err}");
    }
}
