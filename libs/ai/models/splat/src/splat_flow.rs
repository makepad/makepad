//! `LatentSeqMMFlowModel` — TripoSplat's rectified-flow denoiser over a fixed
//! 8192-token splat latent plus one camera token.
//!
//! Per forward:
//! * `input_layer` lifts the 16-channel latent to 1024 and the Sobol anchor
//!   embedding is added to it;
//! * two AdaLN-modulated `noise_refiner` blocks run on the latent alone and
//!   two UNMODULATED `context_refiner` blocks on the image condition;
//! * a 2-layer MLP lifts the 5-channel camera token;
//! * the three are concatenated (8192 + 4101 + 1 = 12294 tokens) and 24
//!   AdaLN-modulated blocks run over the whole sequence;
//! * the latent and camera slices get a weightless layer norm, the top-level
//!   `shift_table` affine, and their own output heads.
//!
//! Every block's rope is a `RePo3DRotaryEmbedding`: a LEARNED projection of
//! that block's own input to three deltas per head, turned into `head_dim/2`
//! complex phases with per-axis learned frequencies. It is recomputed per
//! block per forward, which is why the phase tables are built on device
//! ([`crate::backend::gpu_splat_repo3d_tables`]) instead of on the host.
//!
//! The refined image context does not depend on `t` or on the latent, so it
//! is computed once per condition and reused across every sampler step —
//! algebraically identical to the reference, which recomputes it each call.

use crate::splat::{
    pcd_position_embed, pcd_position_embed_v1_freqs, timestep_embedding, SplatWeights,
    FLOW_BLOCKS, FLOW_CAM_CHANNELS, FLOW_COND2_CHANNELS, FLOW_COND_CHANNELS, FLOW_FFN,
    FLOW_FINAL_NORM_EPS, FLOW_HEADS, FLOW_HEAD_DIM, FLOW_IN_CHANNELS, FLOW_MODEL_CHANNELS,
    FLOW_MOD_COLS, FLOW_NORM_EPS, FLOW_OUT_CHANNELS, FLOW_Q_TOKENS, FLOW_REFINER_BLOCKS,
    FLOW_SOBOL_SEED, POS_EMBED_MAX_RES, REPO_FREQ_0, REPO_FREQ_1, REPO_FREQ_2, REPO_HIDDEN,
    REPO_PAIRS, T_FREQ_DIM,
};
use crate::splat_ops::{
    add, attention, concat_rows, gated_residual_mod, gelu_tanh, host_linear, host_silu, layer_norm,
    layer_norm_mod, linear, mul, rms_norm_per_head, rope_pairs_per_head, silu, slice_cols, Device,
    Lin, Ten,
};
use crate::splat_rand::sobol_draw;
use crate::{emit_progress, DiffusionError, ProgressHook, Result};

/// One `RePo3DRotaryEmbedding`.
struct RepoLayer {
    norm_w: Vec<f32>,
    norm_b: Vec<f32>,
    gate_map: Lin,
    content_map: Lin,
    final_map: Lin,
    /// `[freqs_0 | freqs_1 | freqs_2]`, 32 entries.
    freqs: Vec<f32>,
    freqs_dev: Ten,
}

impl RepoLayer {
    fn load(device: Device, weights: &SplatWeights, prefix: &str) -> Result<Self> {
        let c = FLOW_MODEL_CHANNELS;
        let mut freqs = weights.f32_shaped(&format!("{prefix}.freqs_0"), &[REPO_FREQ_0])?;
        freqs.extend(weights.f32_shaped(&format!("{prefix}.freqs_1"), &[REPO_FREQ_1])?);
        freqs.extend(weights.f32_shaped(&format!("{prefix}.freqs_2"), &[REPO_FREQ_2])?);
        Ok(Self {
            norm_w: weights.f32_shaped(&format!("{prefix}.norm.weight"), &[c])?,
            norm_b: weights.f32_shaped(&format!("{prefix}.norm.bias"), &[c])?,
            gate_map: Lin::new(
                device,
                &weights.f32_shaped(&format!("{prefix}.gate_map.weight"), &[REPO_HIDDEN, c])?,
                REPO_HIDDEN,
                c,
                None,
            )?,
            content_map: Lin::new(
                device,
                &weights.f32_shaped(&format!("{prefix}.content_map.weight"), &[REPO_HIDDEN, c])?,
                REPO_HIDDEN,
                c,
                None,
            )?,
            final_map: Lin::new(
                device,
                &weights.f32_shaped(
                    &format!("{prefix}.final_map.weight"),
                    &[3 * FLOW_HEADS, REPO_HIDDEN],
                )?,
                3 * FLOW_HEADS,
                REPO_HIDDEN,
                None,
            )?,
            freqs_dev: Ten::upload(device, &freqs, 1, REPO_PAIRS)?,
            freqs,
        })
    }

