//! The one tensor/op layer every TripoSplat module is written against.
//!
//! Each op has a portable CPU body (the reference, and what the unit tests
//! exercise) and a CUDA body that forwards to the shared `gpu_*` device
//! surface in `makepad-ai-common`. The model code in `splat_dino.rs`,
//! `splat_flow.rs` and `splat_decoder.rs` is written ONCE against this layer,
//! so the CPU reference and the device path cannot drift apart the way two
//! transcriptions of the same network would.
//!
//! Conventions, matching the rest of libs/ai:
//! * activations are row-major `(rows, cols)` = token-major;
//! * a linear weight is `(n, k)` row-major, i.e. PyTorch's `nn.Linear.weight`,
//!   and `linear` computes `x @ w^T + b`;
//! * attention inputs are `(tokens, heads * head_dim)` with the head axis
//!   inside the column axis.

use crate::backend::{
    gpu_add, gpu_attention_packed_cross, gpu_attention_packed_flash2_d64, gpu_concat_cols, gpu_concat_rows,
    gpu_device_available, gpu_download, gpu_gated_residual_mod, gpu_gelu, gpu_layer_norm_mod,
    gpu_layer_norm_mul_add, gpu_linear_f32_resident, gpu_linear_nt_cached_with_precision, gpu_mul,
    gpu_rms_norm_mul_perhead, gpu_rope_half, gpu_silu, gpu_slice_cols, gpu_upload,
    gpu_weight_cache_ensure, GemmPrecision, GpuLinearPart, GpuTensor,
};
use makepad_ai_loader::quant::{f32_to_f16_rn, GGML_TYPE_F16};
use crate::backend::WeightOwner;
use crate::splat::SplatWeights;
use crate::{DiffusionError, Result};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Device {
    /// Portable f32 reference. Correct everywhere, slow — used by the unit
    /// tests and by any tiny-config forward.
    Cpu,
    /// Device-resident f32 path on the shared CUDA surface.
    Cuda,
}

impl Device {
    /// The device the service should run on: CUDA when a device is actually
    /// present, otherwise nothing (the backend fails closed rather than
    /// silently running a 1B-parameter model on the CPU).
    pub fn cuda_if_available() -> Option<Self> {
        if gpu_device_available() {
            Some(Device::Cuda)
        } else {
            None
        }
    }
}

fn model_err(err: String) -> DiffusionError {
    DiffusionError::model(err)
}

fn check_bias(n: usize, bias: Option<&[f32]>) -> Result<()> {
    match bias {
        Some(values) if values.len() != n => {
            Err(DiffusionError::workflow(format!("splat linear bias {} != {n}", values.len())))
        }
        _ => Ok(()),
    }
}

/// Puts the f16 weight `name` (`[n, k]`, rounded to nearest even like torch
/// `.half()`) in the device weight cache as `owner::name`, reading it from
/// `source` only when the cache does not hold it.
fn ensure_f16(owner: &makepad_ai_common::backend::WeightOwner, source: &SplatWeights, name: &str, n: usize, k: usize) -> Result<()> {
    gpu_weight_cache_ensure(owner, name, GGML_TYPE_F16, n, k, false, || {
        let values = source.f32_shaped(name, &[n, k]).map_err(|err| err.to_string())?;
        let mut bytes = Vec::with_capacity(values.len() * 2);
        for value in values {
            bytes.extend_from_slice(&f32_to_f16_rn(value).to_le_bytes());
        }
        Ok(bytes)
    })
    .map_err(model_err)
}

// ---------------------------------------------------------------------------
// Activations
// ---------------------------------------------------------------------------

enum Inner {
    Cpu(Vec<f32>),
    Gpu(GpuTensor),
}

/// A `(rows, cols)` f32 activation living on whichever device the model runs
/// on.
pub struct Ten {
    rows: usize,
    cols: usize,
    inner: Inner,
}

impl Ten {
    pub fn upload(device: Device, values: &[f32], rows: usize, cols: usize) -> Result<Self> {
        if values.len() != rows * cols {
            return Err(DiffusionError::workflow(format!(
                "splat tensor upload expected {}x{} = {} values, got {}",
                rows,
                cols,
                rows * cols,
                values.len()
            )));
        }
        let inner = match device {
            Device::Cpu => Inner::Cpu(values.to_vec()),
            Device::Cuda => Inner::Gpu(gpu_upload(values, rows, cols).map_err(model_err)?),
        };
        Ok(Self { rows, cols, inner })
    }

