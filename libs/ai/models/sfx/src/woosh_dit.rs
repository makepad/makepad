//! Woosh DFlow DiT ("mmmssflux" trunk + FlowMap wrapper), CPU f32.
//!
//! One forward = one distillation-student velocity u(x_t, t, r, cfg, text)
//! for the (128 x 501) latent. 12 layers: 6 dual-stream MMM blocks (separate
//! audio/text parameters, JOINT attention over the concatenated 502+77
//! sequence) then 6 single-stream blocks (fused qkv+mlp over the concat
//! sequence, parallel attention+MLP, flux-single-block style).
//!
//! Reference: woosh/model/dit_blocks.py + dit_flows.py (SFXFlow) +
//! flowmap_from_pretrained.py (FlowMapPreprocessing / FlipSignPostprocessing).
//!
//! Faithfully reproduced quirks (validated per block against the dumps):
//! - partial rope: per 128-dim head, dims 0..112 rotated on ADJACENT pairs
//!   (torch view_as_complex convention), dims 112..128 untouched;
//! - MMM blocks rope each stream with its own table — audio rows 0..501 of
//!   the memtok-prepended YaRN table, text an all-identity table (rope no-op);
//! - single-stream blocks concatenate the FULL 1003-row audio table with the
//!   77-row text table and truncate to the 579-token sequence, so text tokens
//!   there rotate with YaRN rows 501..577 (NOT identity) — the reference
//!   truncation behavior, reproduced exactly;
//! - the DFlow output sign flip is folded into the final linear at load.

use crate::error::{DiffusionError, Result};
use crate::sa3::{gelu_tanh, linear, par_rows, silu, Sa3Tensors};
use crate::woosh::{
    woosh_fixed_fourier, woosh_freqs_cis_audio, woosh_learned_fourier, WOOSH_AUDIO_TOKENS,
    WOOSH_DESC_TOKENS, WOOSH_DIM, WOOSH_HEADS, WOOSH_HEAD_DIM, WOOSH_INTER_DIM, WOOSH_JOINT_TOKENS,
    WOOSH_LATENT_DIM, WOOSH_LATENT_FRAMES, WOOSH_LN_EPS, WOOSH_MM_LAYERS, WOOSH_ROPE_DIM,
    WOOSH_ROPE_FREQS, WOOSH_TIMESTEP_FEATURES,
};

struct ModalityAttn {
    qkv_w: Vec<f32>,
    qkv_b: Vec<f32>,
    norm_q: Vec<f32>,
    norm_k: Vec<f32>,
    out_w: Vec<f32>,
    out_b: Vec<f32>,
    mod_w: Vec<f32>,
    mod_b: Vec<f32>,
}

struct ModalityFfn {
    w1_w: Vec<f32>,
    w1_b: Vec<f32>,
    w2_w: Vec<f32>,
    w2_b: Vec<f32>,
    mod_w: Vec<f32>,
    mod_b: Vec<f32>,
}

struct MmBlock {
    x_attn: ModalityAttn,
    d_attn: ModalityAttn,
    x_ffn: ModalityFfn,
    d_ffn: ModalityFfn,
}

struct SsBlock {
    qkv_mlp_w: Vec<f32>,
    qkv_mlp_b: Vec<f32>,
    out_w: Vec<f32>,
    out_b: Vec<f32>,
    norm_q: Vec<f32>,
    norm_k: Vec<f32>,
    mod_w: Vec<f32>,
    mod_b: Vec<f32>,
}

