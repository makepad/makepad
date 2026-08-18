//! TRELLIS 2 flow DiT trunk (shared by the SS flow and the sparse SLAT
//! flows): 30 pre-LN blocks of AdaLN-Zero (share_mod) self-attention with
//! per-head QK RMS norm + interleaved 3D rope, an UNGATED cross-attention to
//! the DINOv3 image condition, and a tanh-GELU FFN. Weight residency follows
//! the H3 pattern: bf16 trunk gemms stream into the per-model device cache on
//! first touch; the deliberately-f32 modules (input_layer / out_layer) are
//! resident f32 tensors; the timestep MLP + shared modulation run on the host
//! in f32, mirroring the reference's mixed-precision contract.
//!
//! Batch is always 1 (the pipeline never batches), so a "forward" is one
//! packed token sequence — dense 4096 tokens for SS, the voxel token list for
//! SLAT. The caller owns rope tables (grid coords for SS, voxel coords for
//! SLAT) and the device-resident cond tensor.

use crate::backend::{
    gpu_add, gpu_attention_cross_fused_enabled, gpu_attention_packed, gpu_attention_packed_cross,
    gpu_download, gpu_gated_residual_mod,
    gpu_gelu, gpu_layer_norm_mod, gpu_layer_norm_mul_add, gpu_linear_f32_resident,
    gpu_linear_nt_cached, gpu_rms_norm_mul_perhead, gpu_rope_interleaved, gpu_slice_cols,
    gpu_to_f16, gpu_upload, gpu_weight_cache_ensure, GpuLinearPart, GpuTensor,
};
use crate::h3::h3_timestep_embedding;
use crate::trellis::{
    T2RopeTables, TrellisWeights, T2_COND_CHANNELS, T2_DEPTH, T2_FFN_DIM, T2_FINAL_NORM_EPS,
    T2_HEAD_COUNT, T2_HEAD_DIM, T2_MODEL_CHANNELS, T2_NORM_EPS,
};
use crate::{DiffusionError, Result};
use makepad_mlx::MlxDType;
use makepad_ggml::quant::{GGML_TYPE_BF16, GGML_TYPE_F16};

const MOD_COLS: usize = 6 * T2_MODEL_CHANNELS; // 9216 per block
const T_FREQ_DIM: usize = 256;

/// Host + device weights for one flow model, prepared once. `namespace`
/// scopes the device weight cache so all three DiTs stay resident together.
pub struct T2Dit {
    pub weights: TrellisWeights,
    pub namespace: &'static str,
    pub in_channels: usize,
    pub out_channels: usize,
    input_w: GpuTensor,
    input_b: GpuTensor,
    out_w: GpuTensor,
    out_b: GpuTensor,
    t_mlp0_w: Vec<f32>,
    t_mlp0_b: Vec<f32>,
    t_mlp2_w: Vec<f32>,
    t_mlp2_b: Vec<f32>,
    admod_w: Vec<f32>,
    admod_b: Vec<f32>,
    block_modulation: Vec<Vec<f32>>,
    norm2_w: Vec<Vec<f32>>,
    norm2_b: Vec<Vec<f32>>,
    self_qkv_b: Vec<Vec<f32>>,
    self_out_b: Vec<Vec<f32>>,
    self_qn: Vec<Vec<f32>>,
    self_kn: Vec<Vec<f32>>,
    cross_q_b: Vec<Vec<f32>>,
    cross_kv_b: Vec<Vec<f32>>,
    cross_out_b: Vec<Vec<f32>>,
    cross_qn: Vec<Vec<f32>>,
    cross_kn: Vec<Vec<f32>>,
    mlp0_b: Vec<Vec<f32>>,
    mlp2_b: Vec<Vec<f32>>,
    final_norm_ones: Vec<f32>,
    final_norm_zeros: Vec<f32>,
}

#[derive(Default)]
pub struct T2DitDebug {
    pub t_emb: Option<Vec<f32>>,
    pub admod: Option<Vec<f32>>,
    pub block_outputs: Vec<(usize, Vec<f32>)>,
}

