//! CUDA path for the s2mel CFM estimator (child module of `indextts_s2mel`
//! so it can read the oracle-validated CPU structs directly).
//!
//! Precision: F16 device weights + the default f16-operand GEMM spine (the
//! flux-proven production path); everything between GEMMs stays f32 device
//! tensors. The parity gates in `indextts_cuda_validate` compare against the
//! f32 oracle fixtures (cosine + max-abs), same as the CPU port.
//!
//! Key structural facts (all verified against the CPU implementation):
//! - Every per-step modulation is input-independent: `t` follows the exact
//!   reference accumulation (`t_span` f64->f32, `t += dt` in f32), so t1/t2
//!   embeddings, all 27 AdaLayerNorm projections, the FinalLayer mul/add and
//!   the WaveNet cond-gate biases are precomputed at session build into
//!   device tables ([n_steps, 1024] per AdaLN site, indexed per row by a
//!   step-index tensor) and host vectors.
//! - Everything runs TIME-MAJOR `[t_len, ch]`, with the CFG cond and null
//!   passes BATCHED as one `[2*t_len, ch]` pass (rows 0..T cond, T..2T
//!   null): the two passes share every weight, per-step modulation and rope
//!   position, differ only in the x_in condition columns, and attention is
//!   computed per half so the passes never attend across each other. This
//!   halves kernel launches and host sync points per solver step.
//! - The WaveNet k=5 reflect convs become 5 `gpu_gather_rows_colblock`
//!   gathers (per-tap reflect row indices, composed per half) concatenated
//!   into one `[2T, 5*DIM]` operand for a single wide GEMM whose weight
//!   packs all taps (the GEMM performs the tap summation); conv bias +
//!   per-step cond gate bias ride the GEMM bias; the 1x1 res_skip convs are
//!   plain GEMMs.
//! - AdaLayerNorm is `RMSNorm(x)*gamma*w + b` == `gpu_rms_norm_mod_indexed`
//!   with table rows `[w-1 | b]` (the kernel applies `(1+scale)` and
//!   `+shift`); the FinalLayer no-affine LayerNorm eps 1e-6 with
//!   `x*(1+scale)+shift` == `gpu_layer_norm_mul_add(mul=1+scale, add=shift)`.
//! - The WaveNet tanh*sigmoid gate runs fused on device
//!   (`gpu_wavenet_gate`, bias pre-applied through the tap GEMM bias), so a
//!   whole estimator forward has no mid-network host round trips.
//! - The CFG solver mirrors `solve_euler_observed` bit-for-bit on the host
//!   side: same noise draw, prompt zeroing before the loop and after every
//!   step, f32 `x += dt * v`, and the same observer contract.

use super::*;
use crate::backend::{
    gpu_add, gpu_attention_packed, gpu_concat_cols, gpu_concat_rows, gpu_device_available,
    gpu_download, gpu_gather_rows_colblock, gpu_gemm_f16acc_enabled, gpu_layer_norm_mul_add,
    gpu_linear_nt_cached, gpu_rms_norm_mod_indexed, gpu_rope_interleaved, gpu_slice_cols,
    gpu_slice_rows, gpu_swiglu_value_gate, gpu_upload, gpu_upload_u32, gpu_wavenet_gate,
    gpu_weight_cache_ensure, GpuLinearPart, GpuTensor,
};
use makepad_ai_common::quant::{f32_to_f16, GGML_TYPE_F16};

const NS: &str = "indextts_s2mel";
/// AdaLN sites: per block attention_norm (2i) and ffn_norm (2i+1), plus the
/// transformer final norm.
const ADALN_SITES: usize = 2 * DEPTH + 1;

fn gm(error: String) -> DiffusionError {
    DiffusionError::model(format!("s2mel cuda: {error}"))
}

/// True when the CUDA device path is available in this build.
pub fn s2mel_cuda_available() -> bool {
    gpu_device_available()
}

fn f32_to_f16_bytes(values: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 2);
    for &value in values {
        out.extend_from_slice(&f32_to_f16(value).to_le_bytes());
    }
    out
}

