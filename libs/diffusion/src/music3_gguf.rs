//! Host / Metal path for the official audio.cpp Music3 Q4 pack.
//!
//! Reuses ggml Metal `mul_mm` / `mul_mv` (Q4_0, BF16, F16, F32) via
//! [`Music3GgufFile::linear_nt`]. No CUDA, no new shaders.
//! Official Python ModularPipeline names and dims.

use crate::music3::{
    MUSIC3_AR_CFG, MUSIC3_AUDIO_CODE_OFFSET, MUSIC3_AUDIO_END_TOKEN_ID, MUSIC3_AUDIO_VOCAB,
    MUSIC3_LM_FF, MUSIC3_LM_HEAD_DIM, MUSIC3_LM_HEADS, MUSIC3_LM_HIDDEN, MUSIC3_LM_KV_HEADS,
    MUSIC3_LM_LAYERS, MUSIC3_LM_RMS_EPS, MUSIC3_LM_ROPE_THETA, MUSIC3_LM_VOCAB, MUSIC3_NUM_CODEBOOKS,
    MUSIC3_RVQ_FF, MUSIC3_RVQ_HEADS, MUSIC3_RVQ_HIDDEN, MUSIC3_RVQ_LAYERS, MUSIC3_RVQ_MAX_POS,
    MUSIC3_SEMANTIC_VOCAB,
};
use crate::music3_ar::{sample_top_k, TorchPhilox};
use crate::music3_quant::Music3GgufFile;
use crate::sa3::rms_norm_rows;
use crate::{DiffusionError, Result};
use std::cell::RefCell;

#[derive(Clone, Debug)]
pub struct FiniteStats {
    pub count: usize,
    pub finite: usize,
    pub nan: usize,
    pub inf: usize,
    pub min: f32,
    pub max: f32,
    pub mean: f32,
    pub absmax: f32,
}

impl FiniteStats {
    pub fn of(xs: &[f32]) -> Self {
        let mut finite = 0usize;
        let mut nan = 0usize;
        let mut inf = 0usize;
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        let mut absmax = 0f32;
        let mut sum = 0f64;
        for &v in xs {
            if v.is_nan() {
                nan += 1;
                continue;
            }
            if v.is_infinite() {
                inf += 1;
                continue;
            }
            finite += 1;
            if v < min {
                min = v;
            }
            if v > max {
                max = v;
            }
            absmax = absmax.max(v.abs());
            sum += v as f64;
        }
        if finite == 0 {
            min = 0.0;
            max = 0.0;
        }
        Self {
            count: xs.len(),
            finite,
            nan,
            inf,
            min,
            max,
            mean: if finite == 0 {
                0.0
            } else {
                (sum / finite as f64) as f32
            },
            absmax,
        }
    }
}

impl std::fmt::Display for FiniteStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "n={} finite={} nan={} inf={} min={:.6e} max={:.6e} mean={:.6e} absmax={:.6e}",
            self.count, self.finite, self.nan, self.inf, self.min, self.max, self.mean, self.absmax
        )
    }
}

fn silu(x: f32) -> f32 {
    x / (1.0 + (-x).exp())
}

fn swiglu(up: &[f32], gate: &[f32]) -> Vec<f32> {
    up.iter()
        .zip(gate.iter())
        .map(|(u, g)| *u * silu(*g))
        .collect()
}

fn add_inplace(dst: &mut [f32], src: &[f32]) {
    for (d, s) in dst.iter_mut().zip(src.iter()) {
        *d += *s;
    }
}

/// Wall-clock accumulators for the AR/RVQ hot loops. Printed once per
/// generate in the `ar-break` line; ~ns overhead per sample.
pub(crate) mod perf {
    use std::sync::atomic::{AtomicU64, Ordering};

    pub static AR_QKV: AtomicU64 = AtomicU64::new(0);
    pub static AR_ATTN: AtomicU64 = AtomicU64::new(0);
    pub static AR_O: AtomicU64 = AtomicU64::new(0);
    pub static AR_UPGATE: AtomicU64 = AtomicU64::new(0);
    pub static AR_DOWN: AtomicU64 = AtomicU64::new(0);
    pub static RVQ_PREFILL: AtomicU64 = AtomicU64::new(0);
    pub static RVQ_STEP: AtomicU64 = AtomicU64::new(0);
    pub static RVQ_HEAD: AtomicU64 = AtomicU64::new(0);
    pub static RVQ_EMBED: AtomicU64 = AtomicU64::new(0);
    pub static PF_QKV: AtomicU64 = AtomicU64::new(0);
    pub static PF_ATTN: AtomicU64 = AtomicU64::new(0);
    pub static PF_LINEAR: AtomicU64 = AtomicU64::new(0);
    // Resident decode split (the lm[..] slots above stay 0 when resident wins).
    pub static RES_PRE: AtomicU64 = AtomicU64::new(0);
    pub static RES_ATTN: AtomicU64 = AtomicU64::new(0);
    pub static RES_POST: AtomicU64 = AtomicU64::new(0);
    pub static RES_FINAL: AtomicU64 = AtomicU64::new(0);

    pub fn add(slot: &AtomicU64, t0: std::time::Instant) {
        slot.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }

    pub fn secs(slot: &AtomicU64) -> f32 {
        slot.load(Ordering::Relaxed) as f32 / 1e9
    }
}

/// RVQ whole-layer resident decode is the default; `MAKEPAD_MUSIC3_RVQ_HOST`
/// (or the global CPU switch) restores per-GEMM host elementwise.
pub(crate) fn rvq_resident_enabled() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var_os("MAKEPAD_MUSIC3_RVQ_HOST").is_none()
            && std::env::var_os("MAKEPAD_MUSIC3_CPU_LINEAR").is_none()
    })
}

pub(crate) fn repeat_kv(
    x: &[f32],
    tokens: usize,
    kv_heads: usize,
    group: usize,
    head_dim: usize,
) -> Vec<f32> {
    let out_heads = kv_heads * group;
    let mut out = vec![0f32; tokens * out_heads * head_dim];
    for t in 0..tokens {
        for h in 0..kv_heads {
            let src = &x[(t * kv_heads + h) * head_dim..(t * kv_heads + h + 1) * head_dim];
            for g in 0..group {
                let dst_h = h * group + g;
                out[(t * out_heads + dst_h) * head_dim..(t * out_heads + dst_h + 1) * head_dim]
                    .copy_from_slice(src);
            }
        }
    }
    out
}

fn apply_rope_half(
    x: &mut [f32],
    tokens: usize,
    heads: usize,
    head_dim: usize,
    cos: &[f32],
    sin: &[f32],
) {
    let half = head_dim / 2;
    for t in 0..tokens {
        for h in 0..heads {
            let base = (t * heads + h) * head_dim;
            for i in 0..half {
                let a = x[base + i];
                let b = x[base + half + i];
                let c = cos[t * half + i];
                let s = sin[t * half + i];
                x[base + i] = a * c - b * s;
                x[base + half + i] = b * c + a * s;
            }
        }
    }
}

pub(crate) fn full_attn(
    q: &[f32],
    k: &[f32],
    v: &[f32],
    tokens: usize,
    heads: usize,
    head_dim: usize,
    scale: f32,
) -> Vec<f32> {
    if let Some(out) = crate::metal_accel::flash_attn_packed(
        q, k, v, tokens, tokens, heads, head_dim, scale,
    ) {
        return out;
    }
    let mut out = vec![0f32; tokens * heads * head_dim];
    let mut scores = vec![0f32; tokens];
    for t in 0..tokens {
        for h in 0..heads {
            let q_vec = &q[(t * heads + h) * head_dim..(t * heads + h + 1) * head_dim];
            let mut max_s = f32::NEG_INFINITY;
            for s in 0..tokens {
                let k_vec = &k[(s * heads + h) * head_dim..(s * heads + h + 1) * head_dim];
                let mut acc = 0f32;
                for i in 0..head_dim {
                    acc += q_vec[i] * k_vec[i];
                }
                let sc = acc * scale;
                scores[s] = sc;
                if sc > max_s {
                    max_s = sc;
                }
            }
            let mut denom = 0f32;
            for s in 0..tokens {
                let e = (scores[s] - max_s).exp();
                scores[s] = e;
                denom += e;
            }
            let inv = 1.0 / denom.max(1e-30);
            let dst = &mut out[(t * heads + h) * head_dim..(t * heads + h + 1) * head_dim];
            dst.fill(0.0);
            for s in 0..tokens {
                let w = scores[s] * inv;
                let v_vec = &v[(s * heads + h) * head_dim..(s * heads + h + 1) * head_dim];
                for i in 0..head_dim {
                    dst[i] += w * v_vec[i];
                }
            }
        }
    }
    out
}

