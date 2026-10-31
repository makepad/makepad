//! YuE2 MoT backbone on the device `gpu_*` surface: the AR set (score +
//! codec tokens, KV-cached decode) and the NAR set (flow-matching velocity
//! over the latent tokens, attending to the AR prefix cache).
//!
//! Numerics: bf16 prefill/NAR GEMMs, optional Q8_0 AR decode GEMVs, f32
//! accumulation and an f32 residual spine (the reference runs it in bf16).
//! Prefill/NAR attention is grouped-query FA2 with RN-even bf16 operands;
//! decode uses split-KV f32 math over f32 or bf16 cache storage.
//! MAKEPAD_YUE2_NAR_PREC=fast changes only the NAR transformer linears and
//! attention; the default bf16 path, AR prefix, timestep/input/output maps
//! and VAE retain their original arithmetic.

use crate::backend::{
    gpu_add, gpu_add_rms_norm_into, gpu_add_rms_norm_keyed, gpu_add_row_position_into,
    gpu_alloc_bf16, gpu_alloc_f32,
    gpu_attention_gqa_decode_split, gpu_attention_gqa_decode_split_bf16,
    gpu_attention_gqa_decode_split_device, gpu_attention_gqa_decode_split_device_bf16,
    gpu_attention_gqa_flash_bf16,
    gpu_attention_gqa_flash_bf16_buf_into, gpu_copy_f32_to_bf16_into, gpu_copy_into, gpu_download,
    gpu_gather_rows_cached_bf16, gpu_linear_nt_cached_bf16_f32acc,
    gpu_linear_nt_cached_bf16_f32acc_into, gpu_linear_nt_cached_bf16_f32acc_keyed,
    gpu_linear_nt_cached_precision_into, gpu_linear_scratch,
    gpu_linear_nt_cached_q8_0_f32acc_keyed,
    gpu_linear_nt_cached_q8_0_swiglu_keyed,
    gpu_qk_rms_norm_rope_cache, gpu_qk_rms_norm_rope_cache_bf16,
    gpu_qkv_rms_rope_bf16_into, gpu_rms_norm_mul_keyed,
    gpu_rope_half, gpu_runtime_trim, gpu_silu, gpu_slice_cols, gpu_slice_rows,
    gpu_swiglu_value_gate, gpu_swiglu_value_gate_into, gpu_upload, gpu_upload_into,
    gpu_upload_u32, gpu_weight_cache_ensure, gpu_weight_cache_ensure_bf16_keyed,
    gpu_weight_cache_ensure_bf16_precision_keyed,
    gpu_weight_cache_ensure_q8_0_from_bf16_keyed,
    gpu_weight_cache_ensure_q8_0_concat_bf16_keyed,
    gpu_weight_cache_evict_prefix, GpuAttentionPrecision, GpuBf16Buf, GpuLinearPart,
    GpuLinearPrecision, GpuLinearScratch, GpuTensor,
};
use crate::yue2::{
    Yue2Phase, Yue2Weights, YUE2_FF, YUE2_HEADS, YUE2_HEAD_DIM, YUE2_HIDDEN, YUE2_KV_HEADS,
    YUE2_CONTEXT, YUE2_LATENT_DIM, YUE2_LAYERS, YUE2_NAMESPACE, YUE2_RMS_EPS, YUE2_ROPE_THETA,
    YUE2_TIMESTEP_SHIFT, YUE2_TIME_FREQ_DIM, YUE2_VOCAB,
};
use crate::{DiffusionError, Result};
use std::cell::{Cell, RefCell};

const Q_INNER: usize = YUE2_HEADS * YUE2_HEAD_DIM;
const KV_INNER: usize = YUE2_KV_HEADS * YUE2_HEAD_DIM;
const GGML_BF16: u32 = makepad_ai_common::quant::GGML_TYPE_BF16;
const GGML_Q8_0: u32 = makepad_ai_common::quant::GGML_TYPE_Q8_0;
const EMBED_KEY: &str = "yue2::model.embed_tokens.weight";
const FINAL_NORM_KEY: &str = "yue2::model.norm::rms";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Yue2NarPrecision {
    Bf16,
    Fast,
}

/// True when CUDA device 0 is a GeForce part.
fn geforce_device() -> bool {
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        let mut name = [0u8; 256];
        let (mut major, mut minor, mut sms) = (0i32, 0i32, 0i32);
        let mut mem = 0usize;
        // SAFETY: plain out-parameters sized as declared.
        unsafe {
            let _ = makepad_ai_common::llm_ops::device_info(
                0, name.as_mut_ptr(), name.len() as i32, &mut major, &mut minor, &mut mem, &mut sms,
            );
        }
        let len = name.iter().position(|&b| b == 0).unwrap_or(0);
        String::from_utf8_lossy(&name[..len]).contains("GeForce")
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        false
    }
}

impl Yue2NarPrecision {
    pub fn from_env() -> Result<Self> {
        match std::env::var("MAKEPAD_YUE2_NAR_PREC").as_deref() {
            Ok("bf16") => Ok(Self::Bf16),
            // GeForce tensor cores run bf16 x fp32-accumulate at half rate; the
            // f16-accumulate NAR passes the oracle gates (v0 1.33e-2, latents
            // 4.85e-2) and is 21 % faster per NAR layer there. Datacenter/
            // workstation parts keep bf16 (fp32-accumulate is full rate).
            Err(std::env::VarError::NotPresent) => Ok(if geforce_device() { Self::Fast } else { Self::Bf16 }),
            Ok("fast") => Ok(Self::Fast),
            _ => Err(DiffusionError::model("MAKEPAD_YUE2_NAR_PREC must be bf16 or fast")),
        }
    }

    pub fn name(self) -> &'static str {
        match self { Self::Bf16 => "bf16", Self::Fast => "fast" }
    }

    fn linear(self) -> GpuLinearPrecision {
        match self {
            Self::Bf16 => GpuLinearPrecision::Bf16F32,
            // Switch this one choice to Fp8F32 after the laptop/quality gates.
            Self::Fast => GpuLinearPrecision::F16F16,
        }
    }

    fn attention(self) -> GpuAttentionPrecision {
        match self {
            Self::Bf16 => GpuAttentionPrecision::Bf16F32,
            Self::Fast => GpuAttentionPrecision::F16QkPv,
        }
    }
}

fn dev<E: std::fmt::Display>(what: &'static str) -> impl FnOnce(E) -> DiffusionError {
    move |e| DiffusionError::model(format!("yue2 {what}: {e}"))
}

/// Which of the two per-layer parameter sets runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Yue2Set {
    Ar,
    Nar,
}

/// Captured device decode or the per-op reference path with host sampling.
/// Production defaults to Graph; prefill and NAR are shared by both modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Yue2DecodeMode {
    Graph,
    Host,
}

/// Fixed for the model's lifetime, including captured graphs. Q8 keeps an
/// additional Q8_0 copy of AR linears/heads and uses bf16 AR K/V. BF16 is
/// the reference path (bf16 GEMMs, f32 AR K/V). Prefill/NAR always use BF16.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Yue2ArWeights {
    Q8,
    Bf16,
}