pub struct WooshDit {
    // --- preprocessing (Flow-era module reused by the DFlow student) ---
    memory_token: Vec<f32>,    // (1024)
    description_pad: Vec<f32>, // (77 x 1024)
    project_in_w: Vec<f32>,    // (1024 x 128)
    project_in_b: Vec<f32>,
    cond0_w: Vec<f32>, // (4096 x 1024)
    cond0_b: Vec<f32>,
    cond2_w: Vec<f32>, // (1024 x 4096)
    cond2_b: Vec<f32>,
    // Flow-era timestep path — kept only to validate the m_plus tap.
    old_tf_w: Vec<f32>, // (128)
    old_t0_w: Vec<f32>, // (4096 x 256)
    old_t0_b: Vec<f32>,
    old_t2_w: Vec<f32>, // (1024 x 4096)
    old_t2_b: Vec<f32>,
    // --- DFlow (t, r, cfg) timestep path ---
    tf_t: Vec<f32>,   // (128) fourier freq buffers from the checkpoint
    tf_r: Vec<f32>,
    tf_cfg: Vec<f32>,
    ts0_w: Vec<f32>, // (4096 x 768)
    ts0_b: Vec<f32>,
    ts2_w: Vec<f32>, // (1024 x 4096)
    ts2_b: Vec<f32>,
    // --- trunk ---
    mm_blocks: Vec<MmBlock>,
    ss_blocks: Vec<SsBlock>,
    // --- postprocessing (sign flip folded into the linear) ---
    post_mod_w: Vec<f32>, // (2048 x 1024)
    post_mod_b: Vec<f32>,
    post_lin_w: Vec<f32>, // (128 x 1024), negated
    post_lin_b: Vec<f32>, // (128), negated
    /// Memtok-prepended YaRN table, (1003 x 64 x 2) — row layout identical
    /// to the dumped freqs_cis_audio_real.
    freqs: Vec<f32>,
}

/// Pre-embedded text stream (timestep-independent — compute once per prompt).
pub struct WooshCond {
    /// (77 x 1024) to_cond_embed output.
    pub desc: Vec<f32>,
}

/// Debug taps recorded by [`WooshDit::forward_with_taps`] for oracle
/// validation: ("pre_x"/"pre_desc"/"pre_t"/"pre_mplus"/"block{N}_x"/
/// "block{N}_desc", row-major data).
pub type WooshTaps = Vec<(String, Vec<f32>)>;

