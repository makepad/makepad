//! GGUF generate graph matching the official ModularPipeline:
//! prefill → AR (CFG 1.5, top-k 50) → RVQ 7 residuals → cond encoder →
//! DiT Euler + CFG 1.7 → vocoder. No CUDA.

use crate::music3::{
    music3_latent_len, MUSIC3_AR_CFG, MUSIC3_AR_TOP_K, MUSIC3_AUDIO_CODE_OFFSET,
    MUSIC3_AUDIO_END_TOKEN_ID, MUSIC3_AUDIO_VOCAB, MUSIC3_COND_HIDDEN, MUSIC3_COND_LAYERS,
    MUSIC3_DIT_CONCAT, MUSIC3_DIT_COND, MUSIC3_DIT_DIM, MUSIC3_DIT_FF, MUSIC3_DIT_FOURIER,
    MUSIC3_DIT_HEAD_DIM, MUSIC3_DIT_HEADS, MUSIC3_DIT_IN_CHANNELS, MUSIC3_DIT_LAYERS,
    MUSIC3_DIT_ROPE, MUSIC3_FLOW_CFG, MUSIC3_LM_HEAD_DIM, MUSIC3_LM_HEADS, MUSIC3_LM_HIDDEN,
    MUSIC3_LM_KV_HEADS, MUSIC3_LM_LAYERS, MUSIC3_LM_RMS_EPS, MUSIC3_NUM_CODEBOOKS,
    MUSIC3_RVQ_HIDDEN, MUSIC3_SEMANTIC_VOCAB,
};
use crate::music3_ar::{sample_top_k, sample_top_k_ids, torch_randn, TorchPhilox};
use crate::music3_gguf::{
    attn_q_vs_kv_gqa, causal_attn_gqa, full_attn, perf, FiniteStats, Music3GgufLm, Music3GgufRvq,
};
use crate::music3_quant::{Music3GgufFile, Music3GgufPack, Music3GgufRole};
use crate::music3_vocoder::Music3Vocoder;
use makepad_ai_sfx::sa3::rms_norm_rows;
use crate::{DiffusionError, Result};
use std::f32::consts::PI;

const LN_EPS: f32 = 1e-5;

fn add_inplace(dst: &mut [f32], src: &[f32]) {
    for (d, s) in dst.iter_mut().zip(src.iter()) {
        *d += *s;
    }
}

fn silu(x: f32) -> f32 {
    x / (1.0 + (-x).exp())
}

fn swiglu(up: &[f32], gate: &[f32]) -> Vec<f32> {
    if let Some(sg) = crate::backend::try_silu_f32(gate) {
        if let Some(out) = crate::backend::try_mul_f32(up, &[up.len()], &sg, &[sg.len()]) {
            return out;
        }
        return up.iter().zip(sg.iter()).map(|(u, g)| *u * *g).collect();
    }
    up.iter()
        .zip(gate.iter())
        .map(|(u, g)| *u * silu(*g))
        .collect()
}

fn dit_ffn(
    file: &Music3GgufFile,
    prep: &DitPrepared,
    layer: usize,
    normed: &[f32],
    rows: usize,
) -> Result<Vec<f32>> {
    let ns = format!("music3-{}", file.role.as_str());
    let in_n = format!("transformer_blocks.{layer}.ff_in.weight");
    let out_n = format!("transformer_blocks.{layer}.ff_out.weight");
    let in_bk = format!("transformer_blocks.{layer}.ff_in.bias");
    let out_bk = format!("transformer_blocks.{layer}.ff_out.bias");
    if let (Some(ff_in), Some(ff_out)) = (file.keyed_mat(&ns, &in_n), file.keyed_mat(&ns, &out_n)) {
        let swap = std::env::var_os("MAKEPAD_MUSIC3_SWIGLU_SWAP").is_some();
        if let Some(y) = makepad_ggml::backend::metal::try_dit_ffn_resident(
            normed,
            rows,
            MUSIC3_DIT_DIM,
            MUSIC3_DIT_FF,
            &prep.ff_in_b[layer],
            &prep.ff_out_b[layer],
            &in_bk,
            &out_bk,
            swap,
            ff_in,
            ff_out,
            |a, b| file.load_keyed_bytes(a, b),
        ) {
            if y.len() == rows * MUSIC3_DIT_DIM && y.iter().any(|v| v.is_finite() && *v != 0.0) {
                return Ok(y);
            }
        }
    }
    let mut ff = file.linear_nt(&in_n, normed, rows)?;
    add_bias_rows(&mut ff, &prep.ff_in_b[layer], rows);
    let gated = swiglu_rows(&ff, rows, MUSIC3_DIT_FF);
    let mut ff = file.linear_nt(&out_n, &gated, rows)?;
    add_bias_rows(&mut ff, &prep.ff_out_b[layer], rows);
    Ok(ff)
}