impl Yue2ArWeights {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "q8" => Some(Self::Q8),
            "bf16" => Some(Self::Bf16),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Q8 => "q8",
            Self::Bf16 => "bf16",
        }
    }

    fn from_env() -> Result<Self> {
        match std::env::var("MAKEPAD_YUE2_AR_WEIGHTS") {
            Ok(value) => Self::parse(&value).ok_or_else(|| {
                DiffusionError::model("MAKEPAD_YUE2_AR_WEIGHTS must be q8 or bf16")
            }),
            // Q8 passed the oracle gates (tf semantic 2.19e-2, 252/256) and an
            // 8-seed WER comparison against bf16 (mean 41.6 % vs 51.6 %, noise-bound).
            Err(std::env::VarError::NotPresent) => Ok(Self::Q8),
            Err(e) => Err(DiffusionError::model(format!("MAKEPAD_YUE2_AR_WEIGHTS: {e}"))),
        }
    }
}

impl Yue2DecodeMode {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "graph" => Some(Self::Graph),
            "host" => Some(Self::Host),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Graph => "graph",
            Self::Host => "host",
        }
    }
}

struct WeightName {
    tensor: String,
    key: String,
    q8_key: String,
}

impl WeightName {
    fn linear(base: &str) -> Self {
        let tensor = format!("{base}.weight");
        let key = format!("{YUE2_NAMESPACE}::{tensor}");
        let q8_key = format!("{key}::q8_0");
        Self { tensor, key, q8_key }
    }

    fn norm(base: &str) -> Self {
        Self {
            tensor: format!("{base}.weight"),
            key: format!("{YUE2_NAMESPACE}::{base}::rms"),
            q8_key: String::new(),
        }
    }
}

struct SetNames {
    input_norm: WeightName,
    post_norm: WeightName,
    q_norm: WeightName,
    k_norm: WeightName,
    q: WeightName,
    k: WeightName,
    v: WeightName,
    o: WeightName,
    up: WeightName,
    gate: WeightName,
    down: WeightName,
}

impl Yue2Set {
    fn names(self, layer: usize) -> SetNames {
        let p = format!("model.layers.{layer}");
        let (input, post, attn, mlp) = match self {
            Self::Ar => ("input_layernorm", "post_attention_layernorm", "self_attn", "mlp"),
            Self::Nar => ("nar_input_layernorm", "nar_pre_mlp_layernorm", "nar_self_attn", "nar_mlp"),
        };
        let mut names = SetNames {
            input_norm: WeightName::norm(&format!("{p}.{input}")),
            post_norm: WeightName::norm(&format!("{p}.{post}")),
            q_norm: WeightName::norm(&format!("{p}.{attn}.q_norm")),
            k_norm: WeightName::norm(&format!("{p}.{attn}.k_norm")),
            q: WeightName::linear(&format!("{p}.{attn}.q_proj")),
            k: WeightName::linear(&format!("{p}.{attn}.k_proj")),
            v: WeightName::linear(&format!("{p}.{attn}.v_proj")),
            o: WeightName::linear(&format!("{p}.{attn}.o_proj")),
            up: WeightName::linear(&format!("{p}.{mlp}.up_proj")),
            gate: WeightName::linear(&format!("{p}.{mlp}.gate_proj")),
            down: WeightName::linear(&format!("{p}.{mlp}.down_proj")),
        };
        // Only packed QKV and up|gate Q8 copies are resident. BF16 names
        // remain individual for prefill/NAR; no duplicate Q8 allocations.
        names.q.q8_key = format!("{YUE2_NAMESPACE}::{p}.{attn}.qkv_proj::q8_0");
        names.up.q8_key = format!("{YUE2_NAMESPACE}::{p}.{mlp}.up_gate_proj::q8_0");
        names
    }
}

struct SetNorms {
    names: Vec<SetNames>,
    input: Vec<Vec<f32>>,
    post: Vec<Vec<f32>>,
    q: Vec<Vec<f32>>,
    k: Vec<Vec<f32>>,
}

impl SetNorms {
    fn load(w: &Yue2Weights, set: Yue2Set) -> Result<Self> {
        let mut out = Self { names: vec![], input: vec![], post: vec![], q: vec![], k: vec![] };
        for layer in 0..YUE2_LAYERS {
            let n = set.names(layer);
            out.input.push(w.tensor_f32(&n.input_norm.tensor)?);
            out.post.push(w.tensor_f32(&n.post_norm.tensor)?);
            out.q.push(w.tensor_f32(&n.q_norm.tensor)?);
            out.k.push(w.tensor_f32(&n.k_norm.tensor)?);
            out.names.push(n);
        }
        Ok(out)
    }
}

/// The checkpoint, precomputed weight keys and lazily uploaded RoPE tables.
pub struct Yue2Model {
    pub weights: Yue2Weights,
    ar: SetNorms,
    nar: SetNorms,
    final_norm: Vec<f32>,
    inv_freq: Vec<f32>,
    rope: RefCell<Option<(GpuTensor, GpuTensor)>>,
    decode_mode: Cell<Yue2DecodeMode>,
    ar_weights: Yue2ArWeights,
    nar_precision: Yue2NarPrecision,
    vae2llm_bias: Vec<f32>,
    llm2vae_bias: Vec<f32>,
    time_b0: Vec<f32>,
    time_b2: Vec<f32>,
}

/// Per-layer K/V of one AR sequence, preallocated to `capacity` rows.
pub struct Yue2KvCache {
    layers: Vec<Yue2KvLayer>,
    ar_weights: Yue2ArWeights,
    pub len: usize,
    pub capacity: usize,
}

enum Yue2KvLayer {
    F32 { k: GpuTensor, v: GpuTensor },
    Bf16 { k: GpuBf16Buf, v: GpuBf16Buf },
}

impl Yue2KvCache {
    pub fn new(capacity: usize, ar_weights: Yue2ArWeights) -> Result<Self> {
        let mut layers = Vec::with_capacity(YUE2_LAYERS);
        for _ in 0..YUE2_LAYERS {
            layers.push(match ar_weights {
                Yue2ArWeights::Q8 => Yue2KvLayer::Bf16 {
                    k: gpu_alloc_bf16(capacity, KV_INNER).map_err(dev("kv alloc"))?,
                    v: gpu_alloc_bf16(capacity, KV_INNER).map_err(dev("kv alloc"))?,
                },
                Yue2ArWeights::Bf16 => Yue2KvLayer::F32 {
                    k: gpu_alloc_f32(capacity, KV_INNER).map_err(dev("kv alloc"))?,
                    v: gpu_alloc_f32(capacity, KV_INNER).map_err(dev("kv alloc"))?,
                },
            });
        }
        Ok(Self { layers, ar_weights, len: 0, capacity })
    }

    fn write(&self, layer: usize, pos: usize, k: &GpuTensor, v: &GpuTensor) -> Result<()> {
        let rows = k.rows();
        if pos + rows > self.capacity {
            return Err(DiffusionError::model(format!(
                "yue2 KV cache capacity {} exceeded by {}",
                self.capacity,
                pos + rows
            )));
        }
        match &self.layers[layer] {
            Yue2KvLayer::F32 { k: kd, v: vd } => {
                let kd = gpu_slice_rows(kd, pos, rows).map_err(dev("kv view"))?;
                let vd = gpu_slice_rows(vd, pos, rows).map_err(dev("kv view"))?;
                gpu_copy_into(k, &kd).map_err(dev("kv write"))?;
                gpu_copy_into(v, &vd).map_err(dev("kv write"))?;
            }
            Yue2KvLayer::Bf16 { k: kd, v: vd } => {
                gpu_copy_f32_to_bf16_into(k, kd, pos).map_err(dev("kv write"))?;
                gpu_copy_f32_to_bf16_into(v, vd, pos).map_err(dev("kv write"))?;
            }
        }
        Ok(())
    }