    /// `(cos, sin)` phase tables of shape `(tokens, heads * 32)`.
    fn tables(&self, hidden: &Ten) -> Result<(Ten, Ten)> {
        // norm -> silu(gate) * content -> final_map -> (tokens, heads*3)
        let normed = layer_norm(hidden, Some(&self.norm_w), Some(&self.norm_b), 1e-5)?;
        let gate = silu(&linear(&normed, &self.gate_map)?)?;
        let content = linear(&normed, &self.content_map)?;
        drop(normed);
        let delta = linear(&mul(&gate, &content)?, &self.final_map)?;
        drop((gate, content));
        splat_repo_tables(&delta, &self.freqs_dev, &self.freqs)
    }
}

/// Phase tables from a `(tokens, heads*3)` delta projection. `clamp_mul` in
/// the reference is `x*tanh(f) + x.detach()*(f - tanh(f))`, which without
/// autograd is exactly `x * f`, so the angle is `delta[axis] * freq * pi`.
fn splat_repo_tables(delta: &Ten, freqs_dev: &Ten, freqs_host: &[f32]) -> Result<(Ten, Ten)> {
    match delta.device() {
        Device::Cuda => {
            let (cos, sin) = crate::backend::gpu_splat_repo3d_tables(
                delta.as_gpu()?,
                freqs_dev.as_gpu()?,
                FLOW_HEADS,
                REPO_PAIRS,
                REPO_FREQ_0,
                REPO_FREQ_1,
            )
            .map_err(DiffusionError::model)?;
            Ok((Ten::adopt_gpu(cos), Ten::adopt_gpu(sin)))
        }
        Device::Cpu => {
            let values = delta.to_host()?;
            let rows = delta.rows();
            let cols = FLOW_HEADS * REPO_PAIRS;
            let mut cos = vec![0.0f32; rows * cols];
            let mut sin = vec![0.0f32; rows * cols];
            for row in 0..rows {
                for head in 0..FLOW_HEADS {
                    let base = (row * FLOW_HEADS + head) * 3;
                    let out = (row * FLOW_HEADS + head) * REPO_PAIRS;
                    for p in 0..REPO_PAIRS {
                        let axis = if p < REPO_FREQ_0 {
                            0
                        } else if p < REPO_FREQ_0 + REPO_FREQ_1 {
                            1
                        } else {
                            2
                        };
                        let angle = values[base + axis] * freqs_host[p] * std::f32::consts::PI;
                        cos[out + p] = angle.cos();
                        sin[out + p] = angle.sin();
                    }
                }
            }
            Ok((
                Ten::upload(Device::Cpu, &cos, rows, cols)?,
                Ten::upload(Device::Cpu, &sin, rows, cols)?,
            ))
        }
    }
}

/// One `UnifiedTransformerBlock`.
struct FlowBlock {
    /// `None` for the unmodulated `context_refiner` blocks, which instead
    /// carry affine LayerNorm parameters.
    shift_table: Option<Vec<f32>>,
    norm1: Option<(Vec<f32>, Vec<f32>)>,
    norm2: Option<(Vec<f32>, Vec<f32>)>,
    qkv: Lin,
    q_norm: Vec<f32>,
    k_norm: Vec<f32>,
    out: Lin,
    mlp0: Lin,
    mlp2: Lin,
}