fn swiglu_rows(ff: &[f32], rows: usize, ff_dim: usize) -> Vec<f32> {
    let mut values = vec![0f32; rows * ff_dim];
    let mut gates = vec![0f32; rows * ff_dim];
    let swap = std::env::var_os("MAKEPAD_MUSIC3_SWIGLU_SWAP").is_some();
    for r in 0..rows {
        let row = &ff[r * ff_dim * 2..(r + 1) * ff_dim * 2];
        let (a, b) = row.split_at(ff_dim);
        let (value, gate) = if swap { (b, a) } else { (a, b) };
        values[r * ff_dim..(r + 1) * ff_dim].copy_from_slice(value);
        gates[r * ff_dim..(r + 1) * ff_dim].copy_from_slice(gate);
    }
    swiglu(&values, &gates)
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

fn apply_rope_partial(
    x: &mut [f32],
    tokens: usize,
    heads: usize,
    head_dim: usize,
    rope_dim: usize,
    cos: &[f32],
    sin: &[f32],
) {
    let half = rope_dim / 2;
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

fn layer_norm(x: &mut [f32], weight: &[f32], bias: &[f32], width: usize) {
    if width > 0 && x.len() % width == 0 {
        let rows = x.len() / width;
        if let Some(out) = crate::backend::try_layer_norm_mul_add_f32(
            x,
            &[rows, width],
            weight,
            &[width],
            bias,
            &[width],
            LN_EPS,
        ) {
            if out.len() == x.len() {
                x.copy_from_slice(&out);
                return;
            }
        }
    }
    for row in x.chunks_mut(width) {
        let mut mean = 0f32;
        for v in row.iter() {
            mean += *v;
        }
        mean /= width as f32;
        let mut var = 0f32;
        for v in row.iter() {
            let d = *v - mean;
            var += d * d;
        }
        let inv = (var / width as f32 + LN_EPS).sqrt().recip();
        for (v, (w, b)) in row.iter_mut().zip(weight.iter().zip(bias.iter())) {
            *v = (*v - mean) * inv * *w + *b;
        }
    }
}

fn add_bias_rows(x: &mut [f32], bias: &[f32], rows: usize) {
    let w = bias.len();
    for r in 0..rows {
        for (v, b) in x[r * w..(r + 1) * w].iter_mut().zip(bias.iter()) {
            *v += *b;
        }
    }
}

fn kth_largest(vals: &[f32], k: usize) -> f32 {
    let mut vals: Vec<f32> = vals.iter().copied().filter(|v| v.is_finite()).collect();
    if vals.is_empty() {
        return f32::NEG_INFINITY;
    }
    let k = k.max(1).min(vals.len());
    vals.select_nth_unstable_by(k - 1, |a, b| {
        b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal)
    });
    vals[k - 1]
}

// ---------------------------------------------------------------------------
// LM KV session
// ---------------------------------------------------------------------------

pub struct Music3GgufLmSession {
    k: Vec<Vec<f32>>,
    v: Vec<Vec<f32>>,
    pos: usize,
    layers: usize,
    last: Vec<f32>,
}

impl Music3GgufLm {
    pub fn prefill_session(
        &self,
        file: &Music3GgufFile,
        ids: &[u32],
        layers: usize,
    ) -> Result<Music3GgufLmSession> {
        let layers = layers.min(MUSIC3_LM_LAYERS).max(1);
        let mut session = Music3GgufLmSession {
            k: vec![Vec::new(); layers],
            v: vec![Vec::new(); layers],
            pos: 0,
            layers,
            last: vec![0f32; MUSIC3_LM_HIDDEN],
        };
        self.fill_cache(file, ids, &mut session)?;
        Ok(session)
    }

    /// Cond+uncond prefill with stacked `m=2*seq` Metal GEMMs. Attention stays
    /// per-stream so the two prompts cannot see each other.
    pub fn prefill_pair(
        &self,
        file: &Music3GgufFile,
        cond_ids: &[u32],
        uncond_ids: &[u32],
        layers: usize,
    ) -> Result<(Music3GgufLmSession, Music3GgufLmSession)> {
        if cond_ids.len() != uncond_ids.len() || cond_ids.is_empty() {
            return Err(DiffusionError::model("music3 gguf LM pair prefill length"));
        }
        let layers = layers.min(MUSIC3_LM_LAYERS).max(1);
        let n = cond_ids.len();
        let mut cond = Music3GgufLmSession {
            k: vec![Vec::new(); layers],
            v: vec![Vec::new(); layers],
            pos: 0,
            layers,
            last: vec![0f32; MUSIC3_LM_HIDDEN],
        };
        let mut uncond = Music3GgufLmSession {
            k: vec![Vec::new(); layers],
            v: vec![Vec::new(); layers],
            pos: 0,
            layers,
            last: vec![0f32; MUSIC3_LM_HIDDEN],
        };
        let mut hidden = Vec::with_capacity(2 * n * MUSIC3_LM_HIDDEN);
        hidden.extend(self.embed(file, cond_ids)?);
        hidden.extend(self.embed(file, uncond_ids)?);
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
        let kv_inner = n * MUSIC3_LM_KV_HEADS * MUSIC3_LM_HEAD_DIM;
        let q_inner = n * MUSIC3_LM_HEADS * MUSIC3_LM_HEAD_DIM;
        let scale = 1.0 / (MUSIC3_LM_HEAD_DIM as f32).sqrt();
        let m2 = 2 * n;
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
            let (mut q, mut k, v) = file.linear_nt_qkv(&qn, &kn, &vn, &normed, m2)?;
            rms_norm_rows(&mut k, &self.k_norm[layer], MUSIC3_LM_HEAD_DIM, MUSIC3_LM_RMS_EPS);
            rms_norm_rows(&mut q, &self.q_norm[layer], MUSIC3_LM_HEAD_DIM, MUSIC3_LM_RMS_EPS);
            for batch in 0..2 {
                apply_rope_half(
                    &mut q[batch * q_inner..(batch + 1) * q_inner],
                    n,
                    MUSIC3_LM_HEADS,
                    MUSIC3_LM_HEAD_DIM,
                    &cos,
                    &sin,
                );
                apply_rope_half(
                    &mut k[batch * kv_inner..(batch + 1) * kv_inner],
                    n,
                    MUSIC3_LM_KV_HEADS,
                    MUSIC3_LM_HEAD_DIM,
                    &cos,
                    &sin,
                );
            }
            cond.k[layer] = k[..kv_inner].to_vec();
            cond.v[layer] = v[..kv_inner].to_vec();
            uncond.k[layer] = k[kv_inner..].to_vec();
            uncond.v[layer] = v[kv_inner..].to_vec();
            let mut attn = Vec::with_capacity(2 * q_inner);
            for (session, qb) in [(&cond, 0), (&uncond, 1)] {
                attn.extend(causal_attn_gqa(
                    &q[qb * q_inner..(qb + 1) * q_inner],
                    &session.k[layer],
                    &session.v[layer],
                    n,
                    MUSIC3_LM_HEADS,
                    MUSIC3_LM_KV_HEADS,
                    MUSIC3_LM_HEAD_DIM,
                    scale,
                ));
            }
            let attn = file.linear_nt(&format!("{p}.self_attn.o_proj.weight"), &attn, m2)?;
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
            let (up, gate) = file.linear_nt_up_gate(&un, &gn, &normed, m2)?;
            let ff = swiglu(&up, &gate);
            let ff = file.linear_nt(&format!("{p}.mlp.down_proj.weight"), &ff, m2)?;
            add_inplace(&mut hidden, &ff);
        }
        rms_norm_rows(
            &mut hidden,
            &self.final_norm,
            MUSIC3_LM_HIDDEN,
            MUSIC3_LM_RMS_EPS,
        );
        cond.pos = n;
        uncond.pos = n;
        let last_c = (n - 1) * MUSIC3_LM_HIDDEN;
        let last_u = n * MUSIC3_LM_HIDDEN + last_c;
        cond.last = hidden[last_c..last_c + MUSIC3_LM_HIDDEN].to_vec();
        uncond.last = hidden[last_u..last_u + MUSIC3_LM_HIDDEN].to_vec();
        Ok((cond, uncond))
    }

    fn fill_cache(
        &self,
        file: &Music3GgufFile,
        ids: &[u32],
        session: &mut Music3GgufLmSession,
    ) -> Result<()> {
        let n = ids.len();
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
        let kv_inner = MUSIC3_LM_KV_HEADS * MUSIC3_LM_HEAD_DIM;
        for layer in 0..session.layers {
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
            let t0 = std::time::Instant::now();
            let (mut q, mut k, v) = file.linear_nt_qkv(&qn, &kn, &vn, &normed, n)?;
            if layer < 2 {
                eprintln!(
                    "  pf-l{layer} qkv={:.1}ms",
                    t0.elapsed().as_secs_f32() * 1e3
                );
            }
            perf::add(&perf::PF_QKV, t0);
            rms_norm_rows(&mut k, &self.k_norm[layer], MUSIC3_LM_HEAD_DIM, MUSIC3_LM_RMS_EPS);
            apply_rope_half(&mut k, n, MUSIC3_LM_KV_HEADS, MUSIC3_LM_HEAD_DIM, &cos, &sin);
            session.k[layer] = k;
            session.v[layer] = v;
            rms_norm_rows(&mut q, &self.q_norm[layer], MUSIC3_LM_HEAD_DIM, MUSIC3_LM_RMS_EPS);
            apply_rope_half(&mut q, n, MUSIC3_LM_HEADS, MUSIC3_LM_HEAD_DIM, &cos, &sin);
            let scale = 1.0 / (MUSIC3_LM_HEAD_DIM as f32).sqrt();
            let t0 = std::time::Instant::now();
            let attn = causal_attn_gqa(
                &q,
                &session.k[layer],
                &session.v[layer],
                n,
                MUSIC3_LM_HEADS,
                MUSIC3_LM_KV_HEADS,
                MUSIC3_LM_HEAD_DIM,
                scale,
            );
            perf::add(&perf::PF_ATTN, t0);
            let t0 = std::time::Instant::now();
            let t_op = std::time::Instant::now();
            let attn = file.linear_nt(&format!("{p}.self_attn.o_proj.weight"), &attn, n)?;
            let ms_o = t_op.elapsed().as_secs_f32() * 1e3;
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
            let t_op = std::time::Instant::now();
            let (up, gate) = file.linear_nt_up_gate(&un, &gn, &normed, n)?;
            let ms_upgate = t_op.elapsed().as_secs_f32() * 1e3;
            let t_op = std::time::Instant::now();
            let ff = swiglu(&up, &gate);
            let ms_swiglu = t_op.elapsed().as_secs_f32() * 1e3;
            let t_op = std::time::Instant::now();
            let ff = file.linear_nt(&format!("{p}.mlp.down_proj.weight"), &ff, n)?;
            let ms_down = t_op.elapsed().as_secs_f32() * 1e3;
            add_inplace(&mut hidden, &ff);
            perf::add(&perf::PF_LINEAR, t0);
            if layer < 2 {
                eprintln!(
                    "  pf-l{layer} n={n} o={ms_o:.1}ms upgate={ms_upgate:.1}ms swiglu={ms_swiglu:.1}ms down={ms_down:.1}ms"
                );
            }
            let _ = kv_inner;
        }
        session.pos = n;
        rms_norm_rows(
            &mut hidden,
            &self.final_norm,
            MUSIC3_LM_HIDDEN,
            MUSIC3_LM_RMS_EPS,
        );
        session.last = hidden[(n - 1) * MUSIC3_LM_HIDDEN..].to_vec();
        Ok(())
    }

    pub fn step_embed(
        &self,
        file: &Music3GgufFile,
        session: &mut Music3GgufLmSession,
        embed: &[f32],
    ) -> Result<Vec<f32>> {
        if embed.len() != MUSIC3_LM_HIDDEN {
            return Err(DiffusionError::model("gguf LM step embed width"));
        }
        let mut hidden = embed.to_vec();
        let pos = session.pos;
        let half = MUSIC3_LM_HEAD_DIM / 2;
        let mut cos = vec![0f32; half];
        let mut sin = vec![0f32; half];
        for j in 0..half {
            let angle = pos as f32 * self.rope_inv_freq[j];
            cos[j] = angle.cos();
            sin[j] = angle.sin();
        }
        let scale = 1.0 / (MUSIC3_LM_HEAD_DIM as f32).sqrt();
        for layer in 0..session.layers {
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
            let (mut q, mut k, v) = file.linear_nt_qkv(&qn, &kn, &vn, &normed, 1)?;
            rms_norm_rows(&mut q, &self.q_norm[layer], MUSIC3_LM_HEAD_DIM, MUSIC3_LM_RMS_EPS);
            rms_norm_rows(&mut k, &self.k_norm[layer], MUSIC3_LM_HEAD_DIM, MUSIC3_LM_RMS_EPS);
            apply_rope_half(&mut q, 1, MUSIC3_LM_HEADS, MUSIC3_LM_HEAD_DIM, &cos, &sin);
            apply_rope_half(&mut k, 1, MUSIC3_LM_KV_HEADS, MUSIC3_LM_HEAD_DIM, &cos, &sin);
            session.k[layer].extend_from_slice(&k);
            session.v[layer].extend_from_slice(&v);
            let t = session.pos + 1;
            let attn = attn_q_vs_kv_gqa(
                &q,
                &session.k[layer],
                &session.v[layer],
                t,
                MUSIC3_LM_HEADS,
                MUSIC3_LM_KV_HEADS,
                MUSIC3_LM_HEAD_DIM,
                scale,
            );
            let attn = file.linear_nt(&format!("{p}.self_attn.o_proj.weight"), &attn, 1)?;
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
            let (up, gate) = file.linear_nt_up_gate(&un, &gn, &normed, 1)?;
            let ff = swiglu(&up, &gate);
            let ff = file.linear_nt(&format!("{p}.mlp.down_proj.weight"), &ff, 1)?;
            add_inplace(&mut hidden, &ff);
        }
        session.pos += 1;
        rms_norm_rows(
            &mut hidden,
            &self.final_norm,
            MUSIC3_LM_HIDDEN,
            MUSIC3_LM_RMS_EPS,
        );
        session.last = hidden.clone();
        Ok(hidden)
    }

    fn try_step_embed_pair_resident(
        &self,
        file: &Music3GgufFile,
        cond: &mut Music3GgufLmSession,
        uncond: &mut Music3GgufLmSession,
        embed_c: &[f32],
        embed_u: &[f32],
    ) -> Option<()> {
        use makepad_ggml::backend::metal;
        use std::sync::OnceLock;
        static LOG: OnceLock<()> = OnceLock::new();
        let ns = format!("music3-{}", file.role.as_str());
        let mut hidden = Vec::with_capacity(2 * MUSIC3_LM_HIDDEN);
        hidden.extend_from_slice(embed_c);
        hidden.extend_from_slice(embed_u);
        let pos = cond.pos;
        let half = MUSIC3_LM_HEAD_DIM / 2;
        let mut cos = vec![0f32; half];
        let mut sin = vec![0f32; half];
        for j in 0..half {
            let angle = pos as f32 * self.rope_inv_freq[j];
            cos[j] = angle.cos();
            sin[j] = angle.sin();
        }
        let scale = 1.0 / (MUSIC3_LM_HEAD_DIM as f32).sqrt();
        let q_w = MUSIC3_LM_HEADS * MUSIC3_LM_HEAD_DIM;
        let kv_w = MUSIC3_LM_KV_HEADS * MUSIC3_LM_HEAD_DIM;
        for layer in 0..cond.layers {
            let p = format!("model.layers.{layer}");
            let qn = format!("{p}.self_attn.q_proj.weight");
            let kn = format!("{p}.self_attn.k_proj.weight");
            let vn = format!("{p}.self_attn.v_proj.weight");
            let on = format!("{p}.self_attn.o_proj.weight");
            let un = format!("{p}.mlp.up_proj.weight");
            let gn = format!("{p}.mlp.gate_proj.weight");
            let dn = format!("{p}.mlp.down_proj.weight");
            let qm = file.keyed_mat(&ns, &qn)?;
            let km = file.keyed_mat(&ns, &kn)?;
            let vm = file.keyed_mat(&ns, &vn)?;
            let om = file.keyed_mat(&ns, &on)?;
            let um = file.keyed_mat(&ns, &un)?;
            let gm = file.keyed_mat(&ns, &gn)?;
            let dm = file.keyed_mat(&ns, &dn)?;
            let in_key = format!("{p}.input_layernorm.weight");
            let qn_key = format!("{p}.self_attn.q_norm.weight");
            let kn_key = format!("{p}.self_attn.k_norm.weight");
            let post_key = format!("{p}.post_attention_layernorm.weight");
            let upload = if layer == 0 { Some(hidden.as_slice()) } else { None };
            let t0 = std::time::Instant::now();
            let (mut q, mut k, v) = metal::try_ar_pre_attn(
                upload,
                2,
                MUSIC3_LM_HIDDEN,
                MUSIC3_LM_HEAD_DIM,
                &self.input_norm[layer],
                Some((
                    self.q_norm[layer].as_slice(),
                    self.k_norm[layer].as_slice(),
                    qn_key.as_str(),
                    kn_key.as_str(),
                )),
                &in_key,
                MUSIC3_LM_RMS_EPS,
                qm,
                km,
                vm,
                |a, b| file.load_keyed_bytes(a, b),
            )?;
            perf::add(&perf::RES_PRE, t0);
            LOG.get_or_init(|| {
                eprintln!("[music3] ar resident layer on (m=2 hidden={MUSIC3_LM_HIDDEN})");
            });
            for batch in 0..2 {
                apply_rope_half(
                    &mut q[batch * q_w..(batch + 1) * q_w],
                    1,
                    MUSIC3_LM_HEADS,
                    MUSIC3_LM_HEAD_DIM,
                    &cos,
                    &sin,
                );
                apply_rope_half(
                    &mut k[batch * kv_w..(batch + 1) * kv_w],
                    1,
                    MUSIC3_LM_KV_HEADS,
                    MUSIC3_LM_HEAD_DIM,
                    &cos,
                    &sin,
                );
            }
            cond.k[layer].extend_from_slice(&k[..kv_w]);
            cond.v[layer].extend_from_slice(&v[..kv_w]);
            uncond.k[layer].extend_from_slice(&k[kv_w..]);
            uncond.v[layer].extend_from_slice(&v[kv_w..]);
            let t = cond.pos + 1;
            let t0 = std::time::Instant::now();
            let mut attn = Vec::with_capacity(2 * q_w);
            for (session, qb) in [(&*cond, 0), (&*uncond, 1)] {
                attn.extend(attn_q_vs_kv_gqa(
                    &q[qb * q_w..(qb + 1) * q_w],
                    &session.k[layer],
                    &session.v[layer],
                    t,
                    MUSIC3_LM_HEADS,
                    MUSIC3_LM_KV_HEADS,
                    MUSIC3_LM_HEAD_DIM,
                    scale,
                ));
            }
            perf::add(&perf::RES_ATTN, t0);
            let t0 = std::time::Instant::now();
            metal::try_ar_post_attn(
                &attn,
                2,
                MUSIC3_LM_HIDDEN,
                &self.post_attn_norm[layer],
                &post_key,
                MUSIC3_LM_RMS_EPS,
                om,
                um,
                gm,
                dm,
                |a, b| file.load_keyed_bytes(a, b),
            )?;
            perf::add(&perf::RES_POST, t0);
        }
        let t0 = std::time::Instant::now();
        let hidden = metal::try_ar_final_rms(
            2,
            MUSIC3_LM_HIDDEN,
            &self.final_norm,
            "model.norm.weight",
            MUSIC3_LM_RMS_EPS,
        )?;
        perf::add(&perf::RES_FINAL, t0);
        cond.pos += 1;
        uncond.pos += 1;
        cond.last = hidden[..MUSIC3_LM_HIDDEN].to_vec();
        uncond.last = hidden[MUSIC3_LM_HIDDEN..].to_vec();
        Some(())
    }

    /// One decode step for cond+uncond with stacked m=2 Metal GEMMs.
    pub fn step_embed_pair(
        &self,
        file: &Music3GgufFile,
        cond: &mut Music3GgufLmSession,
        uncond: &mut Music3GgufLmSession,
        embed_c: &[f32],
        embed_u: &[f32],
    ) -> Result<()> {
        if embed_c.len() != MUSIC3_LM_HIDDEN || embed_u.len() != MUSIC3_LM_HIDDEN {
            return Err(DiffusionError::model("gguf LM pair embed width"));
        }
        if cond.pos != uncond.pos || cond.layers != uncond.layers {
            return Err(DiffusionError::model("gguf LM pair session mismatch"));
        }
        // Whole-layer resident decode is the default: 1 command-buffer wait
        // per layer instead of 4, zero transient buffer allocs after the
        // pool warms. (It lost 3-5s before the pool + into-dst plumbing;
        // with allocation churn gone it wins.) MAKEPAD_MUSIC3_AR_HOST
        // restores the per-GEMM host-elementwise path.
        if std::env::var_os("MAKEPAD_MUSIC3_AR_HOST").is_none()
            && std::env::var_os("MAKEPAD_MUSIC3_CPU_LINEAR").is_none()
        {
            let pos0 = cond.pos;
            let klen: Vec<usize> = cond.k.iter().map(|k| k.len()).collect();
            if self
                .try_step_embed_pair_resident(file, cond, uncond, embed_c, embed_u)
                .is_some()
            {
                return Ok(());
            }
            // A mid-stack miss leaves partially appended KV; trim before the
            // host rerun appends again.
            if cond.pos == pos0 {
                for (layer, len) in klen.iter().enumerate() {
                    cond.k[layer].truncate(*len);
                    cond.v[layer].truncate(*len);
                    uncond.k[layer].truncate(*len);
                    uncond.v[layer].truncate(*len);
                }
            }
        }
        let mut hidden = Vec::with_capacity(2 * MUSIC3_LM_HIDDEN);
        hidden.extend_from_slice(embed_c);
        hidden.extend_from_slice(embed_u);
        let pos = cond.pos;
        let half = MUSIC3_LM_HEAD_DIM / 2;
        let mut cos = vec![0f32; half];
        let mut sin = vec![0f32; half];
        for j in 0..half {
            let angle = pos as f32 * self.rope_inv_freq[j];
            cos[j] = angle.cos();
            sin[j] = angle.sin();
        }
        let scale = 1.0 / (MUSIC3_LM_HEAD_DIM as f32).sqrt();
        let q_w = MUSIC3_LM_HEADS * MUSIC3_LM_HEAD_DIM;
        let kv_w = MUSIC3_LM_KV_HEADS * MUSIC3_LM_HEAD_DIM;
        for layer in 0..cond.layers {
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
            let t0 = std::time::Instant::now();
            let (mut q, mut k, v) = file.linear_nt_qkv(&qn, &kn, &vn, &normed, 2)?;
            perf::add(&perf::AR_QKV, t0);
            rms_norm_rows(&mut q, &self.q_norm[layer], MUSIC3_LM_HEAD_DIM, MUSIC3_LM_RMS_EPS);
            rms_norm_rows(&mut k, &self.k_norm[layer], MUSIC3_LM_HEAD_DIM, MUSIC3_LM_RMS_EPS);
            for batch in 0..2 {
                apply_rope_half(
                    &mut q[batch * q_w..(batch + 1) * q_w],
                    1,
                    MUSIC3_LM_HEADS,
                    MUSIC3_LM_HEAD_DIM,
                    &cos,
                    &sin,
                );
                apply_rope_half(
                    &mut k[batch * kv_w..(batch + 1) * kv_w],
                    1,
                    MUSIC3_LM_KV_HEADS,
                    MUSIC3_LM_HEAD_DIM,
                    &cos,
                    &sin,
                );
            }
            cond.k[layer].extend_from_slice(&k[..kv_w]);
            cond.v[layer].extend_from_slice(&v[..kv_w]);
            uncond.k[layer].extend_from_slice(&k[kv_w..]);
            uncond.v[layer].extend_from_slice(&v[kv_w..]);
            let t = cond.pos + 1;
            let t0 = std::time::Instant::now();
            let mut attn = Vec::with_capacity(2 * q_w);
            for (session, qb) in [(&*cond, 0), (&*uncond, 1)] {
                attn.extend(attn_q_vs_kv_gqa(
                    &q[qb * q_w..(qb + 1) * q_w],
                    &session.k[layer],
                    &session.v[layer],
                    t,
                    MUSIC3_LM_HEADS,
                    MUSIC3_LM_KV_HEADS,
                    MUSIC3_LM_HEAD_DIM,
                    scale,
                ));
            }
            perf::add(&perf::AR_ATTN, t0);
            let t0 = std::time::Instant::now();
            let attn = file.linear_nt(&format!("{p}.self_attn.o_proj.weight"), &attn, 2)?;
            perf::add(&perf::AR_O, t0);
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
            let t0 = std::time::Instant::now();
            let (up, gate) = file.linear_nt_up_gate(&un, &gn, &normed, 2)?;
            perf::add(&perf::AR_UPGATE, t0);
            let ff = swiglu(&up, &gate);
            let t0 = std::time::Instant::now();
            let ff = file.linear_nt(&format!("{p}.mlp.down_proj.weight"), &ff, 2)?;
            perf::add(&perf::AR_DOWN, t0);
            add_inplace(&mut hidden, &ff);
        }
        cond.pos += 1;
        uncond.pos += 1;
        rms_norm_rows(
            &mut hidden,
            &self.final_norm,
            MUSIC3_LM_HIDDEN,
            MUSIC3_LM_RMS_EPS,
        );
        cond.last = hidden[..MUSIC3_LM_HIDDEN].to_vec();
        uncond.last = hidden[MUSIC3_LM_HIDDEN..].to_vec();
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// RVQ residual sample (Python _generate_depth_codes)
// ---------------------------------------------------------------------------

fn rvq_project(file: &Music3GgufFile, hidden: &[f32]) -> Result<Vec<f32>> {
    file.linear_nt("projection.weight", hidden, 1)
}

fn rvq_head(file: &Music3GgufFile, hidden: &[f32], head: usize) -> Result<Vec<f32>> {
    file.linear_nt(&format!("audio_heads.{head}.weight"), hidden, 1)
}

fn embed_audio_frame(
    lm: &Music3GgufFile,
    rvq: &Music3GgufFile,
    semantic: u32,
    resid: &[u32],
) -> Result<Vec<f32>> {
    let mut embeds = lm.gather_rows("model.embed_tokens.weight", &[semantic])?;
    for (i, &code) in resid.iter().enumerate() {
        let idx = code + i as u32 * MUSIC3_AUDIO_VOCAB as u32;
        let extra = rvq.gather_rows("audio_embeddings.weight", &[idx])?;
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

fn rvq_depth_sample_cfg(
    file: &Music3GgufFile,
    rvq: &Music3GgufRvq,
    cond_h: &[f32],
    uncond_h: &[f32],
    sem_embed: &[f32],
    rng: &mut TorchPhilox,
    force_codes: Option<&[u32]>,
    dump_top: bool,
) -> Result<(Vec<u32>, Vec<f32>)> {
    let mut p0 = Vec::with_capacity(2 * MUSIC3_RVQ_HIDDEN);
    p0.extend_from_slice(cond_h);
    p0.extend_from_slice(uncond_h);
    let p0 = file.linear_nt("projection.weight", &p0, 2)?;
    let p1 = rvq_project(file, sem_embed)?;
    let mut seq_c = Vec::new();
    let mut seq_u = Vec::new();
    seq_c.extend_from_slice(&p0[..MUSIC3_RVQ_HIDDEN]);
    seq_c.extend_from_slice(&p1);
    seq_u.extend_from_slice(&p0[MUSIC3_RVQ_HIDDEN..]);
    seq_u.extend_from_slice(&p1);
    let t0 = std::time::Instant::now();
    let (mut sess_c, mut sess_u) = rvq.prefill_pair(file, &seq_c, &seq_u, 2)?;
    perf::add(&perf::RVQ_PREFILL, t0);
    let mut codes = Vec::new();
    let mut parts = Vec::new();
    for head in 0..(MUSIC3_NUM_CODEBOOKS - 1) {
        if head > 0 {
            let token = &seq_c[seq_c.len() - MUSIC3_RVQ_HIDDEN..];
            let t0 = std::time::Instant::now();
            rvq.step_pair(file, &mut sess_c, &mut sess_u, token, token)?;
            perf::add(&perf::RVQ_STEP, t0);
        }
        parts.extend_from_slice(&sess_c.last);
        let mut last2 = Vec::with_capacity(2 * MUSIC3_RVQ_HIDDEN);
        last2.extend_from_slice(&sess_c.last);
        last2.extend_from_slice(&sess_u.last);
        let t0 = std::time::Instant::now();
        let logits = file.linear_nt(&format!("audio_heads.{head}.weight"), &last2, 2)?;
        perf::add(&perf::RVQ_HEAD, t0);
        if logits.len() != 2 * MUSIC3_AUDIO_VOCAB {
            return Err(DiffusionError::model("music3 gguf RVQ pair head"));
        }
        let logits_c = logits[..MUSIC3_AUDIO_VOCAB].to_vec();
        let logits_u = &logits[MUSIC3_AUDIO_VOCAB..];
        let mut guided = logits_c;
        for (c, u) in guided.iter_mut().zip(logits_u.iter()) {
            *c = *u + MUSIC3_AR_CFG * (*c - *u);
        }
        let greedy = std::env::var_os("MAKEPAD_MUSIC3_AR_GREEDY").is_some()
            || std::env::var_os("MAKEPAD_MUSIC3_RVQ_GREEDY").is_some();
        if dump_top && head < 2 {
            let mut top: Vec<(f32, usize)> = guided
                .iter()
                .copied()
                .enumerate()
                .filter(|(_, v)| v.is_finite())
                .map(|(i, v)| (v, i))
                .collect();
            top.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            eprint!("  rvq-h{head} top5");
            for (v, i) in top.iter().take(5) {
                eprint!(" {i}:{v:.3}");
            }
            if head == 0 {
                let official = [271usize, 197, 481];
                eprint!("  @");
                for i in official {
                    if i < guided.len() {
                        eprint!(" {i}:{:.3}", guided[i]);
                    }
                }
            }
            eprintln!();
        }
        let code = if let Some(forced) = force_codes {
            if head < forced.len() {
                let _ = sample_top_k(&guided, MUSIC3_AR_TOP_K, rng);
                forced[head]
            } else {
                sample_top_k(&guided, MUSIC3_AR_TOP_K, rng) as u32
            }
        } else if greedy {
            guided
                .iter()
                .enumerate()
                .filter(|(_, v)| v.is_finite())
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, _)| i as u32)
                .unwrap_or(0)
        } else {
            sample_top_k(&guided, MUSIC3_AR_TOP_K, rng) as u32
        };
        codes.push(code);
        if head + 1 < MUSIC3_NUM_CODEBOOKS - 1 {
            let idx = code + head as u32 * MUSIC3_AUDIO_VOCAB as u32;
            let t0 = std::time::Instant::now();
            let embed = file.gather_rows("audio_embeddings.weight", &[idx])?;
            let proj = rvq_project(file, &embed)?;
            perf::add(&perf::RVQ_EMBED, t0);
            seq_c.extend_from_slice(&proj);
            seq_u.extend_from_slice(&proj);
        }
    }
    Ok((codes, parts))
}

/// Cond+uncond AR. Returns fused hiddens `[frames, 8*4096]` (cond last + 7 RVQ).
pub fn gguf_ar_sample(
    pack: &Music3GgufPack,
    lm: &Music3GgufLm,
    rvq: &Music3GgufRvq,
    cond_ids: &[u32],
    uncond_ids: &[u32],
    max_frames: usize,
    seed: u64,
    layers: usize,
    progress: &mut dyn FnMut(&str, usize, usize),
) -> Result<Vec<f32>> {
    gguf_ar_sample_forced(
        pack,
        lm,
        rvq,
        cond_ids,
        uncond_ids,
        max_frames,
        seed,
        layers,
        None,
        None,
        progress,
    )
}

pub fn gguf_ar_sample_forced(
    pack: &Music3GgufPack,
    lm: &Music3GgufLm,
    rvq: &Music3GgufRvq,
    cond_ids: &[u32],
    uncond_ids: &[u32],
    max_frames: usize,
    seed: u64,
    layers: usize,
    force_semantic: Option<&[u32]>,
    force_rvq: Option<&[u32]>,
    progress: &mut dyn FnMut(&str, usize, usize),
) -> Result<Vec<f32>> {
    // Stacked pair prefill (m=2*seq) is opt-in: one run matched last_h but
    // decode lm_step doubled, likely from the larger prefill working set.
    let (mut cond_s, mut uncond_s) = if cond_ids.len() == uncond_ids.len()
        && !cond_ids.is_empty()
        && std::env::var_os("MAKEPAD_MUSIC3_PAIR_PREFILL").is_some()
    {
        progress("prefill-pair", 0, 1);
        lm.prefill_pair(&pack.language_model, cond_ids, uncond_ids, layers)?
    } else {
        progress("prefill-cond", 0, 1);
        let t_pf = std::time::Instant::now();
        let cond_s = lm.prefill_session(&pack.language_model, cond_ids, layers)?;
        eprintln!("  prefill-pass1 {:.2}s", t_pf.elapsed().as_secs_f32());
        progress("prefill-uncond", 0, 1);
        let t_pf = std::time::Instant::now();
        let uncond_s = lm.prefill_session(&pack.language_model, uncond_ids, layers)?;
        eprintln!("  prefill-pass2 {:.2}s", t_pf.elapsed().as_secs_f32());
        (cond_s, uncond_s)
    };
    // Prefill's m=100..208 GEMMs leave ~1 GB of large recycled transients in
    // the pool; decode uses none of those sizes. Drop them at the phase edge
    // (pair-prefill runs showed decode slowing while they stayed retained).
    makepad_ggml::backend::metal::transient_pool_clear();
    // One up-front KV allocation per layer instead of Vec-doubling copies
    // of multi-MB caches mid-generation.
    let kv_row = MUSIC3_LM_KV_HEADS * MUSIC3_LM_HEAD_DIM;
    for session in [&mut cond_s, &mut uncond_s] {
        for layer in 0..session.layers {
            let need = (session.pos + max_frames + 8) * kv_row;
            let have = session.k[layer].len();
            session.k[layer].reserve(need.saturating_sub(have));
            let have = session.v[layer].len();
            session.v[layer].reserve(need.saturating_sub(have));
        }
    }
    if let Ok(path) = std::env::var("MAKEPAD_MUSIC3_DUMP_LAST_H") {
        let mut bytes = Vec::with_capacity(cond_s.last.len() * 4);
        for v in &cond_s.last {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let _ = std::fs::write(&path, bytes);
        eprintln!("  dumped last_h {} -> {path}", FiniteStats::of(&cond_s.last));
    }
    if let Ok(path) = std::env::var("MAKEPAD_MUSIC3_LOAD_LAST_H") {
        let bytes = std::fs::read(&path).map_err(|err| {
            DiffusionError::workflow(format!("load last_h {path}: {err}"))
        })?;
        if bytes.len() == cond_s.last.len() * 4 {
            for (i, chunk) in bytes.chunks_exact(4).enumerate() {
                cond_s.last[i] = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            }
            eprintln!("  loaded last_h {} from {path}", FiniteStats::of(&cond_s.last));
        } else {
            eprintln!(
                "  last_h file {} bytes, expected {}",
                bytes.len(),
                cond_s.last.len() * 4
            );
        }
    }
    let mut rng = TorchPhilox::new(seed);
    let mut hiddens = Vec::new();
    let mut semantic_toks = Vec::new();
    let lo = MUSIC3_AUDIO_CODE_OFFSET;
    let mut t_head = 0f32;
    let mut t_rvq = 0f32;
    let mut t_step = 0f32;
    let mut t_step_win = 0f32;
    for frame_index in 0..=max_frames {
        progress("ar", frame_index, max_frames);
        let t0 = std::time::Instant::now();
        let (end_c, sem_c, end_u, sem_u) =
            lm.head_audio_pair(&pack.language_model, &cond_s.last, &uncond_s.last)?;
        t_head += t0.elapsed().as_secs_f32();
        let mut cond_pack = Vec::with_capacity(1 + MUSIC3_SEMANTIC_VOCAB);
        let mut uncond_pack = Vec::with_capacity(1 + MUSIC3_SEMANTIC_VOCAB);
        cond_pack.push(end_c);
        cond_pack.extend_from_slice(&sem_c);
        uncond_pack.push(end_u);
        uncond_pack.extend_from_slice(&sem_u);
        let thresh = kth_largest(&cond_pack, MUSIC3_AR_TOP_K);
        let mut guided = cond_pack.clone();
        for (g, (c, u)) in guided
            .iter_mut()
            .zip(cond_pack.iter().zip(uncond_pack.iter()))
        {
            *g = *u + MUSIC3_AR_CFG * (*c - *u);
            if *c < thresh {
                *g = f32::NEG_INFINITY;
            }
        }
        if frame_index == 0 {
            let mut top: Vec<(f32, usize)> = guided
                .iter()
                .copied()
                .enumerate()
                .filter(|(_, v)| v.is_finite())
                .map(|(i, v)| (v, i))
                .collect();
            top.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            eprint!("  ar-first-guided top10");
            for (v, i) in top.iter().take(10) {
                let tok = if *i == 0 {
                    MUSIC3_AUDIO_END_TOKEN_ID
                } else {
                    lo + (*i as u32 - 1)
                };
                eprint!(" {tok}:{v:.3}");
            }
            let head: Vec<f32> = cond_s.last.iter().copied().take(8).collect();
            eprintln!(
                "  last_h {} head={head:?} cond_last {}",
                FiniteStats::of(&cond_s.last),
                FiniteStats::of(&sem_c)
            );
        }
        let greedy = std::env::var_os("MAKEPAD_MUSIC3_AR_GREEDY").is_some();
        let mut pack_ids = vec![MUSIC3_AUDIO_END_TOKEN_ID];
        for i in 0..MUSIC3_SEMANTIC_VOCAB {
            pack_ids.push(lo + i as u32);
        }
        let packed = if greedy {
            guided
                .iter()
                .enumerate()
                .filter(|(_, v)| v.is_finite())
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, _)| i)
                .unwrap_or(0)
        } else {
            sample_top_k_ids(&guided, &pack_ids, MUSIC3_AR_TOP_K, &mut rng)
        };
        let mut tok = if packed == 0 {
            MUSIC3_AUDIO_END_TOKEN_ID
        } else {
            lo + (packed as u32 - 1)
        };
        if let Some(forced) = force_semantic {
            if frame_index < forced.len() {
                tok = forced[frame_index];
            }
        }
        if tok == MUSIC3_AUDIO_END_TOKEN_ID {
            eprintln!("  ar-eos frame={frame_index}");
            break;
        }
        semantic_toks.push(tok);
        if frame_index < 16 || frame_index + 1 == max_frames {
            eprintln!("  ar-tok[{frame_index}] packed={packed} tok={tok}");
        }
        let sem_embed = pack
            .language_model
            .gather_rows("model.embed_tokens.weight", &[tok])?;
        let force_frame = force_rvq.and_then(|all| {
            let start = frame_index * 7;
            if start + 7 <= all.len() {
                Some(&all[start..start + 7])
            } else {
                None
            }
        });
        let t0 = std::time::Instant::now();
        let (resid, depth) = rvq_depth_sample_cfg(
            &pack.rvq,
            rvq,
            &cond_s.last,
            &uncond_s.last,
            &sem_embed,
            &mut rng,
            force_frame,
            frame_index < 3,
        )?;
        t_rvq += t0.elapsed().as_secs_f32();
        if frame_index < 3 {
            eprintln!("  ar-rvq[{frame_index}] {resid:?}");
        }
        if frame_index > 0 {
            hiddens.extend_from_slice(&cond_s.last);
            hiddens.extend_from_slice(&depth);
        }
        if music3_ar_frames(&hiddens) >= max_frames {
            break;
        }
        let feedback = embed_audio_frame(&pack.language_model, &pack.rvq, tok, &resid)?;
        // 16-frame lm_step windows separate a flat slowdown (durable state)
        // from a decaying transient (residency/clock recovery after prefill).
        if frame_index % 16 == 0 && frame_index > 0 {
            eprintln!(
                "  lm-step-win f{}..{} {:.2}s",
                frame_index - 16,
                frame_index,
                t_step - t_step_win
            );
            t_step_win = t_step;
        }
        let t0 = std::time::Instant::now();
        lm.step_embed_pair(
            &pack.language_model,
            &mut cond_s,
            &mut uncond_s,
            &feedback,
            &feedback,
        )?;
        t_step += t0.elapsed().as_secs_f32();
    }
    let show: Vec<u32> = semantic_toks.iter().copied().take(32).collect();
    eprintln!(
        "  ar-semantic n={} first32={show:?}",
        semantic_toks.len()
    );
    eprintln!("  ar-break head={t_head:.2}s rvq={t_rvq:.2}s lm_step={t_step:.2}s");
    eprintln!(
        "  ar-perf lm[qkv={:.2} attn={:.2} o={:.2} upgate={:.2} down={:.2}] rvq[pf={:.2} step={:.2} head={:.2} embed={:.2}] prefill[qkv={:.2} attn={:.2} lin={:.2}]",
        perf::secs(&perf::AR_QKV),
        perf::secs(&perf::AR_ATTN),
        perf::secs(&perf::AR_O),
        perf::secs(&perf::AR_UPGATE),
        perf::secs(&perf::AR_DOWN),
        perf::secs(&perf::RVQ_PREFILL),
        perf::secs(&perf::RVQ_STEP),
        perf::secs(&perf::RVQ_HEAD),
        perf::secs(&perf::RVQ_EMBED),
        perf::secs(&perf::PF_QKV),
        perf::secs(&perf::PF_ATTN),
        perf::secs(&perf::PF_LINEAR),
    );
    eprintln!(
        "  ar-res-perf pre={:.2} attn={:.2} post={:.2} final={:.2}",
        perf::secs(&perf::RES_PRE),
        perf::secs(&perf::RES_ATTN),
        perf::secs(&perf::RES_POST),
        perf::secs(&perf::RES_FINAL),
    );
    Ok(hiddens)
}

fn music3_ar_frames(hiddens: &[f32]) -> usize {
    let width = MUSIC3_COND_LAYERS * MUSIC3_COND_HIDDEN;
    if width == 0 {
        0
    } else {
        hiddens.len() / width
    }
}

// ---------------------------------------------------------------------------
// DiT Euler (Python ModularPipeline flow)
// ---------------------------------------------------------------------------

/// Per-op wall clock across all Euler steps, printed once as `dit-perf`.
mod dit_perf {
    use std::sync::atomic::{AtomicU64, Ordering};

    pub static LN: AtomicU64 = AtomicU64::new(0);
    pub static QKV: AtomicU64 = AtomicU64::new(0);
    pub static ROPE: AtomicU64 = AtomicU64::new(0);
    pub static ATTN: AtomicU64 = AtomicU64::new(0);
    pub static O: AtomicU64 = AtomicU64::new(0);
    pub static FFN: AtomicU64 = AtomicU64::new(0);

    pub fn add(slot: &AtomicU64, t0: std::time::Instant) {
        slot.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }

    pub fn secs(slot: &AtomicU64) -> f32 {
        slot.load(Ordering::Relaxed) as f32 / 1e9
    }
}

/// DiT-scoped LayerNorm: threaded CPU rows by default. The generic
/// [`layer_norm`] Metal path costs a submit plus a 2×7MB round trip per call
/// (576 calls per 8-step generate) for ~1ms of arithmetic. Rows are
/// independent, so threading is bit-exact vs the serial CPU reference.
/// `MAKEPAD_MUSIC3_DIT_LN_METAL` restores the old path.
fn dit_layer_norm(x: &mut [f32], weight: &[f32], bias: &[f32], width: usize) {
    if std::env::var_os("MAKEPAD_MUSIC3_DIT_LN_METAL").is_some() {
        return layer_norm(x, weight, bias, width);
    }
    makepad_ai_sfx::sa3::par_rows(x, width, &|_row, row| {
        let mut mean = 0f32;
        for v in row.iter() {
            mean += *v;
        }
        mean /= width as f32;
        let mut var = 0f32;
        for v in row.iter() {
            let d = *v - mean;
            var += d * d;
        }
        let inv = (var / width as f32 + LN_EPS).sqrt().recip();
        for (v, (w, b)) in row.iter_mut().zip(weight.iter().zip(bias.iter())) {
            *v = (*v - mean) * inv * *w + *b;
        }
    });
}

struct DitPrepared {
    time_proj: Vec<f32>,
    time_w1: Vec<f32>,
    time_b1: Vec<f32>,
    time_w2: Vec<f32>,
    time_b2: Vec<f32>,
    norm1_w: Vec<Vec<f32>>,
    norm1_b: Vec<Vec<f32>>,
    norm2_w: Vec<Vec<f32>>,
    norm2_b: Vec<Vec<f32>>,
    ff_in_b: Vec<Vec<f32>>,
    ff_out_b: Vec<Vec<f32>>,
    rope_inv_freq: Vec<f32>,
}

impl DitPrepared {
    fn load(file: &Music3GgufFile) -> Result<Self> {
        let mut norm1_w = Vec::new();
        let mut norm1_b = Vec::new();
        let mut norm2_w = Vec::new();
        let mut norm2_b = Vec::new();
        let mut ff_in_b = Vec::new();
        let mut ff_out_b = Vec::new();
        for layer in 0..MUSIC3_DIT_LAYERS {
            norm1_w.push(file.read_f32_any(&format!("transformer_blocks.{layer}.norm1.weight"))?);
            norm1_b.push(file.read_f32_any(&format!("transformer_blocks.{layer}.norm1.bias"))?);
            norm2_w.push(file.read_f32_any(&format!("transformer_blocks.{layer}.norm2.weight"))?);
            norm2_b.push(file.read_f32_any(&format!("transformer_blocks.{layer}.norm2.bias"))?);
            ff_in_b.push(file.read_f32_any(&format!("transformer_blocks.{layer}.ff_in.bias"))?);
            ff_out_b.push(file.read_f32_any(&format!("transformer_blocks.{layer}.ff_out.bias"))?);
        }
        let half = MUSIC3_DIT_ROPE / 2;
        let mut rope_inv_freq = Vec::with_capacity(half);
        for j in 0..half {
            rope_inv_freq.push(1.0 / 10_000f32.powf(2.0 * j as f32 / MUSIC3_DIT_ROPE as f32));
        }
        let time_proj = file.read_f32_any("time_proj.weight")?;
        let time_w1 = file.read_f32_any("time_embed.linear_1.weight")?;
        let time_b1 = file.read_f32_any("time_embed.linear_1.bias")?;
        let time_w2 = file.read_f32_any("time_embed.linear_2.weight")?;
        let time_b2 = file.read_f32_any("time_embed.linear_2.bias")?;
        {
            let i1 = file.require("time_embed.linear_1.weight")?;
            let i2 = file.require("time_embed.linear_2.weight")?;
            eprintln!(
                "  time_w1 len={} dims={:?} {}  time_w2 len={} dims={:?} {}",
                time_w1.len(),
                i1.dimensions,
                FiniteStats::of(&time_w1),
                time_w2.len(),
                i2.dimensions,
                FiniteStats::of(&time_w2)
            );
            // ggml [k,n]=[256,2048]: row0 = W_torch[0, :] ; stride-k = W_torch[:, 0]
            let row0: Vec<f32> = time_w1.iter().copied().take(8).collect();
            let col0: Vec<f32> = (0..8)
                .map(|o| time_w1[o * MUSIC3_DIT_FOURIER])
                .collect();
            eprintln!("  time_w1 ggml_row0[:8]={row0:?}");
            eprintln!("  time_w1 ggml_col0[:8]={col0:?}");
            let tp: Vec<f32> = time_proj.iter().copied().take(8).collect();
            let b1: Vec<f32> = time_b1.iter().copied().take(8).collect();
            eprintln!("  time_proj[:8]={tp:?}  time_b1[:8]={b1:?}");
        }
        Ok(Self {
            time_proj,
            time_w1,
            time_b1,
            time_w2,
            time_b2,
            norm1_w,
            norm1_b,
            norm2_w,
            norm2_b,
            ff_in_b,
            ff_out_b,
            rope_inv_freq,
        })
    }
}

fn time_embed_feat(p: &DitPrepared, t: f32) -> Vec<f32> {
    let nfreq = MUSIC3_DIT_FOURIER / 2;
    let mut feat = vec![0f32; MUSIC3_DIT_FOURIER];
    for i in 0..nfreq {
        let angle = 2.0 * PI * t * p.time_proj[i];
        feat[i] = angle.cos();
        feat[nfreq + i] = angle.sin();
    }
    feat
}

fn time_embed(p: &DitPrepared, t: f32) -> Vec<f32> {
    time_embed_from_feat(p, &time_embed_feat(p, t))
}

/// ggml `[k, n]` = `[in, out]`: output `o` is the contiguous `k`-vector at `o * k`.
/// Same contract as `linear_nt` / PyTorch `Linear.weight[out, in]`.
fn time_embed_from_feat(p: &DitPrepared, feat: &[f32]) -> Vec<f32> {
    time_embed_layout(p, feat, false)
}

fn time_embed_layout(p: &DitPrepared, feat: &[f32], in_major: bool) -> Vec<f32> {
    let mut h = vec![0f32; MUSIC3_DIT_DIM];
    for o in 0..MUSIC3_DIT_DIM {
        let mut acc = p.time_b1[o];
        for i in 0..MUSIC3_DIT_FOURIER {
            let idx = if in_major {
                i * MUSIC3_DIT_DIM + o
            } else {
                o * MUSIC3_DIT_FOURIER + i
            };
            acc += p.time_w1[idx] * feat[i];
        }
        h[o] = silu(acc);
    }
    let mut out = vec![0f32; MUSIC3_DIT_DIM];
    for o in 0..MUSIC3_DIT_DIM {
        let mut acc = p.time_b2[o];
        for i in 0..MUSIC3_DIT_DIM {
            let idx = if in_major {
                i * MUSIC3_DIT_DIM + o
            } else {
                o * MUSIC3_DIT_DIM + i
            };
            acc += p.time_w2[idx] * h[i];
        }
        out[o] = acc;
    }
    out
}

fn time_embed_linear_nt(file: &Music3GgufFile, p: &DitPrepared, t: f32) -> Result<Vec<f32>> {
    let feat = time_embed_feat(p, t);
    let mut h = file.linear_nt("time_embed.linear_1.weight", &feat, 1)?;
    for (v, b) in h.iter_mut().zip(p.time_b1.iter()) {
        *v = silu(*v + *b);
    }
    let mut out = file.linear_nt("time_embed.linear_2.weight", &h, 1)?;
    for (v, b) in out.iter_mut().zip(p.time_b2.iter()) {
        *v += *b;
    }
    Ok(out)
}

fn official_flow_sigmas(steps: usize) -> Vec<f32> {
    let n = steps.max(1);
    let start = 1.0f32;
    let end = 1.0 / n as f32;
    let mut sigmas = Vec::with_capacity(n + 1);
    for i in 0..n {
        let u = if n == 1 { 0.0 } else { i as f32 / (n - 1) as f32 };
        let raw = start + (end - start) * u;
        sigmas.push(1.0 - raw);
    }
    sigmas.push(1.0);
    if std::env::var_os("MAKEPAD_MUSIC3_SIGMA_FLIP").is_some() {
        // Standard FM 1→0 instead of official invert 0→1.
        for s in &mut sigmas {
            *s = 1.0 - *s;
        }
    }
    sigmas
}

fn dit_forward(
    file: &Music3GgufFile,
    prep: &DitPrepared,
    latents: &[f32],
    cond: &[f32],
    tokens: usize,
    timestep: f32,
) -> Result<Vec<f32>> {
    let mut cat = vec![0f32; tokens * MUSIC3_DIT_CONCAT];
    for t in 0..tokens {
        for c in 0..MUSIC3_DIT_IN_CHANNELS {
            cat[t * MUSIC3_DIT_CONCAT + c] = latents[c * tokens + t];
        }
        for c in 0..MUSIC3_DIT_COND {
            cat[t * MUSIC3_DIT_CONCAT + 2 * MUSIC3_DIT_IN_CHANNELS + c] = cond[t * MUSIC3_DIT_COND + c];
        }
    }
    let pre = file.linear_nt("preprocess_conv.weight", &cat, tokens)?;
    for (x, p) in cat.iter_mut().zip(pre.iter()) {
        *x += *p;
    }
    let mut hidden = file.linear_nt("proj_in.weight", &cat, tokens)?;
    let temb = match std::env::var("MAKEPAD_MUSIC3_TEMB").ok().as_deref() {
        Some("in_major") => time_embed_layout(prep, &time_embed_feat(prep, timestep), true),
        Some("linear_nt") => time_embed_linear_nt(file, prep, timestep)?,
        _ => time_embed(prep, timestep),
    };
    if std::env::var_os("MAKEPAD_MUSIC3_DIT_TRACE").is_some() {
        eprintln!(
            "  dit-proj_in {} temb {}",
            FiniteStats::of(&hidden),
            FiniteStats::of(&temb)
        );
    }
    let mut seq = Vec::with_capacity((tokens + 1) * MUSIC3_DIT_DIM);
    seq.extend_from_slice(&temb);
    seq.extend_from_slice(&hidden);
    hidden = seq;
    let n = tokens + 1;
    let rot_half = MUSIC3_DIT_ROPE / 2;
    let mut cos = vec![0f32; n * rot_half];
    let mut sin = vec![0f32; n * rot_half];
    for pos in 0..n {
        for j in 0..rot_half {
            let angle = pos as f32 * prep.rope_inv_freq[j];
            cos[pos * rot_half + j] = angle.cos();
            sin[pos * rot_half + j] = angle.sin();
        }
    }
    let scale = 1.0 / (MUSIC3_DIT_HEAD_DIM as f32).sqrt();
    let trace = std::env::var_os("MAKEPAD_MUSIC3_DIT_TRACE").is_some();
    if trace {
        eprintln!(
            "  dit-in t={timestep:.3} hidden {}",
            FiniteStats::of(&hidden)
        );
    }
    for layer in 0..MUSIC3_DIT_LAYERS {
        let mut normed = hidden.clone();
        dit_layer_norm(&mut normed, &prep.norm1_w[layer], &prep.norm1_b[layer], MUSIC3_DIT_DIM);
        let qn = format!("transformer_blocks.{layer}.attn.to_q.weight");
        let kn = format!("transformer_blocks.{layer}.attn.to_k.weight");
        let vn = format!("transformer_blocks.{layer}.attn.to_v.weight");
        let (mut q, mut k, v) = file.linear_nt_qkv(&qn, &kn, &vn, &normed, n)?;
        apply_rope_partial(
            &mut q,
            n,
            MUSIC3_DIT_HEADS,
            MUSIC3_DIT_HEAD_DIM,
            MUSIC3_DIT_ROPE,
            &cos,
            &sin,
        );
        apply_rope_partial(
            &mut k,
            n,
            MUSIC3_DIT_HEADS,
            MUSIC3_DIT_HEAD_DIM,
            MUSIC3_DIT_ROPE,
            &cos,
            &sin,
        );
        let attn = full_attn(
            &q,
            &k,
            &v,
            n,
            MUSIC3_DIT_HEADS,
            MUSIC3_DIT_HEAD_DIM,
            scale,
        );
        let attn = file.linear_nt(
            &format!("transformer_blocks.{layer}.attn.to_out.0.weight"),
            &attn,
            n,
        )?;
        add_inplace(&mut hidden, &attn);
        let mut normed = hidden.clone();
        dit_layer_norm(&mut normed, &prep.norm2_w[layer], &prep.norm2_b[layer], MUSIC3_DIT_DIM);
        let ff = dit_ffn(file, prep, layer, &normed, n)?;
        add_inplace(&mut hidden, &ff);
        if trace && (layer < 2 || layer + 1 == MUSIC3_DIT_LAYERS || layer == 17) {
            eprintln!("  dit-h L{layer} {}", FiniteStats::of(&hidden));
        }
    }
    let tok = hidden[MUSIC3_DIT_DIM..].to_vec();
    let hidden = file.linear_nt("proj_out.weight", &tok, tokens)?;
    let post = file.linear_nt("postprocess_conv.weight", &hidden, tokens)?;
    let mut tok_out = hidden;
    add_inplace(&mut tok_out, &post);
    let mut out = vec![0f32; MUSIC3_DIT_IN_CHANNELS * tokens];
    for t in 0..tokens {
        for c in 0..MUSIC3_DIT_IN_CHANNELS {
            out[c * tokens + t] = tok_out[t * MUSIC3_DIT_IN_CHANNELS + c];
        }
    }
    Ok(out)
}

/// Cond + uncond DiT in one stacked `m=2N` Metal pass. Attention / RoPE stay
/// per-sequence so CFG does not cross-attend.
fn dit_forward_cfg(
    file: &Music3GgufFile,
    prep: &DitPrepared,
    latents: &[f32],
    cond: &[f32],
    tokens: usize,
    timestep: f32,
) -> Result<(Vec<f32>, Vec<f32>)> {
    let zeros = vec![0f32; cond.len()];
    let mut cat = vec![0f32; 2 * tokens * MUSIC3_DIT_CONCAT];
    for (batch, cond_b) in [cond, zeros.as_slice()].into_iter().enumerate() {
        for t in 0..tokens {
            let row = (batch * tokens + t) * MUSIC3_DIT_CONCAT;
            for c in 0..MUSIC3_DIT_IN_CHANNELS {
                cat[row + c] = latents[c * tokens + t];
            }
            for c in 0..MUSIC3_DIT_COND {
                cat[row + 2 * MUSIC3_DIT_IN_CHANNELS + c] = cond_b[t * MUSIC3_DIT_COND + c];
            }
        }
    }
    let m2 = 2 * tokens;
    let pre = file.linear_nt("preprocess_conv.weight", &cat, m2)?;
    for (x, p) in cat.iter_mut().zip(pre.iter()) {
        *x += *p;
    }
    let stacked = file.linear_nt("proj_in.weight", &cat, m2)?;
    let temb = match std::env::var("MAKEPAD_MUSIC3_TEMB").ok().as_deref() {
        Some("in_major") => time_embed_layout(prep, &time_embed_feat(prep, timestep), true),
        Some("linear_nt") => time_embed_linear_nt(file, prep, timestep)?,
        _ => time_embed(prep, timestep),
    };
    let n = tokens + 1;
    let mut hidden = vec![0f32; 2 * n * MUSIC3_DIT_DIM];
    for batch in 0..2 {
        let src = &stacked[batch * tokens * MUSIC3_DIT_DIM..(batch + 1) * tokens * MUSIC3_DIT_DIM];
        let dst = &mut hidden[batch * n * MUSIC3_DIT_DIM..(batch + 1) * n * MUSIC3_DIT_DIM];
        dst[..MUSIC3_DIT_DIM].copy_from_slice(&temb);
        dst[MUSIC3_DIT_DIM..].copy_from_slice(src);
    }
    let rot_half = MUSIC3_DIT_ROPE / 2;
    let mut cos = vec![0f32; n * rot_half];
    let mut sin = vec![0f32; n * rot_half];
    for pos in 0..n {
        for j in 0..rot_half {
            let angle = pos as f32 * prep.rope_inv_freq[j];
            cos[pos * rot_half + j] = angle.cos();
            sin[pos * rot_half + j] = angle.sin();
        }
    }
    let scale = 1.0 / (MUSIC3_DIT_HEAD_DIM as f32).sqrt();
    let qkv_w = n * MUSIC3_DIT_HEADS * MUSIC3_DIT_HEAD_DIM;
    let mut normed = vec![0f32; hidden.len()];
    for layer in 0..MUSIC3_DIT_LAYERS {
        let t0 = std::time::Instant::now();
        normed.copy_from_slice(&hidden);
        dit_layer_norm(&mut normed, &prep.norm1_w[layer], &prep.norm1_b[layer], MUSIC3_DIT_DIM);
        dit_perf::add(&dit_perf::LN, t0);
        let qn = format!("transformer_blocks.{layer}.attn.to_q.weight");
        let kn = format!("transformer_blocks.{layer}.attn.to_k.weight");
        let vn = format!("transformer_blocks.{layer}.attn.to_v.weight");
        let t0 = std::time::Instant::now();
        let (mut q, mut k, v) = file.linear_nt_qkv(&qn, &kn, &vn, &normed, 2 * n)?;
        dit_perf::add(&dit_perf::QKV, t0);
        let t0 = std::time::Instant::now();
        for batch in 0..2 {
            let qs = batch * qkv_w;
            apply_rope_partial(
                &mut q[qs..qs + qkv_w],
                n,
                MUSIC3_DIT_HEADS,
                MUSIC3_DIT_HEAD_DIM,
                MUSIC3_DIT_ROPE,
                &cos,
                &sin,
            );
            apply_rope_partial(
                &mut k[qs..qs + qkv_w],
                n,
                MUSIC3_DIT_HEADS,
                MUSIC3_DIT_HEAD_DIM,
                MUSIC3_DIT_ROPE,
                &cos,
                &sin,
            );
        }
        dit_perf::add(&dit_perf::ROPE, t0);
        let t0 = std::time::Instant::now();
        let mut attn = Vec::with_capacity(2 * qkv_w);
        for batch in 0..2 {
            let s = batch * qkv_w;
            attn.extend(full_attn(
                &q[s..s + qkv_w],
                &k[s..s + qkv_w],
                &v[s..s + qkv_w],
                n,
                MUSIC3_DIT_HEADS,
                MUSIC3_DIT_HEAD_DIM,
                scale,
            ));
        }
        dit_perf::add(&dit_perf::ATTN, t0);
        let t0 = std::time::Instant::now();
        let attn = file.linear_nt(
            &format!("transformer_blocks.{layer}.attn.to_out.0.weight"),
            &attn,
            2 * n,
        )?;
        dit_perf::add(&dit_perf::O, t0);
        add_inplace(&mut hidden, &attn);
        let t0 = std::time::Instant::now();
        normed.copy_from_slice(&hidden);
        dit_layer_norm(&mut normed, &prep.norm2_w[layer], &prep.norm2_b[layer], MUSIC3_DIT_DIM);
        dit_perf::add(&dit_perf::LN, t0);
        let t0 = std::time::Instant::now();
        let ff = dit_ffn(file, prep, layer, &normed, 2 * n)?;
        dit_perf::add(&dit_perf::FFN, t0);
        add_inplace(&mut hidden, &ff);
    }
    let mut tok = Vec::with_capacity(2 * tokens * MUSIC3_DIT_DIM);
    for batch in 0..2 {
        let seq = &hidden[batch * n * MUSIC3_DIT_DIM..(batch + 1) * n * MUSIC3_DIT_DIM];
        tok.extend_from_slice(&seq[MUSIC3_DIT_DIM..]);
    }
    let hidden = file.linear_nt("proj_out.weight", &tok, m2)?;
    let post = file.linear_nt("postprocess_conv.weight", &hidden, m2)?;
    let mut tok_out = hidden;
    add_inplace(&mut tok_out, &post);
    let mut outs = [vec![0f32; MUSIC3_DIT_IN_CHANNELS * tokens], vec![0f32; MUSIC3_DIT_IN_CHANNELS * tokens]];
    for batch in 0..2 {
        let src = &tok_out[batch * tokens * MUSIC3_DIT_IN_CHANNELS..(batch + 1) * tokens * MUSIC3_DIT_IN_CHANNELS];
        for t in 0..tokens {
            for c in 0..MUSIC3_DIT_IN_CHANNELS {
                outs[batch][c * tokens + t] = src[t * MUSIC3_DIT_IN_CHANNELS + c];
            }
        }
    }
    let [v_cond, v_uncond] = outs;
    Ok((v_cond, v_uncond))
}

fn dit_euler(
    file: &Music3GgufFile,
    prep: &DitPrepared,
    cond: &[f32],
    tokens: usize,
    steps: usize,
    seed: u64,
    progress: &mut dyn FnMut(&str, usize, usize),
) -> Result<Vec<f32>> {
    dit_euler_x0(file, prep, cond, None, tokens, steps, seed, progress)
}

fn dit_euler_x0(
    file: &Music3GgufFile,
    prep: &DitPrepared,
    cond: &[f32],
    x0: Option<&[f32]>,
    tokens: usize,
    steps: usize,
    seed: u64,
    progress: &mut dyn FnMut(&str, usize, usize),
) -> Result<Vec<f32>> {
    let mut x = if let Some(x0) = x0 {
        if x0.len() != MUSIC3_DIT_IN_CHANNELS * tokens {
            return Err(DiffusionError::workflow("dit x0 length"));
        }
        x0.to_vec()
    } else {
        torch_randn(seed, MUSIC3_DIT_IN_CHANNELS * tokens)
    };
    let sigmas = official_flow_sigmas(steps);
    for i in 0..steps {
        progress("dit", i, steps);
        let t = sigmas[i];
        let (v_cond, v_uncond) = if std::env::var_os("MAKEPAD_MUSIC3_DIT_SERIAL_CFG").is_some() {
            let zeros = vec![0f32; cond.len()];
            (
                dit_forward(file, prep, &x, cond, tokens, t)?,
                dit_forward(file, prep, &x, &zeros, tokens, t)?,
            )
        } else {
            dit_forward_cfg(file, prep, &x, cond, tokens, t)?
        };
        let dt = sigmas[i + 1] - sigmas[i];
        if i == 0 {
            eprintln!(
                "  dit-v0 t={t:.3} dt={dt:.3} cond {} uncond {}",
                FiniteStats::of(&v_cond),
                FiniteStats::of(&v_uncond)
            );
        }
        for j in 0..x.len() {
            let v = v_uncond[j] + MUSIC3_FLOW_CFG * (v_cond[j] - v_uncond[j]);
            x[j] += dt * v;
        }
    }
    progress("dit", steps, steps);
    eprintln!(
        "  dit-perf ln={:.2}s qkv={:.2}s rope={:.2}s attn={:.2}s o={:.2}s ffn={:.2}s",
        dit_perf::secs(&dit_perf::LN),
        dit_perf::secs(&dit_perf::QKV),
        dit_perf::secs(&dit_perf::ROPE),
        dit_perf::secs(&dit_perf::ATTN),
        dit_perf::secs(&dit_perf::O),
        dit_perf::secs(&dit_perf::FFN),
    );
    Ok(x)
}

/// Full GGUF generate: AR → cond → DiT → vocoder. Planar stereo `[2, samples]`.
pub fn gguf_generate(
    pack: &Music3GgufPack,
    cond_ids: &[u32],
    uncond_ids: &[u32],
    max_frames: usize,
    steps: usize,
    seed: u64,
    lm_layers: usize,
    force_semantic: Option<&[u32]>,
    force_rvq: Option<&[u32]>,
    cond_ref: Option<&[f32]>,
    progress: &mut dyn FnMut(&str, usize, usize),
) -> Result<Vec<f32>> {
    let t_all = std::time::Instant::now();
    let lm = Music3GgufLm::prepare(&pack.language_model)?;
    let rvq = Music3GgufRvq::load(&pack.rvq)?;
    let t_ar = std::time::Instant::now();
    let hiddens = gguf_ar_sample_forced(
        pack,
        &lm,
        &rvq,
        cond_ids,
        uncond_ids,
        max_frames,
        seed,
        lm_layers,
        force_semantic,
        force_rvq,
        progress,
    )?;
    drop(lm);
    drop(rvq);
    makepad_ggml::backend::metal::ar_resident_clear();
    pack.language_model.clear_byte_cache();
    pack.rvq.clear_byte_cache();
    let frames = music3_ar_frames(&hiddens);
    if frames == 0 {
        return Err(DiffusionError::workflow("music3 gguf AR emitted no frames"));
    }
    eprintln!(
        "  time ar {:.2}s frames={frames} ({:.2}s/frame)",
        t_ar.elapsed().as_secs_f32(),
        t_ar.elapsed().as_secs_f32() / frames.max(1) as f32
    );
    progress("cond", 0, 1);
    let t_cond = std::time::Instant::now();
    let enc = pack.load_condition_encoder()?;
    let cond = enc.forward(&hiddens, frames)?;
    drop(enc);
    pack.condition.clear_byte_cache();
    eprintln!("  time cond {:.2}s {}", t_cond.elapsed().as_secs_f32(), FiniteStats::of(&cond));
    if let Some(pref) = cond_ref {
        if pref.len() == cond.len() {
            let mut dot = 0f64;
            let mut na = 0f64;
            let mut nb = 0f64;
            let mut max_d = 0f32;
            for (a, b) in cond.iter().zip(pref.iter()) {
                dot += (*a as f64) * (*b as f64);
                na += (*a as f64) * (*a as f64);
                nb += (*b as f64) * (*b as f64);
                max_d = max_d.max((*a - *b).abs());
            }
            let cos = dot / (na.sqrt() * nb.sqrt() + 1e-12);
            eprintln!("  cond vs official cos={cos:.4} maxabs={max_d:.4}");
        } else {
            eprintln!("  cond-ref len {} != cond {}", pref.len(), cond.len());
        }
    }
    if std::env::var_os("MAKEPAD_MUSIC3_AR_ONLY").is_some() {
        eprintln!("  ar-only: skip dit/vocoder");
        return Ok(vec![0f32; 2]);
    }
    // Vocoder GGUF dequant is ~18s of host work and touches a different file
    // than DiT. Hide it behind Euler.
    let voc_path = pack.paths.vocoder.clone();
    let voc_jh = std::thread::spawn(move || -> Result<Music3Vocoder> {
        let file = Music3GgufFile::open(Music3GgufRole::Vocoder, voc_path)?;
        Music3Vocoder::load_with(|name| file.read_f32(name))
    });
    let tokens = music3_latent_len(frames);
    let dit = DitPrepared::load(&pack.transformer)?;
    let t_dit = std::time::Instant::now();
    let latents = dit_euler(
        &pack.transformer,
        &dit,
        &cond,
        tokens,
        steps,
        seed,
        progress,
    )?;
    drop(dit);
    pack.transformer.clear_byte_cache();
    eprintln!("  time dit {:.2}s steps={steps} tokens={tokens}", t_dit.elapsed().as_secs_f32());
    eprintln!("  latents {}", FiniteStats::of(&latents));
    {
        let c = MUSIC3_DIT_IN_CHANNELS;
        let mut top: Vec<(f32, usize)> = (0..c)
            .map(|ch| {
                let row = &latents[ch * tokens..(ch + 1) * tokens];
                let e = row.iter().map(|v| v * v).sum::<f32>() / tokens.max(1) as f32;
                (e.sqrt(), ch)
            })
            .collect();
        top.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        eprint!("  latent-ch rms");
        for (rms, ch) in top.iter().take(8) {
            eprint!(" c{ch}={rms:.3}");
        }
        eprintln!();
    }
    progress("vocoder", 0, 1);
    let t_voc = std::time::Instant::now();
    let voc = voc_jh
        .join()
        .map_err(|_| DiffusionError::workflow("vocoder load thread"))??;
    let t_load = t_voc.elapsed().as_secs_f32();
    let audio = voc.decode(&latents, tokens)?;
    eprintln!(
        "  time vocoder {:.2}s (wait-load {t_load:.2}s)  total {:.2}s",
        t_voc.elapsed().as_secs_f32(),
        t_all.elapsed().as_secs_f32()
    );
    Ok(audio)
}

/// Transformer-only probe: time-embed layouts + one dummy DiT forward.
/// Skips AR so a layout check is seconds, not minutes.
pub fn gguf_dit_probe(pack: &Music3GgufPack, tokens: usize, seed: u64) -> Result<()> {
    let file = &pack.transformer;
    let prep = DitPrepared::load(file)?;
    eprintln!("  time_proj {}", FiniteStats::of(&prep.time_proj));
    for &t in &[0.0f32, 0.25, 0.5, 1.0] {
        let feat = time_embed_feat(&prep, t);
        let out_major = time_embed_layout(&prep, &feat, false);
        let in_major = time_embed_layout(&prep, &feat, true);
        let nt = time_embed_linear_nt(file, &prep, t)?;
        let head = |v: &[f32]| -> String {
            v.iter()
                .take(4)
                .map(|x| format!("{x:.4}"))
                .collect::<Vec<_>>()
                .join(",")
        };
        eprintln!(
            "  temb t={t:.2} out_major {} head=[{}]  in_major {} head=[{}]  linear_nt {} head=[{}]",
            FiniteStats::of(&out_major),
            head(&out_major),
            FiniteStats::of(&in_major),
            head(&in_major),
            FiniteStats::of(&nt),
            head(&nt),
        );
    }
    let mut rng = seed ^ 0xD17E_E001;
    let mut x = vec![0f32; MUSIC3_DIT_IN_CHANNELS * tokens];
    for v in &mut x {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        let u = (rng >> 33) as f32 / (1u32 << 31) as f32;
        *v = (-2.0 * u.max(1e-12).ln()).sqrt() * if rng & 1 == 0 { 1.0 } else { -1.0 };
    }
    let cond = vec![0f32; tokens * MUSIC3_DIT_COND];
    let v0 = dit_forward(file, &prep, &x, &cond, tokens, 0.0)?;
    eprintln!("  dummy-v t=0 tokens={tokens} {}", FiniteStats::of(&v0));
    {
        let mut top: Vec<(f32, usize)> = (0..MUSIC3_DIT_IN_CHANNELS)
            .map(|ch| {
                let row = &v0[ch * tokens..(ch + 1) * tokens];
                let e = row.iter().map(|x| x * x).sum::<f32>() / tokens.max(1) as f32;
                (e.sqrt(), ch)
            })
            .collect();
        top.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        eprint!("  dummy-v ch rms");
        for (rms, ch) in top.iter().take(8) {
            eprint!(" c{ch}={rms:.3}");
        }
        eprintln!();
    }
    Ok(())
}

/// One DiT forward from host-provided channel-major latents `[C,T]` and token-major cond `[T,2048]`.
pub fn gguf_dit_forward_from(
    pack: &Music3GgufPack,
    latents: &[f32],
    cond: &[f32],
    tokens: usize,
    timestep: f32,
) -> Result<Vec<f32>> {
    let prep = DitPrepared::load(&pack.transformer)?;
    dit_forward(&pack.transformer, &prep, latents, cond, tokens, timestep)
}

/// Euler sample from a provided condition `[T, 2048]`.
pub fn gguf_dit_from_cond(
    pack: &Music3GgufPack,
    cond: &[f32],
    tokens: usize,
    steps: usize,
    seed: u64,
    progress: &mut dyn FnMut(&str, usize, usize),
) -> Result<Vec<f32>> {
    gguf_dit_from_cond_x0(pack, cond, None, tokens, steps, seed, progress)
}

pub fn gguf_dit_from_cond_x0(
    pack: &Music3GgufPack,
    cond: &[f32],
    x0: Option<&[f32]>,
    tokens: usize,
    steps: usize,
    seed: u64,
    progress: &mut dyn FnMut(&str, usize, usize),
) -> Result<Vec<f32>> {
    let prep = DitPrepared::load(&pack.transformer)?;
    dit_euler_x0(
        &pack.transformer,
        &prep,
        cond,
        x0,
        tokens,
        steps,
        seed,
        progress,
    )
}