/// Weight-cached F16 linear over f32 rows; empty bias slice means no bias.
fn lin(x: &GpuTensor, keys: &[&str], ns_cols: &[usize], bias: &[f32]) -> Result<GpuTensor> {
    debug_assert_eq!(keys.len(), ns_cols.len());
    let parts: Vec<GpuLinearPart<'_>> = keys
        .iter()
        .zip(ns_cols)
        .map(|(key, &n)| GpuLinearPart {
            bt_ggml_type: GGML_TYPE_F16,
            n,
            cache_key: key,
            bytes: &[],
        })
        .collect();
    gpu_linear_nt_cached(x, NS, &parts, bias).map_err(gm)
}

fn ensure_weight(key: &str, w: &[f32], out_dim: usize, in_dim: usize) -> Result<()> {
    debug_assert_eq!(w.len(), out_dim * in_dim);
    // Every s2mel gemm has m = t_len rows > 1, so the cached-gemm lookup key
    // carries the ::a16 suffix exactly when f16acc gemms are enabled — the
    // ensure key must match (same rule as trellis_dit::ensure_linear).
    let want_a16 = gpu_gemm_f16acc_enabled();
    gpu_weight_cache_ensure(NS, key, GGML_TYPE_F16, out_dim, in_dim, want_a16, || {
        Ok(f32_to_f16_bytes(w))
    })
    .map_err(gm)
}

/// Per-step host-side modulation data (everything the AdaLN tables don't
/// cover).
struct StepMods {
    /// Per WN layer: conv bias + cond gate slice, folded into the k=0 GEMM
    /// bias `[2*DIM]` (tanh half then sigmoid half, matching conv out
    /// channels).
    wn_gate_bias: Vec<Vec<f32>>,
    /// FinalLayer modulate: `1 + scale` / `shift` `[DIM]` each.
    fl_mul: Vec<f32>,
    fl_add: Vec<f32>,
}

/// Session-lifetime device state: cached weights + per-step modulation
/// tables. Built once per loaded estimator; independent of `t_len`.
pub struct S2melCudaSession {
    n_steps: usize,
    steps: Vec<StepMods>,
    /// One `[n_steps, 2*DIM]` table per AdaLN site, rows `[w-1 | b]`.
    adaln_tables: Vec<GpuTensor>,
    /// One `[1, DIM]` RMS gamma per AdaLN site.
    adaln_gammas: Vec<GpuTensor>,
}

impl S2melCudaSession {
    pub fn new(est: &CfmEstimator, n_steps: usize) -> Result<Self> {
        if n_steps == 0 {
            return Err(gm("n_steps must be > 0".into()));
        }
        Self::ensure_weights(est)?;

        // Exact reference time accumulation (t_span f64->f32, t += dt f32).
        let t_span: Vec<f32> =
            (0..=n_steps).map(|i| (i as f64 / n_steps as f64) as f32).collect();
        let mut t = t_span[0];
        let mut t_values = Vec::with_capacity(n_steps);
        for step in 1..=n_steps {
            t_values.push(t);
            t += t_span[step] - t_span[step - 1];
        }

        let mut steps = Vec::with_capacity(n_steps);
        let mut site_rows: Vec<Vec<f32>> = vec![Vec::with_capacity(n_steps * 2 * DIM); ADALN_SITES];
        for &tv in &t_values {
            let t1 = est.t_embedder.embed(tv);
            let t2 = est.t_embedder2.embed(tv);

            // AdaLN site rows [w-1 | b] from project_layer(t1).
            for (site, adaln) in Self::adaln_sites(est).into_iter().enumerate() {
                let wb = linear(&t1, &adaln.proj_w, Some(&adaln.proj_b), 1, DIM, 2 * DIM);
                let row = &mut site_rows[site];
                for i in 0..DIM {
                    row.push(wb[i] - 1.0);
                }
                row.extend_from_slice(&wb[DIM..]);
            }

            // FinalLayer modulate from Linear(SiLU(t1)): [shift | scale].
            let mut c = t1.clone();
            for v in c.iter_mut() {
                *v = silu(*v);
            }
            let sb = linear(&c, &est.fl_adaln_w, Some(&est.fl_adaln_b), 1, DIM, 2 * DIM);
            let (shift, scale) = sb.split_at(DIM);
            let fl_mul: Vec<f32> = scale.iter().map(|&s| 1.0 + s).collect();
            let fl_add = shift.to_vec();

            // WN cond gate biases: conv bias + per-layer slice of
            // cond_layer(t2), both halves.
            let g = linear(&t2, &est.wn_cond_w, Some(&est.wn_cond_b), 1, DIM, 2 * DIM * WN_LAYERS);
            let mut wn_gate_bias = Vec::with_capacity(WN_LAYERS);
            for l in 0..WN_LAYERS {
                let g_l = &g[l * 2 * DIM..(l + 1) * 2 * DIM];
                let conv = &est.wn_in[l];
                debug_assert_eq!(conv.b.len(), 2 * DIM);
                let bias: Vec<f32> = conv.b.iter().zip(g_l).map(|(&b, &gv)| b + gv).collect();
                wn_gate_bias.push(bias);
            }
            steps.push(StepMods { wn_gate_bias, fl_mul, fl_add });
        }

        let mut adaln_tables = Vec::with_capacity(ADALN_SITES);
        let mut adaln_gammas = Vec::with_capacity(ADALN_SITES);
        for (site, adaln) in Self::adaln_sites(est).into_iter().enumerate() {
            adaln_tables.push(gpu_upload(&site_rows[site], n_steps, 2 * DIM).map_err(gm)?);
            adaln_gammas.push(gpu_upload(&adaln.gamma, 1, DIM).map_err(gm)?);
        }

        Ok(Self { n_steps, steps, adaln_tables, adaln_gammas })
    }