    fn attend(&self, layer: usize, q: &GpuTensor, sequence: usize, scale: f32) -> Result<GpuTensor> {
        match &self.layers[layer] {
            Yue2KvLayer::F32 { k, v } => gpu_attention_gqa_decode_split(
                q, k, v, sequence, YUE2_HEADS, YUE2_KV_HEADS, scale,
            ),
            Yue2KvLayer::Bf16 { k, v } => gpu_attention_gqa_decode_split_bf16(
                q, k, v, sequence, YUE2_HEADS, YUE2_KV_HEADS, scale,
            ),
        }.map_err(dev("decode attn"))
    }

    fn attend_device(
        &self, layer: usize, q: &GpuTensor, sequence: &GpuTensor, scale: f32,
    ) -> Result<GpuTensor> {
        match &self.layers[layer] {
            Yue2KvLayer::F32 { k, v } => gpu_attention_gqa_decode_split_device(
                q, k, v, sequence, YUE2_HEADS, YUE2_KV_HEADS, scale,
            ),
            Yue2KvLayer::Bf16 { k, v } => gpu_attention_gqa_decode_split_device_bf16(
                q, k, v, sequence, YUE2_HEADS, YUE2_KV_HEADS, scale,
            ),
        }.map_err(dev("decode attn"))
    }
}

fn bf16_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(2)
        .map(|b| f32::from_bits((u16::from_le_bytes([b[0], b[1]]) as u32) << 16))
        .collect()
}

fn bf16_round(v: f32) -> f32 {
    if !v.is_finite() {
        return v;
    }
    let bits = v.to_bits();
    f32::from_bits(bits.wrapping_add(0x7fff + ((bits >> 16) & 1)) & 0xffff_0000)
}

impl Yue2Model {
    pub fn load(weights: Yue2Weights) -> Result<Self> {
        let ar_weights = Yue2ArWeights::from_env()?;
        let nar_precision = Yue2NarPrecision::from_env()?;
        let ar = SetNorms::load(&weights, Yue2Set::Ar)?;
        let nar = SetNorms::load(&weights, Yue2Set::Nar)?;
        let final_norm = weights.tensor_f32("model.norm.weight")?;
        let half = YUE2_HEAD_DIM / 2;
        // Reference: 1 / base ** (arange(0, hd, 2) / hd) in f32.
        let inv_freq = (0..half)
            .map(|j| 1.0 / YUE2_ROPE_THETA.powf((2 * j) as f32 / YUE2_HEAD_DIM as f32))
            .collect();
        Ok(Self {
            vae2llm_bias: weights.tensor_f32("vae2llm.bias")?,
            llm2vae_bias: weights.tensor_f32("llm2vae.bias")?,
            time_b0: weights.tensor_f32("time_embedder.mlp.0.bias")?,
            time_b2: weights.tensor_f32("time_embedder.mlp.2.bias")?,
            weights,
            ar,
            nar,
            final_norm,
            inv_freq,
            rope: RefCell::new(None),
            decode_mode: Cell::new(Yue2DecodeMode::Graph),
            ar_weights,
            nar_precision,
        })
    }

    pub fn set_decode_mode(&self, mode: Yue2DecodeMode) {
        self.decode_mode.set(mode);
    }

    pub fn decode_mode(&self) -> Yue2DecodeMode {
        self.decode_mode.get()
    }

    pub fn ar_weights(&self) -> Yue2ArWeights {
        self.ar_weights
    }

    pub fn nar_precision(&self) -> Yue2NarPrecision {
        self.nar_precision
    }

    fn norms(&self, set: Yue2Set) -> &SetNorms {
        match set {
            Yue2Set::Ar => &self.ar,
            Yue2Set::Nar => &self.nar,
        }
    }

    fn ensure(&self, name: &str, n: usize, k: usize) -> Result<()> {
        let w = &self.weights;
        gpu_weight_cache_ensure(YUE2_NAMESPACE, name, GGML_BF16, n, k, false, || {
            w.tensor_bytes(name).map_err(|e| e.to_string())
        })
        .map_err(dev("weight upload"))
    }

    /// `x @ [w_0; w_1; ...]^T` (+ bias): one GEMM over concatenated parts.
    fn linear(&self, x: &GpuTensor, parts: &[(&str, usize)], bias: &[f32]) -> Result<GpuTensor> {
        let mut list = Vec::with_capacity(parts.len());
        for &(name, n) in parts {
            self.ensure(name, n, x.cols())?;
            list.push(GpuLinearPart { bt_ggml_type: GGML_BF16, n, cache_key: name, bytes: &[] });
        }
        gpu_linear_nt_cached_bf16_f32acc(x, YUE2_NAMESPACE, &list, bias).map_err(dev("linear"))
    }

    fn linear_keyed<const N: usize>(
        &self,
        x: &GpuTensor,
        parts: [(&WeightName, usize); N],
        decode: bool,
    ) -> Result<GpuTensor> {
        let q8 = decode && self.ar_weights == Yue2ArWeights::Q8;
        if q8 {
            self.ensure_q8_parts(parts, x.cols())?;
            let part = GpuLinearPart {
                bt_ggml_type: GGML_Q8_0,
                n: parts.iter().map(|(_, n)| *n).sum(),
                cache_key: &parts[0].0.q8_key,
                bytes: &[],
            };
            return gpu_linear_nt_cached_q8_0_f32acc_keyed(x, &[part]).map_err(dev("q8 linear"));
        }
        for (name, n) in parts {
            gpu_weight_cache_ensure_bf16_keyed(&name.key, n, x.cols(), || {
                self.weights.tensor_bytes(&name.tensor).map_err(|e| e.to_string())
            })
            .map_err(dev("weight upload"))?;
        }
        let list = parts.map(|(name, n)| GpuLinearPart {
            bt_ggml_type: GGML_BF16,
            n,
            cache_key: &name.key,
            bytes: &[],
        });
        gpu_linear_nt_cached_bf16_f32acc_keyed(x, &list).map_err(dev("linear"))
    }

    fn ensure_q8_parts<const N: usize>(&self, parts: [(&WeightName, usize); N], k: usize) -> Result<()> {
        for (name, n) in parts {
            gpu_weight_cache_ensure_bf16_keyed(&name.key, n, k, || {
                self.weights.tensor_bytes(&name.tensor).map_err(|e| e.to_string())
            }).map_err(dev("weight upload"))?;
        }
        let list = parts.map(|(name, n)| GpuLinearPart {
            bt_ggml_type: GGML_BF16, n, cache_key: name.key.as_str(), bytes: &[],
        });
        gpu_weight_cache_ensure_q8_0_concat_bf16_keyed(&parts[0].0.q8_key, &list, k)
            .map_err(dev("packed q8 weight quantization"))
    }

    fn decode_add_rms(
        &self, input: &GpuTensor, weight: &WeightName, residual: &GpuTensor,
        norm_key: &str, scale: &[f32],
    ) -> Result<(GpuTensor, GpuTensor)> {
        let update = self.linear_keyed(input, [(weight, YUE2_HIDDEN)], true)?;
        gpu_add_rms_norm_keyed(residual, &update, norm_key, scale, YUE2_RMS_EPS)
            .map_err(dev("linear add rms"))
    }