impl WooshDit {
    pub fn load(weights: &Sa3Tensors) -> Result<Self> {
        let d = WOOSH_DIM;
        let inter = WOOSH_INTER_DIM;
        let get = |name: &str, len: usize| -> Result<Vec<f32>> {
            let full = format!("dit.{name}");
            let v = weights.f32(&full)?;
            if v.len() != len {
                return Err(DiffusionError::model(format!(
                    "woosh dit tensor {full}: {} values, expected {len}",
                    v.len()
                )));
            }
            Ok(v)
        };
        let attn = |p: &str| -> Result<ModalityAttn> {
            Ok(ModalityAttn {
                qkv_w: get(&format!("{p}.qkv.weight"), 3 * d * d)?,
                qkv_b: get(&format!("{p}.qkv.bias"), 3 * d)?,
                norm_q: get(&format!("{p}.norm_q.weight"), WOOSH_HEAD_DIM)?,
                norm_k: get(&format!("{p}.norm_k.weight"), WOOSH_HEAD_DIM)?,
                out_w: get(&format!("{p}.out_proj.weight"), d * d)?,
                out_b: get(&format!("{p}.out_proj.bias"), d)?,
                mod_w: get(&format!("{p}.mod_proj.weight"), 3 * d * d)?,
                mod_b: get(&format!("{p}.mod_proj.bias"), 3 * d)?,
            })
        };
        let ffn = |p: &str| -> Result<ModalityFfn> {
            Ok(ModalityFfn {
                w1_w: get(&format!("{p}.w1.weight"), inter * d)?,
                w1_b: get(&format!("{p}.w1.bias"), inter)?,
                w2_w: get(&format!("{p}.w2.weight"), d * inter)?,
                w2_b: get(&format!("{p}.w2.bias"), d)?,
                mod_w: get(&format!("{p}.mod_proj.weight"), 3 * d * d)?,
                mod_b: get(&format!("{p}.mod_proj.bias"), 3 * d)?,
            })
        };
        let mut mm_blocks = Vec::with_capacity(WOOSH_MM_LAYERS);
        for l in 0..WOOSH_MM_LAYERS {
            mm_blocks.push(MmBlock {
                x_attn: attn(&format!("layers.{l}.attn.modalities.x"))?,
                d_attn: attn(&format!("layers.{l}.attn.modalities.description"))?,
                x_ffn: ffn(&format!("layers.{l}.ffns.x"))?,
                d_ffn: ffn(&format!("layers.{l}.ffns.description"))?,
            });
        }
        let mut ss_blocks = Vec::with_capacity(WOOSH_MM_LAYERS);
        for l in WOOSH_MM_LAYERS..2 * WOOSH_MM_LAYERS {
            let p = format!("layers.{l}");
            ss_blocks.push(SsBlock {
                qkv_mlp_w: get(&format!("{p}.qkv_mlp.weight"), (3 * d + inter) * d)?,
                qkv_mlp_b: get(&format!("{p}.qkv_mlp.bias"), 3 * d + inter)?,
                out_w: get(&format!("{p}.out_proj.weight"), d * (d + inter))?,
                out_b: get(&format!("{p}.out_proj.bias"), d)?,
                norm_q: get(&format!("{p}.norm_q.weight"), WOOSH_HEAD_DIM)?,
                norm_k: get(&format!("{p}.norm_k.weight"), WOOSH_HEAD_DIM)?,
                mod_w: get(&format!("{p}.mod_proj.weight"), 3 * d * d)?,
                mod_b: get(&format!("{p}.mod_proj.bias"), 3 * d)?,
            });
        }
        // DFlow: postprocessing output is negated -> fold into the linear.
        let mut post_lin_w = get("postprocessing.old_postprocessing.linear.weight", WOOSH_LATENT_DIM * d)?;
        let mut post_lin_b = get("postprocessing.old_postprocessing.linear.bias", WOOSH_LATENT_DIM)?;
        for v in post_lin_w.iter_mut().chain(post_lin_b.iter_mut()) {
            *v = -*v;
        }
        let half_tf = WOOSH_TIMESTEP_FEATURES / 2;
        Ok(Self {
            memory_token: get("preprocessing.old_preprocessing.memory_tokens_rope", d)?,
            description_pad: get(
                "preprocessing.old_preprocessing.description_pad",
                WOOSH_DESC_TOKENS * d,
            )?,
            project_in_w: get("preprocessing.old_preprocessing.project_in.weight", d * WOOSH_LATENT_DIM)?,
            project_in_b: get("preprocessing.old_preprocessing.project_in.bias", d)?,
            cond0_w: get("preprocessing.old_preprocessing.to_cond_embed.0.weight", inter * d)?,
            cond0_b: get("preprocessing.old_preprocessing.to_cond_embed.0.bias", inter)?,
            cond2_w: get("preprocessing.old_preprocessing.to_cond_embed.2.weight", d * inter)?,
            cond2_b: get("preprocessing.old_preprocessing.to_cond_embed.2.bias", d)?,
            old_tf_w: get("preprocessing.old_preprocessing.timestep_features.weight", half_tf)?,
            old_t0_w: get(
                "preprocessing.old_preprocessing.to_timestep_embed.0.weight",
                inter * WOOSH_TIMESTEP_FEATURES,
            )?,
            old_t0_b: get("preprocessing.old_preprocessing.to_timestep_embed.0.bias", inter)?,
            old_t2_w: get("preprocessing.old_preprocessing.to_timestep_embed.2.weight", d * inter)?,
            old_t2_b: get("preprocessing.old_preprocessing.to_timestep_embed.2.bias", d)?,
            tf_t: get("preprocessing.timestep_features_t.freqs", half_tf)?,
            tf_r: get("preprocessing.timestep_features_r.freqs", half_tf)?,
            tf_cfg: get("preprocessing.cfg_features.freqs", half_tf)?,
            ts0_w: get("preprocessing.to_timestep_embed.0.weight", inter * 3 * WOOSH_TIMESTEP_FEATURES)?,
            ts0_b: get("preprocessing.to_timestep_embed.0.bias", inter)?,
            ts2_w: get("preprocessing.to_timestep_embed.2.weight", d * inter)?,
            ts2_b: get("preprocessing.to_timestep_embed.2.bias", d)?,
            mm_blocks,
            ss_blocks,
            post_mod_w: get("postprocessing.old_postprocessing.mod_proj.weight", 2 * d * d)?,
            post_mod_b: get("postprocessing.old_postprocessing.mod_proj.bias", 2 * d)?,
            post_lin_w,
            post_lin_b,
            freqs: woosh_freqs_cis_audio(),
        })
    }