    /// AdaLN sites in table order: block i attention_norm at 2i, ffn_norm at
    /// 2i+1, transformer final norm last.
    fn adaln_sites(est: &CfmEstimator) -> Vec<&AdaLayerNorm> {
        let mut sites = Vec::with_capacity(ADALN_SITES);
        for block in &est.blocks {
            sites.push(&block.attn_norm);
            sites.push(&block.ffn_norm);
        }
        sites.push(&est.final_norm);
        sites
    }

    /// Uploads every GEMM weight into the F16 device cache (idempotent).
    fn ensure_weights(est: &CfmEstimator) -> Result<()> {
        let merge_in = 2 * MELS + DIM + STYLE;
        ensure_weight("merge", &est.merge_w, DIM, merge_in)?;
        ensure_weight("cond_proj", &est.cond_proj_w, DIM, DIM)?;
        for (i, block) in est.blocks.iter().enumerate() {
            ensure_weight(&format!("blk{i}.wqkv"), &block.wqkv, 3 * DIM, DIM)?;
            ensure_weight(&format!("blk{i}.wo"), &block.wo, DIM, DIM)?;
            // SwiGLU value (w3) first, gate (w1) second — the fused GEMM
            // output feeds gpu_swiglu_value_gate(value, gate).
            ensure_weight(&format!("blk{i}.w3"), &block.w3, FFN, DIM)?;
            ensure_weight(&format!("blk{i}.w1"), &block.w1, FFN, DIM)?;
            ensure_weight(&format!("blk{i}.w2"), &block.w2, DIM, FFN)?;
            ensure_weight(&format!("blk{i}.skip"), &block.skip_w, DIM, 2 * DIM)?;
        }
        ensure_weight("skip_long", &est.skip_linear_w, DIM, DIM + MELS)?;
        ensure_weight("conv1", &est.conv1_w, DIM, DIM)?;
        ensure_weight("res_proj", &est.res_proj_w, DIM, DIM)?;
        ensure_weight("fl_linear", &est.fl_linear_w, DIM, DIM)?;
        ensure_weight("conv2", &est.conv2_w, MELS, DIM)?;
        for (l, conv) in est.wn_in.iter().enumerate() {
            debug_assert_eq!((conv.out_ch, conv.in_ch, conv.k), (2 * DIM, DIM, WN_KERNEL));
            // All 5 taps packed into one wide weight [2*DIM, 5*DIM]; the
            // operand is the tap-major concat of the 5 gathered row sets, so
            // the GEMM performs the tap summation of the reflect conv.
            let mut wk = vec![0f32; 2 * DIM * WN_KERNEL * DIM];
            for o in 0..2 * DIM {
                for kk in 0..WN_KERNEL {
                    for i2 in 0..DIM {
                        wk[(o * WN_KERNEL + kk) * DIM + i2] =
                            conv.w[(o * DIM + i2) * WN_KERNEL + kk];
                    }
                }
            }
            ensure_weight(&format!("wn{l}.tap5"), &wk, 2 * DIM, WN_KERNEL * DIM)?;
            let rs = &est.wn_res_skip[l];
            debug_assert_eq!((rs.in_ch, rs.k), (DIM, 1));
            ensure_weight(&format!("wn{l}.rs"), &rs.w, rs.out_ch, DIM)?;
        }
        Ok(())
    }