    fn decode_swiglu(&self, input: &GpuTensor, names: &SetNames) -> Result<GpuTensor> {
        let parts = [(&names.up, YUE2_FF), (&names.gate, YUE2_FF)];
        if self.ar_weights == Yue2ArWeights::Q8 {
            self.ensure_q8_parts(parts, input.cols())?;
            gpu_linear_nt_cached_q8_0_swiglu_keyed(input, &names.up.q8_key, YUE2_FF)
                .map_err(dev("q8 linear swiglu"))
        } else {
            let up_gate = self.linear_keyed(input, parts, true)?;
            gpu_swiglu_value_gate(&up_gate).map_err(dev("swiglu"))
        }
    }

    fn rms_keyed(&self, x: &GpuTensor, group: usize, key: &str, scale: &[f32]) -> Result<GpuTensor> {
        gpu_rms_norm_mul_keyed(x, group, key, scale, YUE2_RMS_EPS).map_err(dev("rms"))
    }

    /// The same f32 trigonometry as prefill, built once for the full
    /// context. A decode step only creates device views of one table row.
    pub fn rope_tables(&self, start: usize, len: usize) -> Result<(GpuTensor, GpuTensor)> {
        if start.checked_add(len).filter(|&end| end <= YUE2_CONTEXT).is_none() {
            return Err(DiffusionError::model("yue2 RoPE range exceeds context"));
        }
        let half = YUE2_HEAD_DIM / 2;
        let mut tables = self.rope.borrow_mut();
        if tables.is_none() {
            let mut cos = vec![0.0f32; YUE2_CONTEXT * half];
            let mut sin = vec![0.0f32; YUE2_CONTEXT * half];
            for i in 0..YUE2_CONTEXT {
                let pos = i as f32;
                for j in 0..half {
                    let a = pos * self.inv_freq[j];
                    cos[i * half + j] = a.cos();
                    sin[i * half + j] = a.sin();
                }
            }
            *tables = Some((
                gpu_upload(&cos, YUE2_CONTEXT, half).map_err(dev("rope"))?,
                gpu_upload(&sin, YUE2_CONTEXT, half).map_err(dev("rope"))?,
            ));
        }
        let (cos, sin) = tables.as_ref().expect("RoPE tables initialized");
        Ok((
            gpu_slice_rows(cos, start, len).map_err(dev("rope view"))?,
            gpu_slice_rows(sin, start, len).map_err(dev("rope view"))?,
        ))
    }

    /// Input norm, fused q|k|v projection, q/k RMSNorm per head, RoPE.
    fn qkv(
        &self,
        set: Yue2Set,
        layer: usize,
        x: &GpuTensor,
        cos: &GpuTensor,
        sin: &GpuTensor,
        decode: bool,
    ) -> Result<(GpuTensor, GpuTensor, GpuTensor)> {
        let decode = decode && set == Yue2Set::Ar;
        let norms = self.norms(set);
        let names = &norms.names[layer];
        let normed = self.rms_keyed(x, YUE2_HIDDEN, &names.input_norm.key, &norms.input[layer])?;
        let qkv = self.linear_keyed(&normed, [(&names.q, Q_INNER), (&names.k, KV_INNER), (&names.v, KV_INNER)], decode)?;
        drop(normed);
        let q = gpu_slice_cols(&qkv, 0, Q_INNER).map_err(dev("split q"))?;
        let k = gpu_slice_cols(&qkv, Q_INNER, KV_INNER).map_err(dev("split k"))?;
        let v = gpu_slice_cols(&qkv, Q_INNER + KV_INNER, KV_INNER).map_err(dev("split v"))?;
        drop(qkv);
        let q = self.rms_keyed(&q, YUE2_HEAD_DIM, &names.q_norm.key, &norms.q[layer])?;
        let k = self.rms_keyed(&k, YUE2_HEAD_DIM, &names.k_norm.key, &norms.k[layer])?;
        let half = YUE2_HEAD_DIM / 2;
        let q = gpu_rope_half(&q, YUE2_HEADS, half, cos, sin).map_err(dev("rope q"))?;
        let k = gpu_rope_half(&k, YUE2_KV_HEADS, half, cos, sin).map_err(dev("rope k"))?;
        Ok((q, k, v))
    }

    /// o_proj + residual, post norm, SwiGLU MLP + residual.
    fn tail(&self, set: Yue2Set, layer: usize, x: &GpuTensor, attn: &GpuTensor, decode: bool) -> Result<GpuTensor> {
        let decode = decode && set == Yue2Set::Ar;
        let norms = self.norms(set);
        let names = &norms.names[layer];
        let o = self.linear_keyed(attn, [(&names.o, YUE2_HIDDEN)], decode)?;
        let x = gpu_add(x, &o).map_err(dev("residual"))?;
        drop(o);
        let normed = self.rms_keyed(&x, YUE2_HIDDEN, &names.post_norm.key, &norms.post[layer])?;
        let up_gate = self.linear_keyed(&normed, [(&names.up, YUE2_FF), (&names.gate, YUE2_FF)], decode)?;
        drop(normed);
        let ff = gpu_swiglu_value_gate(&up_gate).map_err(dev("swiglu"))?;
        drop(up_gate);
        let down = self.linear_keyed(&ff, [(&names.down, YUE2_HIDDEN)], decode)?;
        gpu_add(&x, &down).map_err(dev("residual"))
    }

    fn embed_rows(&self, ids: &[u32]) -> Result<GpuTensor> {
        if ids.is_empty() || ids.iter().any(|&id| id as usize >= YUE2_VOCAB) {
            return Err(DiffusionError::model("yue2 embedding ids empty or out of range"));
        }
        self.ensure_embed()?;
        let indices = gpu_upload_u32(ids).map_err(dev("embed ids"))?;
        gpu_gather_rows_cached_bf16(&indices, EMBED_KEY, YUE2_VOCAB, YUE2_HIDDEN)
            .map_err(dev("embed gather"))
    }

    fn ensure_embed(&self) -> Result<()> {
        gpu_weight_cache_ensure_bf16_keyed(EMBED_KEY, YUE2_VOCAB, YUE2_HIDDEN, || {
            self.weights.tensor_bytes("model.embed_tokens.weight").map_err(|e| e.to_string())
        })
        .map_err(dev("embed upload"))
    }

    /// AR forward of `ids` at positions `start..` with causal attention over
    /// the cache (which must hold exactly `start` rows). Appends K/V.
    /// Returns the pre-final-norm hidden `[n, H]`. Prefill and host decode
    /// always use this per-op path; graph decode uses `ar_decode_device`.
    pub fn ar_forward(&self, cache: &mut Yue2KvCache, ids: &[u32]) -> Result<GpuTensor> {
        if cache.ar_weights != self.ar_weights {
            return Err(DiffusionError::model("yue2 AR cache/model precision mismatch"));
        }
        let start = cache.len;
        let n = ids.len();
        if n == 0 {
            return Err(DiffusionError::workflow("yue2 AR forward: empty ids"));
        }
        if start.checked_add(n).is_none_or(|end| end > cache.capacity || end > YUE2_CONTEXT) {
            return Err(DiffusionError::workflow("yue2 AR forward exceeds cache/context"));
        }
        let (cos, sin) = self.rope_tables(start, n)?;
        let scale = 1.0 / (YUE2_HEAD_DIM as f32).sqrt();
        let mut x = self.embed_rows(ids)?;
        let decode = start > 0 && n == 1;
        for layer in 0..YUE2_LAYERS {
            let (q, k, v) = self.qkv(Yue2Set::Ar, layer, &x, &cos, &sin, decode)?;
            cache.write(layer, start, &k, &v)?;
            let attn = if start == 0 {
                gpu_attention_gqa_flash_bf16(&q, &k, &v, YUE2_HEADS, YUE2_KV_HEADS, scale, true,
                    GpuAttentionPrecision::Bf16F32)
                    .map_err(dev("prefill attn"))?
            } else if n == 1 {
                cache.attend(layer, &q, start + 1, scale)?
            } else {
                return Err(DiffusionError::workflow(
                    "yue2 AR forward: chunked prefill after a cached prefix is not supported",
                ));
            };
            drop((q, k, v));
            x = self.tail(Yue2Set::Ar, layer, &x, &attn, decode)?;
        }
        cache.len = start + n;
        Ok(x)
    }