pub(crate) fn causal_attn(
    q: &[f32],
    k: &[f32],
    v: &[f32],
    tokens: usize,
    heads: usize,
    head_dim: usize,
    scale: f32,
) -> Vec<f32> {
    let mut out = vec![0f32; tokens * heads * head_dim];
    let mut scores = vec![0f32; tokens];
    for t in 0..tokens {
        for h in 0..heads {
            let q_vec = &q[(t * heads + h) * head_dim..(t * heads + h + 1) * head_dim];
            let mut max_s = f32::NEG_INFINITY;
            for s in 0..=t {
                let k_vec = &k[(s * heads + h) * head_dim..(s * heads + h + 1) * head_dim];
                let mut acc = 0f32;
                for i in 0..head_dim {
                    acc += q_vec[i] * k_vec[i];
                }
                let sc = acc * scale;
                scores[s] = sc;
                if sc > max_s {
                    max_s = sc;
                }
            }
            let mut denom = 0f32;
            for s in 0..=t {
                let e = (scores[s] - max_s).exp();
                scores[s] = e;
                denom += e;
            }
            let inv = 1.0 / denom.max(1e-30);
            let dst = &mut out[(t * heads + h) * head_dim..(t * heads + h + 1) * head_dim];
            dst.fill(0.0);
            for s in 0..=t {
                let w = scores[s] * inv;
                let v_vec = &v[(s * heads + h) * head_dim..(s * heads + h + 1) * head_dim];
                for i in 0..head_dim {
                    dst[i] += w * v_vec[i];
                }
            }
        }
    }
    out
}

/// GQA causal attention straight off the compact `[t, kv_heads, d]` cache.
/// Bit-exact with `repeat_kv` + [`causal_attn`]: the repeated rows are the
/// same values, and every per-head accumulation runs in the same order.
/// Rows of `(t, h)` pairs are independent, so the head loop threads freely.
pub(crate) fn causal_attn_gqa(
    q: &[f32],
    k: &[f32],
    v: &[f32],
    tokens: usize,
    heads: usize,
    kv_heads: usize,
    head_dim: usize,
    scale: f32,
) -> Vec<f32> {
    let group = heads / kv_heads;
    let mut out = vec![0f32; tokens * heads * head_dim];
    let threads = std::thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(4)
        .clamp(1, 8);
    let rows: Vec<(usize, &mut [f32])> = out.chunks_mut(heads * head_dim).enumerate().collect();
    let chunk = rows.len().div_ceil(threads);
    std::thread::scope(|scope| {
        let mut rest = rows;
        while !rest.is_empty() {
            let take = rest.len().min(chunk);
            let batch: Vec<(usize, &mut [f32])> = rest.drain(..take).collect();
            scope.spawn(move || {
                let mut scores = vec![0f32; tokens];
                for (t, row) in batch {
                    for h in 0..heads {
                        let hk = h / group;
                        let q_vec = &q[(t * heads + h) * head_dim..(t * heads + h + 1) * head_dim];
                        let mut max_s = f32::NEG_INFINITY;
                        for s in 0..=t {
                            let k_vec = &k
                                [(s * kv_heads + hk) * head_dim..(s * kv_heads + hk + 1) * head_dim];
                            let mut acc = 0f32;
                            for i in 0..head_dim {
                                acc += q_vec[i] * k_vec[i];
                            }
                            let sc = acc * scale;
                            scores[s] = sc;
                            if sc > max_s {
                                max_s = sc;
                            }
                        }
                        let mut denom = 0f32;
                        for s in 0..=t {
                            let e = (scores[s] - max_s).exp();
                            scores[s] = e;
                            denom += e;
                        }
                        let inv = 1.0 / denom.max(1e-30);
                        let dst = &mut row[h * head_dim..(h + 1) * head_dim];
                        for s in 0..=t {
                            let w = scores[s] * inv;
                            let v_vec = &v
                                [(s * kv_heads + hk) * head_dim..(s * kv_heads + hk + 1) * head_dim];
                            for i in 0..head_dim {
                                dst[i] += w * v_vec[i];
                            }
                        }
                    }
                }
            });
        }
    });
    out
}