    /// The rope table (1003 x 64 x 2), exposed for oracle validation.
    pub fn freqs_cis(&self) -> &[f32] {
        &self.freqs
    }

    /// Projects the raw TE hidden state (77 x 1024, with `mask`) through the
    /// description_pad replacement + to_cond_embed MLP. Timestep-independent.
    pub fn embed_condition(&self, te_hidden: &[f32], mask: &[f32]) -> Result<WooshCond> {
        let d = WOOSH_DIM;
        let n = WOOSH_DESC_TOKENS;
        if te_hidden.len() != n * d || mask.len() != n {
            return Err(DiffusionError::model(format!(
                "woosh cond: {}/{} inputs, expected {} x {d}",
                te_hidden.len(),
                mask.len(),
                n
            )));
        }
        let mut padded = vec![0f32; n * d];
        for row in 0..n {
            let src = if mask[row] > 0.5 {
                &te_hidden[row * d..(row + 1) * d]
            } else {
                &self.description_pad[row * d..(row + 1) * d]
            };
            padded[row * d..(row + 1) * d].copy_from_slice(src);
        }
        let mut hidden = linear(&padded, &self.cond0_w, Some(&self.cond0_b), n, d, WOOSH_INTER_DIM);
        for v in hidden.iter_mut() {
            *v = silu(*v);
        }
        let desc = linear(&hidden, &self.cond2_w, Some(&self.cond2_b), n, WOOSH_INTER_DIM, d);
        Ok(WooshCond { desc })
    }

    /// The DFlow (t, r, cfg) modulation vector (1024, final SiLU applied).
    pub fn time_embed(&self, t: f32, r: f32, cfg: f32) -> Vec<f32> {
        let mut features = Vec::with_capacity(3 * WOOSH_TIMESTEP_FEATURES);
        features.extend(woosh_fixed_fourier(t, &self.tf_t));
        features.extend(woosh_fixed_fourier(r, &self.tf_r));
        features.extend(woosh_fixed_fourier(cfg, &self.tf_cfg));
        let mut hidden = linear(
            &features,
            &self.ts0_w,
            Some(&self.ts0_b),
            1,
            3 * WOOSH_TIMESTEP_FEATURES,
            WOOSH_INTER_DIM,
        );
        for v in hidden.iter_mut() {
            *v = silu(*v);
        }
        let mut t_vec = linear(&hidden, &self.ts2_w, Some(&self.ts2_b), 1, WOOSH_INTER_DIM, WOOSH_DIM);
        for v in t_vec.iter_mut() {
            *v = silu(*v);
        }
        t_vec
    }

    /// The Flow-era m_plus vector — has no consumers in the DFlow forward;
    /// computed only to validate the dumped tap.
    pub fn m_plus(&self, t: f32) -> Vec<f32> {
        let features = woosh_learned_fourier(t, &self.old_tf_w);
        let mut hidden = linear(
            &features,
            &self.old_t0_w,
            Some(&self.old_t0_b),
            1,
            WOOSH_TIMESTEP_FEATURES,
            WOOSH_INTER_DIM,
        );
        for v in hidden.iter_mut() {
            *v = silu(*v);
        }
        linear(&hidden, &self.old_t2_w, Some(&self.old_t2_b), 1, WOOSH_INTER_DIM, WOOSH_DIM)
    }

    /// One velocity prediction. `latents` is channel-major (128 x 501) like
    /// the reference tensors; returns channel-major (128 x 501).
    pub fn forward(
        &self,
        latents: &[f32],
        t: f32,
        r: f32,
        cfg: f32,
        cond: &WooshCond,
    ) -> Result<Vec<f32>> {
        self.forward_with_taps(latents, t, r, cfg, cond, None)
    }