impl FlowBlock {
    fn load(device: Device, weights: &SplatWeights, prefix: &str, modulated: bool) -> Result<Self> {
        let c = FLOW_MODEL_CHANNELS;
        let (shift_table, norm1, norm2) = if modulated {
            (
                Some(weights.f32_shaped(&format!("{prefix}.shift_table"), &[1, FLOW_MOD_COLS])?),
                None,
                None,
            )
        } else {
            (
                None,
                Some((
                    weights.f32_shaped(&format!("{prefix}.norm1.weight"), &[c])?,
                    weights.f32_shaped(&format!("{prefix}.norm1.bias"), &[c])?,
                )),
                Some((
                    weights.f32_shaped(&format!("{prefix}.norm2.weight"), &[c])?,
                    weights.f32_shaped(&format!("{prefix}.norm2.bias"), &[c])?,
                )),
            )
        };
        Ok(Self {
            shift_table,
            norm1,
            norm2,
            qkv: Lin::new(
                device,
                &weights.f32_shaped(&format!("{prefix}.attn.qkv.weight"), &[3 * c, c])?,
                3 * c,
                c,
                Some(&weights.f32_shaped(&format!("{prefix}.attn.qkv.bias"), &[3 * c])?),
            )?,
            q_norm: weights
                .f32_shaped(&format!("{prefix}.attn.q_norm.gamma"), &[FLOW_HEADS, FLOW_HEAD_DIM])?,
            k_norm: weights
                .f32_shaped(&format!("{prefix}.attn.k_norm.gamma"), &[FLOW_HEADS, FLOW_HEAD_DIM])?,
            out: Lin::new(
                device,
                &weights.f32_shaped(&format!("{prefix}.attn.out.weight"), &[c, c])?,
                c,
                c,
                Some(&weights.f32_shaped(&format!("{prefix}.attn.out.bias"), &[c])?),
            )?,
            mlp0: Lin::new(
                device,
                &weights.f32_shaped(&format!("{prefix}.mlp.mlp.0.weight"), &[FLOW_FFN, c])?,
                FLOW_FFN,
                c,
                Some(&weights.f32_shaped(&format!("{prefix}.mlp.mlp.0.bias"), &[FLOW_FFN])?),
            )?,
            mlp2: Lin::new(
                device,
                &weights.f32_shaped(&format!("{prefix}.mlp.mlp.2.weight"), &[c, FLOW_FFN])?,
                c,
                FLOW_FFN,
                Some(&weights.f32_shaped(&format!("{prefix}.mlp.mlp.2.bias"), &[c])?),
            )?,
        })
    }

    /// `mods` is the per-block modulation row `t_mod + shift_table`, already
    /// uploaded; `None` for the unmodulated context refiner.
    fn forward(&self, x: Ten, mods: Option<&Ten>, repo: &RepoLayer) -> Result<Ten> {
        let c = FLOW_MODEL_CHANNELS;
        let scale = 1.0 / (FLOW_HEAD_DIM as f32).sqrt();
        let (cos, sin) = repo.tables(&x)?;

        let normed = match (mods, &self.norm1) {
            (Some(mods), _) => layer_norm_mod(&x, mods, c, 0, FLOW_NORM_EPS)?,
            (None, Some((w, b))) => layer_norm(&x, Some(w), Some(b), FLOW_NORM_EPS)?,
            (None, None) => return Err(DiffusionError::model("splat flow block has no norm1")),
        };
        let qkv = linear(&normed, &self.qkv)?;
        drop(normed);
        let q = slice_cols(&qkv, 0, c)?;
        let k = slice_cols(&qkv, c, c)?;
        let v = slice_cols(&qkv, 2 * c, c)?;
        drop(qkv);
        // Reference order: rope FIRST, then the per-head RMS norm. The two do
        // not commute (rope mixes the pairs that gamma scales element-wise).
        let q = rope_pairs_per_head(&q, FLOW_HEADS, &cos, &sin)?;
        let k = rope_pairs_per_head(&k, FLOW_HEADS, &cos, &sin)?;
        drop((cos, sin));
        let q = rms_norm_per_head(&q, FLOW_HEADS, FLOW_HEAD_DIM, &self.q_norm)?;
        let k = rms_norm_per_head(&k, FLOW_HEADS, FLOW_HEAD_DIM, &self.k_norm)?;
        let attn = attention(&q, &k, &v, FLOW_HEADS, scale)?;
        drop((q, k, v));
        let attn = linear(&attn, &self.out)?;
        let mut x = match mods {
            Some(mods) => gated_residual_mod(&x, &attn, mods, 2 * c)?,
            None => add(&x, &attn)?,
        };
        drop(attn);

        let normed = match (mods, &self.norm2) {
            (Some(mods), _) => layer_norm_mod(&x, mods, 4 * c, 3 * c, FLOW_NORM_EPS)?,
            (None, Some((w, b))) => layer_norm(&x, Some(w), Some(b), FLOW_NORM_EPS)?,
            (None, None) => return Err(DiffusionError::model("splat flow block has no norm2")),
        };
        let ff = gelu_tanh(&linear(&normed, &self.mlp0)?)?;
        drop(normed);
        let ff = linear(&ff, &self.mlp2)?;
        x = match mods {
            Some(mods) => gated_residual_mod(&x, &ff, mods, 5 * c)?,
            None => add(&x, &ff)?,
        };
        Ok(x)
    }
}