/// One query token against the compact GQA `[t, kv_heads, d]` cache.
/// Bit-exact with `repeat_kv` + [`attn_q_vs_kv`] (same values, same order).
pub(crate) fn attn_q_vs_kv_gqa(
    q: &[f32],
    k: &[f32],
    v: &[f32],
    kv_tokens: usize,
    heads: usize,
    kv_heads: usize,
    head_dim: usize,
    scale: f32,
) -> Vec<f32> {
    let group = heads / kv_heads;
    let mut out = vec![0f32; heads * head_dim];
    // Deliberately serial. A scoped-spawn-per-call version (8 threads × 72
    // calls/frame) saved ~1-2s steady-state but triggered a ~17-19s decaying
    // stall over the first ~16 decode frames (thread-spawn storms while the
    // OS drains post-prefill memory churn) — measured in q8_8fable2..4.
    let mut scores = vec![0f32; kv_tokens];
    for (h, dst) in out.chunks_mut(head_dim).enumerate() {
        attn_one_head_gqa(
            q, k, v, kv_tokens, kv_heads, head_dim, scale, group, h, &mut scores, dst,
        );
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn attn_one_head_gqa(
    q: &[f32],
    k: &[f32],
    v: &[f32],
    kv_tokens: usize,
    kv_heads: usize,
    head_dim: usize,
    scale: f32,
    group: usize,
    h: usize,
    scores: &mut [f32],
    dst: &mut [f32],
) {
    let hk = h / group;
    let q_vec = &q[h * head_dim..(h + 1) * head_dim];
    let mut max_s = f32::NEG_INFINITY;
    for s in 0..kv_tokens {
        let k_vec = &k[(s * kv_heads + hk) * head_dim..(s * kv_heads + hk + 1) * head_dim];
        let mut acc = 0f32;
        for i in 0..head_dim {
            acc += q_vec[i] * k_vec[i];
        }
        scores[s] = acc * scale;
        max_s = max_s.max(scores[s]);
    }
    let mut denom = 0f32;
    for s in 0..kv_tokens {
        let e = (scores[s] - max_s).exp();
        scores[s] = e;
        denom += e;
    }
    let inv = 1.0 / denom.max(1e-30);
    dst.fill(0.0);
    for s in 0..kv_tokens {
        let w = scores[s] * inv;
        let v_vec = &v[(s * kv_heads + hk) * head_dim..(s * kv_heads + hk + 1) * head_dim];
        for i in 0..head_dim {
            dst[i] += w * v_vec[i];
        }
    }
}

/// One query token against a cached K/V sequence.
pub(crate) fn attn_q_vs_kv(
    q: &[f32],
    k: &[f32],
    v: &[f32],
    kv_tokens: usize,
    heads: usize,
    head_dim: usize,
    scale: f32,
) -> Vec<f32> {
    let mut out = vec![0f32; heads * head_dim];
    for h in 0..heads {
        let q_vec = &q[h * head_dim..(h + 1) * head_dim];
        let mut scores = vec![0f32; kv_tokens];
        let mut max_s = f32::NEG_INFINITY;
        for s in 0..kv_tokens {
            let k_vec = &k[(s * heads + h) * head_dim..(s * heads + h + 1) * head_dim];
            let mut acc = 0f32;
            for i in 0..head_dim {
                acc += q_vec[i] * k_vec[i];
            }
            scores[s] = acc * scale;
            max_s = max_s.max(scores[s]);
        }
        let mut denom = 0f32;
        for s in 0..kv_tokens {
            let e = (scores[s] - max_s).exp();
            scores[s] = e;
            denom += e;
        }
        let inv = 1.0 / denom.max(1e-30);
        let dst = &mut out[h * head_dim..(h + 1) * head_dim];
        for s in 0..kv_tokens {
            let w = scores[s] * inv;
            let v_vec = &v[(s * heads + h) * head_dim..(s * heads + h + 1) * head_dim];
            for i in 0..head_dim {
                dst[i] += w * v_vec[i];
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// RVQ 4-layer (BF16 linears via Metal).
// ---------------------------------------------------------------------------

pub struct Music3GgufRvq {
    input_norm: Vec<Vec<f32>>,
    post_attn_norm: Vec<Vec<f32>>,
    final_norm: Vec<f32>,
    pos_embedding: Vec<f32>,
}

pub struct Music3GgufRvqSession {
    k: Vec<Vec<f32>>,
    v: Vec<Vec<f32>>,
    pos: usize,
    pub last: Vec<f32>,
}

impl Music3GgufRvq {
    pub fn load(file: &Music3GgufFile) -> Result<Self> {
        let mut input_norm = Vec::with_capacity(MUSIC3_RVQ_LAYERS);
        let mut post_attn_norm = Vec::with_capacity(MUSIC3_RVQ_LAYERS);
        for layer in 0..MUSIC3_RVQ_LAYERS {
            input_norm.push(file.read_f32_any(&format!("layers.{layer}.input_layernorm.weight"))?);
            post_attn_norm.push(
                file.read_f32_any(&format!("layers.{layer}.post_attention_layernorm.weight"))?,
            );
        }
        Ok(Self {
            input_norm,
            post_attn_norm,
            final_norm: file.read_f32_any("norm.weight")?,
            pos_embedding: file.read_f32_any("pos_embedding.weight")?,
        })
    }

    /// `inputs` is `[seq, 4096]`. Returns the same shape after pos + 4 layers + RMSNorm.
    pub fn forward(&self, file: &Music3GgufFile, inputs: &[f32], seq: usize) -> Result<Vec<f32>> {
        if seq == 0 || seq > MUSIC3_RVQ_MAX_POS || inputs.len() != seq * MUSIC3_RVQ_HIDDEN {
            return Err(DiffusionError::model(format!(
                "music3 gguf RVQ input {} for seq {seq}",
                inputs.len()
            )));
        }
        if self.pos_embedding.len() < seq * MUSIC3_RVQ_HIDDEN {
            return Err(DiffusionError::model("music3 gguf RVQ pos_embedding short"));
        }
        let mut hidden = inputs.to_vec();
        add_inplace(&mut hidden, &self.pos_embedding[..seq * MUSIC3_RVQ_HIDDEN]);
        let head_dim = MUSIC3_RVQ_HIDDEN / MUSIC3_RVQ_HEADS;
        let scale = 1.0 / (head_dim as f32).sqrt();
        for layer in 0..MUSIC3_RVQ_LAYERS {
            let mut normed = hidden.clone();
            rms_norm_rows(
                &mut normed,
                &self.input_norm[layer],
                MUSIC3_RVQ_HIDDEN,
                MUSIC3_LM_RMS_EPS,
            );
            let qn = format!("layers.{layer}.attn.to_q.weight");
            let kn = format!("layers.{layer}.attn.to_k.weight");
            let vn = format!("layers.{layer}.attn.to_v.weight");
            let (q, k, v) = file.linear_nt_qkv(&qn, &kn, &vn, &normed, seq)?;
            let attn = causal_attn(&q, &k, &v, seq, MUSIC3_RVQ_HEADS, head_dim, scale);
            let attn = file.linear_nt(&format!("layers.{layer}.attn.to_out.weight"), &attn, seq)?;
            add_inplace(&mut hidden, &attn);

            let mut normed = hidden.clone();
            rms_norm_rows(
                &mut normed,
                &self.post_attn_norm[layer],
                MUSIC3_RVQ_HIDDEN,
                MUSIC3_LM_RMS_EPS,
            );
            let un = format!("layers.{layer}.up_proj.weight");
            let gn = format!("layers.{layer}.gate_proj.weight");
            let (up, gate) = file.linear_nt_up_gate(&un, &gn, &normed, seq)?;
            if up.len() != seq * MUSIC3_RVQ_FF || gate.len() != seq * MUSIC3_RVQ_FF {
                return Err(DiffusionError::model(format!(
                    "music3 gguf RVQ ff {} / {} expected {}",
                    up.len(),
                    gate.len(),
                    seq * MUSIC3_RVQ_FF
                )));
            }
            let ff = swiglu(&up, &gate);
            let ff = file.linear_nt(&format!("layers.{layer}.down_proj.weight"), &ff, seq)?;
            add_inplace(&mut hidden, &ff);
        }
        rms_norm_rows(
            &mut hidden,
            &self.final_norm,
            MUSIC3_RVQ_HIDDEN,
            MUSIC3_LM_RMS_EPS,
        );
        Ok(hidden)
    }

    /// Prefill KV for a short prompt (`p0` + semantic, seq=2). Later residual
    /// tokens use [`Self::step`] so each head does not rerun the whole stack.
    pub fn prefill_session(
        &self,
        file: &Music3GgufFile,
        inputs: &[f32],
        seq: usize,
    ) -> Result<Music3GgufRvqSession> {
        if seq == 0 || inputs.len() != seq * MUSIC3_RVQ_HIDDEN {
            return Err(DiffusionError::model("music3 gguf RVQ prefill shape"));
        }
        let mut session = Music3GgufRvqSession {
            k: vec![Vec::new(); MUSIC3_RVQ_LAYERS],
            v: vec![Vec::new(); MUSIC3_RVQ_LAYERS],
            pos: 0,
            last: vec![0f32; MUSIC3_RVQ_HIDDEN],
        };
        let mut h = inputs.to_vec();
        add_inplace(&mut h, &self.pos_embedding[..seq * MUSIC3_RVQ_HIDDEN]);
        let head_dim = MUSIC3_RVQ_HIDDEN / MUSIC3_RVQ_HEADS;
        for layer in 0..MUSIC3_RVQ_LAYERS {
            let mut normed = h.clone();
            rms_norm_rows(
                &mut normed,
                &self.input_norm[layer],
                MUSIC3_RVQ_HIDDEN,
                MUSIC3_LM_RMS_EPS,
            );
            let qn = format!("layers.{layer}.attn.to_q.weight");
            let kn = format!("layers.{layer}.attn.to_k.weight");
            let vn = format!("layers.{layer}.attn.to_v.weight");
            let (q, k, v) = file.linear_nt_qkv(&qn, &kn, &vn, &normed, seq)?;
            session.k[layer] = k;
            session.v[layer] = v;
            let scale = 1.0 / (head_dim as f32).sqrt();
            let attn = causal_attn(&q, &session.k[layer], &session.v[layer], seq, MUSIC3_RVQ_HEADS, head_dim, scale);
            let attn = file.linear_nt(&format!("layers.{layer}.attn.to_out.weight"), &attn, seq)?;
            add_inplace(&mut h, &attn);
            let mut normed = h.clone();
            rms_norm_rows(
                &mut normed,
                &self.post_attn_norm[layer],
                MUSIC3_RVQ_HIDDEN,
                MUSIC3_LM_RMS_EPS,
            );
            let un = format!("layers.{layer}.up_proj.weight");
            let gn = format!("layers.{layer}.gate_proj.weight");
            let (up, gate) = file.linear_nt_up_gate(&un, &gn, &normed, seq)?;
            let ff = swiglu(&up, &gate);
            let ff = file.linear_nt(&format!("layers.{layer}.down_proj.weight"), &ff, seq)?;
            add_inplace(&mut h, &ff);
        }
        rms_norm_rows(
            &mut h,
            &self.final_norm,
            MUSIC3_RVQ_HIDDEN,
            MUSIC3_LM_RMS_EPS,
        );
        session.pos = seq;
        session.last = h[(seq - 1) * MUSIC3_RVQ_HIDDEN..].to_vec();
        Ok(session)
    }

    pub fn step(
        &self,
        file: &Music3GgufFile,
        session: &mut Music3GgufRvqSession,
        token: &[f32],
    ) -> Result<Vec<f32>> {
        if token.len() != MUSIC3_RVQ_HIDDEN {
            return Err(DiffusionError::model("music3 gguf RVQ step width"));
        }
        if session.pos >= MUSIC3_RVQ_MAX_POS {
            return Err(DiffusionError::model("music3 gguf RVQ step past max pos"));
        }
        let pos = session.pos;
        let mut hidden = token.to_vec();
        let off = pos * MUSIC3_RVQ_HIDDEN;
        add_inplace(&mut hidden, &self.pos_embedding[off..off + MUSIC3_RVQ_HIDDEN]);
        let head_dim = MUSIC3_RVQ_HIDDEN / MUSIC3_RVQ_HEADS;
        let scale = 1.0 / (head_dim as f32).sqrt();
        for layer in 0..MUSIC3_RVQ_LAYERS {
            let mut normed = hidden.clone();
            rms_norm_rows(
                &mut normed,
                &self.input_norm[layer],
                MUSIC3_RVQ_HIDDEN,
                MUSIC3_LM_RMS_EPS,
            );
            let qn = format!("layers.{layer}.attn.to_q.weight");
            let kn = format!("layers.{layer}.attn.to_k.weight");
            let vn = format!("layers.{layer}.attn.to_v.weight");
            let (q, k, v) = file.linear_nt_qkv(&qn, &kn, &vn, &normed, 1)?;
            session.k[layer].extend_from_slice(&k);
            session.v[layer].extend_from_slice(&v);
            let t = session.pos + 1;
            let attn = attn_q_vs_kv(
                &q,
                &session.k[layer],
                &session.v[layer],
                t,
                MUSIC3_RVQ_HEADS,
                head_dim,
                scale,
            );
            let attn = file.linear_nt(&format!("layers.{layer}.attn.to_out.weight"), &attn, 1)?;
            add_inplace(&mut hidden, &attn);
            let mut normed = hidden.clone();
            rms_norm_rows(
                &mut normed,
                &self.post_attn_norm[layer],
                MUSIC3_RVQ_HIDDEN,
                MUSIC3_LM_RMS_EPS,
            );
            let un = format!("layers.{layer}.up_proj.weight");
            let gn = format!("layers.{layer}.gate_proj.weight");
            let (up, gate) = file.linear_nt_up_gate(&un, &gn, &normed, 1)?;
            let ff = swiglu(&up, &gate);
            let ff = file.linear_nt(&format!("layers.{layer}.down_proj.weight"), &ff, 1)?;
            add_inplace(&mut hidden, &ff);
        }
        session.pos += 1;
        rms_norm_rows(
            &mut hidden,
            &self.final_norm,
            MUSIC3_RVQ_HIDDEN,
            MUSIC3_LM_RMS_EPS,
        );
        session.last = hidden.clone();
        Ok(hidden)
    }

    /// Cond+uncond prefill with stacked `m=2*seq` Metal GEMMs.
    pub fn prefill_pair(
        &self,
        file: &Music3GgufFile,
        cond_in: &[f32],
        uncond_in: &[f32],
        seq: usize,
    ) -> Result<(Music3GgufRvqSession, Music3GgufRvqSession)> {
        if seq == 0
            || cond_in.len() != seq * MUSIC3_RVQ_HIDDEN
            || uncond_in.len() != seq * MUSIC3_RVQ_HIDDEN
        {
            return Err(DiffusionError::model("music3 gguf RVQ pair prefill"));
        }
        let mut hidden = Vec::with_capacity(2 * seq * MUSIC3_RVQ_HIDDEN);
        hidden.extend_from_slice(cond_in);
        hidden.extend_from_slice(uncond_in);
        let pos = &self.pos_embedding[..seq * MUSIC3_RVQ_HIDDEN];
        add_inplace(&mut hidden[..seq * MUSIC3_RVQ_HIDDEN], pos);
        add_inplace(&mut hidden[seq * MUSIC3_RVQ_HIDDEN..], pos);
        if rvq_resident_enabled() {
            if let Some(pair) = self.try_prefill_pair_resident(file, &hidden, seq) {
                return Ok(pair);
            }
        }
        let head_dim = MUSIC3_RVQ_HIDDEN / MUSIC3_RVQ_HEADS;
        let scale = 1.0 / (head_dim as f32).sqrt();
        let mut k_c = vec![Vec::new(); MUSIC3_RVQ_LAYERS];
        let mut v_c = vec![Vec::new(); MUSIC3_RVQ_LAYERS];
        let mut k_u = vec![Vec::new(); MUSIC3_RVQ_LAYERS];
        let mut v_u = vec![Vec::new(); MUSIC3_RVQ_LAYERS];
        let row = seq * MUSIC3_RVQ_HIDDEN;
        for layer in 0..MUSIC3_RVQ_LAYERS {
            let mut normed = hidden.clone();
            rms_norm_rows(
                &mut normed,
                &self.input_norm[layer],
                MUSIC3_RVQ_HIDDEN,
                MUSIC3_LM_RMS_EPS,
            );
            let qn = format!("layers.{layer}.attn.to_q.weight");
            let kn = format!("layers.{layer}.attn.to_k.weight");
            let vn = format!("layers.{layer}.attn.to_v.weight");
            let (q, k, v) = file.linear_nt_qkv(&qn, &kn, &vn, &normed, 2 * seq)?;
            let kv_w = seq * MUSIC3_RVQ_HEADS * head_dim;
            k_c[layer] = k[..kv_w].to_vec();
            k_u[layer] = k[kv_w..].to_vec();
            v_c[layer] = v[..kv_w].to_vec();
            v_u[layer] = v[kv_w..].to_vec();
            let mut attn = Vec::with_capacity(2 * kv_w);
            attn.extend(causal_attn(
                &q[..kv_w],
                &k_c[layer],
                &v_c[layer],
                seq,
                MUSIC3_RVQ_HEADS,
                head_dim,
                scale,
            ));
            attn.extend(causal_attn(
                &q[kv_w..],
                &k_u[layer],
                &v_u[layer],
                seq,
                MUSIC3_RVQ_HEADS,
                head_dim,
                scale,
            ));
            let attn = file.linear_nt(&format!("layers.{layer}.attn.to_out.weight"), &attn, 2 * seq)?;
            add_inplace(&mut hidden, &attn);
            let mut normed = hidden.clone();
            rms_norm_rows(
                &mut normed,
                &self.post_attn_norm[layer],
                MUSIC3_RVQ_HIDDEN,
                MUSIC3_LM_RMS_EPS,
            );
            let un = format!("layers.{layer}.up_proj.weight");
            let gn = format!("layers.{layer}.gate_proj.weight");
            let (up, gate) = file.linear_nt_up_gate(&un, &gn, &normed, 2 * seq)?;
            let ff = swiglu(&up, &gate);
            let ff = file.linear_nt(&format!("layers.{layer}.down_proj.weight"), &ff, 2 * seq)?;
            add_inplace(&mut hidden, &ff);
            let _ = row;
        }
        rms_norm_rows(
            &mut hidden,
            &self.final_norm,
            MUSIC3_RVQ_HIDDEN,
            MUSIC3_LM_RMS_EPS,
        );
        let cond = Music3GgufRvqSession {
            k: k_c,
            v: v_c,
            pos: seq,
            last: hidden[(seq - 1) * MUSIC3_RVQ_HIDDEN..seq * MUSIC3_RVQ_HIDDEN].to_vec(),
        };
        let uncond = Music3GgufRvqSession {
            k: k_u,
            v: v_u,
            pos: seq,
            last: hidden[(2 * seq - 1) * MUSIC3_RVQ_HIDDEN..].to_vec(),
        };
        Ok((cond, uncond))
    }

    /// Whole-layer resident RVQ decode step: RMS + QKV + o/up/gate/down and
    /// the elementwise glue stay on GPU (one command buffer per layer half via
    /// `try_ar_pre_attn`/`try_ar_post_attn`); only the tiny t≤8 attention runs
    /// on host. BF16 weights ride the F16 sidecar namespace.
    fn try_step_pair_resident(
        &self,
        file: &Music3GgufFile,
        cond: &mut Music3GgufRvqSession,
        uncond: &mut Music3GgufRvqSession,
        hidden: &[f32],
    ) -> Option<()> {
        use makepad_ggml::backend::metal;
        let ns = format!("music3-{}", file.role.as_str());
        let ns_f16 = format!("music3-{}-f16", file.role.as_str());
        let head_dim = MUSIC3_RVQ_HIDDEN / MUSIC3_RVQ_HEADS;
        let scale = 1.0 / (head_dim as f32).sqrt();
        let kv_w = MUSIC3_RVQ_HEADS * head_dim;
        for layer in 0..MUSIC3_RVQ_LAYERS {
            let qn = format!("layers.{layer}.attn.to_q.weight");
            let kn = format!("layers.{layer}.attn.to_k.weight");
            let vn = format!("layers.{layer}.attn.to_v.weight");
            let on = format!("layers.{layer}.attn.to_out.weight");
            let un = format!("layers.{layer}.up_proj.weight");
            let gn = format!("layers.{layer}.gate_proj.weight");
            let dn = format!("layers.{layer}.down_proj.weight");
            let qm = file.keyed_mat_any(&ns, &ns_f16, &qn)?;
            let km = file.keyed_mat_any(&ns, &ns_f16, &kn)?;
            let vm = file.keyed_mat_any(&ns, &ns_f16, &vn)?;
            let om = file.keyed_mat_any(&ns, &ns_f16, &on)?;
            let um = file.keyed_mat_any(&ns, &ns_f16, &un)?;
            let gm = file.keyed_mat_any(&ns, &ns_f16, &gn)?;
            let dm = file.keyed_mat_any(&ns, &ns_f16, &dn)?;
            let in_key = format!("layers.{layer}.input_layernorm.weight");
            let post_key = format!("layers.{layer}.post_attention_layernorm.weight");
            let upload = if layer == 0 { Some(hidden) } else { None };
            let (q, k, v) = metal::try_ar_pre_attn(
                upload,
                2,
                MUSIC3_RVQ_HIDDEN,
                head_dim,
                &self.input_norm[layer],
                None,
                &in_key,
                MUSIC3_LM_RMS_EPS,
                qm,
                km,
                vm,
                |a, b| file.load_keyed_bytes(a, b),
            )?;
            cond.k[layer].extend_from_slice(&k[..kv_w]);
            cond.v[layer].extend_from_slice(&v[..kv_w]);
            uncond.k[layer].extend_from_slice(&k[kv_w..]);
            uncond.v[layer].extend_from_slice(&v[kv_w..]);
            let t = cond.pos + 1;
            let mut attn = Vec::with_capacity(2 * kv_w);
            attn.extend(attn_q_vs_kv(
                &q[..kv_w],
                &cond.k[layer],
                &cond.v[layer],
                t,
                MUSIC3_RVQ_HEADS,
                head_dim,
                scale,
            ));
            attn.extend(attn_q_vs_kv(
                &q[kv_w..],
                &uncond.k[layer],
                &uncond.v[layer],
                t,
                MUSIC3_RVQ_HEADS,
                head_dim,
                scale,
            ));
            metal::try_ar_post_attn(
                &attn,
                2,
                MUSIC3_RVQ_HIDDEN,
                &self.post_attn_norm[layer],
                &post_key,
                MUSIC3_LM_RMS_EPS,
                om,
                um,
                gm,
                dm,
                |a, b| file.load_keyed_bytes(a, b),
            )?;
        }
        let out = metal::try_ar_final_rms(
            2,
            MUSIC3_RVQ_HIDDEN,
            &self.final_norm,
            "norm.weight",
            MUSIC3_LM_RMS_EPS,
        )?;
        cond.pos += 1;
        uncond.pos += 1;
        cond.last = out[..MUSIC3_RVQ_HIDDEN].to_vec();
        uncond.last = out[MUSIC3_RVQ_HIDDEN..].to_vec();
        Some(())
    }

    /// Resident RVQ pair prefill (`m = 2*seq` rows through the same GPU
    /// layer halves). Sessions are freshly assigned, so a partial failure
    /// falls back to the host loop without repair.
    fn try_prefill_pair_resident(
        &self,
        file: &Music3GgufFile,
        hidden_in: &[f32],
        seq: usize,
    ) -> Option<(Music3GgufRvqSession, Music3GgufRvqSession)> {
        use makepad_ggml::backend::metal;
        let ns = format!("music3-{}", file.role.as_str());
        let ns_f16 = format!("music3-{}-f16", file.role.as_str());
        let head_dim = MUSIC3_RVQ_HIDDEN / MUSIC3_RVQ_HEADS;
        let scale = 1.0 / (head_dim as f32).sqrt();
        let kv_w = seq * MUSIC3_RVQ_HEADS * head_dim;
        let m = 2 * seq;
        let mut k_c = vec![Vec::new(); MUSIC3_RVQ_LAYERS];
        let mut v_c = vec![Vec::new(); MUSIC3_RVQ_LAYERS];
        let mut k_u = vec![Vec::new(); MUSIC3_RVQ_LAYERS];
        let mut v_u = vec![Vec::new(); MUSIC3_RVQ_LAYERS];
        for layer in 0..MUSIC3_RVQ_LAYERS {
            let qn = format!("layers.{layer}.attn.to_q.weight");
            let kn = format!("layers.{layer}.attn.to_k.weight");
            let vn = format!("layers.{layer}.attn.to_v.weight");
            let on = format!("layers.{layer}.attn.to_out.weight");
            let un = format!("layers.{layer}.up_proj.weight");
            let gn = format!("layers.{layer}.gate_proj.weight");
            let dn = format!("layers.{layer}.down_proj.weight");
            let qm = file.keyed_mat_any(&ns, &ns_f16, &qn)?;
            let km = file.keyed_mat_any(&ns, &ns_f16, &kn)?;
            let vm = file.keyed_mat_any(&ns, &ns_f16, &vn)?;
            let om = file.keyed_mat_any(&ns, &ns_f16, &on)?;
            let um = file.keyed_mat_any(&ns, &ns_f16, &un)?;
            let gm = file.keyed_mat_any(&ns, &ns_f16, &gn)?;
            let dm = file.keyed_mat_any(&ns, &ns_f16, &dn)?;
            let in_key = format!("layers.{layer}.input_layernorm.weight");
            let post_key = format!("layers.{layer}.post_attention_layernorm.weight");
            let upload = if layer == 0 { Some(hidden_in) } else { None };
            let (q, k, v) = metal::try_ar_pre_attn(
                upload,
                m,
                MUSIC3_RVQ_HIDDEN,
                head_dim,
                &self.input_norm[layer],
                None,
                &in_key,
                MUSIC3_LM_RMS_EPS,
                qm,
                km,
                vm,
                |a, b| file.load_keyed_bytes(a, b),
            )?;
            k_c[layer] = k[..kv_w].to_vec();
            k_u[layer] = k[kv_w..].to_vec();
            v_c[layer] = v[..kv_w].to_vec();
            v_u[layer] = v[kv_w..].to_vec();
            let mut attn = Vec::with_capacity(2 * kv_w);
            attn.extend(causal_attn(
                &q[..kv_w],
                &k_c[layer],
                &v_c[layer],
                seq,
                MUSIC3_RVQ_HEADS,
                head_dim,
                scale,
            ));
            attn.extend(causal_attn(
                &q[kv_w..],
                &k_u[layer],
                &v_u[layer],
                seq,
                MUSIC3_RVQ_HEADS,
                head_dim,
                scale,
            ));
            metal::try_ar_post_attn(
                &attn,
                m,
                MUSIC3_RVQ_HIDDEN,
                &self.post_attn_norm[layer],
                &post_key,
                MUSIC3_LM_RMS_EPS,
                om,
                um,
                gm,
                dm,
                |a, b| file.load_keyed_bytes(a, b),
            )?;
        }
        let out = metal::try_ar_final_rms(
            m,
            MUSIC3_RVQ_HIDDEN,
            &self.final_norm,
            "norm.weight",
            MUSIC3_LM_RMS_EPS,
        )?;
        let cond = Music3GgufRvqSession {
            k: k_c,
            v: v_c,
            pos: seq,
            last: out[(seq - 1) * MUSIC3_RVQ_HIDDEN..seq * MUSIC3_RVQ_HIDDEN].to_vec(),
        };
        let uncond = Music3GgufRvqSession {
            k: k_u,
            v: v_u,
            pos: seq,
            last: out[(m - 1) * MUSIC3_RVQ_HIDDEN..].to_vec(),
        };
        Some((cond, uncond))
    }

    pub fn step_pair(
        &self,
        file: &Music3GgufFile,
        cond: &mut Music3GgufRvqSession,
        uncond: &mut Music3GgufRvqSession,
        token_c: &[f32],
        token_u: &[f32],
    ) -> Result<()> {
        if token_c.len() != MUSIC3_RVQ_HIDDEN || token_u.len() != MUSIC3_RVQ_HIDDEN {
            return Err(DiffusionError::model("music3 gguf RVQ pair step width"));
        }
        if cond.pos != uncond.pos || cond.pos >= MUSIC3_RVQ_MAX_POS {
            return Err(DiffusionError::model("music3 gguf RVQ pair step pos"));
        }
        let pos = cond.pos;
        let mut hidden = Vec::with_capacity(2 * MUSIC3_RVQ_HIDDEN);
        hidden.extend_from_slice(token_c);
        hidden.extend_from_slice(token_u);
        let off = pos * MUSIC3_RVQ_HIDDEN;
        let pos_row = &self.pos_embedding[off..off + MUSIC3_RVQ_HIDDEN];
        add_inplace(&mut hidden[..MUSIC3_RVQ_HIDDEN], pos_row);
        add_inplace(&mut hidden[MUSIC3_RVQ_HIDDEN..], pos_row);
        if rvq_resident_enabled() {
            let klen: Vec<usize> = cond.k.iter().map(|k| k.len()).collect();
            if self
                .try_step_pair_resident(file, cond, uncond, &hidden)
                .is_some()
            {
                return Ok(());
            }
            // A mid-stack miss leaves partially grown KV; trim before the
            // host rerun appends again.
            if cond.pos == pos {
                for (layer, len) in klen.iter().enumerate() {
                    cond.k[layer].truncate(*len);
                    cond.v[layer].truncate(*len);
                    uncond.k[layer].truncate(*len);
                    uncond.v[layer].truncate(*len);
                }
            }
        }
        let head_dim = MUSIC3_RVQ_HIDDEN / MUSIC3_RVQ_HEADS;
        let scale = 1.0 / (head_dim as f32).sqrt();
        let kv_w = MUSIC3_RVQ_HEADS * head_dim;
        for layer in 0..MUSIC3_RVQ_LAYERS {
            let mut normed = hidden.clone();
            rms_norm_rows(
                &mut normed,
                &self.input_norm[layer],
                MUSIC3_RVQ_HIDDEN,
                MUSIC3_LM_RMS_EPS,
            );
            let qn = format!("layers.{layer}.attn.to_q.weight");
            let kn = format!("layers.{layer}.attn.to_k.weight");
            let vn = format!("layers.{layer}.attn.to_v.weight");
            let (q, k, v) = file.linear_nt_qkv(&qn, &kn, &vn, &normed, 2)?;
            cond.k[layer].extend_from_slice(&k[..kv_w]);
            cond.v[layer].extend_from_slice(&v[..kv_w]);
            uncond.k[layer].extend_from_slice(&k[kv_w..]);
            uncond.v[layer].extend_from_slice(&v[kv_w..]);
            let t = cond.pos + 1;
            let mut attn = Vec::with_capacity(2 * kv_w);
            attn.extend(attn_q_vs_kv(
                &q[..kv_w],
                &cond.k[layer],
                &cond.v[layer],
                t,
                MUSIC3_RVQ_HEADS,
                head_dim,
                scale,
            ));
            attn.extend(attn_q_vs_kv(
                &q[kv_w..],
                &uncond.k[layer],
                &uncond.v[layer],
                t,
                MUSIC3_RVQ_HEADS,
                head_dim,
                scale,
            ));
            let attn = file.linear_nt(&format!("layers.{layer}.attn.to_out.weight"), &attn, 2)?;
            add_inplace(&mut hidden, &attn);
            let mut normed = hidden.clone();
            rms_norm_rows(
                &mut normed,
                &self.post_attn_norm[layer],
                MUSIC3_RVQ_HIDDEN,
                MUSIC3_LM_RMS_EPS,
            );
            let un = format!("layers.{layer}.up_proj.weight");
            let gn = format!("layers.{layer}.gate_proj.weight");
            let (up, gate) = file.linear_nt_up_gate(&un, &gn, &normed, 2)?;
            let ff = swiglu(&up, &gate);
            let ff = file.linear_nt(&format!("layers.{layer}.down_proj.weight"), &ff, 2)?;
            add_inplace(&mut hidden, &ff);
        }
        cond.pos += 1;
        uncond.pos += 1;
        rms_norm_rows(
            &mut hidden,
            &self.final_norm,
            MUSIC3_RVQ_HIDDEN,
            MUSIC3_LM_RMS_EPS,
        );
        cond.last = hidden[..MUSIC3_RVQ_HIDDEN].to_vec();
        uncond.last = hidden[MUSIC3_RVQ_HIDDEN..].to_vec();
        Ok(())
    }

    pub fn project(&self, file: &Music3GgufFile, hidden: &[f32]) -> Result<Vec<f32>> {
        if hidden.len() != MUSIC3_RVQ_HIDDEN {
            return Err(DiffusionError::model("music3 gguf RVQ project width"));
        }
        file.linear_nt("projection.weight", hidden, 1)
    }

    pub fn audio_head(&self, file: &Music3GgufFile, hidden: &[f32], head: usize) -> Result<Vec<f32>> {
        if hidden.len() != MUSIC3_RVQ_HIDDEN || head >= MUSIC3_NUM_CODEBOOKS - 1 {
            return Err(DiffusionError::model(format!("music3 gguf RVQ head {head}")));
        }
        let y = file.linear_nt(&format!("audio_heads.{head}.weight"), hidden, 1)?;
        if y.len() != MUSIC3_AUDIO_VOCAB {
            return Err(DiffusionError::model(format!(
                "music3 gguf RVQ head {head} logits {}",
                y.len()
            )));
        }
        Ok(y)
    }

    pub fn audio_embed(&self, file: &Music3GgufFile, head: usize, code: u32) -> Result<Vec<f32>> {
        if head >= MUSIC3_NUM_CODEBOOKS - 1 || code as usize >= MUSIC3_AUDIO_VOCAB {
            return Err(DiffusionError::model(format!(
                "music3 gguf RVQ embed head={head} code={code}"
            )));
        }
        let idx = code + head as u32 * MUSIC3_AUDIO_VOCAB as u32;
        file.gather_rows("audio_embeddings.weight", &[idx])
    }

    /// Official `_generate_depth_codes`: cond+uncond last-hidden, CFG 1.5 on
    /// each residual head, one shared code, cond-row hiddens only.
    pub fn depth_sample_cfg(
        &self,
        file: &Music3GgufFile,
        cond_hidden: &[f32],
        uncond_hidden: &[f32],
        semantic_embed: &[f32],
        rng: &mut TorchPhilox,
        top_k: usize,
    ) -> Result<(Vec<u32>, Vec<f32>)> {
        if cond_hidden.len() != MUSIC3_RVQ_HIDDEN
            || uncond_hidden.len() != MUSIC3_RVQ_HIDDEN
            || semantic_embed.len() != MUSIC3_RVQ_HIDDEN
        {
            return Err(DiffusionError::model("music3 gguf RVQ cfg shapes"));
        }
        let p0c = self.project(file, cond_hidden)?;
        let p0u = self.project(file, uncond_hidden)?;
        let p1 = self.project(file, semantic_embed)?;
        let mut seq_c = Vec::with_capacity(MUSIC3_NUM_CODEBOOKS * MUSIC3_RVQ_HIDDEN);
        let mut seq_u = Vec::with_capacity(MUSIC3_NUM_CODEBOOKS * MUSIC3_RVQ_HIDDEN);
        seq_c.extend_from_slice(&p0c);
        seq_c.extend_from_slice(&p1);
        seq_u.extend_from_slice(&p0u);
        seq_u.extend_from_slice(&p1);
        let mut codes = Vec::with_capacity(MUSIC3_NUM_CODEBOOKS - 1);
        let mut parts = Vec::with_capacity((MUSIC3_NUM_CODEBOOKS - 1) * MUSIC3_RVQ_HIDDEN);
        for head in 0..(MUSIC3_NUM_CODEBOOKS - 1) {
            let n = seq_c.len() / MUSIC3_RVQ_HIDDEN;
            let out_c = self.forward(file, &seq_c, n)?;
            let out_u = self.forward(file, &seq_u, n)?;
            let last_c = &out_c[(n - 1) * MUSIC3_RVQ_HIDDEN..];
            let last_u = &out_u[(n - 1) * MUSIC3_RVQ_HIDDEN..];
            parts.extend_from_slice(last_c);
            let logits_c = self.audio_head(file, last_c, head)?;
            let logits_u = self.audio_head(file, last_u, head)?;
            let mut guided = logits_c;
            for (c, u) in guided.iter_mut().zip(logits_u.iter()) {
                *c = *u + MUSIC3_AR_CFG * (*c - *u);
            }
            let code = sample_top_k(&guided, top_k, rng) as u32;
            codes.push(code);
            if head + 1 < MUSIC3_NUM_CODEBOOKS - 1 {
                let embed = self.audio_embed(file, head, code)?;
                let proj = self.project(file, &embed)?;
                seq_c.extend_from_slice(&proj);
                seq_u.extend_from_slice(&proj);
            }
        }
        Ok((codes, parts))
    }
}

/// Embed one complete RVQ frame for LM feedback.
/// `semantic_token` is the raw LM token (`code + AUDIO_CODE_OFFSET`).
pub fn music3_gguf_embed_audio_frame(
    lm: &Music3GgufFile,
    rvq: &Music3GgufFile,
    rvq_mod: &Music3GgufRvq,
    semantic_token: u32,
    resid: &[u32],
) -> Result<Vec<f32>> {
    if resid.len() != MUSIC3_NUM_CODEBOOKS - 1 {
        return Err(DiffusionError::model("music3 gguf audio frame residual count"));
    }
    let mut embeds = lm.gather_f16_rows("model.embed_tokens.weight", &[semantic_token])?;
    for (i, &code) in resid.iter().enumerate() {
        let extra = rvq_mod.audio_embed(rvq, i, code)?;
        for (e, x) in embeds.iter_mut().zip(extra.iter()) {
            *e += *x;
        }
    }
    let scale = (MUSIC3_NUM_CODEBOOKS as f32).sqrt().recip();
    for e in &mut embeds {
        *e *= scale;
    }
    Ok(embeds)
}

// ---------------------------------------------------------------------------
// Qwen3-8B Q4_0 prefill (official inner `language_model.model(...)`).
// ---------------------------------------------------------------------------

pub struct Music3GgufLm {
    pub(crate) input_norm: Vec<Vec<f32>>,
    pub(crate) post_attn_norm: Vec<Vec<f32>>,
    pub(crate) q_norm: Vec<Vec<f32>>,
    pub(crate) k_norm: Vec<Vec<f32>>,
    pub(crate) final_norm: Vec<f32>,
    pub(crate) rope_inv_freq: Vec<f32>,
    audio_head: RefCell<Option<AudioHeadCache>>,
}

impl Music3GgufLm {
    pub fn prepare(file: &Music3GgufFile) -> Result<Self> {
        let mut input_norm = Vec::with_capacity(MUSIC3_LM_LAYERS);
        let mut post_attn_norm = Vec::with_capacity(MUSIC3_LM_LAYERS);
        let mut q_norm = Vec::with_capacity(MUSIC3_LM_LAYERS);
        let mut k_norm = Vec::with_capacity(MUSIC3_LM_LAYERS);
        for layer in 0..MUSIC3_LM_LAYERS {
            let p = format!("model.layers.{layer}");
            input_norm.push(file.read_f32_any(&format!("{p}.input_layernorm.weight"))?);
            post_attn_norm.push(file.read_f32_any(&format!("{p}.post_attention_layernorm.weight"))?);
            q_norm.push(file.read_f32_any(&format!("{p}.self_attn.q_norm.weight"))?);
            k_norm.push(file.read_f32_any(&format!("{p}.self_attn.k_norm.weight"))?);
        }
        let half = MUSIC3_LM_HEAD_DIM / 2;
        let mut rope_inv_freq = Vec::with_capacity(half);
        for j in 0..half {
            rope_inv_freq
                .push(1.0 / MUSIC3_LM_ROPE_THETA.powf(2.0 * j as f32 / MUSIC3_LM_HEAD_DIM as f32));
        }
        Ok(Self {
            input_norm,
            post_attn_norm,
            q_norm,
            k_norm,
            final_norm: file.read_f32_any("model.norm.weight")?,
            rope_inv_freq,
            audio_head: RefCell::new(None),
        })
    }

    pub fn embed(&self, file: &Music3GgufFile, ids: &[u32]) -> Result<Vec<f32>> {
        let hidden = file.gather_rows("model.embed_tokens.weight", ids)?;
        if hidden.len() != ids.len() * MUSIC3_LM_HIDDEN {
            return Err(DiffusionError::model(format!(
                "music3 gguf embed {} for {} tokens",
                hidden.len(),
                ids.len()
            )));
        }
        Ok(hidden)
    }

    /// Inner `language_model.model(...)`: tokens → `[T, 4096]` after final RMSNorm.
    /// `layers` caps the stack (official is 36).
    pub fn prefill(
        &self,
        file: &Music3GgufFile,
        ids: &[u32],
        layers: usize,
    ) -> Result<Vec<f32>> {
        let n = ids.len();
        if n == 0 {
            return Err(DiffusionError::workflow("music3 gguf LM prefill: empty ids"));
        }
        let layers = layers.min(MUSIC3_LM_LAYERS).max(1);
        let mut hidden = self.embed(file, ids)?;
        let half = MUSIC3_LM_HEAD_DIM / 2;
        let mut cos = vec![0f32; n * half];
        let mut sin = vec![0f32; n * half];
        for pos in 0..n {
            for j in 0..half {
                let angle = pos as f32 * self.rope_inv_freq[j];
                cos[pos * half + j] = angle.cos();
                sin[pos * half + j] = angle.sin();
            }
        }
        let q_inner = MUSIC3_LM_HEADS * MUSIC3_LM_HEAD_DIM;
        let kv_inner = MUSIC3_LM_KV_HEADS * MUSIC3_LM_HEAD_DIM;
        let group = MUSIC3_LM_HEADS / MUSIC3_LM_KV_HEADS;
        let scale = 1.0 / (MUSIC3_LM_HEAD_DIM as f32).sqrt();
        for layer in 0..layers {
            let p = format!("model.layers.{layer}");
            let mut normed = hidden.clone();
            rms_norm_rows(
                &mut normed,
                &self.input_norm[layer],
                MUSIC3_LM_HIDDEN,
                MUSIC3_LM_RMS_EPS,
            );
            let qn = format!("{p}.self_attn.q_proj.weight");
            let kn = format!("{p}.self_attn.k_proj.weight");
            let vn = format!("{p}.self_attn.v_proj.weight");
            let (mut q, mut k, v) = file.linear_nt_qkv(&qn, &kn, &vn, &normed, n)?;
            if q.len() != n * q_inner || k.len() != n * kv_inner || v.len() != n * kv_inner {
                return Err(DiffusionError::model(format!(
                    "music3 gguf LM layer {layer} qkv {}/{}/{}",
                    q.len(),
                    k.len(),
                    v.len()
                )));
            }
            rms_norm_rows(&mut q, &self.q_norm[layer], MUSIC3_LM_HEAD_DIM, MUSIC3_LM_RMS_EPS);
            rms_norm_rows(&mut k, &self.k_norm[layer], MUSIC3_LM_HEAD_DIM, MUSIC3_LM_RMS_EPS);
            apply_rope_half(&mut q, n, MUSIC3_LM_HEADS, MUSIC3_LM_HEAD_DIM, &cos, &sin);
            apply_rope_half(&mut k, n, MUSIC3_LM_KV_HEADS, MUSIC3_LM_HEAD_DIM, &cos, &sin);
            let k_full = repeat_kv(&k, n, MUSIC3_LM_KV_HEADS, group, MUSIC3_LM_HEAD_DIM);
            let v_full = repeat_kv(&v, n, MUSIC3_LM_KV_HEADS, group, MUSIC3_LM_HEAD_DIM);
            let attn = causal_attn(
                &q,
                &k_full,
                &v_full,
                n,
                MUSIC3_LM_HEADS,
                MUSIC3_LM_HEAD_DIM,
                scale,
            );
            let attn = file.linear_nt(&format!("{p}.self_attn.o_proj.weight"), &attn, n)?;
            add_inplace(&mut hidden, &attn);

            let mut normed = hidden.clone();
            rms_norm_rows(
                &mut normed,
                &self.post_attn_norm[layer],
                MUSIC3_LM_HIDDEN,
                MUSIC3_LM_RMS_EPS,
            );
            let un = format!("{p}.mlp.up_proj.weight");
            let gn = format!("{p}.mlp.gate_proj.weight");
            let (up, gate) = file.linear_nt_up_gate(&un, &gn, &normed, n)?;
            if up.len() != n * MUSIC3_LM_FF || gate.len() != n * MUSIC3_LM_FF {
                return Err(DiffusionError::model(format!(
                    "music3 gguf LM layer {layer} ff {}/{}",
                    up.len(),
                    gate.len()
                )));
            }
            let ff = swiglu(&up, &gate);
            let ff = file.linear_nt(&format!("{p}.mlp.down_proj.weight"), &ff, n)?;
            add_inplace(&mut hidden, &ff);
        }
        rms_norm_rows(
            &mut hidden,
            &self.final_norm,
            MUSIC3_LM_HIDDEN,
            MUSIC3_LM_RMS_EPS,
        );
        Ok(hidden)
    }

    /// `lm_head` on the last token. Vocab is 200k Q4_0 — streamed via Metal.
    pub fn head_last(&self, file: &Music3GgufFile, hidden: &[f32], tokens: usize) -> Result<Vec<f32>> {
        if tokens == 0 || hidden.len() != tokens * MUSIC3_LM_HIDDEN {
            return Err(DiffusionError::model(format!(
                "music3 gguf lm_head hidden {} for {tokens} tokens",
                hidden.len()
            )));
        }
        let last = &hidden[(tokens - 1) * MUSIC3_LM_HIDDEN..];
        let logits = file.linear_nt("lm_head.weight", last, 1)?;
        if logits.len() != MUSIC3_LM_VOCAB {
            return Err(DiffusionError::model(format!(
                "music3 gguf lm_head {} logits, expected {MUSIC3_LM_VOCAB}",
                logits.len()
            )));
        }
        Ok(logits)
    }

    /// Cond+uncond audio head in one `m=2` GEMV: the 134 MB sliced-head
    /// sidecar is read once per frame instead of twice. Same `mul_mv_ext`
    /// per-row reduction as `m=1` (nxpsg picks by `ne00`, `ne11 < 3`), so
    /// rows are bit-identical to two single calls.
    pub fn head_audio_pair(
        &self,
        file: &Music3GgufFile,
        cond_h: &[f32],
        uncond_h: &[f32],
    ) -> Result<(f32, Vec<f32>, f32, Vec<f32>)> {
        if cond_h.len() != MUSIC3_LM_HIDDEN || uncond_h.len() != MUSIC3_LM_HIDDEN {
            return Err(DiffusionError::model("music3 gguf lm_head_audio pair"));
        }
        if file.has_tensor("lm_head_sliced.weight") {
            let mut h2 = Vec::with_capacity(2 * MUSIC3_LM_HIDDEN);
            h2.extend_from_slice(cond_h);
            h2.extend_from_slice(uncond_h);
            let y = file.linear_nt("lm_head_sliced.weight", &h2, 2)?;
            let w = 1 + MUSIC3_SEMANTIC_VOCAB;
            if y.len() != 2 * w {
                return Err(DiffusionError::model(format!(
                    "music3 gguf sliced head pair {}",
                    y.len()
                )));
            }
            return Ok((y[0], y[1..w].to_vec(), y[w], y[w + 1..].to_vec()));
        }
        let (end_c, sem_c) = self.head_audio(file, cond_h)?;
        let (end_u, sem_u) = self.head_audio(file, uncond_h)?;
        Ok((end_c, sem_c, end_u, sem_u))
    }

    /// GEMV only `<|audio_end|>` + the 16384-entry semantic codebook.
    pub fn head_audio(&self, file: &Music3GgufFile, hidden: &[f32]) -> Result<(f32, Vec<f32>)> {
        if hidden.len() != MUSIC3_LM_HIDDEN {
            return Err(DiffusionError::model("music3 gguf lm_head_audio hidden"));
        }
        if file.has_tensor("lm_head_sliced.weight") {
            let y = file.linear_nt("lm_head_sliced.weight", hidden, 1)?;
            if y.len() != 1 + MUSIC3_SEMANTIC_VOCAB {
                return Err(DiffusionError::model(format!(
                    "music3 gguf sliced head {}",
                    y.len()
                )));
            }
            return Ok((y[0], y[1..].to_vec()));
        }
        let cache = self.ensure_audio_head(file)?;
        let end = dot(hidden, &cache.end_row);
        let mut sem = vec![0f32; MUSIC3_SEMANTIC_VOCAB];
        for (i, slot) in sem.iter_mut().enumerate() {
            let w = &cache.sem_w[i * MUSIC3_LM_HIDDEN..(i + 1) * MUSIC3_LM_HIDDEN];
            *slot = dot(hidden, w);
        }
        Ok((end, sem))
    }

    fn ensure_audio_head(&self, file: &Music3GgufFile) -> Result<std::cell::Ref<'_, AudioHeadCache>> {
        if self.audio_head.borrow().is_none() {
            let width = MUSIC3_LM_HIDDEN;
            let expect = (1 + MUSIC3_SEMANTIC_VOCAB) * width;
            // joemattie Q8 pack stores only audio_end + 16384 semantic rows.
            let rows = if file.has_tensor("lm_head_sliced.weight") {
                file.read_f32_any("lm_head_sliced.weight")?
            } else {
                let end = MUSIC3_AUDIO_END_TOKEN_ID;
                let lo = MUSIC3_AUDIO_CODE_OFFSET;
                let mut ids = Vec::with_capacity(1 + MUSIC3_SEMANTIC_VOCAB);
                ids.push(end);
                for i in 0..MUSIC3_SEMANTIC_VOCAB {
                    ids.push(lo + i as u32);
                }
                file.gather_rows("lm_head.weight", &ids)?
            };
            if rows.len() != expect {
                return Err(DiffusionError::model(format!(
                    "music3 gguf audio head rows {} expected {expect}",
                    rows.len()
                )));
            }
            let end_row = rows[..width].to_vec();
            let sem_w = rows[width..].to_vec();
            *self.audio_head.borrow_mut() = Some(AudioHeadCache { end_row, sem_w });
        }
        Ok(std::cell::Ref::map(self.audio_head.borrow(), |s| {
            s.as_ref().unwrap()
        }))
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| *x * *y).sum()
}

struct AudioHeadCache {
    end_row: Vec<f32>,
    sem_w: Vec<f32>,
}

pub fn topk_ids(logits: &[f32], k: usize) -> Vec<(usize, f32)> {
    let mut idx: Vec<usize> = (0..logits.len())
        .filter(|&i| logits[i].is_finite())
        .collect();
    idx.sort_by(|&a, &b| {
        logits[b]
            .partial_cmp(&logits[a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    idx.truncate(k.max(1));
    idx.into_iter().map(|i| (i, logits[i])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_count_nans() {
        let s = FiniteStats::of(&[1.0, f32::NAN, f32::INFINITY, -2.0]);
        assert_eq!(s.finite, 2);
        assert_eq!(s.nan, 1);
        assert_eq!(s.inf, 1);
        assert_eq!(s.absmax, 2.0);
    }

    #[test]
    fn rope_and_repeat_kv_shapes() {
        let mut x = vec![0f32; 2 * 2 * 4];
        x[0] = 1.0;
        x[2] = 1.0;
        let cos = vec![1.0, 0.0, 1.0, 0.0];
        let sin = vec![0.0, 1.0, 0.0, 1.0];
        apply_rope_half(&mut x, 2, 2, 4, &cos, &sin);
        let kv = vec![1f32; 2 * 1 * 4];
        let full = repeat_kv(&kv, 2, 1, 4, 4);
        assert_eq!(full.len(), 2 * 4 * 4);
        assert!(full.iter().all(|v| *v == 1.0));
    }
}