    /// [`Self::forward`] optionally recording per-stage taps for validation.
    pub fn forward_with_taps(
        &self,
        latents: &[f32],
        t: f32,
        r: f32,
        cfg: f32,
        cond: &WooshCond,
        mut taps: Option<&mut WooshTaps>,
    ) -> Result<Vec<f32>> {
        let d = WOOSH_DIM;
        let frames = WOOSH_LATENT_FRAMES;
        let c = WOOSH_LATENT_DIM;
        let a_len = WOOSH_AUDIO_TOKENS;
        if latents.len() != c * frames {
            return Err(DiffusionError::model(format!(
                "woosh dit latents: {} values, expected {}",
                latents.len(),
                c * frames
            )));
        }

        let t_vec = self.time_embed(t, r, cfg);

        // Audio stream: memory token + project_in(latent frames).
        // (channel-major -> token rows, then a 128->1024 linear per row)
        let mut frame_rows = vec![0f32; frames * c];
        for ch in 0..c {
            for f in 0..frames {
                frame_rows[f * c + ch] = latents[ch * frames + f];
            }
        }
        let projected = linear(&frame_rows, &self.project_in_w, Some(&self.project_in_b), frames, c, d);
        let mut audio = vec![0f32; a_len * d];
        audio[..d].copy_from_slice(&self.memory_token);
        audio[d..].copy_from_slice(&projected);

        let mut desc = cond.desc.clone();

        if let Some(taps) = taps.as_deref_mut() {
            taps.push(("pre_x".into(), audio.clone()));
            taps.push(("pre_desc".into(), desc.clone()));
            taps.push(("pre_t".into(), t_vec.clone()));
            taps.push(("pre_mplus".into(), self.m_plus(t)));
        }

        // Rope row tables: audio tokens use rows 0..502 (memtok row + YaRN
        // 0..500); the single-stream joint sequence uses rows 0..579 of the
        // same table (the reference's concat-truncate: text tokens rotate
        // with YaRN rows 501..577). MMM text rope is identity (skipped).
        let audio_rope = &self.freqs[..a_len * WOOSH_ROPE_FREQS * 2];
        let joint_rope = &self.freqs[..WOOSH_JOINT_TOKENS * WOOSH_ROPE_FREQS * 2];

        for (index, block) in self.mm_blocks.iter().enumerate() {
            self.mm_forward(block, &mut audio, &mut desc, &t_vec, audio_rope);
            if let Some(taps) = taps.as_deref_mut() {
                taps.push((format!("block{index:02}_x"), audio.clone()));
                taps.push((format!("block{index:02}_desc"), desc.clone()));
            }
        }
        for (offset, block) in self.ss_blocks.iter().enumerate() {
            let index = WOOSH_MM_LAYERS + offset;
            self.ss_forward(block, &mut audio, &mut desc, &t_vec, joint_rope);
            if let Some(taps) = taps.as_deref_mut() {
                taps.push((format!("block{index:02}_x"), audio.clone()));
                taps.push((format!("block{index:02}_desc"), desc.clone()));
            }
        }

        // Postprocessing: strip the memory token, AdaLN, linear (sign folded).
        let x = &audio[d..];
        let mods = linear(&t_vec, &self.post_mod_w, Some(&self.post_mod_b), 1, d, 2 * d);
        let (bias, scale) = mods.split_at(d);
        let mut normed = x.to_vec();
        layer_norm_rows_noaffine(&mut normed, d);
        par_rows(&mut normed, d, &|_row, row| {
            for (i, v) in row.iter_mut().enumerate() {
                *v = (1.0 + scale[i]) * *v + bias[i];
            }
        });
        let out_rows = linear(&normed, &self.post_lin_w, Some(&self.post_lin_b), frames, d, c);
        let mut out = vec![0f32; c * frames];
        for f in 0..frames {
            for ch in 0..c {
                out[ch * frames + f] = out_rows[f * c + ch];
            }
        }
        Ok(out)
    }