    pub fn zeros(device: Device, rows: usize, cols: usize) -> Result<Self> {
        Self::upload(device, &vec![0.0f32; rows * cols], rows, cols)
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn device(&self) -> Device {
        match &self.inner {
            Inner::Cpu(_) => Device::Cpu,
            Inner::Gpu(_) => Device::Cuda,
        }
    }

    pub fn to_host(&self) -> Result<Vec<f32>> {
        match &self.inner {
            Inner::Cpu(values) => Ok(values.clone()),
            Inner::Gpu(tensor) => gpu_download(tensor).map_err(model_err),
        }
    }

    fn cpu(&self) -> Result<&[f32]> {
        match &self.inner {
            Inner::Cpu(values) => Ok(values),
            Inner::Gpu(_) => Err(DiffusionError::workflow("splat: expected a host tensor")),
        }
    }

    fn gpu(&self) -> Result<&GpuTensor> {
        match &self.inner {
            Inner::Gpu(tensor) => Ok(tensor),
            Inner::Cpu(_) => Err(DiffusionError::workflow("splat: expected a device tensor")),
        }
    }

    fn from_cpu(values: Vec<f32>, rows: usize, cols: usize) -> Self {
        Self {
            rows,
            cols,
            inner: Inner::Cpu(values),
        }
    }

    fn from_gpu(tensor: GpuTensor) -> Self {
        Self {
            rows: tensor.rows(),
            cols: tensor.cols(),
            inner: Inner::Gpu(tensor),
        }
    }

    /// Borrow the device handle (for the few ops that call a `gpu_*` entry
    /// point directly rather than through this module).
    pub fn as_gpu(&self) -> Result<&GpuTensor> {
        self.gpu()
    }

    /// Wrap a device tensor produced by such a call.
    pub fn adopt_gpu(tensor: GpuTensor) -> Self {
        Self::from_gpu(tensor)
    }
}

// ---------------------------------------------------------------------------
// Weights
// ---------------------------------------------------------------------------

/// One `nn.Linear`: an `(n, k)` weight plus an optional length-`n` bias.
pub struct Lin {
    pub n: usize,
    pub k: usize,
    weight: LinWeight,
}

enum LinWeight {
    /// f32 weight and bias on the linear's device: the CPU reference, or
    /// device-resident f32.
    F32 { weight: Ten, bias: Option<Ten> },
    /// The reference's `convert_module_to_f16` linears on CUDA: the weight is
    /// rounded to f16 (torch `.half()`) and lives in the device weight cache
    /// under its owner's namespace as `key`, its tensor name; the GEMM takes f16
    /// operands with f32 accumulation and output. No host copy is kept: when
    /// memory pressure evicted the cache entry, the next call reads the
    /// tensor from `source` and rounds it again.
    F16 { owner: Arc<WeightOwner>, key: String, source: Arc<SplatWeights>, bias: Vec<f32> },
}

/// f16 operands, f32 accumulation and output, whatever the process-wide
/// Flux precision switches say.
const F16_OPERANDS: GemmPrecision = GemmPrecision { f16_accumulate: false, f16_activations: false };

impl Lin {
    pub fn new(device: Device, weight: &[f32], n: usize, k: usize, bias: Option<&[f32]>) -> Result<Self> {
        if weight.len() != n * k {
            return Err(DiffusionError::workflow(format!(
                "splat linear expected {n}x{k} weights, got {}",
                weight.len()
            )));
        }
        check_bias(n, bias)?;
        let bias = match bias {
            Some(values) => Some(Ten::upload(device, values, 1, n)?),
            None => None,
        };
        Ok(Self {
            n,
            k,
            weight: LinWeight::F32 { weight: Ten::upload(device, weight, n, k)?, bias },
        })
    }