/// One denoiser output.
pub struct FlowVelocity {
    /// `(8192, 16)`.
    pub latent: Vec<f32>,
    /// `(1, 5)`.
    pub camera: Vec<f32>,
}

/// The refined image condition, reusable across every sampler step.
pub struct FlowContext {
    hidden: Ten,
}

pub struct SplatFlow {
    device: Device,
    input_layer: Lin,
    cond_embedder: Lin,
    cond_embedder2: Lin,
    t_mlp0_w: Vec<f32>,
    t_mlp0_b: Vec<f32>,
    t_mlp2_w: Vec<f32>,
    t_mlp2_b: Vec<f32>,
    admod_w: Vec<f32>,
    admod_b: Vec<f32>,
    /// `(8192, 1024)` Sobol anchor embedding, added to the lifted latent.
    pos_embed: Ten,
    noise_refiner: Vec<FlowBlock>,
    noise_repo: Vec<RepoLayer>,
    context_refiner: Vec<FlowBlock>,
    context_repo: Vec<RepoLayer>,
    blocks: Vec<FlowBlock>,
    repo: Vec<RepoLayer>,
    cam_mlp0: Lin,
    cam_mlp2: Lin,
    /// `(2, 1024)` = `[shift | scale]`.
    shift_table: Vec<f32>,
    out_layer: Lin,
    cam_out_layer: Lin,
}

impl SplatFlow {
    pub fn prepare(
        device: Device,
        weights: &SplatWeights,
        mut progress: Option<ProgressHook>,
    ) -> Result<Self> {
        Self::prepare_sized(device, weights, FLOW_Q_TOKENS, &mut progress)
    }