    /// One dual-stream MMM block: joint attention with per-modality
    /// parameters, then per-modality gated FFNs.
    fn mm_forward(
        &self,
        block: &MmBlock,
        audio: &mut Vec<f32>,
        desc: &mut Vec<f32>,
        t_vec: &[f32],
        audio_rope: &[f32],
    ) {
        let d = WOOSH_DIM;
        let a_len = audio.len() / d;
        let d_len = desc.len() / d;
        let joint = a_len + d_len;

        // Per-modality attention pre-compute: LN -> AdaLN -> qkv -> per-head
        // RMS -> rope (audio only; text rope is identity).
        let (aq, ak, av, a_gate) =
            attn_precompute(&block.x_attn, audio, a_len, t_vec, Some(audio_rope));
        let (dq, dk, dv, d_gate) = attn_precompute(&block.d_attn, desc, d_len, t_vec, None);

        let mut q = aq;
        q.extend_from_slice(&dq);
        let mut k = ak;
        k.extend_from_slice(&dk);
        let mut v = av;
        v.extend_from_slice(&dv);
        let z = attention_joint(&q, &k, &v, joint);

        let a_out = linear(&z[..a_len * d], &block.x_attn.out_w, Some(&block.x_attn.out_b), a_len, d, d);
        gated_residual(audio, &a_out, &a_gate);
        let d_out = linear(&z[a_len * d..], &block.d_attn.out_w, Some(&block.d_attn.out_b), d_len, d, d);
        gated_residual(desc, &d_out, &d_gate);

        ffn_forward(&block.x_ffn, audio, a_len, t_vec);
        ffn_forward(&block.d_ffn, desc, d_len, t_vec);
    }

    /// One single-stream block: concat streams, shared AdaLN + fused
    /// qkv_mlp, parallel attention + GELU MLP, shared out_proj, gated
    /// residual per stream.
    fn ss_forward(
        &self,
        block: &SsBlock,
        audio: &mut Vec<f32>,
        desc: &mut Vec<f32>,
        t_vec: &[f32],
        joint_rope: &[f32],
    ) {
        let d = WOOSH_DIM;
        let inter = WOOSH_INTER_DIM;
        let a_len = audio.len() / d;
        let d_len = desc.len() / d;
        let joint = a_len + d_len;

        let mut x = Vec::with_capacity(joint * d);
        x.extend_from_slice(audio);
        x.extend_from_slice(desc);
        layer_norm_rows_noaffine(&mut x, d);

        let mods = linear(t_vec, &block.mod_w, Some(&block.mod_b), 1, d, 3 * d);
        let (bias, rest) = mods.split_at(d);
        let (scale, gate) = rest.split_at(d);
        par_rows(&mut x, d, &|_row, row| {
            for (i, v) in row.iter_mut().enumerate() {
                *v = (1.0 + scale[i]) * *v + bias[i];
            }
        });

        let fused = linear(&x, &block.qkv_mlp_w, Some(&block.qkv_mlp_b), joint, d, 3 * d + inter);
        let mut q = vec![0f32; joint * d];
        let mut k = vec![0f32; joint * d];
        let mut v = vec![0f32; joint * d];
        // act(mlp) is assembled directly into the out_proj input layout
        // [attn 1024 | mlp 4096] per row.
        let mut out_in = vec![0f32; joint * (d + inter)];
        let width = 3 * d + inter;
        for row in 0..joint {
            let src = &fused[row * width..(row + 1) * width];
            q[row * d..(row + 1) * d].copy_from_slice(&src[..d]);
            k[row * d..(row + 1) * d].copy_from_slice(&src[d..2 * d]);
            v[row * d..(row + 1) * d].copy_from_slice(&src[2 * d..3 * d]);
            let mlp_dst = &mut out_in[row * (d + inter) + d..(row + 1) * (d + inter)];
            for (dst, &s) in mlp_dst.iter_mut().zip(&src[3 * d..]) {
                *dst = gelu_tanh(s);
            }
        }
        rms_per_head(&mut q, &block.norm_q);
        rms_per_head(&mut k, &block.norm_k);
        rope_rows(&mut q, joint_rope);
        rope_rows(&mut k, joint_rope);
        let z = attention_joint(&q, &k, &v, joint);
        for row in 0..joint {
            out_in[row * (d + inter)..row * (d + inter) + d]
                .copy_from_slice(&z[row * d..(row + 1) * d]);
        }
        let out = linear(&out_in, &block.out_w, Some(&block.out_b), joint, d + inter, d);
        gated_residual(audio, &out[..a_len * d], gate);
        gated_residual(desc, &out[a_len * d..], gate);
    }
}