/// Per-stage cross-attention KV cache. The image condition is FIXED across
/// every sampler step of a stage, so each block's post-rms K and raw V are
/// computed once per (stage, cond) instead of per forward — without this,
/// 30 to_kv gemms + k-rms passes per forward are recomputed for identical
/// inputs. One cache per cond tensor (pos and neg get their own).
#[derive(Default)]
pub struct T2CrossKv {
    layers: Vec<Option<(GpuTensor, GpuTensor)>>,
}

/// T2_KV_CACHE=0 disables the per-stage cross KV cache (A/B knob).
pub fn t2_kv_cache_enabled() -> bool {
    match std::env::var("T2_KV_CACHE") {
        Ok(value) => value != "0",
        Err(_) => true,
    }
}

fn host_f32(weights: &TrellisWeights, name: &str, len: usize) -> Result<Vec<f32>> {
    let values = weights.tensor_f32(name)?;
    if values.len() != len {
        return Err(DiffusionError::model(format!(
            "trellis tensor '{name}' expected {len} values, got {}",
            values.len()
        )));
    }
    Ok(values)
}

fn upload_f32(weights: &TrellisWeights, name: &str, rows: usize, cols: usize) -> Result<GpuTensor> {
    let values = host_f32(weights, name, rows * cols)?;
    gpu_upload(&values, rows, cols).map_err(DiffusionError::model)
}

impl T2Dit {
    pub fn prepare(
        weights: TrellisWeights,
        namespace: &'static str,
        in_channels: usize,
        out_channels: usize,
    ) -> Result<Self> {
        let c = T2_MODEL_CHANNELS;
        let mut block_modulation = Vec::with_capacity(T2_DEPTH);
        let mut norm2_w = Vec::with_capacity(T2_DEPTH);
        let mut norm2_b = Vec::with_capacity(T2_DEPTH);
        let mut self_qkv_b = Vec::with_capacity(T2_DEPTH);
        let mut self_out_b = Vec::with_capacity(T2_DEPTH);
        let mut self_qn = Vec::with_capacity(T2_DEPTH);
        let mut self_kn = Vec::with_capacity(T2_DEPTH);
        let mut cross_q_b = Vec::with_capacity(T2_DEPTH);
        let mut cross_kv_b = Vec::with_capacity(T2_DEPTH);
        let mut cross_out_b = Vec::with_capacity(T2_DEPTH);
        let mut cross_qn = Vec::with_capacity(T2_DEPTH);
        let mut cross_kn = Vec::with_capacity(T2_DEPTH);
        let mut mlp0_b = Vec::with_capacity(T2_DEPTH);
        let mut mlp2_b = Vec::with_capacity(T2_DEPTH);
        for layer in 0..T2_DEPTH {
            let p = format!("blocks.{layer}");
            block_modulation.push(host_f32(&weights, &format!("{p}.modulation"), MOD_COLS)?);
            norm2_w.push(host_f32(&weights, &format!("{p}.norm2.weight"), c)?);
            norm2_b.push(host_f32(&weights, &format!("{p}.norm2.bias"), c)?);
            self_qkv_b.push(host_f32(&weights, &format!("{p}.self_attn.to_qkv.bias"), 3 * c)?);
            self_out_b.push(host_f32(&weights, &format!("{p}.self_attn.to_out.bias"), c)?);
            self_qn.push(host_f32(&weights, &format!("{p}.self_attn.q_rms_norm.gamma"), c)?);
            self_kn.push(host_f32(&weights, &format!("{p}.self_attn.k_rms_norm.gamma"), c)?);
            cross_q_b.push(host_f32(&weights, &format!("{p}.cross_attn.to_q.bias"), c)?);
            cross_kv_b.push(host_f32(&weights, &format!("{p}.cross_attn.to_kv.bias"), 2 * c)?);
            cross_out_b.push(host_f32(&weights, &format!("{p}.cross_attn.to_out.bias"), c)?);
            cross_qn.push(host_f32(&weights, &format!("{p}.cross_attn.q_rms_norm.gamma"), c)?);
            cross_kn.push(host_f32(&weights, &format!("{p}.cross_attn.k_rms_norm.gamma"), c)?);
            mlp0_b.push(host_f32(&weights, &format!("{p}.mlp.mlp.0.bias"), T2_FFN_DIM)?);
            mlp2_b.push(host_f32(&weights, &format!("{p}.mlp.mlp.2.bias"), c)?);
        }
        Ok(Self {
            input_w: upload_f32(&weights, "input_layer.weight", c, in_channels)?,
            input_b: upload_f32(&weights, "input_layer.bias", 1, c)?,
            out_w: upload_f32(&weights, "out_layer.weight", out_channels, c)?,
            out_b: upload_f32(&weights, "out_layer.bias", 1, out_channels)?,
            t_mlp0_w: host_f32(&weights, "t_embedder.mlp.0.weight", c * T_FREQ_DIM)?,
            t_mlp0_b: host_f32(&weights, "t_embedder.mlp.0.bias", c)?,
            t_mlp2_w: host_f32(&weights, "t_embedder.mlp.2.weight", c * c)?,
            t_mlp2_b: host_f32(&weights, "t_embedder.mlp.2.bias", c)?,
            admod_w: host_f32(&weights, "adaLN_modulation.1.weight", MOD_COLS * c)?,
            admod_b: host_f32(&weights, "adaLN_modulation.1.bias", MOD_COLS)?,
            block_modulation,
            norm2_w,
            norm2_b,
            self_qkv_b,
            self_out_b,
            self_qn,
            self_kn,
            cross_q_b,
            cross_kv_b,
            cross_out_b,
            cross_qn,
            cross_kn,
            mlp0_b,
            mlp2_b,
            final_norm_ones: vec![1.0; c],
            final_norm_zeros: vec![0.0; c],
            weights,
            namespace,
            in_channels,
            out_channels,
        })
    }