    /// [`Self::prepare`] with an explicit latent token count so a tiny
    /// synthetic config can be built in tests.
    pub fn prepare_sized(
        device: Device,
        weights: &SplatWeights,
        q_tokens: usize,
        progress: &mut Option<ProgressHook>,
    ) -> Result<Self> {
        let c = FLOW_MODEL_CHANNELS;
        let total = FLOW_REFINER_BLOCKS * 2 + FLOW_BLOCKS;
        let mut done = 0usize;
        let tick = |progress: &mut Option<ProgressHook>, done: &mut usize| -> Result<()> {
            *done += 1;
            if progress.is_some() {
                emit_progress(
                    progress,
                    &format!("load flow block {done}/{total}"),
                    *done as f64 / total as f64,
                )?;
            }
            Ok(())
        };

        let mut noise_refiner = Vec::with_capacity(FLOW_REFINER_BLOCKS);
        let mut noise_repo = Vec::with_capacity(FLOW_REFINER_BLOCKS);
        for i in 0..FLOW_REFINER_BLOCKS {
            noise_refiner.push(FlowBlock::load(
                device,
                weights,
                &format!("noise_refiner.{i}"),
                true,
            )?);
            noise_repo.push(RepoLayer::load(
                device,
                weights,
                &format!("noise_repo_layers.{i}"),
            )?);
            tick(progress, &mut done)?;
        }
        let mut context_refiner = Vec::with_capacity(FLOW_REFINER_BLOCKS);
        let mut context_repo = Vec::with_capacity(FLOW_REFINER_BLOCKS);
        for i in 0..FLOW_REFINER_BLOCKS {
            context_refiner.push(FlowBlock::load(
                device,
                weights,
                &format!("context_refiner.{i}"),
                false,
            )?);
            context_repo.push(RepoLayer::load(
                device,
                weights,
                &format!("context_repo_layers.{i}"),
            )?);
            tick(progress, &mut done)?;
        }
        let mut blocks = Vec::with_capacity(FLOW_BLOCKS);
        let mut repo = Vec::with_capacity(FLOW_BLOCKS);
        for i in 0..FLOW_BLOCKS {
            blocks.push(FlowBlock::load(device, weights, &format!("blocks.{i}"), true)?);
            repo.push(RepoLayer::load(device, weights, &format!("repo_layers.{i}"))?);
            tick(progress, &mut done)?;
        }

        // The Sobol anchors and their embedding are model constants.
        let anchors = sobol_draw(3, q_tokens, FLOW_SOBOL_SEED);
        let freqs = pcd_position_embed_v1_freqs(c, 3, POS_EMBED_MAX_RES);
        let pos = pcd_position_embed(&anchors, 3, &freqs, 2.0 * std::f32::consts::PI, c);

        Ok(Self {
            device,
            input_layer: Lin::new(
                device,
                &weights.f32_shaped("input_layer.weight", &[c, FLOW_IN_CHANNELS])?,
                c,
                FLOW_IN_CHANNELS,
                Some(&weights.f32_shaped("input_layer.bias", &[c])?),
            )?,
            cond_embedder: Lin::new(
                device,
                &weights.f32_shaped("cond_embedder.weight", &[c, FLOW_COND_CHANNELS])?,
                c,
                FLOW_COND_CHANNELS,
                Some(&weights.f32_shaped("cond_embedder.bias", &[c])?),
            )?,
            cond_embedder2: Lin::new(
                device,
                &weights.f32_shaped("cond_embedder2.weight", &[c, FLOW_COND2_CHANNELS])?,
                c,
                FLOW_COND2_CHANNELS,
                Some(&weights.f32_shaped("cond_embedder2.bias", &[c])?),
            )?,
            t_mlp0_w: weights.f32_shaped("t_embedder.mlp.0.weight", &[c, T_FREQ_DIM])?,
            t_mlp0_b: weights.f32_shaped("t_embedder.mlp.0.bias", &[c])?,
            t_mlp2_w: weights.f32_shaped("t_embedder.mlp.2.weight", &[c, c])?,
            t_mlp2_b: weights.f32_shaped("t_embedder.mlp.2.bias", &[c])?,
            admod_w: weights.f32_shaped("adaLN_modulation.1.weight", &[FLOW_MOD_COLS, c])?,
            admod_b: weights.f32_shaped("adaLN_modulation.1.bias", &[FLOW_MOD_COLS])?,
            pos_embed: Ten::upload(device, &pos, q_tokens, c)?,
            noise_refiner,
            noise_repo,
            context_refiner,
            context_repo,
            blocks,
            repo,
            cam_mlp0: Lin::new(
                device,
                &weights.f32_shaped("cam_refiner.mlp.0.weight", &[c, FLOW_CAM_CHANNELS])?,
                c,
                FLOW_CAM_CHANNELS,
                Some(&weights.f32_shaped("cam_refiner.mlp.0.bias", &[c])?),
            )?,
            cam_mlp2: Lin::new(
                device,
                &weights.f32_shaped("cam_refiner.mlp.2.weight", &[c, c])?,
                c,
                c,
                Some(&weights.f32_shaped("cam_refiner.mlp.2.bias", &[c])?),
            )?,
            shift_table: weights.f32_shaped("shift_table", &[1, 2, c])?,
            out_layer: Lin::new(
                device,
                &weights.f32_shaped("out_layer.weight", &[FLOW_OUT_CHANNELS, c])?,
                FLOW_OUT_CHANNELS,
                c,
                Some(&weights.f32_shaped("out_layer.bias", &[FLOW_OUT_CHANNELS])?),
            )?,
            cam_out_layer: Lin::new(
                device,
                &weights.f32_shaped("cam_out_layer.weight", &[FLOW_CAM_CHANNELS, c])?,
                FLOW_CAM_CHANNELS,
                c,
                Some(&weights.f32_shaped("cam_out_layer.bias", &[FLOW_CAM_CHANNELS])?),
            )?,
        })
    }

    pub fn device(&self) -> Device {
        self.device
    }