    /// One AR row using only persistent device inputs. Returns the residual
    /// and its final norm, letting decode fuse the last residual add with
    /// the head's norm too. This does not mutate the host cache length;
    /// the session advances it once after each actual execution/replay.
    pub fn ar_decode_device(
        &self,
        cache: &Yue2KvCache,
        token: &GpuTensor,
        position: &GpuTensor,
        sequence: &GpuTensor,
    ) -> Result<(GpuTensor, GpuTensor)> {
        if cache.ar_weights != self.ar_weights {
            return Err(DiffusionError::model("yue2 AR cache/model precision mismatch"));
        }
        self.ensure_embed()?;
        let (cos, sin) = self.rope_tables(0, YUE2_CONTEXT)?;
        let mut x = gpu_gather_rows_cached_bf16(token, EMBED_KEY, YUE2_VOCAB, YUE2_HIDDEN)
            .map_err(dev("embed gather"))?;
        let norms = &self.ar;
        let mut normed = self.rms_keyed(
            &x, YUE2_HIDDEN, &norms.names[0].input_norm.key, &norms.input[0],
        )?;
        let scale = 1.0 / (YUE2_HEAD_DIM as f32).sqrt();
        for layer in 0..YUE2_LAYERS {
            let names = &norms.names[layer];
            let qkv = self.linear_keyed(
                &normed, [(&names.q, Q_INNER), (&names.k, KV_INNER), (&names.v, KV_INNER)], true,
            )?;
            drop(normed);
            let q = match &cache.layers[layer] {
                Yue2KvLayer::F32 { k, v } => gpu_qk_rms_norm_rope_cache(
                    &qkv, &names.q_norm.key, &norms.q[layer], &names.k_norm.key, &norms.k[layer],
                    &cos, &sin, position, k, v, YUE2_HEADS, YUE2_KV_HEADS, YUE2_RMS_EPS,
                ),
                Yue2KvLayer::Bf16 { k, v } => gpu_qk_rms_norm_rope_cache_bf16(
                    &qkv, &names.q_norm.key, &norms.q[layer], &names.k_norm.key, &norms.k[layer],
                    &cos, &sin, position, k, v, YUE2_HEADS, YUE2_KV_HEADS, YUE2_RMS_EPS,
                ),
            }.map_err(dev("qk norm rope cache"))?;
            drop(qkv);
            let attn = cache.attend_device(layer, &q, sequence, scale)?;
            drop(q);
            let (residual, ff_in) = self.decode_add_rms(
                &attn, &names.o, &x, &names.post_norm.key, &norms.post[layer],
            )?;
            drop(attn);
            drop(x);
            let ff = self.decode_swiglu(&ff_in, names)?;
            drop(ff_in);
            let (key, weights) = if layer + 1 < YUE2_LAYERS {
                (norms.names[layer + 1].input_norm.key.as_str(), norms.input[layer + 1].as_slice())
            } else {
                (FINAL_NORM_KEY, self.final_norm.as_slice())
            };
            (x, normed) = self.decode_add_rms(
                &ff, &names.down, &residual, key, weights,
            )?;
        }
        Ok((x, normed))
    }

    /// Final norm + the phase's lm_head window on the LAST row of `hidden`.
    /// Returns logits for vocabulary rows `[lo, lo + n)`. `decode` must be
    /// false for prefill's first draw, even for a one-token prefix.
    pub fn head_window(&self, hidden: &GpuTensor, phase: Yue2Phase, decode: bool) -> Result<(u32, Vec<f32>)> {
        let (lo, logits) = self.head_window_device(hidden, phase, decode)?;
        Ok((lo, gpu_download(&logits).map_err(dev("logits download"))?))
    }

    /// Final norm and head projection without the host logits transfer.
    pub fn head_window_device(&self, hidden: &GpuTensor, phase: Yue2Phase, decode: bool) -> Result<(u32, GpuTensor)> {
        let last = gpu_slice_rows(hidden, hidden.rows() - 1, 1).map_err(dev("last row"))?;
        let normed = self.rms_keyed(&last, YUE2_HIDDEN, FINAL_NORM_KEY, &self.final_norm)?;
        Ok((phase.head_rows().0, self.head_from_normed(&normed, phase, decode)?))
    }

    pub fn head_from_normed(&self, normed: &GpuTensor, phase: Yue2Phase, decode: bool) -> Result<GpuTensor> {
        let (_, n) = phase.head_rows();
        let q8 = decode && self.ar_weights == Yue2ArWeights::Q8;
        self.ensure_head(phase, false)?;
        if q8 { self.ensure_head(phase, true)?; }
        let part = GpuLinearPart {
            bt_ggml_type: if q8 { GGML_Q8_0 } else { GGML_BF16 },
            n: n as usize, cache_key: Self::head_key(phase, q8), bytes: &[],
        };
        if q8 {
            gpu_linear_nt_cached_q8_0_f32acc_keyed(normed, &[part]).map_err(dev("q8 lm_head"))
        } else {
            gpu_linear_nt_cached_bf16_f32acc_keyed(normed, &[part]).map_err(dev("lm_head"))
        }
    }

    /// Every device weight the model uses, as (cache key, rows, cols).
    fn weight_list(&self) -> Vec<(String, usize, usize)> {
        let mut out = Vec::new();
        for set in [Yue2Set::Ar, Yue2Set::Nar] {
            for layer in 0..YUE2_LAYERS {
                let n = &self.norms(set).names[layer];
                out.push((n.q.tensor.clone(), Q_INNER, YUE2_HIDDEN));
                out.push((n.k.tensor.clone(), KV_INNER, YUE2_HIDDEN));
                out.push((n.v.tensor.clone(), KV_INNER, YUE2_HIDDEN));
                out.push((n.o.tensor.clone(), YUE2_HIDDEN, Q_INNER));
                out.push((n.up.tensor.clone(), YUE2_FF, YUE2_HIDDEN));
                out.push((n.gate.tensor.clone(), YUE2_FF, YUE2_HIDDEN));
                out.push((n.down.tensor.clone(), YUE2_HIDDEN, YUE2_FF));
            }
        }
        out.push(("time_embedder.mlp.0.weight".into(), YUE2_HIDDEN, YUE2_TIME_FREQ_DIM));
        out.push(("time_embedder.mlp.2.weight".into(), YUE2_HIDDEN, YUE2_HIDDEN));
        out.push(("vae2llm.weight".into(), YUE2_HIDDEN, YUE2_LATENT_DIM));
        out.push(("llm2vae.weight".into(), YUE2_LATENT_DIM, YUE2_HIDDEN));
        out
    }

    fn ar_weight_list(&self) -> Vec<(&WeightName, usize, usize)> {
        let mut out = Vec::with_capacity(YUE2_LAYERS * 7);
        for n in &self.ar.names {
            out.extend([
                (&n.q, Q_INNER, YUE2_HIDDEN),
                (&n.k, KV_INNER, YUE2_HIDDEN),
                (&n.v, KV_INNER, YUE2_HIDDEN),
                (&n.o, YUE2_HIDDEN, Q_INNER),
                (&n.up, YUE2_FF, YUE2_HIDDEN),
                (&n.gate, YUE2_FF, YUE2_HIDDEN),
                (&n.down, YUE2_HIDDEN, YUE2_FF),
            ]);
        }
        out
    }