    /// Stream-ensure one trunk linear weight into the device cache and hand
    /// back the empty-bytes part for the cached gemm.
    fn ensure_linear<'a>(&self, name: &'a str, n: usize, k: usize) -> Result<GpuLinearPart<'a>> {
        let (dtype, _shape) = self.weights.tensor_dtype_shape(name)?;
        let ggml_type = match dtype {
            MlxDType::BF16 => GGML_TYPE_BF16,
            MlxDType::F16 => GGML_TYPE_F16,
            other => {
                return Err(DiffusionError::model(format!(
                    "trellis trunk weight '{name}' has unsupported dtype {other:?}"
                )))
            }
        };
        let want_a16 = crate::backend::gpu_gemm_f16acc_enabled();
        gpu_weight_cache_ensure(self.namespace, name, ggml_type, n, k, want_a16, || {
            self.weights.tensor_bytes(name).map_err(|err| err.to_string())
        })
        .map_err(DiffusionError::model)?;
        Ok(GpuLinearPart {
            bt_ggml_type: ggml_type,
            n,
            cache_key: name,
            bytes: &[],
        })
    }

    fn linear_cached(&self, x: &GpuTensor, name: &str, n: usize, bias: &[f32]) -> Result<GpuTensor> {
        let part = self.ensure_linear(name, n, x.cols())?;
        let parts = [part];
        gpu_linear_nt_cached(x, self.namespace, &parts, bias).map_err(DiffusionError::model)
    }

    /// Host timestep path: sinusoid(1000t) -> mlp -> t_emb, then the shared
    /// AdaLN projection silu(t_emb) -> (6*C) — both f32 like the reference
    /// (t_embedder / adaLN_modulation are NOT part of `blocks`, so they stay
    /// f32 there too).
    fn compute_admod(&self, t1000: f32) -> (Vec<f32>, Vec<f32>) {
        let c = T2_MODEL_CHANNELS;
        let sinusoid = h3_timestep_embedding(&[t1000]);
        debug_assert_eq!(sinusoid.len(), T_FREQ_DIM);
        let mut hidden = host_linear(&sinusoid, T_FREQ_DIM, &self.t_mlp0_w, c, &self.t_mlp0_b);
        host_silu(&mut hidden);
        let t_emb = host_linear(&hidden, c, &self.t_mlp2_w, c, &self.t_mlp2_b);
        let mut silu_temb = t_emb.clone();
        host_silu(&mut silu_temb);
        let admod = host_linear(&silu_temb, c, &self.admod_w, MOD_COLS, &self.admod_b);
        (t_emb, admod)
    }

    /// One full forward: x is (tokens, in_channels) host f32, cond is the
    /// device-resident (cond_len, 1024) condition, rope tables cover every
    /// token. `cross_kv`: per-stage KV cache for this cond (filled on the
    /// first forward, reused after). Returns the (tokens, out_channels)
    /// velocity on the host.
    pub fn forward(
        &self,
        x: &[f32],
        tokens: usize,
        t1000: f32,
        cond: &GpuTensor,
        rope: &(GpuTensor, GpuTensor),
        mut cross_kv: Option<&mut T2CrossKv>,
        debug_blocks: &[usize],
    ) -> Result<(Vec<f32>, T2DitDebug)> {
        let c = T2_MODEL_CHANNELS;
        if x.len() != tokens * self.in_channels {
            return Err(DiffusionError::workflow("t2 dit input shape mismatch"));
        }
        let mut debug = T2DitDebug::default();
        let scale = 1.0 / (T2_HEAD_DIM as f32).sqrt();

        // Input projection (f32 module).
        let x_in = gpu_upload(x, tokens, self.in_channels).map_err(DiffusionError::model)?;
        let mut hidden = gpu_linear_f32_resident(&x_in, &self.input_w, Some(&self.input_b))
            .map_err(DiffusionError::model)?;
        drop(x_in);

        // Shared modulation: one (T2_DEPTH, 9216) device matrix per forward,
        // row i = blocks[i].modulation + adaLN(t_emb).
        let (t_emb, admod) = self.compute_admod(t1000);
        if !debug_blocks.is_empty() {
            debug.t_emb = Some(t_emb);
            debug.admod = Some(admod.clone());
        }
        let mut mods_host = vec![0.0f32; T2_DEPTH * MOD_COLS];
        for layer in 0..T2_DEPTH {
            let row = &mut mods_host[layer * MOD_COLS..(layer + 1) * MOD_COLS];
            for (dst, (base, add)) in row
                .iter_mut()
                .zip(self.block_modulation[layer].iter().zip(admod.iter()))
            {
                *dst = base + add;
            }
        }
        let mods = gpu_upload(&mods_host, T2_DEPTH, MOD_COLS).map_err(DiffusionError::model)?;

        for layer in 0..T2_DEPTH {
            let p = format!("blocks.{layer}");
            let base = layer * MOD_COLS;

            // --- self-attention (AdaLN-modulated, gated) ---
            let normed = gpu_layer_norm_mod(&hidden, &mods, base + c, base, T2_NORM_EPS)
                .map_err(DiffusionError::model)?;
            let qkv = self.linear_cached(
                &normed,
                &format!("{p}.self_attn.to_qkv.weight"),
                3 * c,
                &self.self_qkv_b[layer],
            )?;
            drop(normed);
            let q = gpu_slice_cols(&qkv, 0, c).map_err(DiffusionError::model)?;
            let k = gpu_slice_cols(&qkv, c, c).map_err(DiffusionError::model)?;
            let v = gpu_slice_cols(&qkv, 2 * c, c).map_err(DiffusionError::model)?;
            drop(qkv);
            let q = gpu_rms_norm_mul_perhead(
                &q,
                T2_HEAD_COUNT,
                T2_HEAD_DIM,
                self.namespace,
                &format!("{p}.self_attn.q_rms_norm"),
                &self.self_qn[layer],
                0.0,
            )
            .map_err(DiffusionError::model)?;
            let k = gpu_rms_norm_mul_perhead(
                &k,
                T2_HEAD_COUNT,
                T2_HEAD_DIM,
                self.namespace,
                &format!("{p}.self_attn.k_rms_norm"),
                &self.self_kn[layer],
                0.0,
            )
            .map_err(DiffusionError::model)?;
            let q = gpu_rope_interleaved(&q, T2_HEAD_COUNT, &rope.0, &rope.1)
                .map_err(DiffusionError::model)?;
            let k = gpu_rope_interleaved(&k, T2_HEAD_COUNT, &rope.0, &rope.1)
                .map_err(DiffusionError::model)?;
            let attn = gpu_attention_packed(&q, &k, &v, T2_HEAD_COUNT, scale)
                .map_err(DiffusionError::model)?;
            drop((q, k, v));
            let attn = self.linear_cached(
                &attn,
                &format!("{p}.self_attn.to_out.weight"),
                c,
                &self.self_out_b[layer],
            )?;
            hidden = gpu_gated_residual_mod(&hidden, &attn, &mods, base + 2 * c)
                .map_err(DiffusionError::model)?;
            drop(attn);

            // --- cross-attention (affine LN, ungated, unmodulated) ---
            let normed = gpu_layer_norm_mul_add(
                &hidden,
                &self.norm2_w[layer],
                &self.norm2_b[layer],
                T2_NORM_EPS,
            )
            .map_err(DiffusionError::model)?;
            let q = self.linear_cached(
                &normed,
                &format!("{p}.cross_attn.to_q.weight"),
                c,
                &self.cross_q_b[layer],
            )?;
            drop(normed);
            let q = gpu_rms_norm_mul_perhead(
                &q,
                T2_HEAD_COUNT,
                T2_HEAD_DIM,
                self.namespace,
                &format!("{p}.cross_attn.q_rms_norm"),
                &self.cross_qn[layer],
                0.0,
            )
            .map_err(DiffusionError::model)?;
            // Post-rms K + raw V: from the cache when this (stage, cond) has
            // them, computed (and cached) otherwise.
            let cache_hit = cross_kv
                .as_ref()
                .is_some_and(|cache| cache.layers.get(layer).is_some_and(|e| e.is_some()));
            let mut kv_local: Option<(GpuTensor, GpuTensor)> = None;
            if !cache_hit {
                let kv = self.linear_cached(
                    cond,
                    &format!("{p}.cross_attn.to_kv.weight"),
                    2 * c,
                    &self.cross_kv_b[layer],
                )?;
                let k = gpu_slice_cols(&kv, 0, c).map_err(DiffusionError::model)?;
                let v = gpu_slice_cols(&kv, c, c).map_err(DiffusionError::model)?;
                drop(kv);
                let k = gpu_rms_norm_mul_perhead(
                    &k,
                    T2_HEAD_COUNT,
                    T2_HEAD_DIM,
                    self.namespace,
                    &format!("{p}.cross_attn.k_rms_norm"),
                    &self.cross_kn[layer],
                    0.0,
                )
                .map_err(DiffusionError::model)?;
                match cross_kv.as_mut() {
                    Some(cache) => {
                        // Cache f16 when the fused cross kernel (its native
                        // operand type) will consume it: halves the resident
                        // cache (HR pos+neg would otherwise hold ~3GB f32 on
                        // an already-tight card).
                        let (k, v) = if gpu_attention_cross_fused_enabled() {
                            (
                                gpu_to_f16(&k).map_err(DiffusionError::model)?,
                                gpu_to_f16(&v).map_err(DiffusionError::model)?,
                            )
                        } else {
                            (k, v)
                        };
                        if cache.layers.is_empty() {
                            cache.layers.resize_with(T2_DEPTH, || None);
                        }
                        cache.layers[layer] = Some((k, v));
                    }
                    None => kv_local = Some((k, v)),
                }
            }
            let (k_ref, v_ref): (&GpuTensor, &GpuTensor) = match (&kv_local, cross_kv.as_ref()) {
                (Some((k, v)), _) => (k, v),
                (None, Some(cache)) => {
                    let (k, v) = cache.layers[layer]
                        .as_ref()
                        .ok_or_else(|| DiffusionError::model("cross kv cache slot empty"))?;
                    (k, v)
                }
                (None, None) => {
                    return Err(DiffusionError::model("cross kv unavailable"));
                }
            };
            let cross = gpu_attention_packed_cross(&q, k_ref, v_ref, T2_HEAD_COUNT, scale)
                .map_err(DiffusionError::model)?;
            drop(q);
            drop(kv_local);
            let cross = self.linear_cached(
                &cross,
                &format!("{p}.cross_attn.to_out.weight"),
                c,
                &self.cross_out_b[layer],
            )?;
            hidden = gpu_add(&hidden, &cross).map_err(DiffusionError::model)?;
            drop(cross);

            // --- FFN (AdaLN-modulated, gated) ---
            let normed = gpu_layer_norm_mod(&hidden, &mods, base + 4 * c, base + 3 * c, T2_NORM_EPS)
                .map_err(DiffusionError::model)?;
            let ff = self.linear_cached(
                &normed,
                &format!("{p}.mlp.mlp.0.weight"),
                T2_FFN_DIM,
                &self.mlp0_b[layer],
            )?;
            drop(normed);
            let ff = gpu_gelu(&ff).map_err(DiffusionError::model)?;
            let ff = self.linear_cached(
                &ff,
                &format!("{p}.mlp.mlp.2.weight"),
                c,
                &self.mlp2_b[layer],
            )?;
            hidden = gpu_gated_residual_mod(&hidden, &ff, &mods, base + 5 * c)
                .map_err(DiffusionError::model)?;
            drop(ff);

            if debug_blocks.contains(&layer) {
                debug
                    .block_outputs
                    .push((layer, gpu_download(&hidden).map_err(DiffusionError::model)?));
            }
        }

        // Final weightless layer norm (F.layer_norm default eps) + f32 head.
        let hidden = gpu_layer_norm_mul_add(
            &hidden,
            &self.final_norm_ones,
            &self.final_norm_zeros,
            T2_FINAL_NORM_EPS,
        )
        .map_err(DiffusionError::model)?;
        let out = gpu_linear_f32_resident(&hidden, &self.out_w, Some(&self.out_b))
            .map_err(DiffusionError::model)?;
        let velocity = gpu_download(&out).map_err(DiffusionError::model)?;
        Ok((velocity, debug))
    }
}