    /// The linear `name` (`[n, k]`) of `source`, one the reference runs in
    /// fp16. On CUDA it takes f16 GEMM operands (see [`LinWeight::F16`]); on
    /// the CPU it is the f32 reference.
    pub fn new_f16(
        device: Device,
        source: &Arc<SplatWeights>,
        name: &str,
        n: usize,
        k: usize,
        bias: Option<&[f32]>,
        owner: &Arc<WeightOwner>,
    ) -> Result<Self> {
        if device == Device::Cpu {
            return Self::new(device, &source.f32_shaped(name, &[n, k])?, n, k, bias);
        }
        check_bias(n, bias)?;
        ensure_f16(&owner, source, name, n, k)?;
        Ok(Self {
            n,
            k,
            weight: LinWeight::F16 {
                owner: Arc::clone(owner),
                key: name.to_string(),
                source: source.clone(),
                bias: bias.map(<[f32]>::to_vec).unwrap_or_default(),
            },
        })
    }
}

// ---------------------------------------------------------------------------
// Ops
// ---------------------------------------------------------------------------

/// `x @ w^T + b`.
pub fn linear(x: &Ten, lin: &Lin) -> Result<Ten> {
    if x.cols != lin.k {
        return Err(DiffusionError::workflow(format!(
            "splat linear input {} cols != {}",
            x.cols, lin.k
        )));
    }
    match (&x.inner, &lin.weight) {
        (Inner::Cpu(values), LinWeight::F32 { weight, bias }) => {
            let w = weight.cpu()?;
            let bias = match bias {
                Some(bias) => Some(bias.cpu()?),
                None => None,
            };
            let mut out = vec![0.0f32; x.rows * lin.n];
            for row in 0..x.rows {
                let src = &values[row * lin.k..(row + 1) * lin.k];
                let dst = &mut out[row * lin.n..(row + 1) * lin.n];
                for (j, value) in dst.iter_mut().enumerate() {
                    let wr = &w[j * lin.k..(j + 1) * lin.k];
                    let mut sum = bias.map_or(0.0, |b| b[j]);
                    for (a, b) in src.iter().zip(wr) {
                        sum += *a * *b;
                    }
                    *value = sum;
                }
            }
            Ok(Ten::from_cpu(out, x.rows, lin.n))
        }
        (Inner::Gpu(tensor), LinWeight::F32 { weight, bias }) => {
            let bias = match bias {
                Some(bias) => Some(bias.gpu()?),
                None => None,
            };
            let out = gpu_linear_f32_resident(tensor, weight.gpu()?, bias).map_err(model_err)?;
            Ok(Ten::from_gpu(out))
        }
        (Inner::Gpu(tensor), LinWeight::F16 { owner, key, source, bias }) => {
            ensure_f16(&owner, source, key, lin.n, lin.k)?;
            let part = GpuLinearPart { bt_ggml_type: GGML_TYPE_F16, n: lin.n, cache_key: key.as_str(), bytes: &[] };
            let out = gpu_linear_nt_cached_with_precision(tensor, &owner, &[part], bias, F16_OPERANDS)
                .map_err(model_err)?;
            Ok(Ten::from_gpu(out))
        }
        (Inner::Cpu(_), LinWeight::F16 { .. }) => {
            Err(DiffusionError::workflow("splat: an f16 linear takes device activations"))
        }
    }
}

/// `F.layer_norm` over the last axis with optional affine parameters.
pub fn layer_norm(x: &Ten, weight: Option<&[f32]>, bias: Option<&[f32]>, eps: f32) -> Result<Ten> {
    match &x.inner {
        Inner::Cpu(values) => {
            let mut out = vec![0.0f32; values.len()];
            for row in 0..x.rows {
                let src = &values[row * x.cols..(row + 1) * x.cols];
                let mean = src.iter().sum::<f32>() / x.cols as f32;
                let var = src.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / x.cols as f32;
                let inv = 1.0 / (var + eps).sqrt();
                let dst = &mut out[row * x.cols..(row + 1) * x.cols];
                for (j, (value, src)) in dst.iter_mut().zip(src).enumerate() {
                    let normed = (src - mean) * inv;
                    *value = normed * weight.map_or(1.0, |w| w[j]) + bias.map_or(0.0, |b| b[j]);
                }
            }
            Ok(Ten::from_cpu(out, x.rows, x.cols))
        }
        Inner::Gpu(tensor) => {
            let ones;
            let zeros;
            let weight = match weight {
                Some(values) => values,
                None => {
                    ones = vec![1.0f32; x.cols];
                    &ones
                }
            };
            let bias = match bias {
                Some(values) => values,
                None => {
                    zeros = vec![0.0f32; x.cols];
                    &zeros
                }
            };
            let out = gpu_layer_norm_mul_add(tensor, weight, bias, eps).map_err(model_err)?;
            Ok(Ten::from_gpu(out))
        }
    }
}

/// Weightless layer norm followed by the AdaLN affine: `ln(x) * (1 + scale) +
/// shift`, with `scale`/`shift` read out of a single flat modulation row.
pub fn layer_norm_mod(
    x: &Ten,
    mods: &Ten,
    scale_offset: usize,
    shift_offset: usize,
    eps: f32,
) -> Result<Ten> {
    match (&x.inner, &mods.inner) {
        (Inner::Cpu(values), Inner::Cpu(mod_values)) => {
            let scale = &mod_values[scale_offset..scale_offset + x.cols];
            let shift = &mod_values[shift_offset..shift_offset + x.cols];
            let mut out = vec![0.0f32; values.len()];
            for row in 0..x.rows {
                let src = &values[row * x.cols..(row + 1) * x.cols];
                let mean = src.iter().sum::<f32>() / x.cols as f32;
                let var = src.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / x.cols as f32;
                let inv = 1.0 / (var + eps).sqrt();
                let dst = &mut out[row * x.cols..(row + 1) * x.cols];
                for (j, (value, src)) in dst.iter_mut().zip(src).enumerate() {
                    *value = (src - mean) * inv * (1.0 + scale[j]) + shift[j];
                }
            }
            Ok(Ten::from_cpu(out, x.rows, x.cols))
        }
        (Inner::Gpu(tensor), Inner::Gpu(mod_tensor)) => {
            let out = gpu_layer_norm_mod(tensor, mod_tensor, scale_offset, shift_offset, eps)
                .map_err(model_err)?;
            Ok(Ten::from_gpu(out))
        }
        _ => Err(DiffusionError::workflow("splat: mixed-device layer_norm_mod")),
    }
}

/// `x + h * gate`, with `gate` read out of a flat modulation row.
pub fn gated_residual_mod(x: &Ten, h: &Ten, mods: &Ten, gate_offset: usize) -> Result<Ten> {
    match (&x.inner, &h.inner, &mods.inner) {
        (Inner::Cpu(xv), Inner::Cpu(hv), Inner::Cpu(mv)) => {
            let gate = &mv[gate_offset..gate_offset + x.cols];
            let mut out = vec![0.0f32; xv.len()];
            for row in 0..x.rows {
                for j in 0..x.cols {
                    let index = row * x.cols + j;
                    out[index] = xv[index] + hv[index] * gate[j];
                }
            }
            Ok(Ten::from_cpu(out, x.rows, x.cols))
        }
        (Inner::Gpu(xt), Inner::Gpu(ht), Inner::Gpu(mt)) => {
            let out = gpu_gated_residual_mod(xt, ht, mt, gate_offset).map_err(model_err)?;
            Ok(Ten::from_gpu(out))
        }
        _ => Err(DiffusionError::workflow("splat: mixed-device gated residual")),
    }
}

pub fn add(a: &Ten, b: &Ten) -> Result<Ten> {
    match (&a.inner, &b.inner) {
        (Inner::Cpu(av), Inner::Cpu(bv)) => Ok(Ten::from_cpu(
            av.iter().zip(bv).map(|(x, y)| x + y).collect(),
            a.rows,
            a.cols,
        )),
        (Inner::Gpu(at), Inner::Gpu(bt)) => Ok(Ten::from_gpu(gpu_add(at, bt).map_err(model_err)?)),
        _ => Err(DiffusionError::workflow("splat: mixed-device add")),
    }
}

pub fn mul(a: &Ten, b: &Ten) -> Result<Ten> {
    match (&a.inner, &b.inner) {
        (Inner::Cpu(av), Inner::Cpu(bv)) => Ok(Ten::from_cpu(
            av.iter().zip(bv).map(|(x, y)| x * y).collect(),
            a.rows,
            a.cols,
        )),
        (Inner::Gpu(at), Inner::Gpu(bt)) => Ok(Ten::from_gpu(gpu_mul(at, bt).map_err(model_err)?)),
        _ => Err(DiffusionError::workflow("splat: mixed-device mul")),
    }
}

pub fn silu(x: &Ten) -> Result<Ten> {
    match &x.inner {
        Inner::Cpu(values) => Ok(Ten::from_cpu(
            values.iter().map(|v| v / (1.0 + (-v).exp())).collect(),
            x.rows,
            x.cols,
        )),
        Inner::Gpu(tensor) => Ok(Ten::from_gpu(gpu_silu(tensor).map_err(model_err)?)),
    }
}

/// `nn.GELU(approximate="tanh")`.
pub fn gelu_tanh(x: &Ten) -> Result<Ten> {
    match &x.inner {
        Inner::Cpu(values) => Ok(Ten::from_cpu(
            values.iter().map(|v| host_gelu_tanh(*v)).collect(),
            x.rows,
            x.cols,
        )),
        Inner::Gpu(tensor) => Ok(Ten::from_gpu(gpu_gelu(tensor).map_err(model_err)?)),
    }
}

pub fn host_gelu_tanh(x: f32) -> f32 {
    const SQRT_2_OVER_PI: f32 = 0.797_884_56;
    0.5 * x * (1.0 + (SQRT_2_OVER_PI * (x + 0.044_715 * x * x * x)).tanh())
}

/// `MultiHeadRMSNorm`: `F.normalize(x, dim=-1) * gamma * sqrt(head_dim)`,
/// which is exactly an eps-free RMS norm scaled by `gamma`. `gamma` is
/// `(heads, head_dim)` row-major.
/// `owner`/`cache_key` name the DEVICE-CACHED copy of `gamma`. The
/// shared kernel uploads the vector once under `{owner}::{key}::rmsph`
/// and reuses it forever, so two different gammas must never share a key.
pub fn rms_norm_per_head(
    x: &Ten,
    heads: usize,
    head_dim: usize,
    gamma: &[f32],
    owner: &makepad_ai_common::backend::WeightOwner,
    cache_key: &str,
) -> Result<Ten> {
    if x.cols != heads * head_dim || gamma.len() != heads * head_dim {
        return Err(DiffusionError::workflow("splat rms-norm shape mismatch"));
    }
    match &x.inner {
        Inner::Cpu(values) => {
            let mut out = vec![0.0f32; values.len()];
            for row in 0..x.rows {
                for head in 0..heads {
                    let base = row * x.cols + head * head_dim;
                    let src = &values[base..base + head_dim];
                    let norm = src.iter().map(|v| v * v).sum::<f32>().sqrt();
                    // F.normalize's own eps floor.
                    let inv = 1.0 / norm.max(1e-12);
                    let scale = (head_dim as f32).sqrt();
                    for j in 0..head_dim {
                        out[base + j] = src[j] * inv * gamma[head * head_dim + j] * scale;
                    }
                }
            }
            Ok(Ten::from_cpu(out, x.rows, x.cols))
        }
        Inner::Gpu(tensor) => {
            // The shared kernel is `x / rms(x) * gamma` with a caller-supplied
            // eps; rms = ||x|| / sqrt(head_dim), so it equals the reference
            // formula above.
            let out = gpu_rms_norm_mul_perhead(
                tensor,
                heads,
                head_dim,
                owner,
                cache_key,
                gamma,
                0.0,
            )
            .map_err(model_err)?;
            Ok(Ten::from_gpu(out))
        }
    }
}

/// DINOv3's rotate-half rope: `x * cos + rotate_half(x) * sin` per head, with
/// `cos`/`sin` as `(rows, rot_half)` tables shared by every head (the
/// reference's `.tile(2)` makes the second half a copy of the first).
pub fn rope_half(
    x: &Ten,
    heads: usize,
    rot_half: usize,
    cos: &Ten,
    sin: &Ten,
    skip_prefix_rows: usize,
) -> Result<Ten> {
    let head_dim = rot_half * 2;
    if x.cols != heads * head_dim {
        return Err(DiffusionError::workflow("splat rope-half shape mismatch"));
    }
    match &x.inner {
        Inner::Cpu(values) => {
            let cos = cos.cpu()?;
            let sin = sin.cpu()?;
            let mut out = values.clone();
            for row in skip_prefix_rows..x.rows {
                for head in 0..heads {
                    let base = row * x.cols + head * head_dim;
                    for j in 0..rot_half {
                        let c = cos[row * rot_half + j];
                        let s = sin[row * rot_half + j];
                        let lo = values[base + j];
                        let hi = values[base + rot_half + j];
                        // rotate_half([a, b]) = [-b, a]
                        out[base + j] = lo * c - hi * s;
                        out[base + rot_half + j] = hi * c + lo * s;
                    }
                }
            }
            Ok(Ten::from_cpu(out, x.rows, x.cols))
        }
        Inner::Gpu(tensor) => {
            // The prefix rows carry identity table entries (cos = 1, sin = 0)
            // so the device kernel can run over the whole sequence.
            let out =
                gpu_rope_half(tensor, heads, rot_half, cos.gpu()?, sin.gpu()?).map_err(model_err)?;
            Ok(Ten::from_gpu(out))
        }
    }
}

/// The flow model's `apply_rotary_emb`: complex multiply on INTERLEAVED value
/// pairs, with a per-token-AND-per-head phase table `(rows, heads * pairs)`
/// produced by [`crate::splat_flow`]'s RePo3D layers.
pub fn rope_pairs_per_head(x: &Ten, heads: usize, cos: &Ten, sin: &Ten) -> Result<Ten> {
    if x.cols % heads != 0 {
        return Err(DiffusionError::workflow("splat rope head mismatch"));
    }
    let head_dim = x.cols / heads;
    let pairs = head_dim / 2;
    if cos.rows != x.rows || cos.cols != heads * pairs || sin.rows != x.rows || sin.cols != cos.cols
    {
        return Err(DiffusionError::workflow("splat rope table mismatch"));
    }
    match &x.inner {
        Inner::Cpu(values) => {
            let cos = cos.cpu()?;
            let sin = sin.cpu()?;
            let mut out = vec![0.0f32; values.len()];
            for row in 0..x.rows {
                for head in 0..heads {
                    let base = row * x.cols + head * head_dim;
                    let table = row * heads * pairs + head * pairs;
                    for p in 0..pairs {
                        let c = cos[table + p];
                        let s = sin[table + p];
                        let re = values[base + 2 * p];
                        let im = values[base + 2 * p + 1];
                        out[base + 2 * p] = re * c - im * s;
                        out[base + 2 * p + 1] = re * s + im * c;
                    }
                }
            }
            Ok(Ten::from_cpu(out, x.rows, x.cols))
        }
        Inner::Gpu(tensor) => {
            let out = crate::backend::gpu_splat_rope_pairs_per_head(
                tensor,
                heads,
                cos.gpu()?,
                sin.gpu()?,
            )
            .map_err(model_err)?;
            Ok(Ten::from_gpu(out))
        }
    }
}

/// Scaled dot-product attention. `q` is `(Lq, heads*head_dim)`, `k`/`v` are
/// `(Lkv, heads*head_dim)`.
pub fn attention(q: &Ten, k: &Ten, v: &Ten, heads: usize, scale: f32) -> Result<Ten> {
    if q.cols % heads != 0 || k.cols != q.cols || v.cols != q.cols || k.rows != v.rows {
        return Err(DiffusionError::workflow("splat attention shape mismatch"));
    }
    let head_dim = q.cols / heads;
    match (&q.inner, &k.inner, &v.inner) {
        (Inner::Cpu(qv), Inner::Cpu(kv), Inner::Cpu(vv)) => {
            let mut out = vec![0.0f32; q.rows * q.cols];
            let mut scores = vec![0.0f32; k.rows];
            for row in 0..q.rows {
                for head in 0..heads {
                    let qbase = row * q.cols + head * head_dim;
                    let mut max = f32::NEG_INFINITY;
                    for (kk, score) in scores.iter_mut().enumerate() {
                        let kbase = kk * k.cols + head * head_dim;
                        let mut dot = 0.0f32;
                        for j in 0..head_dim {
                            dot += qv[qbase + j] * kv[kbase + j];
                        }
                        *score = dot * scale;
                        max = max.max(*score);
                    }
                    let mut sum = 0.0f32;
                    for score in scores.iter_mut() {
                        *score = (*score - max).exp();
                        sum += *score;
                    }
                    let inv = 1.0 / sum;
                    let obase = row * q.cols + head * head_dim;
                    for (kk, score) in scores.iter().enumerate() {
                        let weight = *score * inv;
                        let vbase = kk * v.cols + head * head_dim;
                        for j in 0..head_dim {
                            out[obase + j] += weight * vv[vbase + j];
                        }
                    }
                }
            }
            Ok(Ten::from_cpu(out, q.rows, q.cols))
        }
        (Inner::Gpu(qt), Inner::Gpu(kt), Inner::Gpu(vt)) => {
            // head_dim 64 (DINOv3, the flow trunk and both decoders) takes the
            // FA2 kernel: f16 Q/K/V operands with f32 online softmax and
            // accumulation. The reference runs every one of these modules in
            // fp16/bf16 under SDPA, so this is its precision class, and the
            // scores never leave registers. The f32 composite wrote the whole
            // (heads, Lq, Lkv) score matrix out in query chunks: 74.5 ms
            // against 5.6 ms per trunk block on a 24 GB GPU. Other head sizes keep
            // the CROSS entry point, which bounds its scores buffer.
            let out = if head_dim == 64 {
                gpu_attention_packed_flash2_d64(qt, kt, vt, heads, scale)
            } else {
                gpu_attention_packed_cross(qt, kt, vt, heads, scale)
            }
            .map_err(model_err)?;
            Ok(Ten::from_gpu(out))
        }
        _ => Err(DiffusionError::workflow("splat: mixed-device attention")),
    }
}

pub fn slice_cols(x: &Ten, start: usize, count: usize) -> Result<Ten> {
    if start + count > x.cols {
        return Err(DiffusionError::workflow("splat slice out of range"));
    }
    match &x.inner {
        Inner::Cpu(values) => {
            let mut out = vec![0.0f32; x.rows * count];
            for row in 0..x.rows {
                out[row * count..(row + 1) * count]
                    .copy_from_slice(&values[row * x.cols + start..row * x.cols + start + count]);
            }
            Ok(Ten::from_cpu(out, x.rows, count))
        }
        Inner::Gpu(tensor) => Ok(Ten::from_gpu(
            gpu_slice_cols(tensor, start, count).map_err(model_err)?,
        )),
    }
}

pub fn concat_rows(a: &Ten, b: &Ten) -> Result<Ten> {
    if a.cols != b.cols {
        return Err(DiffusionError::workflow("splat concat width mismatch"));
    }
    match (&a.inner, &b.inner) {
        (Inner::Cpu(av), Inner::Cpu(bv)) => {
            let mut out = av.clone();
            out.extend_from_slice(bv);
            Ok(Ten::from_cpu(out, a.rows + b.rows, a.cols))
        }
        (Inner::Gpu(at), Inner::Gpu(bt)) => Ok(Ten::from_gpu(
            gpu_concat_rows(at, bt).map_err(model_err)?,
        )),
        _ => Err(DiffusionError::workflow("splat: mixed-device concat")),
    }
}

pub fn concat_cols(a: &Ten, b: &Ten) -> Result<Ten> {
    if a.rows != b.rows {
        return Err(DiffusionError::workflow("splat concat height mismatch"));
    }
    match (&a.inner, &b.inner) {
        (Inner::Cpu(av), Inner::Cpu(bv)) => {
            let cols = a.cols + b.cols;
            let mut out = vec![0.0f32; a.rows * cols];
            for row in 0..a.rows {
                out[row * cols..row * cols + a.cols]
                    .copy_from_slice(&av[row * a.cols..(row + 1) * a.cols]);
                out[row * cols + a.cols..(row + 1) * cols]
                    .copy_from_slice(&bv[row * b.cols..(row + 1) * b.cols]);
            }
            Ok(Ten::from_cpu(out, a.rows, cols))
        }
        (Inner::Gpu(at), Inner::Gpu(bt)) => Ok(Ten::from_gpu(
            gpu_concat_cols(&[at, bt]).map_err(model_err)?,
        )),
        _ => Err(DiffusionError::workflow("splat: mixed-device concat")),
    }
}

/// Single-row host linear — the timestep/modulation path never has more than
/// one row and the reference keeps it in f32 outside the block dtype.
pub fn host_linear(x: &[f32], k: usize, weight: &[f32], n: usize, bias: Option<&[f32]>) -> Vec<f32> {
    debug_assert_eq!(x.len(), k);
    debug_assert_eq!(weight.len(), n * k);
    let mut out = vec![0.0f32; n];
    for (j, value) in out.iter_mut().enumerate() {
        let mut sum = bias.map_or(0.0, |b| b[j]);
        let wr = &weight[j * k..(j + 1) * k];
        for (a, b) in x.iter().zip(wr) {
            sum += *a * *b;
        }
        *value = sum;
    }
    out
}

pub fn host_silu(values: &mut [f32]) {
    for value in values.iter_mut() {
        *value = *value / (1.0 + (-*value).exp());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ten(values: &[f32], rows: usize, cols: usize) -> Ten {
        Ten::upload(Device::Cpu, values, rows, cols).unwrap()
    }

    #[test]
    fn linear_is_x_times_w_transpose_plus_bias() {
        // x = [[1, 2]], w = [[1, 0], [0, 1], [1, 1]], b = [10, 20, 30]
        let x = ten(&[1.0, 2.0], 1, 2);
        let lin = Lin::new(
            Device::Cpu,
            &[1.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            3,
            2,
            Some(&[10.0, 20.0, 30.0]),
        )
        .unwrap();
        assert_eq!(linear(&x, &lin).unwrap().to_host().unwrap(), vec![11.0, 22.0, 33.0]);
    }

    #[test]
    fn layer_norm_centers_and_scales() {
        let x = ten(&[1.0, 2.0, 3.0], 1, 3);
        let out = layer_norm(&x, None, None, 0.0).unwrap().to_host().unwrap();
        // mean 2, population std sqrt(2/3)
        let s = (2.0f32 / 3.0).sqrt();
        assert!((out[0] + 1.0 / s).abs() < 1e-5);
        assert!(out[1].abs() < 1e-6);
        assert!((out[2] - 1.0 / s).abs() < 1e-5);
        // Affine parameters ride on top.
        let out = layer_norm(&x, Some(&[2.0, 2.0, 2.0]), Some(&[1.0, 1.0, 1.0]), 0.0)
            .unwrap()
            .to_host()
            .unwrap();
        assert!((out[1] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn layer_norm_mod_applies_one_plus_scale_then_shift() {
        let x = ten(&[1.0, 2.0, 3.0], 1, 3);
        // mods row = [shift(3) | scale(3)]
        let mods = ten(&[1.0, 1.0, 1.0, 1.0, 1.0, 1.0], 1, 6);
        let out = layer_norm_mod(&x, &mods, 3, 0, 0.0).unwrap().to_host().unwrap();
        let s = (2.0f32 / 3.0).sqrt();
        // normalized * (1 + 1) + 1
        assert!((out[1] - 1.0).abs() < 1e-6);
        assert!((out[2] - (2.0 / s + 1.0)).abs() < 1e-4);
    }

    #[test]
    fn gated_residual_scales_the_branch_only() {
        let x = ten(&[1.0, 1.0], 1, 2);
        let h = ten(&[10.0, 10.0], 1, 2);
        let mods = ten(&[0.5, 2.0], 1, 2);
        assert_eq!(
            gated_residual_mod(&x, &h, &mods, 0).unwrap().to_host().unwrap(),
            vec![6.0, 21.0]
        );
    }

    #[test]
    fn rms_norm_per_head_is_l2_normalize_times_gamma_times_sqrt_dim() {
        let owner = WeightOwner::new("splat-rms-test");
        // One head, dim 4, x = [1,1,1,1] -> ||x|| = 2, normalize -> 0.5 each,
        // * sqrt(4) = 2 -> 1.0 each, * gamma.
        let x = ten(&[1.0, 1.0, 1.0, 1.0], 1, 4);
        let out = rms_norm_per_head(&x, 1, 4, &[1.0, 2.0, 3.0, 4.0], &owner, "a")
            .unwrap()
            .to_host()
            .unwrap();
        assert_eq!(out, vec![1.0, 2.0, 3.0, 4.0]);
        // Two heads must normalize independently.
        let x = ten(&[1.0, 0.0, 0.0, 3.0], 1, 4);
        let out = rms_norm_per_head(&x, 2, 2, &[1.0, 1.0, 1.0, 1.0], &owner, "b")
            .unwrap()
            .to_host()
            .unwrap();
        let r = 2.0f32.sqrt();
        assert!((out[0] - r).abs() < 1e-6);
        assert!((out[3] - r).abs() < 1e-6);
    }

    #[test]
    fn rope_half_rotates_the_two_halves_together() {
        // head_dim 4, rot_half 2. cos = 0, sin = 1 -> [a,b,c,d] -> [-c,-d,a,b]
        let x = ten(&[1.0, 2.0, 3.0, 4.0], 1, 4);
        let cos = ten(&[0.0, 0.0], 1, 2);
        let sin = ten(&[1.0, 1.0], 1, 2);
        let out = rope_half(&x, 1, 2, &cos, &sin, 0).unwrap().to_host().unwrap();
        assert_eq!(out, vec![-3.0, -4.0, 1.0, 2.0]);
        // Prefix rows are left untouched on the CPU path.
        let x = ten(&[1.0, 2.0, 3.0, 4.0], 1, 4);
        let out = rope_half(&x, 1, 2, &cos, &sin, 1).unwrap().to_host().unwrap();
        assert_eq!(out, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn rope_pairs_per_head_is_a_complex_multiply_on_interleaved_pairs() {
        // 2 heads x head_dim 2 -> 1 pair per head; head 0 gets a quarter turn,
        // head 1 is identity.
        let x = ten(&[1.0, 0.0, 1.0, 0.0], 1, 4);
        let cos = ten(&[0.0, 1.0], 1, 2);
        let sin = ten(&[1.0, 0.0], 1, 2);
        let out = rope_pairs_per_head(&x, 2, &cos, &sin).unwrap().to_host().unwrap();
        assert_eq!(out, vec![0.0, 1.0, 1.0, 0.0]);
    }

    #[test]
    fn attention_with_uniform_scores_averages_the_values() {
        // q = 0 -> all scores equal -> output is the mean of v.
        let q = ten(&[0.0, 0.0], 1, 2);
        let k = ten(&[1.0, 1.0, 2.0, 2.0], 2, 2);
        let v = ten(&[0.0, 0.0, 4.0, 8.0], 2, 2);
        let out = attention(&q, &k, &v, 1, 1.0).unwrap().to_host().unwrap();
        assert!((out[0] - 2.0).abs() < 1e-6);
        assert!((out[1] - 4.0).abs() < 1e-6);
        // A large positive score on one key selects it.
        let q = ten(&[10.0, 10.0], 1, 2);
        let out = attention(&q, &k, &v, 1, 1.0).unwrap().to_host().unwrap();
        assert!((out[0] - 4.0).abs() < 1e-4);
    }

    #[test]
    fn gelu_tanh_matches_hand_values() {
        assert_eq!(host_gelu_tanh(0.0), 0.0);
        assert!((host_gelu_tanh(1.0) - 0.841_192).abs() < 1e-5);
        assert!((host_gelu_tanh(-1.0) + 0.158_808).abs() < 1e-5);
    }

    #[test]
    fn slice_and_concat_round_trip() {
        let x = ten(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], 2, 3);
        let left = slice_cols(&x, 0, 1).unwrap();
        let right = slice_cols(&x, 1, 2).unwrap();
        assert_eq!(left.to_host().unwrap(), vec![1.0, 4.0]);
        assert_eq!(right.to_host().unwrap(), vec![2.0, 3.0, 5.0, 6.0]);
        assert_eq!(
            concat_cols(&left, &right).unwrap().to_host().unwrap(),
            vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]
        );
        let stacked = concat_rows(&x, &x).unwrap();
        assert_eq!(stacked.rows(), 4);
        assert_eq!(stacked.to_host().unwrap().len(), 12);
    }
}