    /// Additional resident bytes when both AR representations are kept.
    /// Does not subtract the AR KV saving (114688 bytes/capacity row/branch)
    /// or assume it reduces the NAR peak, whose cache was already bf16.
    pub fn ar_q8_extra_bytes(&self) -> u64 {
        if self.ar_weights != Yue2ArWeights::Q8 { return 0; }
        let linears: u64 = self.ar_weight_list().iter().map(|(_, n, k)| (*n * *k / 32 * 34) as u64).sum();
        let heads: u64 = [Yue2Phase::Abc, Yue2Phase::Semantic].iter()
            .map(|p| p.head_rows().1 as u64 * YUE2_HIDDEN as u64 / 32 * 34).sum();
        linears + heads
    }

    /// Upload every weight and quantize AR decode copies on device up front
    /// (no first-token stall), reporting resident bytes via
    /// `progress(done_bytes, total_bytes)`.
    pub fn upload_all(&self, progress: &mut dyn FnMut(u64, u64) -> Result<()>) -> Result<()> {
        let list = self.weight_list();
        let heads = [Yue2Phase::Abc, Yue2Phase::Semantic];
        let head_bytes: u64 = heads.iter().map(|p| p.head_rows().1 as u64 * YUE2_HIDDEN as u64 * 2).sum();
        let embed_bytes = (YUE2_VOCAB * YUE2_HIDDEN * 2) as u64;
        let total: u64 = list.iter().map(|(_, n, k)| (*n * *k * 2) as u64).sum::<u64>()
            + head_bytes + embed_bytes + self.ar_q8_extra_bytes();
        let mut done = 0u64;
        progress(done, total)?;
        self.ensure_embed()?;
        done += embed_bytes;
        progress(done, total)?;
        for (name, n, k) in &list {
            self.ensure(name, *n, *k)?;
            done += (*n * *k * 2) as u64;
            progress(done, total)?;
        }
        for phase in heads {
            self.ensure_head(phase, false)?;
            done += phase.head_rows().1 as u64 * YUE2_HIDDEN as u64 * 2;
            progress(done, total)?;
        }
        if self.ar_weights == Yue2ArWeights::Q8 {
            // Retain every bf16 AR weight: each later acoustic chunk needs
            // another AR prefill. Only these additional keys serve decode.
            for names in &self.ar.names {
                self.ensure_q8_parts(
                    [(&names.q, Q_INNER), (&names.k, KV_INNER), (&names.v, KV_INNER)], YUE2_HIDDEN,
                )?;
                done += ((Q_INNER + 2 * KV_INNER) * YUE2_HIDDEN / 32 * 34) as u64;
                progress(done, total)?;
                self.ensure_q8_parts([(&names.up, YUE2_FF), (&names.gate, YUE2_FF)], YUE2_HIDDEN)?;
                done += (2 * YUE2_FF * YUE2_HIDDEN / 32 * 34) as u64;
                progress(done, total)?;
                for (name, k) in [(&names.o, Q_INNER), (&names.down, YUE2_FF)] {
                    self.ensure_q8_parts([(name, YUE2_HIDDEN)], k)?;
                    done += (YUE2_HIDDEN * k / 32 * 34) as u64;
                    progress(done, total)?;
                }
            }
            for phase in heads {
                self.ensure_head(phase, true)?;
                done += phase.head_rows().1 as u64 * YUE2_HIDDEN as u64 / 32 * 34;
                progress(done, total)?;
            }
        }
        Ok(())
    }

    fn head_key(phase: Yue2Phase, q8: bool) -> &'static str {
        match (phase, q8) {
            (Yue2Phase::Abc, false) => "yue2::lm_head.weight#abc",
            (Yue2Phase::Semantic, false) => "yue2::lm_head.weight#semantic",
            (Yue2Phase::Abc, true) => "yue2::lm_head.weight#abc::q8_0",
            (Yue2Phase::Semantic, true) => "yue2::lm_head.weight#semantic::q8_0",
        }
    }

    fn ensure_head(&self, phase: Yue2Phase, q8: bool) -> Result<()> {
        let (lo, n) = phase.head_rows();
        if q8 {
            gpu_weight_cache_ensure_q8_0_from_bf16_keyed(
                Self::head_key(phase, true), Self::head_key(phase, false), n as usize, YUE2_HIDDEN,
            ).map_err(dev("q8 lm_head quantization"))
        } else {
            gpu_weight_cache_ensure_bf16_keyed(Self::head_key(phase, false), n as usize, YUE2_HIDDEN, || {
                self.weights.tensor_row_range_bytes("lm_head.weight", lo as u64, n as u64)
                    .map_err(|e| e.to_string())
            }).map_err(dev("lm_head upload"))
        }
    }

    /// Release every cached device weight of the model.
    pub fn evict(&self) -> Result<usize> {
        drop(self.rope.borrow_mut().take());
        gpu_weight_cache_evict_prefix(&format!("{YUE2_NAMESPACE}::")).map_err(dev("evict"))
    }

    // ---- NAR ---------------------------------------------------------------

    /// Time embedding row for one raw timestep (reference `_shift_t_value`
    /// in bf16, sinusoid cos|sin, MLP).
    fn time_row(&self, raw_t: f32) -> Result<GpuTensor> {
        let t = bf16_round(raw_t);
        let sig = bf16_round(1.0 / (1.0 + (-t).exp()));
        let shifted = bf16_round(YUE2_TIMESTEP_SHIFT * sig / (1.0 + (YUE2_TIMESTEP_SHIFT - 1.0) * sig));
        let half = YUE2_TIME_FREQ_DIM / 2;
        let mut emb = vec![0.0f32; YUE2_TIME_FREQ_DIM];
        for j in 0..half {
            let freq = (-(10000.0f32).ln() * j as f32 / half as f32).exp();
            let a = shifted * freq;
            emb[j] = bf16_round(a.cos());
            emb[half + j] = bf16_round(a.sin());
        }
        let x = gpu_upload(&emb, 1, YUE2_TIME_FREQ_DIM).map_err(dev("time upload"))?;
        let h = self.linear(&x, &[("time_embedder.mlp.0.weight", YUE2_HIDDEN)], &self.time_b0)?;
        let h = gpu_silu(&h).map_err(dev("time silu"))?;
        self.linear(&h, &[("time_embedder.mlp.2.weight", YUE2_HIDDEN)], &self.time_b2)
    }
}

/// Prepared projections. Cache lookup and operand conversion use only
/// caller-owned storage during evaluation; even the qualified keys are fixed.
struct NarLinear<const N: usize> {
    keys: [String; N],
    widths: [usize; N],
    fused_fast: bool,
}