    /// `t_embedder` + the shared AdaLN projection, both f32 on the host like
    /// the reference (neither module is part of `blocks`).
    fn timestep_modulation(&self, t1000: f32) -> (Vec<f32>, Vec<f32>) {
        let c = FLOW_MODEL_CHANNELS;
        let sinusoid = timestep_embedding(t1000, T_FREQ_DIM, 10000.0);
        let mut hidden = host_linear(
            &sinusoid,
            T_FREQ_DIM,
            &self.t_mlp0_w,
            c,
            Some(&self.t_mlp0_b),
        );
        host_silu(&mut hidden);
        let t_emb = host_linear(&hidden, c, &self.t_mlp2_w, c, Some(&self.t_mlp2_b));
        let mut silu_t = t_emb.clone();
        host_silu(&mut silu_t);
        let admod = host_linear(
            &silu_t,
            c,
            &self.admod_w,
            FLOW_MOD_COLS,
            Some(&self.admod_b),
        );
        (t_emb, admod)
    }

    /// Project and refine one image condition. `feature1` is
    /// `(tokens, 1280)`, `feature2` is `(tokens, 128)`; pass all zeros for the
    /// negative condition (`neg_cond = zeros_like(cond)`).
    pub fn encode_context(&self, feature1: &[f32], feature2: &[f32]) -> Result<FlowContext> {
        let tokens = feature1.len() / FLOW_COND_CHANNELS;
        if feature1.len() != tokens * FLOW_COND_CHANNELS
            || feature2.len() != tokens * FLOW_COND2_CHANNELS
        {
            return Err(DiffusionError::workflow("splat flow condition shape mismatch"));
        }
        let f1 = Ten::upload(self.device, feature1, tokens, FLOW_COND_CHANNELS)?;
        let f2 = Ten::upload(self.device, feature2, tokens, FLOW_COND2_CHANNELS)?;
        let mut hidden = add(
            &linear(&f1, &self.cond_embedder)?,
            &linear(&f2, &self.cond_embedder2)?,
        )?;
        drop((f1, f2));
        for (block, repo) in self.context_refiner.iter().zip(&self.context_repo) {
            hidden = block.forward(hidden, None, repo)?;
        }
        Ok(FlowContext { hidden })
    }

    /// One denoiser call: `model(x_t, 1000*t, cond)`.
    pub fn forward(
        &self,
        latent: &[f32],
        camera: &[f32],
        t1000: f32,
        context: &FlowContext,
    ) -> Result<FlowVelocity> {
        let c = FLOW_MODEL_CHANNELS;
        let tokens = latent.len() / FLOW_IN_CHANNELS;
        if latent.len() != tokens * FLOW_IN_CHANNELS || camera.len() != FLOW_CAM_CHANNELS {
            return Err(DiffusionError::workflow("splat flow input shape mismatch"));
        }
        if tokens != self.pos_embed.rows() {
            return Err(DiffusionError::workflow("splat flow token count mismatch"));
        }

        let (t_emb, admod) = self.timestep_modulation(t1000);

        let z = Ten::upload(self.device, latent, tokens, FLOW_IN_CHANNELS)?;
        let mut h_x = add(&linear(&z, &self.input_layer)?, &self.pos_embed)?;
        drop(z);
        for (block, repo) in self.noise_refiner.iter().zip(&self.noise_repo) {
            let mods = self.block_mods(block, &admod)?;
            h_x = block.forward(h_x, Some(&mods), repo)?;
        }

        let cam = Ten::upload(self.device, camera, 1, FLOW_CAM_CHANNELS)?;
        let h_cam = linear(&gelu_tanh(&linear(&cam, &self.cam_mlp0)?)?, &self.cam_mlp2)?;
        drop(cam);

        let mut h = concat_rows(&h_x, &context.hidden)?;
        let latent_rows = h_x.rows();
        drop(h_x);
        h = concat_rows(&h, &h_cam)?;
        drop(h_cam);

        for (block, repo) in self.blocks.iter().zip(&self.repo) {
            let mods = self.block_mods(block, &admod)?;
            h = block.forward(h, Some(&mods), repo)?;
        }

        // Weightless layer norm on the latent and camera slices, then the
        // top-level shift_table affine (both branches share shift/scale).
        let total_rows = h.rows();
        let host = h.to_host()?;
        drop(h);
        let mut latent_rows_host = host[..latent_rows * c].to_vec();
        let mut cam_row = host[(total_rows - 1) * c..].to_vec();
        drop(host);

        let shift: Vec<f32> = self.shift_table[..c]
            .iter()
            .zip(&t_emb)
            .map(|(s, t)| s + t)
            .collect();
        let scale: Vec<f32> = self.shift_table[c..2 * c]
            .iter()
            .zip(&t_emb)
            .map(|(s, t)| s + t)
            .collect();
        host_final_affine(&mut latent_rows_host, c, &shift, &scale);
        host_final_affine(&mut cam_row, c, &shift, &scale);

        let latent_ten = Ten::upload(self.device, &latent_rows_host, latent_rows, c)?;
        let velocity = linear(&latent_ten, &self.out_layer)?.to_host()?;
        drop(latent_ten);
        let cam_ten = Ten::upload(self.device, &cam_row, 1, c)?;
        let cam_velocity = linear(&cam_ten, &self.cam_out_layer)?.to_host()?;
        Ok(FlowVelocity {
            latent: velocity,
            camera: cam_velocity,
        })
    }

