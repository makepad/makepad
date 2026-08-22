//! SA3 Small SFX DiT (459M): 20-block ContinuousTransformer d1024/16h with
//! adaLN global conditioning, 64 learned memory tokens, partial RoPE(32),
//! per-head rms qk-norm, SwiGLU FF, cross-attention over the 257 conditioning
//! tokens and a per-block local-conditioning MLP (zeros input for t2a).
//!
//! CPU f32. Mirrors models/dit.py + models/transformer.py exactly; validated
//! stage-by-stage against local/sa3_ref/dumps by `sa3-validate`.

use crate::sa3::{
    apply_rope, attention, expo_fourier, linear, rms_norm_rows, rope_tables, sigmoid, silu,
    Sa3Tensors, SA3_COND_DIM, SA3_COND_TOKENS, SA3_DIT_DEPTH, SA3_DIT_DIM, SA3_DIT_HEADS,
    SA3_HEAD_DIM, SA3_LATENT_DIM, SA3_LOCAL_COND_DIM, SA3_MEMORY_TOKENS, SA3_NORM_EPS,
    SA3_QK_NORM_EPS, SA3_TIMESTEP_FEATURES,
};
use crate::Result;

/// How latent padding positions participate in self-attention.
/// The reference without flash-attn (the oracle-dump environment) zeroes the
/// V rows of padded keys; flash-attn varlen (box/ComfyUI) masks them out of
/// the softmax entirely. `VZero` reproduces the dumps bit-for-bit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sa3PadMode {
    VZero,
    Additive,
}

struct DitBlock {
    pre_norm: Vec<f32>,
    cross_norm: Vec<f32>,
    ff_norm: Vec<f32>,
    scale_shift_gate: Vec<f32>,
    self_qkv: Vec<f32>,
    self_out: Vec<f32>,
    self_q_norm: Vec<f32>,
    self_k_norm: Vec<f32>,
    cross_q: Vec<f32>,
    cross_kv: Vec<f32>,
    cross_out: Vec<f32>,
    cross_q_norm: Vec<f32>,
    cross_k_norm: Vec<f32>,
    ff_proj_w: Vec<f32>,
    ff_proj_b: Vec<f32>,
    ff_out_w: Vec<f32>,
    ff_out_b: Vec<f32>,
    /// to_local_embed evaluated at the zero t2a local conditioning: a single
    /// constant 1024-vector added to every latent token.
    local_const: Vec<f32>,
}

pub struct Sa3Dit {
    preprocess_conv: Vec<f32>,
    postprocess_conv: Vec<f32>,
    timestep_w0: Vec<f32>,
    timestep_b0: Vec<f32>,
    timestep_w2: Vec<f32>,
    timestep_b2: Vec<f32>,
    cond_w0: Vec<f32>,
    cond_w2: Vec<f32>,
    global_w0: Vec<f32>,
    global_w2: Vec<f32>,
    memory_tokens: Vec<f32>,
    project_in: Vec<f32>,
    project_out: Vec<f32>,
    gce_w0: Vec<f32>,
    gce_b0: Vec<f32>,
    gce_w2: Vec<f32>,
    gce_b2: Vec<f32>,
    blocks: Vec<DitBlock>,
}