/// Upload rope tables once per stage (coords fixed across sampler steps).
pub fn t2_upload_rope(tables: &T2RopeTables) -> Result<(GpuTensor, GpuTensor)> {
    let cos = gpu_upload(&tables.cos, tables.rows, tables.cos.len() / tables.rows)
        .map_err(DiffusionError::model)?;
    let sin = gpu_upload(&tables.sin, tables.rows, tables.sin.len() / tables.rows)
        .map_err(DiffusionError::model)?;
    Ok((cos, sin))
}

/// Upload the (cond_len, 1024) condition once per stage.
pub fn t2_upload_cond(cond: &[f32]) -> Result<GpuTensor> {
    if cond.len() % T2_COND_CHANNELS != 0 {
        return Err(DiffusionError::workflow("t2 cond width mismatch"));
    }
    gpu_upload(cond, cond.len() / T2_COND_CHANNELS, T2_COND_CHANNELS)
        .map_err(DiffusionError::model)
}

fn host_silu(values: &mut [f32]) {
    for value in values.iter_mut() {
        *value = *value / (1.0 + (-*value).exp());
    }
}

/// Single-row linear (the timestep path only ever has one row).
fn host_linear(x: &[f32], k: usize, w: &[f32], n: usize, bias: &[f32]) -> Vec<f32> {
    debug_assert_eq!(x.len(), k);
    debug_assert_eq!(w.len(), n * k);
    let mut out = vec![0.0f32; n];
    for j in 0..n {
        let mut sum = bias[j];
        let wr = &w[j * k..(j + 1) * k];
        for i in 0..k {
            sum += wr[i] * x[i];
        }
        out[j] = sum;
    }
    out
}