    fn block_mods(&self, block: &FlowBlock, admod: &[f32]) -> Result<Ten> {
        let table = block
            .shift_table
            .as_ref()
            .ok_or_else(|| DiffusionError::model("splat flow block is unmodulated"))?;
        let row: Vec<f32> = table.iter().zip(admod).map(|(s, a)| s + a).collect();
        Ten::upload(self.device, &row, 1, FLOW_MOD_COLS)
    }
}

/// Weightless layer norm followed by `x * (1 + scale) + shift`, applied to
/// every row of a flat `(rows, c)` host buffer.
fn host_final_affine(values: &mut [f32], c: usize, shift: &[f32], scale: &[f32]) {
    let rows = values.len() / c;
    for row in 0..rows {
        let slice = &mut values[row * c..(row + 1) * c];
        let mean = slice.iter().sum::<f32>() / c as f32;
        let var = slice.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / c as f32;
        let inv = 1.0 / (var + FLOW_FINAL_NORM_EPS).sqrt();
        for (j, value) in slice.iter_mut().enumerate() {
            *value = ((*value - mean) * inv) * (1.0 + scale[j]) + shift[j];
        }
    }
}

/// Every tensor name the flow model reads, with its shape.
pub fn flow_expected_tensors() -> Vec<(String, Vec<usize>)> {
    let c = FLOW_MODEL_CHANNELS;
    let mut out = vec![
        ("input_layer.weight".to_string(), vec![c, FLOW_IN_CHANNELS]),
        ("input_layer.bias".to_string(), vec![c]),
        ("cond_embedder.weight".to_string(), vec![c, FLOW_COND_CHANNELS]),
        ("cond_embedder.bias".to_string(), vec![c]),
        ("cond_embedder2.weight".to_string(), vec![c, FLOW_COND2_CHANNELS]),
        ("cond_embedder2.bias".to_string(), vec![c]),
        ("t_embedder.mlp.0.weight".to_string(), vec![c, T_FREQ_DIM]),
        ("t_embedder.mlp.0.bias".to_string(), vec![c]),
        ("t_embedder.mlp.2.weight".to_string(), vec![c, c]),
        ("t_embedder.mlp.2.bias".to_string(), vec![c]),
        ("adaLN_modulation.1.weight".to_string(), vec![FLOW_MOD_COLS, c]),
        ("adaLN_modulation.1.bias".to_string(), vec![FLOW_MOD_COLS]),
        ("cam_refiner.mlp.0.weight".to_string(), vec![c, FLOW_CAM_CHANNELS]),
        ("cam_refiner.mlp.0.bias".to_string(), vec![c]),
        ("cam_refiner.mlp.2.weight".to_string(), vec![c, c]),
        ("cam_refiner.mlp.2.bias".to_string(), vec![c]),
        ("shift_table".to_string(), vec![1, 2, c]),
        ("out_layer.weight".to_string(), vec![FLOW_OUT_CHANNELS, c]),
        ("out_layer.bias".to_string(), vec![FLOW_OUT_CHANNELS]),
        ("cam_out_layer.weight".to_string(), vec![FLOW_CAM_CHANNELS, c]),
        ("cam_out_layer.bias".to_string(), vec![FLOW_CAM_CHANNELS]),
    ];
    let mut block = |prefix: String, modulated: bool| {
        out.extend([
            (format!("{prefix}.attn.qkv.weight"), vec![3 * c, c]),
            (format!("{prefix}.attn.qkv.bias"), vec![3 * c]),
            (format!("{prefix}.attn.q_norm.gamma"), vec![FLOW_HEADS, FLOW_HEAD_DIM]),
            (format!("{prefix}.attn.k_norm.gamma"), vec![FLOW_HEADS, FLOW_HEAD_DIM]),
            (format!("{prefix}.attn.out.weight"), vec![c, c]),
            (format!("{prefix}.attn.out.bias"), vec![c]),
            (format!("{prefix}.mlp.mlp.0.weight"), vec![FLOW_FFN, c]),
            (format!("{prefix}.mlp.mlp.0.bias"), vec![FLOW_FFN]),
            (format!("{prefix}.mlp.mlp.2.weight"), vec![c, FLOW_FFN]),
            (format!("{prefix}.mlp.mlp.2.bias"), vec![c]),
        ]);
        if modulated {
            out.push((format!("{prefix}.shift_table"), vec![1, FLOW_MOD_COLS]));
        } else {
            out.extend([
                (format!("{prefix}.norm1.weight"), vec![c]),
                (format!("{prefix}.norm1.bias"), vec![c]),
                (format!("{prefix}.norm2.weight"), vec![c]),
                (format!("{prefix}.norm2.bias"), vec![c]),
            ]);
        }
    };
    for i in 0..FLOW_REFINER_BLOCKS {
        block(format!("noise_refiner.{i}"), true);
    }
    for i in 0..FLOW_REFINER_BLOCKS {
        block(format!("context_refiner.{i}"), false);
    }
    for i in 0..FLOW_BLOCKS {
        block(format!("blocks.{i}"), true);
    }
    let mut repo = |prefix: String| {
        out.extend([
            (format!("{prefix}.norm.weight"), vec![c]),
            (format!("{prefix}.norm.bias"), vec![c]),
            (format!("{prefix}.gate_map.weight"), vec![REPO_HIDDEN, c]),
            (format!("{prefix}.content_map.weight"), vec![REPO_HIDDEN, c]),
            (
                format!("{prefix}.final_map.weight"),
                vec![3 * FLOW_HEADS, REPO_HIDDEN],
            ),
            (format!("{prefix}.freqs_0"), vec![REPO_FREQ_0]),
            (format!("{prefix}.freqs_1"), vec![REPO_FREQ_1]),
            (format!("{prefix}.freqs_2"), vec![REPO_FREQ_2]),
        ]);
    };
    for i in 0..FLOW_REFINER_BLOCKS {
        repo(format!("noise_repo_layers.{i}"));
    }
    for i in 0..FLOW_REFINER_BLOCKS {
        repo(format!("context_repo_layers.{i}"));
    }
    for i in 0..FLOW_BLOCKS {
        repo(format!("repo_layers.{i}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_dict_contract_matches_the_released_checkpoint_shape() {
        let expected = flow_expected_tensors();
        // 21 top-level + 28 modulated blocks + 8 per repo layer.
        let names: Vec<&str> = expected.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"blocks.23.attn.qkv.weight"));
        assert!(names.contains(&"repo_layers.23.freqs_2"));
        assert!(names.contains(&"context_refiner.1.norm2.bias"));
        // Modulated blocks carry a shift_table and no norm affine; the
        // context refiner is the other way round.
        assert!(names.contains(&"blocks.0.shift_table"));
        assert!(!names.contains(&"blocks.0.norm1.weight"));
        assert!(!names.contains(&"context_refiner.0.shift_table"));
        // The released file has 559 tensors; every one of them is read.
        assert_eq!(expected.len(), 559);
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "duplicate tensor name");
    }

    #[test]
    fn final_affine_is_layer_norm_then_shift_scale() {
        let mut values = vec![1.0f32, 2.0, 3.0];
        host_final_affine(&mut values, 3, &[1.0, 1.0, 1.0], &[0.0, 0.0, 0.0]);
        // normalized middle element is 0 -> 0 * 1 + 1 = 1
        assert!((values[1] - 1.0).abs() < 1e-5);
        assert!(values[0] < 1.0 && values[2] > 1.0);
    }
}