impl Sa3Dit {
    /// Loads from the combined SA3 checkpoint (model.model.* prefix).
    pub fn load(t: &Sa3Tensors) -> Result<Self> {
        let d = SA3_DIT_DIM;
        let n = |s: &str| format!("model.model.{s}");
        let mut blocks = Vec::with_capacity(SA3_DIT_DEPTH);
        for i in 0..SA3_DIT_DEPTH {
            let l = |s: &str| n(&format!("transformer.layers.{i}.{s}"));
            // Precompute to_local_embed(zeros): w2 @ silu(b1) + b2.
            let local_w0 = t.f32_shaped(&l("to_local_embed.0.weight"), &[d, SA3_LOCAL_COND_DIM])?;
            let local_b0 = t.f32_shaped(&l("to_local_embed.0.bias"), &[d])?;
            let local_w2 = t.f32_shaped(&l("to_local_embed.2.weight"), &[d, d])?;
            let local_b2 = t.f32_shaped(&l("to_local_embed.2.bias"), &[d])?;
            let _ = local_w0; // zero input contributes nothing through w0
            let hidden: Vec<f32> = local_b0.iter().map(|&v| silu(v)).collect();
            let mut local_const = linear(&hidden, &local_w2, Some(&local_b2), 1, d, d);
            debug_assert_eq!(local_const.len(), d);
            local_const.truncate(d);
            blocks.push(DitBlock {
                pre_norm: t.f32_shaped(&l("pre_norm.gamma"), &[d])?,
                cross_norm: t.f32_shaped(&l("cross_attend_norm.gamma"), &[d])?,
                ff_norm: t.f32_shaped(&l("ff_norm.gamma"), &[d])?,
                scale_shift_gate: t.f32_shaped(&l("to_scale_shift_gate"), &[6 * d])?,
                self_qkv: t.f32_shaped(&l("self_attn.to_qkv.weight"), &[3 * d, d])?,
                self_out: t.f32_shaped(&l("self_attn.to_out.weight"), &[d, d])?,
                self_q_norm: t.f32_shaped(&l("self_attn.q_norm.gamma"), &[SA3_HEAD_DIM])?,
                self_k_norm: t.f32_shaped(&l("self_attn.k_norm.gamma"), &[SA3_HEAD_DIM])?,
                cross_q: t.f32_shaped(&l("cross_attn.to_q.weight"), &[d, d])?,
                cross_kv: t.f32_shaped(&l("cross_attn.to_kv.weight"), &[2 * d, d])?,
                cross_out: t.f32_shaped(&l("cross_attn.to_out.weight"), &[d, d])?,
                cross_q_norm: t.f32_shaped(&l("cross_attn.q_norm.gamma"), &[SA3_HEAD_DIM])?,
                cross_k_norm: t.f32_shaped(&l("cross_attn.k_norm.gamma"), &[SA3_HEAD_DIM])?,
                ff_proj_w: t.f32_shaped(&l("ff.ff.0.proj.weight"), &[8 * d, d])?,
                ff_proj_b: t.f32_shaped(&l("ff.ff.0.proj.bias"), &[8 * d])?,
                ff_out_w: t.f32_shaped(&l("ff.ff.2.weight"), &[d, 4 * d])?,
                ff_out_b: t.f32_shaped(&l("ff.ff.2.bias"), &[d])?,
                local_const,
            });
        }
        Ok(Self {
            preprocess_conv: t.f32_shaped(&n("preprocess_conv.weight"), &[SA3_LATENT_DIM, SA3_LATENT_DIM, 1])?,
            postprocess_conv: t.f32_shaped(&n("postprocess_conv.weight"), &[SA3_LATENT_DIM, SA3_LATENT_DIM, 1])?,
            timestep_w0: t.f32_shaped(&n("to_timestep_embed.0.weight"), &[d, SA3_TIMESTEP_FEATURES])?,
            timestep_b0: t.f32_shaped(&n("to_timestep_embed.0.bias"), &[d])?,
            timestep_w2: t.f32_shaped(&n("to_timestep_embed.2.weight"), &[d, d])?,
            timestep_b2: t.f32_shaped(&n("to_timestep_embed.2.bias"), &[d])?,
            cond_w0: t.f32_shaped(&n("to_cond_embed.0.weight"), &[d, SA3_COND_DIM])?,
            cond_w2: t.f32_shaped(&n("to_cond_embed.2.weight"), &[d, d])?,
            global_w0: t.f32_shaped(&n("to_global_embed.0.weight"), &[d, SA3_COND_DIM])?,
            global_w2: t.f32_shaped(&n("to_global_embed.2.weight"), &[d, d])?,
            memory_tokens: t.f32_shaped(&n("transformer.memory_tokens"), &[SA3_MEMORY_TOKENS, d])?,
            project_in: t.f32_shaped(&n("transformer.project_in.weight"), &[d, SA3_LATENT_DIM])?,
            project_out: t.f32_shaped(&n("transformer.project_out.weight"), &[SA3_LATENT_DIM, d])?,
            gce_w0: t.f32_shaped(&n("transformer.global_cond_embedder.0.weight"), &[d, d])?,
            gce_b0: t.f32_shaped(&n("transformer.global_cond_embedder.0.bias"), &[d])?,
            gce_w2: t.f32_shaped(&n("transformer.global_cond_embedder.2.weight"), &[6 * d, d])?,
            gce_b2: t.f32_shaped(&n("transformer.global_cond_embedder.2.bias"), &[6 * d])?,
            blocks,
        })
    }