impl<const N: usize> NarLinear<N> {
    fn new(
        model: &Yue2Model, parts: [(&str, usize); N], input: usize, precision: GpuLinearPrecision,
    ) -> Result<Self> {
        let mut keys = parts.map(|(name, _)| format!("{YUE2_NAMESPACE}::{name}"));
        let mut widths = parts.map(|(_, width)| width);
        let fused_fast = precision != GpuLinearPrecision::Bf16F32;
        if fused_fast {
            // One resident matrix for Q|K|V or up|gate, uploaded/converted
            // once. Do not retain individual bf16 shadows for the NAR set.
            let total = widths.iter().sum();
            keys[0] = format!("{}::fused::{precision:?}", keys[0]);
            gpu_weight_cache_ensure_bf16_precision_keyed(&keys[0], total, input, precision, || {
                let mut bytes = Vec::with_capacity(total * input * 2);
                for &(name, _) in &parts {
                    bytes.extend(model.weights.tensor_bytes(name).map_err(|e| e.to_string())?);
                }
                Ok(bytes)
            }).map_err(dev("nar fast weights"))?;
            widths[0] = total;
        } else {
            for &(name, width) in &parts { model.ensure(name, width, input)?; }
        }
        Ok(Self { keys, widths, fused_fast })
    }

    fn run(
        &self, x: &GpuTensor, scratch: &NarLinearScratch, bias: Option<&GpuTensor>, out: &GpuTensor,
    ) -> Result<()> {
        let parts: [(&str, usize); N] = std::array::from_fn(|i| (self.keys[i].as_str(), self.widths[i]));
        if self.fused_fast {
            gpu_linear_nt_cached_precision_into(x,
                scratch.fast.as_ref().ok_or_else(|| DiffusionError::model("missing NAR fast scratch"))?,
                &parts[..1], bias, out).map_err(dev("nar fast linear"))
        } else {
            gpu_linear_nt_cached_bf16_f32acc_into(x, &scratch.bf16, &parts, bias, out)
                .map_err(dev("nar linear"))
        }
    }
}

struct NarLayer {
    input_norm: GpuTensor,
    post_norm: GpuTensor,
    q_norm: GpuTensor,
    k_norm: GpuTensor,
    qkv: NarLinear<3>,
    out: NarLinear<1>,
    up_gate: NarLinear<2>,
    down: NarLinear<1>,
}

impl NarLayer {
    fn new(model: &Yue2Model, layer: usize) -> Result<Self> {
        let norms = &model.nar;
        let names = &norms.names[layer];
        let precision = model.nar_precision.linear();
        let upload = |values: &[f32]| gpu_upload(values, 1, values.len()).map_err(dev("nar norm"));
        Ok(Self {
            input_norm: upload(&norms.input[layer])?,
            post_norm: upload(&norms.post[layer])?,
            q_norm: upload(&norms.q[layer])?,
            k_norm: upload(&norms.k[layer])?,
            qkv: NarLinear::new(model, [
                (&names.q.tensor, Q_INNER),
                (&names.k.tensor, KV_INNER),
                (&names.v.tensor, KV_INNER),
            ], YUE2_HIDDEN, precision)?,
            out: NarLinear::new(model, [(&names.o.tensor, YUE2_HIDDEN)], Q_INNER, precision)?,
            up_gate: NarLinear::new(model, [
                (&names.up.tensor, YUE2_FF),
                (&names.gate.tensor, YUE2_FF),
            ], YUE2_HIDDEN, precision)?,
            down: NarLinear::new(model, [(&names.down.tensor, YUE2_HIDDEN)], YUE2_FF, precision)?,
        })
    }
}

struct NarLinearScratch {
    // The vae2llm/llm2vae maps stay bf16 even with fast NAR layers.
    bf16: GpuBf16Buf,
    fast: Option<GpuLinearScratch>,
}

/// Shared by all NAR layers and evaluations. Nothing here grows in velocity.
struct NarWorkspace {
    input: GpuTensor,
    input_content: GpuTensor,
    x: GpuTensor,
    normed: GpuTensor,
    normed_content: GpuTensor,
    update: GpuTensor,
    qkv: GpuTensor,
    q: GpuBf16Buf,
    attn: GpuTensor,
    up_gate: GpuTensor,
    ff: GpuTensor,
    linear_scratch: NarLinearScratch,
    velocity: GpuTensor,
}

impl NarWorkspace {
    fn new(frames: usize, precision: Yue2NarPrecision) -> Result<Self> {
        let rows = frames + 2;
        let alloc = |cols| gpu_alloc_f32(rows, cols).map_err(dev("nar workspace"));
        // Only the two guard rows stay zero. Every content row is overwritten
        // before use, while the views keep the padded allocations alive.
        let input = gpu_upload(&vec![0.0; rows * YUE2_LATENT_DIM], rows, YUE2_LATENT_DIM)
            .map_err(dev("nar input"))?;
        let input_content = gpu_slice_rows(&input, 1, frames).map_err(dev("nar input view"))?;
        let normed = alloc(YUE2_HIDDEN)?;
        let normed_content = gpu_slice_rows(&normed, 1, frames).map_err(dev("nar norm view"))?;
        Ok(Self {
            input, input_content, normed, normed_content,
            x: alloc(YUE2_HIDDEN)?,
            update: alloc(YUE2_HIDDEN)?,
            qkv: alloc(Q_INNER + 2 * KV_INNER)?,
            q: gpu_alloc_bf16(rows, Q_INNER).map_err(dev("nar q"))?,
            attn: alloc(Q_INNER)?,
            up_gate: alloc(2 * YUE2_FF)?,
            ff: alloc(YUE2_FF)?,
            linear_scratch: NarLinearScratch {
                bf16: gpu_alloc_bf16(rows, if precision == Yue2NarPrecision::Bf16 {
                    YUE2_FF
                } else { YUE2_HIDDEN }).map_err(dev("nar linear scratch"))?,
                fast: if precision == Yue2NarPrecision::Fast {
                    Some(gpu_linear_scratch(rows, YUE2_FF, 2 * YUE2_FF, precision.linear())
                        .map_err(dev("nar fast scratch"))?)
                } else { None },
            },
            velocity: gpu_alloc_f32(frames, YUE2_LATENT_DIM).map_err(dev("nar velocity"))?,
        })
    }
}

/// One acoustic chunk: bf16 [prefix; NAR] K/V per layer, prepared timesteps,
/// and one reusable evaluation workspace. Only NAR K/V rows change in eval.
pub struct Yue2NarChunk {
    attention_precision: GpuAttentionPrecision,
    k: Vec<GpuBf16Buf>,
    v: Vec<GpuBf16Buf>,
    visible: usize,
    cos: GpuTensor,
    sin: GpuTensor,
    pos_emb: GpuTensor,
    time_rows: Vec<GpuTensor>,
    layers: Vec<NarLayer>,
    final_norm: GpuTensor,
    vae2llm: NarLinear<1>,
    llm2vae: NarLinear<1>,
    vae2llm_bias: GpuTensor,
    llm2vae_bias: GpuTensor,
    work: NarWorkspace,
    host_state: Vec<f32>,
}