/// LN -> AdaLN modulate -> qkv -> per-head RMS -> optional partial rope.
/// Returns (q, k, v, gate); q/k/v are (len x 1024) head-contiguous rows.
fn attn_precompute(
    attn: &ModalityAttn,
    stream: &[f32],
    len: usize,
    t_vec: &[f32],
    rope: Option<&[f32]>,
) -> (Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>) {
    let d = WOOSH_DIM;
    let mut normed = stream.to_vec();
    layer_norm_rows_noaffine(&mut normed, d);
    let mods = linear(t_vec, &attn.mod_w, Some(&attn.mod_b), 1, d, 3 * d);
    let (bias, rest) = mods.split_at(d);
    let (scale, gate) = rest.split_at(d);
    par_rows(&mut normed, d, &|_row, row| {
        for (i, v) in row.iter_mut().enumerate() {
            *v = (1.0 + scale[i]) * *v + bias[i];
        }
    });
    let qkv = linear(&normed, &attn.qkv_w, Some(&attn.qkv_b), len, d, 3 * d);
    let mut q = vec![0f32; len * d];
    let mut k = vec![0f32; len * d];
    let mut v = vec![0f32; len * d];
    for row in 0..len {
        let src = &qkv[row * 3 * d..(row + 1) * 3 * d];
        q[row * d..(row + 1) * d].copy_from_slice(&src[..d]);
        k[row * d..(row + 1) * d].copy_from_slice(&src[d..2 * d]);
        v[row * d..(row + 1) * d].copy_from_slice(&src[2 * d..]);
    }
    rms_per_head(&mut q, &attn.norm_q);
    rms_per_head(&mut k, &attn.norm_k);
    if let Some(rope) = rope {
        rope_rows(&mut q, rope);
        rope_rows(&mut k, rope);
    }
    (q, k, v, gate.to_vec())
}

/// stream += gate * value (per row, gate is a 1024 vector).
fn gated_residual(stream: &mut [f32], value: &[f32], gate: &[f32]) {
    let d = WOOSH_DIM;
    par_rows(stream, d, &|row, out| {
        let src = &value[row * d..(row + 1) * d];
        for i in 0..d {
            out[i] += gate[i] * src[i];
        }
    });
}

/// MLP: LN -> AdaLN -> w1 -> gelu-tanh -> w2 -> gated residual.
fn ffn_forward(ffn: &ModalityFfn, stream: &mut Vec<f32>, len: usize, t_vec: &[f32]) {
    let d = WOOSH_DIM;
    let inter = WOOSH_INTER_DIM;
    let mut normed = stream.clone();
    layer_norm_rows_noaffine(&mut normed, d);
    let mods = linear(t_vec, &ffn.mod_w, Some(&ffn.mod_b), 1, d, 3 * d);
    let (bias, rest) = mods.split_at(d);
    let (scale, gate) = rest.split_at(d);
    par_rows(&mut normed, d, &|_row, row| {
        for (i, v) in row.iter_mut().enumerate() {
            *v = (1.0 + scale[i]) * *v + bias[i];
        }
    });
    let mut hidden = linear(&normed, &ffn.w1_w, Some(&ffn.w1_b), len, d, inter);
    par_rows(&mut hidden, inter, &|_row, row| {
        for v in row.iter_mut() {
            *v = gelu_tanh(*v);
        }
    });
    let out = linear(&hidden, &ffn.w2_w, Some(&ffn.w2_b), len, inter, d);
    gated_residual(stream, &out, gate);
}