    /// Euler CFG solve on the device path; the exact CUDA counterpart of
    /// [`CfmEstimator::solve_euler_observed`] (same noise draw, prompt
    /// zeroing, f32 accumulation and observer contract).
    #[allow(clippy::too_many_arguments)]
    pub fn solve_euler_observed(
        &self,
        est: &CfmEstimator,
        mu: &[f32],
        t_len: usize,
        prompt_mel: &[f32],
        prompt_len: usize,
        style: &[f32],
        cfg_rate: f32,
        noise: &mut dyn S2melNoiseSource,
        mut on_velocity: Option<&mut dyn FnMut(usize, &[f32])>,
        mut progress: Option<ProgressHook>,
    ) -> Result<CfmMel> {
        let n_steps = self.n_steps;
        if mu.len() != t_len * DIM || prompt_mel.len() != MELS * prompt_len || prompt_len > t_len {
            return Err(DiffusionError::model(format!(
                "cfm solve (cuda): bad sizes (t_len {t_len}, prompt_len {prompt_len}, mu {}, prompt {})",
                mu.len(),
                prompt_mel.len()
            )));
        }
        let mut x = noise.draw(0, MELS * t_len);
        if x.len() != MELS * t_len {
            return Err(DiffusionError::model(
                "cfm solve (cuda): noise source returned wrong length",
            ));
        }
        let run = S2melCudaRun::new(self, est, mu, t_len, prompt_mel, prompt_len, style)?;
        for c in 0..MELS {
            x[c * t_len..c * t_len + prompt_len].fill(0.0);
        }
        let t_span: Vec<f32> =
            (0..=n_steps).map(|i| (i as f64 / n_steps as f64) as f32).collect();
        let mut t = t_span[0];
        for step in 1..=n_steps {
            emit_progress(
                &mut progress,
                &format!("s2mel-cfm {step}/{n_steps}"),
                (step - 1) as f64 / n_steps as f64,
            )?;
            let dt = t_span[step] - t_span[step - 1];
            let v = run.velocity_cfg(step, &x, cfg_rate)?;
            if let Some(observe) = on_velocity.as_mut() {
                observe(step, &v);
            }
            for (xv, &dv) in x.iter_mut().zip(&v) {
                *xv += dt * dv;
            }
            t += dt;
            let _ = t;
            for c in 0..MELS {
                x[c * t_len..c * t_len + prompt_len].fill(0.0);
            }
        }
        emit_progress(&mut progress, &format!("s2mel-cfm {n_steps}/{n_steps}"), 1.0)?;
        Ok(CfmMel { mel: x, frames: t_len, prompt_frames: prompt_len })
    }
}

/// Per-synthesis device state (t_len-dependent): doubled-batch rope tables,
/// per-tap reflect / step index tensors and the `[2T, 864]` x_in template
/// (rows 0..T cond pass, rows T..2T CFG-null pass).
pub struct S2melCudaRun<'a> {
    session: &'a S2melCudaSession,
    est: &'a CfmEstimator,
    t_len: usize,
    /// `[2*t_len, HEAD_DIM/2]` rope tables (positions restart at row t_len:
    /// the null half sees the same positions as the cond half).
    rope_cos: GpuTensor,
    rope_sin: GpuTensor,
    /// Per conv tap kk: `[2*t_len]` row gather indices composing the reflect
    /// padding with the tap shift, per batch half (never crossing halves).
    tap_idx: Vec<GpuTensor>,
    /// Per step: `[2*t_len]` u32 filled with the step index (AdaLN row).
    step_idx: Vec<GpuTensor>,
    /// `[2*t_len, 784]` device-resident conditioning columns (x_in cols
    /// 80..864: prompt mel, cond projection, style — per half, all
    /// step-constant); each forward uploads only the `[2*t_len, 80]` x^T
    /// block and concats.
    cond_cols: GpuTensor,
}