impl Yue2NarChunk {
    /// `ar_tokens` = prefix + codec ids (offset added) + MUSIC_END; `frames`
    /// = codec count of this chunk (NAR tokens = frames + 2). Timesteps are
    /// prepared once, using the original host sinusoid and bf16 arithmetic.
    /// Includes NAR weight uploads so subsequent evaluations never upload
    /// weights, positions, norms, or time embeddings.
    pub fn prefill(
        model: &Yue2Model, ar_tokens: &[u32], frames: usize, raw_times: &[f32],
    ) -> Result<Self> {
        if ar_tokens.is_empty() || frames == 0 || raw_times.is_empty() {
            return Err(DiffusionError::model("yue2 NAR empty prefix, state, or schedule"));
        }
        // The preceding semantic session (or acoustic chunk) has ended.
        // Release its idle cache blocks before building this chunk's prefix.
        gpu_runtime_trim().map_err(dev("nar phase trim"))?;
        let nar_len = frames + 2;
        let total = ar_tokens.len() + nar_len;
        let mut kcache = Vec::with_capacity(YUE2_LAYERS);
        let mut vcache = Vec::with_capacity(YUE2_LAYERS);
        {
            // Same AR prefill arithmetic, but retain each prefix immediately
            // in the storage consumed by FA2. Never allocate an f32 AR cache.
            let (cos, sin) = model.rope_tables(0, ar_tokens.len())?;
            let mut x = model.embed_rows(ar_tokens)?;
            let scale = 1.0 / (YUE2_HEAD_DIM as f32).sqrt();
            for layer in 0..YUE2_LAYERS {
                let (q, k, v) = model.qkv(Yue2Set::Ar, layer, &x, &cos, &sin, false)?;
                let pk = gpu_alloc_bf16(total, KV_INNER).map_err(dev("nar prefix k"))?;
                let pv = gpu_alloc_bf16(total, KV_INNER).map_err(dev("nar prefix v"))?;
                gpu_copy_f32_to_bf16_into(&k, &pk, 0).map_err(dev("nar prefix k store"))?;
                gpu_copy_f32_to_bf16_into(&v, &pv, 0).map_err(dev("nar prefix v store"))?;
                kcache.push(pk);
                vcache.push(pv);
                let attn = gpu_attention_gqa_flash_bf16(
                    &q, &k, &v, YUE2_HEADS, YUE2_KV_HEADS, scale, true,
                    GpuAttentionPrecision::Bf16F32,
                ).map_err(dev("nar prefix attention"))?;
                drop((q, k, v));
                x = model.tail(Yue2Set::Ar, layer, &x, &attn, false)?;
            }
        }
        // Prefix-sized MLP buffers and dense conversion scratch must not
        // coexist with the persistent NAR workspace (especially at long ctx).
        gpu_runtime_trim().map_err(dev("nar prefill trim"))?;
        let (cos, sin) = model.rope_tables(ar_tokens.len(), nar_len)?;
        let bytes = model
            .weights
            .tensor_row_range_bytes("latent_pos_embed.pe", 0, nar_len as u64)?;
        let pos_emb = gpu_upload(&bf16_to_f32(&bytes), nar_len, YUE2_HIDDEN).map_err(dev("pe"))?;
        let layers = (0..YUE2_LAYERS).map(|i| NarLayer::new(model, i)).collect::<Result<Vec<_>>>()?;
        let time_rows = raw_times.iter().map(|&t| model.time_row(t)).collect::<Result<Vec<_>>>()?;
        let result = Self {
            attention_precision: model.nar_precision.attention(),
            k: kcache, v: vcache, visible: ar_tokens.len(), cos, sin, pos_emb,
            layers, time_rows,
            final_norm: gpu_upload(&model.final_norm, 1, YUE2_HIDDEN).map_err(dev("nar final norm"))?,
            vae2llm: NarLinear::new(model, [("vae2llm.weight", YUE2_HIDDEN)], YUE2_LATENT_DIM, GpuLinearPrecision::Bf16F32)?,
            llm2vae: NarLinear::new(model, [("llm2vae.weight", YUE2_LATENT_DIM)], YUE2_HIDDEN, GpuLinearPrecision::Bf16F32)?,
            vae2llm_bias: gpu_upload(&model.vae2llm_bias, 1, YUE2_HIDDEN).map_err(dev("nar input bias"))?,
            llm2vae_bias: gpu_upload(&model.llm2vae_bias, 1, YUE2_LATENT_DIM).map_err(dev("nar output bias"))?,
            work: NarWorkspace::new(frames, model.nar_precision)?,
            host_state: vec![0.0; frames * YUE2_LATENT_DIM],
        };
        gpu_runtime_trim().map_err(dev("nar setup trim"))?;
        Ok(result)
    }

    /// Host convenience boundary: one state upload and one velocity download.
    /// `time_index` selects a timestep supplied to prefill.
    pub fn velocity(&mut self, state: &[f32], time_index: usize) -> Result<Vec<f32>> {
        if state.len() != self.host_state.len() {
            return Err(DiffusionError::model("yue2 NAR state shape"));
        }
        for (dst, &v) in self.host_state.iter_mut().zip(state) {
            *dst = bf16_round(v);
        }
        gpu_upload_into(&self.work.input_content, &self.host_state).map_err(dev("state upload"))?;
        self.eval(time_index)?;
        gpu_download(&self.work.velocity).map_err(dev("velocity download"))
    }

    /// Device ODE boundary. State must contain bf16-rounded f32 values.
    /// Returned storage is reused by the next evaluation; consume it first.
    pub fn velocity_device(&self, state: &GpuTensor, time_index: usize) -> Result<&GpuTensor> {
        gpu_copy_into(state, &self.work.input_content).map_err(dev("nar state copy"))?;
        self.eval(time_index)?;
        Ok(&self.work.velocity)
    }

    fn eval(&self, time_index: usize) -> Result<()> {
        let time = self.time_rows.get(time_index)
            .ok_or_else(|| DiffusionError::model("yue2 NAR timestep index"))?;
        let w = &self.work;
        self.vae2llm.run(&w.input, &w.linear_scratch, Some(&self.vae2llm_bias), &w.x)?;
        gpu_add_row_position_into(&w.x, time, &self.pos_emb, &w.x).map_err(dev("nar embedding"))?;
        let scale = 1.0 / (YUE2_HEAD_DIM as f32).sqrt();
        for (index, layer) in self.layers.iter().enumerate() {
            // Fold the preceding MLP residual into this layer's input norm.
            let update = if index == 0 { None } else { Some(&w.update) };
            gpu_add_rms_norm_into(&w.x, update, &layer.input_norm, &w.x, &w.normed, YUE2_RMS_EPS)
                .map_err(dev("nar input norm"))?;
            layer.qkv.run(&w.normed, &w.linear_scratch, None, &w.qkv)?;
            gpu_qkv_rms_rope_bf16_into(
                &w.qkv, &layer.q_norm, &layer.k_norm, &self.cos, &self.sin,
                &w.q, &self.k[index], &self.v[index], YUE2_HEAD_DIM, self.visible, YUE2_RMS_EPS,
            ).map_err(dev("nar qkv norm rope"))?;
            gpu_attention_gqa_flash_bf16_buf_into(
                &w.q, &self.k[index], &self.v[index], &w.attn,
                YUE2_HEADS, YUE2_KV_HEADS, scale, false,
                self.attention_precision,
            ).map_err(dev("nar attention"))?;
            layer.out.run(&w.attn, &w.linear_scratch, None, &w.update)?;
            gpu_add_rms_norm_into(&w.x, Some(&w.update), &layer.post_norm, &w.x, &w.normed, YUE2_RMS_EPS)
                .map_err(dev("nar post norm"))?;
            layer.up_gate.run(&w.normed, &w.linear_scratch, None, &w.up_gate)?;
            gpu_swiglu_value_gate_into(&w.up_gate, &w.ff).map_err(dev("nar swiglu"))?;
            layer.down.run(&w.ff, &w.linear_scratch, None, &w.update)?;
        }
        gpu_add_rms_norm_into(&w.x, Some(&w.update), &self.final_norm, &w.x, &w.normed, YUE2_RMS_EPS)
            .map_err(dev("nar final norm"))?;
        self.llm2vae.run(&w.normed_content, &w.linear_scratch, Some(&self.llm2vae_bias), &w.velocity)
    }
}