/// LayerNorm(no affine, eps 1e-6) over 1024-wide rows.
fn layer_norm_rows_noaffine(x: &mut [f32], d: usize) {
    par_rows(x, d, &|_row, row| {
        let mut mean = 0f32;
        for v in row.iter() {
            mean += *v;
        }
        mean /= d as f32;
        let mut var = 0f32;
        for v in row.iter() {
            let dv = *v - mean;
            var += dv * dv;
        }
        var /= d as f32;
        let inv = 1.0 / (var + WOOSH_LN_EPS).sqrt();
        for v in row.iter_mut() {
            *v = (*v - mean) * inv;
        }
    });
}

/// Per-head RMSNorm (shared 128-dim weight, eps 1e-6) over head-contiguous
/// 1024-wide rows.
fn rms_per_head(x: &mut [f32], gamma: &[f32]) {
    let d = WOOSH_DIM;
    let hd = WOOSH_HEAD_DIM;
    par_rows(x, d, &|_row, row| {
        for head in 0..WOOSH_HEADS {
            let slice = &mut row[head * hd..(head + 1) * hd];
            let mut sum = 0f32;
            for v in slice.iter() {
                sum += *v * *v;
            }
            let inv = 1.0 / (sum / hd as f32 + WOOSH_LN_EPS).sqrt();
            for (v, g) in slice.iter_mut().zip(gamma) {
                *v = *v * inv * *g;
            }
        }
    });
}

/// Partial rope: within each 128-dim head rotate ADJACENT pairs of the first
/// 112 dims with the per-row (cos, sin) table (64 pairs stored, first 56
/// used), leaving dims 112..128 untouched.
fn rope_rows(x: &mut [f32], table: &[f32]) {
    let d = WOOSH_DIM;
    let hd = WOOSH_HEAD_DIM;
    let pairs = WOOSH_ROPE_DIM / 2; // 56
    par_rows(x, d, &|row, out| {
        let table_row = &table[row * WOOSH_ROPE_FREQS * 2..];
        for head in 0..WOOSH_HEADS {
            let base = head * hd;
            for j in 0..pairs {
                let cos = table_row[j * 2];
                let sin = table_row[j * 2 + 1];
                let e = out[base + 2 * j];
                let o = out[base + 2 * j + 1];
                out[base + 2 * j] = e * cos - o * sin;
                out[base + 2 * j + 1] = e * sin + o * cos;
            }
        }
    });
}

/// Joint multi-head attention over head-contiguous (tokens x 1024) rows,
/// scale 1/sqrt(128), no mask.
fn attention_joint(q: &[f32], k: &[f32], v: &[f32], tokens: usize) -> Vec<f32> {
    let d = WOOSH_DIM;
    let hd = WOOSH_HEAD_DIM;
    let scale = 1.0 / (hd as f32).sqrt();
    if let Some(out) =
        crate::metal_accel::flash_attn_packed(q, k, v, tokens, tokens, WOOSH_HEADS, hd, scale)
    {
        return out;
    }
    let mut out = vec![0f32; tokens * d];
    par_rows(&mut out, d, &|row, out_row| {
        let mut scores = vec![0f32; tokens];
        for head in 0..WOOSH_HEADS {
            let q_vec = &q[row * d + head * hd..][..hd];
            let mut max_s = f32::NEG_INFINITY;
            for (kr, score) in scores.iter_mut().enumerate() {
                let k_vec = &k[kr * d + head * hd..][..hd];
                let mut dot = 0f32;
                for i in 0..hd {
                    dot += q_vec[i] * k_vec[i];
                }
                *score = dot * scale;
                max_s = max_s.max(*score);
            }
            let mut denom = 0f32;
            for score in scores.iter_mut() {
                *score = (*score - max_s).exp();
                denom += *score;
            }
            let inv = 1.0 / denom;
            let out_vec = &mut out_row[head * hd..(head + 1) * hd];
            out_vec.fill(0.0);
            for (kr, &score) in scores.iter().enumerate() {
                let w = score * inv;
                let v_vec = &v[kr * d + head * hd..][..hd];
                for i in 0..hd {
                    out_vec[i] += w * v_vec[i];
                }
            }
        }
    });
    out
}