impl<'a> S2melCudaRun<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        session: &'a S2melCudaSession,
        est: &'a CfmEstimator,
        mu: &[f32],
        t_len: usize,
        prompt_mel: &[f32],
        prompt_len: usize,
        style: &[f32],
    ) -> Result<Self> {
        let pad = (WN_KERNEL - 1) / 2;
        if t_len <= pad || mu.len() != t_len * DIM || style.len() != STYLE {
            return Err(gm(format!(
                "bad run sizes (t_len {t_len}, mu {}, style {})",
                mu.len(),
                style.len()
            )));
        }
        if prompt_mel.len() != MELS * prompt_len || prompt_len > t_len {
            return Err(gm(format!(
                "bad prompt sizes (prompt_len {prompt_len}, prompt {})",
                prompt_mel.len()
            )));
        }
        let (cos, sin) = moss_rope_tables(t_len, HEAD_DIM, ROPE_BASE);
        let half_cols = HEAD_DIM / 2;
        let mut cos2 = Vec::with_capacity(2 * cos.len());
        cos2.extend_from_slice(&cos);
        cos2.extend_from_slice(&cos);
        let mut sin2 = Vec::with_capacity(2 * sin.len());
        sin2.extend_from_slice(&sin);
        sin2.extend_from_slice(&sin);
        let rope_cos = gpu_upload(&cos2, 2 * t_len, half_cols).map_err(gm)?;
        let rope_sin = gpu_upload(&sin2, 2 * t_len, half_cols).map_err(gm)?;

        // Reflect map over padded positions 0..t_len+2*pad:
        // x[-p] -> x[p], x[len-1+p] -> x[len-1-p].
        let refl_map = |pos: usize| -> u32 {
            if pos < pad {
                (pad - pos) as u32
            } else if pos < pad + t_len {
                (pos - pad) as u32
            } else {
                (t_len - 2 - (pos - pad - t_len)) as u32
            }
        };
        // Per-tap direct gather indices for both batch halves.
        let mut tap_idx = Vec::with_capacity(WN_KERNEL);
        for kk in 0..WN_KERNEL {
            let mut idx = Vec::with_capacity(2 * t_len);
            for tt in 0..t_len {
                idx.push(refl_map(tt + kk));
            }
            for tt in 0..t_len {
                idx.push(t_len as u32 + refl_map(tt + kk));
            }
            tap_idx.push(gpu_upload_u32(&idx).map_err(gm)?);
        }

        let mut step_idx = Vec::with_capacity(session.n_steps);
        for s in 0..session.n_steps {
            step_idx.push(gpu_upload_u32(&vec![s as u32; 2 * t_len]).map_err(gm)?);
        }

        // cond = cond_projection(mu) is step-constant: one GEMM, downloaded
        // into the cond-half template. The null half sees mu = 0, i.e. the
        // projection bias broadcast over time.
        let mu_dev = gpu_upload(mu, t_len, DIM).map_err(gm)?;
        let cond_dev = lin(&mu_dev, &["cond_proj"], &[DIM], &est.cond_proj_b)?;
        let cond = gpu_download(&cond_dev).map_err(gm)?;

        // Step-constant x_in columns 80..864 (prompt mel | cond | style per
        // half), uploaded once; forwards only stream the x^T block.
        let cc = MELS + DIM + STYLE;
        let mut cond_host = vec![0f32; 2 * t_len * cc];
        for tt in 0..t_len {
            let row = &mut cond_host[tt * cc..(tt + 1) * cc];
            for c in 0..MELS {
                row[c] = if tt < prompt_len { prompt_mel[c * prompt_len + tt] } else { 0.0 };
            }
            row[MELS..MELS + DIM].copy_from_slice(&cond[tt * DIM..(tt + 1) * DIM]);
            row[MELS + DIM..].copy_from_slice(style);
            let null_row = &mut cond_host[(t_len + tt) * cc..(t_len + tt + 1) * cc];
            null_row[MELS..MELS + DIM].copy_from_slice(&est.cond_proj_b);
        }
        let cond_cols = gpu_upload(&cond_host, 2 * t_len, cc).map_err(gm)?;

        Ok(Self {
            session,
            est,
            t_len,
            rope_cos,
            rope_sin,
            tap_idx,
            step_idx,
            cond_cols,
        })
    }

    /// CFG-combined velocity for one solver step (1-based, matching the
    /// oracle dump naming): `(1+cfg)*v_cond - cfg*v_null`, channel-major
    /// `[80, t_len]` like the CPU path. One batched device pass computes
    /// both CFG halves.
    pub fn velocity_cfg(&self, step: usize, x: &[f32], cfg_rate: f32) -> Result<Vec<f32>> {
        let both = self.forward_batched(step, x)?;
        let t_len = self.t_len;
        let (v_cond, v_null) = both.split_at(t_len * MELS);
        let mut v = vec![0f32; MELS * t_len];
        for tt in 0..t_len {
            for c in 0..MELS {
                let cond = v_cond[tt * MELS + c];
                let null = v_null[tt * MELS + c];
                v[c * t_len + tt] = (1.0 + cfg_rate) * cond - cfg_rate * null;
            }
        }
        Ok(v)
    }

    /// One batched estimator pass (cond rows 0..T, null rows T..2T) on the
    /// device; `x` channel-major `[80, t_len]`, returns time-major rows
    /// `[2*t_len, 80]`.
    fn forward_batched(&self, step: usize, x: &[f32]) -> Result<Vec<f32>> {
        let t_len = self.t_len;
        let rows2 = 2 * t_len;
        if step == 0 || step > self.session.n_steps || x.len() != MELS * t_len {
            return Err(gm(format!("bad forward args (step {step}, x {})", x.len())));
        }
        let s = step - 1;
        let mods = &self.session.steps[s];
        let idx = &self.step_idx[s];

        // Only the x^T block moves per step; the conditioning columns are
        // device-resident.
        let mut xt = vec![0f32; rows2 * MELS];
        for tt in 0..t_len {
            for c in 0..MELS {
                let value = x[c * t_len + tt];
                xt[tt * MELS + c] = value;
                xt[(t_len + tt) * MELS + c] = value;
            }
        }
        let xt_dev = gpu_upload(&xt, rows2, MELS).map_err(gm)?;
        let x_in_dev = gpu_concat_cols(&[&xt_dev, &self.cond_cols]).map_err(gm)?;
        let mut h = lin(&x_in_dev, &["merge"], &[DIM], &self.est.merge_b)?;

        // Transformer with uvit skips (emit i < 6, receive i > 6, LIFO).
        let scale = 1.0 / (HEAD_DIM as f32).sqrt();
        let mut skips: Vec<GpuTensor> = Vec::new();
        for i in 0..DEPTH {
            let block = &self.est.blocks[i];
            if i > DEPTH / 2 {
                let skip = skips
                    .pop()
                    .ok_or_else(|| gm("uvit skip underflow".into()))?;
                let cat = gpu_concat_cols(&[&h, &skip]).map_err(gm)?;
                h = lin(&cat, &[&format!("blk{i}.skip")], &[DIM], &block.skip_b)?;
            }
            let normed = self.adaln(&h, 2 * i, idx)?;
            let qkv = lin(&normed, &[&format!("blk{i}.wqkv")], &[3 * DIM], &[])?;
            let q = gpu_slice_cols(&qkv, 0, DIM).map_err(gm)?;
            let k = gpu_slice_cols(&qkv, DIM, DIM).map_err(gm)?;
            let v = gpu_slice_cols(&qkv, 2 * DIM, DIM).map_err(gm)?;
            let q = gpu_rope_interleaved(&q, HEADS, &self.rope_cos, &self.rope_sin).map_err(gm)?;
            let k = gpu_rope_interleaved(&k, HEADS, &self.rope_cos, &self.rope_sin).map_err(gm)?;
            // Attention per CFG half: the two passes must not attend across
            // each other.
            let attn = {
                let q_c = gpu_slice_rows(&q, 0, t_len).map_err(gm)?;
                let k_c = gpu_slice_rows(&k, 0, t_len).map_err(gm)?;
                let v_c = gpu_slice_rows(&v, 0, t_len).map_err(gm)?;
                let a_c = gpu_attention_packed(&q_c, &k_c, &v_c, HEADS, scale).map_err(gm)?;
                let q_n = gpu_slice_rows(&q, t_len, t_len).map_err(gm)?;
                let k_n = gpu_slice_rows(&k, t_len, t_len).map_err(gm)?;
                let v_n = gpu_slice_rows(&v, t_len, t_len).map_err(gm)?;
                let a_n = gpu_attention_packed(&q_n, &k_n, &v_n, HEADS, scale).map_err(gm)?;
                gpu_concat_rows(&a_c, &a_n).map_err(gm)?
            };
            let attn = lin(&attn, &[&format!("blk{i}.wo")], &[DIM], &[])?;
            h = gpu_add(&h, &attn).map_err(gm)?;

            let normed = self.adaln(&h, 2 * i + 1, idx)?;
            let f = lin(
                &normed,
                &[&format!("blk{i}.w3"), &format!("blk{i}.w1")],
                &[FFN, FFN],
                &[],
            )?;
            let sw = gpu_swiglu_value_gate(&f).map_err(gm)?;
            let ff = lin(&sw, &[&format!("blk{i}.w2")], &[DIM], &[])?;
            h = gpu_add(&h, &ff).map_err(gm)?;
            if i < DEPTH / 2 {
                skips.push(gpu_slice_rows(&h, 0, rows2).map_err(gm)?);
            }
        }
        let x_res = self.adaln(&h, 2 * DEPTH, idx)?;

        // Long skip: skip_linear(cat[x_res, x^T]).
        let cat = gpu_concat_cols(&[&x_res, &xt_dev]).map_err(gm)?;
        let x_res = lin(&cat, &["skip_long"], &[DIM], &self.est.skip_linear_b)?;

        // WaveNet over conv1(x_res) rows.
        let mut wn_x = lin(&x_res, &["conv1"], &[DIM], &self.est.conv1_b)?;
        let mut wn_out: Option<GpuTensor> = None;
        for l in 0..WN_LAYERS {
            // Reflect-padded k5 conv: 5 per-tap gathers (reflect composed
            // with the tap shift, per half) concatenated into one wide GEMM
            // whose packed weight sums the taps; conv bias + cond gate bias
            // ride the GEMM bias.
            let g0 = gpu_gather_rows_colblock(&wn_x, &self.tap_idx[0], None, DIM).map_err(gm)?;
            let g1 = gpu_gather_rows_colblock(&wn_x, &self.tap_idx[1], None, DIM).map_err(gm)?;
            let g2 = gpu_gather_rows_colblock(&wn_x, &self.tap_idx[2], None, DIM).map_err(gm)?;
            let g3 = gpu_gather_rows_colblock(&wn_x, &self.tap_idx[3], None, DIM).map_err(gm)?;
            let g4 = gpu_gather_rows_colblock(&wn_x, &self.tap_idx[4], None, DIM).map_err(gm)?;
            let taps = gpu_concat_cols(&[&g0, &g1, &g2, &g3, &g4]).map_err(gm)?;
            let acc = lin(
                &taps,
                &[&format!("wn{l}.tap5")],
                &[2 * DIM],
                &mods.wn_gate_bias[l],
            )?;
            // Gate tanh(a)*sigmoid(b) fused on device (bias pre-applied via
            // the GEMM bias above).
            let acts_dev = gpu_wavenet_gate(&acc).map_err(gm)?;
            let rs_conv = &self.est.wn_res_skip[l];
            let rs = lin(&acts_dev, &[&format!("wn{l}.rs")], &[rs_conv.out_ch], &rs_conv.b)?;
            if l < WN_LAYERS - 1 {
                let res = gpu_slice_cols(&rs, 0, DIM).map_err(gm)?;
                wn_x = gpu_add(&wn_x, &res).map_err(gm)?;
                let sk = gpu_slice_cols(&rs, DIM, DIM).map_err(gm)?;
                wn_out = Some(match wn_out {
                    Some(acc_out) => gpu_add(&acc_out, &sk).map_err(gm)?,
                    None => sk,
                });
            } else {
                let acc_out = wn_out.take().ok_or_else(|| gm("wn skip accumulator missing".into()))?;
                wn_out = Some(gpu_add(&acc_out, &rs).map_err(gm)?);
            }
        }
        let wn_out = wn_out.ok_or_else(|| gm("wn produced no output".into()))?;

        // x = wavenet + res_projection(x_res); FinalLayer; conv2.
        let res = lin(&x_res, &["res_proj"], &[DIM], &self.est.res_proj_b)?;
        let rows = gpu_add(&wn_out, &res).map_err(gm)?;
        let rows = gpu_layer_norm_mul_add(&rows, &mods.fl_mul, &mods.fl_add, 1e-6).map_err(gm)?;
        let rows = lin(&rows, &["fl_linear"], &[DIM], &self.est.fl_linear_b)?;
        let out = lin(&rows, &["conv2"], &[MELS], &self.est.conv2_b)?;
        gpu_download(&out).map_err(gm)
    }

    fn adaln(&self, x: &GpuTensor, site: usize, idx: &GpuTensor) -> Result<GpuTensor> {
        gpu_rms_norm_mod_indexed(
            x,
            &self.session.adaln_gammas[site],
            &self.session.adaln_tables[site],
            idx,
            2 * DIM,
            0,
            DIM,
            RMS_EPS,
            false,
        )
        .map_err(gm)
    }
}