    /// to_cond_embed: projects the 257x768 conditioning sequence to 257x1024.
    /// Compute once per generation (t-independent).
    pub fn embed_conditioning(&self, cross_attn_cond: &[f32]) -> Vec<f32> {
        let d = SA3_DIT_DIM;
        let mut h = linear(cross_attn_cond, &self.cond_w0, None, SA3_COND_TOKENS, SA3_COND_DIM, d);
        for v in h.iter_mut() {
            *v = silu(*v);
        }
        linear(&h, &self.cond_w2, None, SA3_COND_TOKENS, d, d)
    }

    /// to_global_embed of the 768-dim seconds conditioning (t-independent).
    pub fn embed_global(&self, global_cond: &[f32]) -> Vec<f32> {
        let d = SA3_DIT_DIM;
        let mut h = linear(global_cond, &self.global_w0, None, 1, SA3_COND_DIM, d);
        for v in h.iter_mut() {
            *v = silu(*v);
        }
        linear(&h, &self.global_w2, None, 1, d, d)
    }

    /// Timestep embedding: ExpoFourier(256) -> Linear+b -> SiLU -> Linear+b.
    pub fn embed_timestep(&self, t: f32) -> Vec<f32> {
        let d = SA3_DIT_DIM;
        let features = expo_fourier(t, SA3_TIMESTEP_FEATURES);
        let mut h = linear(&features, &self.timestep_w0, Some(&self.timestep_b0), 1, SA3_TIMESTEP_FEATURES, d);
        for v in h.iter_mut() {
            *v = silu(*v);
        }
        linear(&h, &self.timestep_w2, Some(&self.timestep_b2), 1, d, d)
    }

    /// One velocity forward. `x` is `[latent_len, 256]` token-major,
    /// `cond_embed` = embed_conditioning(...), `global_embed_pre_t` =
    /// embed_global(...), `valid_len` = attention-valid latent prefix.
    /// Returns v `[latent_len, 256]`.
    pub fn forward(
        &self,
        x: &[f32],
        t: f32,
        cond_embed: &[f32],
        global_embed_pre_t: &[f32],
        latent_len: usize,
        valid_len: usize,
        pad_mode: Sa3PadMode,
    ) -> Result<Vec<f32>> {
        let d = SA3_DIT_DIM;
        let c = SA3_LATENT_DIM;
        let heads = SA3_DIT_HEADS;
        let mem = SA3_MEMORY_TOKENS;
        let seq = mem + latent_len;
        debug_assert_eq!(x.len(), latent_len * c);

        // Global conditioning: to_global_embed(seconds) + timestep embed.
        let temb = self.embed_timestep(t);
        let gemb: Vec<f32> = global_embed_pre_t
            .iter()
            .zip(&temb)
            .map(|(a, b)| a + b)
            .collect();
        // global_cond_embedder -> 6144, shared across blocks.
        let mut g = linear(&gemb, &self.gce_w0, Some(&self.gce_b0), 1, d, d);
        for v in g.iter_mut() {
            *v = silu(*v);
        }
        let gcond = linear(&g, &self.gce_w2, Some(&self.gce_b2), 1, d, 6 * d);

        // preprocess_conv (1x1, no bias) + residual, channels-last rows.
        let conv_w: Vec<f32> = self.preprocess_conv.clone(); // [c_out, c_in]
        let mut x_pre = linear(x, &conv_w, None, latent_len, c, c);
        for (o, i) in x_pre.iter_mut().zip(x) {
            *o += *i;
        }

        // project_in + memory tokens.
        let projected = linear(&x_pre, &self.project_in, None, latent_len, c, d);
        let mut h = vec![0f32; seq * d];
        h[..mem * d].copy_from_slice(&self.memory_tokens);
        h[mem * d..].copy_from_slice(&projected);

        let (rope_cos, rope_sin) = rope_tables(seq);

        // Latent-padding participation (memory tokens always valid).
        let pad_start = mem + valid_len;
        let additive_mask: Option<Vec<f32>> = if valid_len < latent_len
            && pad_mode == Sa3PadMode::Additive
        {
            let mut m = vec![0f32; seq];
            for v in m[pad_start..].iter_mut() {
                *v = f32::NEG_INFINITY;
            }
            Some(m)
        } else {
            None
        };
        let v_zero = valid_len < latent_len && pad_mode == Sa3PadMode::VZero;

        for block in &self.blocks {
            let ssg = &block.scale_shift_gate;
            let ada = |idx: usize, i: usize| ssg[idx * d + i] + gcond[idx * d + i];

            // --- self-attention with adaLN ---
            let mut a = h.clone();
            rms_norm_rows(&mut a, &block.pre_norm, d, SA3_NORM_EPS);
            for row in a.chunks_mut(d) {
                for i in 0..d {
                    row[i] = row[i] * (1.0 + ada(0, i)) + ada(1, i);
                }
            }
            let qkv = linear(&a, &block.self_qkv, None, seq, d, 3 * d);
            let mut q = vec![0f32; seq * d];
            let mut k = vec![0f32; seq * d];
            let mut v = vec![0f32; seq * d];
            for tok in 0..seq {
                let row = &qkv[tok * 3 * d..(tok + 1) * 3 * d];
                q[tok * d..(tok + 1) * d].copy_from_slice(&row[..d]);
                k[tok * d..(tok + 1) * d].copy_from_slice(&row[d..2 * d]);
                v[tok * d..(tok + 1) * d].copy_from_slice(&row[2 * d..]);
            }
            rms_norm_rows(&mut q, &block.self_q_norm, SA3_HEAD_DIM, SA3_QK_NORM_EPS);
            rms_norm_rows(&mut k, &block.self_k_norm, SA3_HEAD_DIM, SA3_QK_NORM_EPS);
            apply_rope(&mut q, &rope_cos, &rope_sin, seq, heads);
            apply_rope(&mut k, &rope_cos, &rope_sin, seq, heads);
            if v_zero {
                for row in v[pad_start * d..].chunks_mut(d) {
                    row.fill(0.0);
                }
            }
            let attn = attention(
                &q,
                &k,
                &v,
                seq,
                seq,
                heads,
                additive_mask.as_deref(),
                1.0 / (SA3_HEAD_DIM as f32).sqrt(),
            );
            let mut out = linear(&attn, &block.self_out, None, seq, d, d);
            for (tok, row) in out.chunks_mut(d).enumerate() {
                let h_row = &mut h[tok * d..(tok + 1) * d];
                for i in 0..d {
                    h_row[i] += row[i] * sigmoid(1.0 - ada(2, i));
                }
            }

            // --- cross-attention (plain residual, no adaLN, no rope) ---
            let mut cnorm = h.clone();
            rms_norm_rows(&mut cnorm, &block.cross_norm, d, SA3_NORM_EPS);
            let mut cq = linear(&cnorm, &block.cross_q, None, seq, d, d);
            let ckv = linear(cond_embed, &block.cross_kv, None, SA3_COND_TOKENS, d, 2 * d);
            let mut ck = vec![0f32; SA3_COND_TOKENS * d];
            let mut cv = vec![0f32; SA3_COND_TOKENS * d];
            for tok in 0..SA3_COND_TOKENS {
                let row = &ckv[tok * 2 * d..(tok + 1) * 2 * d];
                ck[tok * d..(tok + 1) * d].copy_from_slice(&row[..d]);
                cv[tok * d..(tok + 1) * d].copy_from_slice(&row[d..]);
            }
            rms_norm_rows(&mut cq, &block.cross_q_norm, SA3_HEAD_DIM, SA3_QK_NORM_EPS);
            rms_norm_rows(&mut ck, &block.cross_k_norm, SA3_HEAD_DIM, SA3_QK_NORM_EPS);
            let cattn = attention(
                &cq,
                &ck,
                &cv,
                seq,
                SA3_COND_TOKENS,
                heads,
                None,
                1.0 / (SA3_HEAD_DIM as f32).sqrt(),
            );
            let cout = linear(&cattn, &block.cross_out, None, seq, d, d);
            for i in 0..h.len() {
                h[i] += cout[i];
            }

            // --- local conditioning (constant over latent tokens for t2a) ---
            for tok in mem..seq {
                let row = &mut h[tok * d..(tok + 1) * d];
                for i in 0..d {
                    row[i] += block.local_const[i];
                }
            }

            // --- feedforward with adaLN (SwiGLU) ---
            let mut f = h.clone();
            rms_norm_rows(&mut f, &block.ff_norm, d, SA3_NORM_EPS);
            for row in f.chunks_mut(d) {
                for i in 0..d {
                    row[i] = row[i] * (1.0 + ada(3, i)) + ada(4, i);
                }
            }
            let proj = linear(&f, &block.ff_proj_w, Some(&block.ff_proj_b), seq, d, 8 * d);
            let inner_dim = 4 * d;
            let mut inner = vec![0f32; seq * inner_dim];
            for tok in 0..seq {
                let row = &proj[tok * 8 * d..(tok + 1) * 8 * d];
                let out_row = &mut inner[tok * inner_dim..(tok + 1) * inner_dim];
                for i in 0..inner_dim {
                    out_row[i] = row[i] * silu(row[inner_dim + i]);
                }
            }
            let ff_out = linear(&inner, &block.ff_out_w, Some(&block.ff_out_b), seq, inner_dim, d);
            for (tok, row) in ff_out.chunks(d).enumerate() {
                let h_row = &mut h[tok * d..(tok + 1) * d];
                for i in 0..d {
                    h_row[i] += row[i] * sigmoid(1.0 - ada(5, i));
                }
            }
        }

        // Strip memory tokens, project out, postprocess conv residual.
        let latent_h = &h[mem * d..];
        let mut out = linear(latent_h, &self.project_out, None, latent_len, d, c);
        let conv_w: Vec<f32> = self.postprocess_conv.clone();
        let conv = linear(&out, &conv_w, None, latent_len, c, c);
        for (o, cv) in out.iter_mut().zip(conv) {
            *o += cv;
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// CUDA device path (f16 cached weights, f32 activations).
// ---------------------------------------------------------------------------

use crate::sa3::{dev_err, F16Weight, SA3_ROPE_DIM};
use makepad_ai_common::backend::cuda::{
    gpu_add, gpu_attention_packed, gpu_attention_packed_cross, gpu_download,
    gpu_gated_residual_mod, gpu_linear_nt_cached, gpu_rms_norm_mod_indexed, gpu_rms_norm_mul,
    gpu_rope_half, gpu_slice_cols, gpu_slice_rows, gpu_swiglu_value_gate, gpu_upload,
    gpu_upload_u32, GpuTensor,
};

struct DitDeviceBlock {
    qkv: F16Weight,
    self_out: F16Weight,
    cross_q: F16Weight,
    cross_kv: F16Weight,
    cross_out: F16Weight,
    ff_proj: F16Weight,
    ff_out: F16Weight,
}

/// Prepared f16 device weights for the DiT (the small f32 vectors stay on
/// the host inside `Sa3Dit` and are read through `&self` at forward time).
pub struct Sa3DitDevice {
    preprocess: F16Weight,
    postprocess: F16Weight,
    project_in: F16Weight,
    project_out: F16Weight,
    blocks: Vec<DitDeviceBlock>,
}

/// Per-generation device state (conditioning residents, rope tables, local
/// conditioning tensors, block-index tensors for the AdaLN table).
pub struct Sa3DitDeviceRun {
    latent_len: usize,
    valid_len: usize,
    seq: usize,
    /// Per block: rms-normed cross K and raw cross V, resident (cond is
    /// t-independent, so cross K/V are computed once per generation).
    cross_kv: Vec<(GpuTensor, GpuTensor)>,
    global_embed: Vec<f32>,
    memory: GpuTensor,
    rope_cos: GpuTensor,
    rope_sin: GpuTensor,
    /// Per block: [seq, 1024] local-conditioning addend (zeros over the
    /// memory tokens, the block's local_const over latent tokens).
    local_add: Vec<GpuTensor>,
    /// Per block: u32 [seq] filled with the block index (AdaLN table row).
    block_idx: Vec<GpuTensor>,
    /// Per block gammas resident for the mod-norms.
    pre_norm_w: Vec<GpuTensor>,
    ff_norm_w: Vec<GpuTensor>,
}

impl Sa3Dit {
    /// Converts the big linears for the CUDA path.
    pub fn prepare_device(&self) -> Sa3DitDevice {
        let d = SA3_DIT_DIM;
        let c = SA3_LATENT_DIM;
        let blocks = self
            .blocks
            .iter()
            .enumerate()
            .map(|(i, block)| DitDeviceBlock {
                qkv: F16Weight::new(format!("sa3dit.{i}.qkv"), &block.self_qkv, 3 * d, d),
                self_out: F16Weight::new(format!("sa3dit.{i}.so"), &block.self_out, d, d),
                cross_q: F16Weight::new(format!("sa3dit.{i}.cq"), &block.cross_q, d, d),
                cross_kv: F16Weight::new(format!("sa3dit.{i}.ckv"), &block.cross_kv, 2 * d, d),
                cross_out: F16Weight::new(format!("sa3dit.{i}.co"), &block.cross_out, d, d),
                ff_proj: F16Weight::new(format!("sa3dit.{i}.fp"), &block.ff_proj_w, 8 * d, d),
                ff_out: F16Weight::new(format!("sa3dit.{i}.fo"), &block.ff_out_w, d, 4 * d),
            })
            .collect();
        Sa3DitDevice {
            preprocess: F16Weight::new("sa3dit.pre", &self.preprocess_conv, c, c),
            postprocess: F16Weight::new("sa3dit.post", &self.postprocess_conv, c, c),
            project_in: F16Weight::new("sa3dit.pin", &self.project_in, d, c),
            project_out: F16Weight::new("sa3dit.pout", &self.project_out, c, d),
            blocks,
        }
    }

    /// Builds the per-generation device state from host conditioning
    /// (`cond_embed` = embed_conditioning output, 257x1024).
    pub fn begin_device_run(
        &self,
        device: &Sa3DitDevice,
        cond_embed: &[f32],
        global_embed_pre_t: &[f32],
        latent_len: usize,
        valid_len: usize,
    ) -> Result<Sa3DitDeviceRun> {
        let d = SA3_DIT_DIM;
        let mem = SA3_MEMORY_TOKENS;
        let seq = mem + latent_len;

        let cond = gpu_upload(cond_embed, SA3_COND_TOKENS, d).map_err(|e| dev_err("dit cond upload", e))?;
        let mut cross_kv = Vec::with_capacity(self.blocks.len());
        for (i, block) in self.blocks.iter().enumerate() {
            let kv = gpu_linear_nt_cached(&cond, "sa3dit", &[device.blocks[i].cross_kv.part()], &[])
                .map_err(|e| dev_err("dit cross kv", e))?;
            let k = gpu_slice_cols(&kv, 0, d).map_err(|e| dev_err("dit cross k slice", e))?;
            let v = gpu_slice_cols(&kv, d, d).map_err(|e| dev_err("dit cross v slice", e))?;
            let k = gpu_rms_norm_mul(
                &k,
                SA3_HEAD_DIM,
                "sa3dit",
                &format!("b{i}.ckn"),
                &block.cross_k_norm,
                SA3_QK_NORM_EPS,
            )
            .map_err(|e| dev_err("dit cross k norm", e))?;
            cross_kv.push((k, v));
        }

        let memory = gpu_upload(&self.memory_tokens, mem, d).map_err(|e| dev_err("dit memory", e))?;

        let (cos_dup, sin_dup) = rope_tables(seq);
        // rope_half wants the unique half (16 entries) per position.
        let half = SA3_ROPE_DIM / 2;
        let mut cos = vec![0f32; seq * half];
        let mut sin = vec![0f32; seq * half];
        for pos in 0..seq {
            cos[pos * half..(pos + 1) * half]
                .copy_from_slice(&cos_dup[pos * SA3_ROPE_DIM..pos * SA3_ROPE_DIM + half]);
            sin[pos * half..(pos + 1) * half]
                .copy_from_slice(&sin_dup[pos * SA3_ROPE_DIM..pos * SA3_ROPE_DIM + half]);
        }
        let rope_cos = gpu_upload(&cos, seq, half).map_err(|e| dev_err("dit rope cos", e))?;
        let rope_sin = gpu_upload(&sin, seq, half).map_err(|e| dev_err("dit rope sin", e))?;

        let mut local_add = Vec::with_capacity(self.blocks.len());
        let mut block_idx = Vec::with_capacity(self.blocks.len());
        let mut pre_norm_w = Vec::with_capacity(self.blocks.len());
        let mut ff_norm_w = Vec::with_capacity(self.blocks.len());
        for (i, block) in self.blocks.iter().enumerate() {
            let mut add = vec![0f32; seq * d];
            for tok in mem..seq {
                add[tok * d..(tok + 1) * d].copy_from_slice(&block.local_const);
            }
            local_add.push(gpu_upload(&add, seq, d).map_err(|e| dev_err("dit local add", e))?);
            block_idx.push(
                gpu_upload_u32(&vec![i as u32; seq]).map_err(|e| dev_err("dit block idx", e))?,
            );
            pre_norm_w.push(gpu_upload(&block.pre_norm, 1, d).map_err(|e| dev_err("dit pre norm w", e))?);
            ff_norm_w.push(gpu_upload(&block.ff_norm, 1, d).map_err(|e| dev_err("dit ff norm w", e))?);
        }

        Ok(Sa3DitDeviceRun {
            latent_len,
            valid_len,
            seq,
            cross_kv,
            global_embed: global_embed_pre_t.to_vec(),
            memory,
            rope_cos,
            rope_sin,
            local_add,
            block_idx,
            pre_norm_w,
            ff_norm_w,
        })
    }

    /// One velocity forward on the device. Same contract as `forward`.
    pub fn forward_device(
        &self,
        device: &Sa3DitDevice,
        run: &Sa3DitDeviceRun,
        x: &[f32],
        t: f32,
    ) -> Result<Vec<f32>> {
        let d = SA3_DIT_DIM;
        let c = SA3_LATENT_DIM;
        let heads = SA3_DIT_HEADS;
        let mem = SA3_MEMORY_TOKENS;
        let latent_len = run.latent_len;
        let seq = run.seq;

        // Host: timestep + AdaLN table for all blocks (raw scales/shifts,
        // gates pre-transformed to sigmoid(1 - gate)).
        let temb = self.embed_timestep(t);
        let gemb: Vec<f32> = run
            .global_embed
            .iter()
            .zip(&temb)
            .map(|(a, b)| a + b)
            .collect();
        let mut g = linear(&gemb, &self.gce_w0, Some(&self.gce_b0), 1, d, d);
        for v in g.iter_mut() {
            *v = silu(*v);
        }
        let gcond = linear(&g, &self.gce_w2, Some(&self.gce_b2), 1, d, 6 * d);
        let depth = self.blocks.len();
        let mut table = vec![0f32; depth * 6 * d];
        for (b, block) in self.blocks.iter().enumerate() {
            let row = &mut table[b * 6 * d..(b + 1) * 6 * d];
            for chunk in 0..6 {
                for i in 0..d {
                    let value = block.scale_shift_gate[chunk * d + i] + gcond[chunk * d + i];
                    row[chunk * d + i] = if chunk == 2 || chunk == 5 {
                        sigmoid(1.0 - value)
                    } else {
                        value
                    };
                }
            }
        }
        let mods = gpu_upload(&table, depth, 6 * d).map_err(|e| dev_err("dit mods", e))?;

        // Input assembly: preprocess conv (1x1) residual, project_in, memory.
        let x_dev = gpu_upload(x, latent_len, c).map_err(|e| dev_err("dit x upload", e))?;
        let pre = gpu_linear_nt_cached(&x_dev, "sa3dit", &[device.preprocess.part()], &[])
            .map_err(|e| dev_err("dit preprocess", e))?;
        let x_pre = gpu_add(&x_dev, &pre).map_err(|e| dev_err("dit preprocess add", e))?;
        let projected = gpu_linear_nt_cached(&x_pre, "sa3dit", &[device.project_in.part()], &[])
            .map_err(|e| dev_err("dit project in", e))?;
        let mut h = makepad_ai_common::backend::cuda::gpu_concat_rows(&run.memory, &projected)
            .map_err(|e| dev_err("dit memory concat", e))?;

        let scale = 1.0 / (SA3_HEAD_DIM as f32).sqrt();
        let pad_start = mem + run.valid_len;
        let v_zero = run.valid_len < latent_len;
        let half = SA3_ROPE_DIM / 2;

        for (b, block) in self.blocks.iter().enumerate() {
            let dev = &device.blocks[b];
            // --- self attention (adaLN mod-norm + sigmoid(1-gate) residual) ---
            let a = gpu_rms_norm_mod_indexed(
                &h,
                &run.pre_norm_w[b],
                &mods,
                &run.block_idx[b],
                6 * d,
                0,
                d,
                SA3_NORM_EPS,
                false,
            )
            .map_err(|e| dev_err("dit pre mod norm", e))?;
            let qkv = gpu_linear_nt_cached(&a, "sa3dit", &[dev.qkv.part()], &[])
                .map_err(|e| dev_err("dit qkv", e))?;
            let q = gpu_slice_cols(&qkv, 0, d).map_err(|e| dev_err("dit q slice", e))?;
            let k = gpu_slice_cols(&qkv, d, d).map_err(|e| dev_err("dit k slice", e))?;
            let v = gpu_slice_cols(&qkv, 2 * d, d).map_err(|e| dev_err("dit v slice", e))?;
            let q = gpu_rms_norm_mul(
                &q, SA3_HEAD_DIM, "sa3dit", &format!("b{b}.qn"), &block.self_q_norm,
                SA3_QK_NORM_EPS,
            )
            .map_err(|e| dev_err("dit q norm", e))?;
            let k = gpu_rms_norm_mul(
                &k, SA3_HEAD_DIM, "sa3dit", &format!("b{b}.kn"), &block.self_k_norm,
                SA3_QK_NORM_EPS,
            )
            .map_err(|e| dev_err("dit k norm", e))?;
            let q = gpu_rope_half(&q, heads, half, &run.rope_cos, &run.rope_sin)
                .map_err(|e| dev_err("dit rope q", e))?;
            let k = gpu_rope_half(&k, heads, half, &run.rope_cos, &run.rope_sin)
                .map_err(|e| dev_err("dit rope k", e))?;
            let v = if v_zero {
                // Reference V-zeroing: padded latent keys keep their scores
                // but contribute zero V.
                let valid = gpu_slice_rows(&v, 0, pad_start).map_err(|e| dev_err("dit v valid", e))?;
                let zeros = gpu_upload(&vec![0f32; (seq - pad_start) * d], seq - pad_start, d)
                    .map_err(|e| dev_err("dit v zeros", e))?;
                makepad_ai_common::backend::cuda::gpu_concat_rows(&valid, &zeros)
                    .map_err(|e| dev_err("dit v concat", e))?
            } else {
                v
            };
            let attn = gpu_attention_packed(&q, &k, &v, heads, scale)
                .map_err(|e| dev_err("dit self attention", e))?;
            let out = gpu_linear_nt_cached(&attn, "sa3dit", &[dev.self_out.part()], &[])
                .map_err(|e| dev_err("dit self out", e))?;
            h = gpu_gated_residual_mod(&h, &out, &mods, b * 6 * d + 2 * d)
                .map_err(|e| dev_err("dit self gate", e))?;

            // --- cross attention (plain residual, no rope) ---
            let cnorm = gpu_rms_norm_mul(
                &h, d, "sa3dit", &format!("b{b}.cn"), &block.cross_norm, SA3_NORM_EPS,
            )
            .map_err(|e| dev_err("dit cross norm", e))?;
            let cq = gpu_linear_nt_cached(&cnorm, "sa3dit", &[dev.cross_q.part()], &[])
                .map_err(|e| dev_err("dit cross q", e))?;
            let cq = gpu_rms_norm_mul(
                &cq, SA3_HEAD_DIM, "sa3dit", &format!("b{b}.cqn"), &block.cross_q_norm,
                SA3_QK_NORM_EPS,
            )
            .map_err(|e| dev_err("dit cross q norm", e))?;
            let (ck, cv) = &run.cross_kv[b];
            let cattn = gpu_attention_packed_cross(&cq, ck, cv, heads, scale)
                .map_err(|e| dev_err("dit cross attention", e))?;
            let cout = gpu_linear_nt_cached(&cattn, "sa3dit", &[dev.cross_out.part()], &[])
                .map_err(|e| dev_err("dit cross out", e))?;
            h = gpu_add(&h, &cout).map_err(|e| dev_err("dit cross residual", e))?;

            // --- local conditioning (constant addend) ---
            h = gpu_add(&h, &run.local_add[b]).map_err(|e| dev_err("dit local add", e))?;

            // --- feedforward (adaLN mod-norm, SwiGLU, gated residual) ---
            let f = gpu_rms_norm_mod_indexed(
                &h,
                &run.ff_norm_w[b],
                &mods,
                &run.block_idx[b],
                6 * d,
                3 * d,
                4 * d,
                SA3_NORM_EPS,
                false,
            )
            .map_err(|e| dev_err("dit ff mod norm", e))?;
            let proj = gpu_linear_nt_cached(&f, "sa3dit", &[dev.ff_proj.part()], &block.ff_proj_b)
                .map_err(|e| dev_err("dit ff proj", e))?;
            let inner = gpu_swiglu_value_gate(&proj).map_err(|e| dev_err("dit swiglu", e))?;
            let out = gpu_linear_nt_cached(&inner, "sa3dit", &[dev.ff_out.part()], &block.ff_out_b)
                .map_err(|e| dev_err("dit ff out", e))?;
            h = gpu_gated_residual_mod(&h, &out, &mods, b * 6 * d + 5 * d)
                .map_err(|e| dev_err("dit ff gate", e))?;
        }

        // Strip memory tokens, project out, postprocess conv residual.
        let latents = gpu_slice_rows(&h, mem, latent_len).map_err(|e| dev_err("dit strip memory", e))?;
        let out = gpu_linear_nt_cached(&latents, "sa3dit", &[device.project_out.part()], &[])
            .map_err(|e| dev_err("dit project out", e))?;
        let post = gpu_linear_nt_cached(&out, "sa3dit", &[device.postprocess.part()], &[])
            .map_err(|e| dev_err("dit postprocess", e))?;
        let final_out = gpu_add(&out, &post).map_err(|e| dev_err("dit postprocess add", e))?;
        gpu_download(&final_out).map_err(|e| dev_err("dit download", e))
    }
}
