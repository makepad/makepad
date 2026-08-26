//! `OctreeGaussianDecoder`: splat latent -> gaussians.
//!
//! Two transformers share the file, both cross-attending to the 16-channel
//! latent the flow model produced:
//!
//! * `OctreeProbabilityFixedlenDecoder` — 4 AdaLN-modulated CROSS-ONLY blocks
//!   over a point list. Called once per octree level (8 levels): each parent
//!   node predicts an 8-way distribution over its children, and the parent's
//!   sample budget is split across them by systematic resampling. After the
//!   last level every surviving node holds its own share of the requested
//!   anchor count, and the anchors are placed uniformly inside their voxel.
//!   This is where the gaussian count becomes variable: the caller asks for N
//!   gaussians, the sampler draws `N / 32` anchors and the octree decides
//!   WHERE they land, not how many there are.
//! * `ElasticGaussianFixedlenDecoder` — 16 blocks of self-attention +
//!   cross-attention + FFN over those anchors, emitting 480 channels per
//!   anchor = 32 gaussians x (offset 3, SH DC 3, scale 3, rotation 4,
//!   opacity 1, offset scale 1).

use crate::splat::{
    level_embedding, pcd_position_embed, pcd_position_embed_v2_freqs, SplatWeights, GS_BLOCKS,
    GS_COND_CHANNELS, GS_FFN, GS_HEADS, GS_HEAD_DIM, GS_IN_CHANNELS, GS_LAYOUT, GS_MODEL_CHANNELS,
    GS_OUT_CHANNELS, GS_PERTURB_SIZE, GS_PER_POINT, OCT_BLOCKS, OCT_COND_CHANNELS, OCT_FFN,
    OCT_HEADS, OCT_HEAD_DIM, OCT_LEVEL, OCT_MODEL_CHANNELS, OCT_OUT, POS_EMBED_V2_MAX_RES,
    SPLAT_DEC_NAMESPACE, T_FREQ_DIM,
};
use crate::splat_ops::{
    add, attention, gated_residual_mod, gelu_tanh, host_linear, host_silu, layer_norm,
    layer_norm_mod, linear, rms_norm_per_head, slice_cols, Device, Lin, Ten,
};
use crate::splat_rand::{sample_probs_row, SplatRng};
use crate::{DiffusionError, ProgressHook, Result};

/// `MultiHeadAttention(type="cross", qk_rms_norm=True)`.
struct CrossAttn {
    /// Checkpoint prefix, reused as the device weight-cache key for the
    /// per-head RMS gammas.
    name: String,
    to_q: Lin,
    to_kv: Lin,
    q_norm: Vec<f32>,
    k_norm: Vec<f32>,
    to_out: Lin,
}

impl CrossAttn {
    fn load(
        device: Device,
        weights: &SplatWeights,
        prefix: &str,
        channels: usize,
        ctx_channels: usize,
        heads: usize,
    ) -> Result<Self> {
        let head_dim = channels / heads;
        Ok(Self {
            name: prefix.to_string(),
            to_q: Lin::new(
                device,
                &weights.f32_shaped(&format!("{prefix}.to_q.weight"), &[channels, channels])?,
                channels,
                channels,
                Some(&weights.f32_shaped(&format!("{prefix}.to_q.bias"), &[channels])?),
            )?,
            to_kv: Lin::new(
                device,
                &weights.f32_shaped(&format!("{prefix}.to_kv.weight"), &[2 * channels, ctx_channels])?,
                2 * channels,
                ctx_channels,
                Some(&weights.f32_shaped(&format!("{prefix}.to_kv.bias"), &[2 * channels])?),
            )?,
            q_norm: weights
                .f32_shaped(&format!("{prefix}.q_rms_norm.gamma"), &[heads, head_dim])?,
            k_norm: weights
                .f32_shaped(&format!("{prefix}.k_rms_norm.gamma"), &[heads, head_dim])?,
            to_out: Lin::new(
                device,
                &weights.f32_shaped(&format!("{prefix}.to_out.weight"), &[channels, channels])?,
                channels,
                channels,
                Some(&weights.f32_shaped(&format!("{prefix}.to_out.bias"), &[channels])?),
            )?,
        })
    }

    /// `kv` is the projected context, cached across calls (the latent never
    /// changes inside a decode).
    fn forward(&self, x: &Ten, kv: &(Ten, Ten), heads: usize, channels: usize) -> Result<Ten> {
        let head_dim = channels / heads;
        let scale = 1.0 / (head_dim as f32).sqrt();
        let q = linear(x, &self.to_q)?;
        let q = rms_norm_per_head(
            &q,
            heads,
            head_dim,
            &self.q_norm,
            SPLAT_DEC_NAMESPACE,
            &format!("{}.q_rms_norm", self.name),
        )?;
        let attn = attention(&q, &kv.0, &kv.1, heads, scale)?;
        drop(q);
        linear(&attn, &self.to_out)
    }

    /// `to_kv(context)` split into the post-RMS key and the raw value.
    fn project_context(&self, context: &Ten, heads: usize, channels: usize) -> Result<(Ten, Ten)> {
        let head_dim = channels / heads;
        let kv = linear(context, &self.to_kv)?;
        let k = slice_cols(&kv, 0, channels)?;
        let v = slice_cols(&kv, channels, channels)?;
        drop(kv);
        let k = rms_norm_per_head(
            &k,
            heads,
            head_dim,
            &self.k_norm,
            SPLAT_DEC_NAMESPACE,
            &format!("{}.k_rms_norm", self.name),
        )?;
        Ok((k, v))
    }
}

/// `MultiHeadAttention(type="self", qk_rms_norm=True)`.
struct SelfAttn {
    name: String,
    to_qkv: Lin,
    q_norm: Vec<f32>,
    k_norm: Vec<f32>,
    to_out: Lin,
}

impl SelfAttn {
    fn load(device: Device, weights: &SplatWeights, prefix: &str, channels: usize, heads: usize) -> Result<Self> {
        let head_dim = channels / heads;
        Ok(Self {
            name: prefix.to_string(),
            to_qkv: Lin::new(
                device,
                &weights.f32_shaped(&format!("{prefix}.to_qkv.weight"), &[3 * channels, channels])?,
                3 * channels,
                channels,
                Some(&weights.f32_shaped(&format!("{prefix}.to_qkv.bias"), &[3 * channels])?),
            )?,
            q_norm: weights
                .f32_shaped(&format!("{prefix}.q_rms_norm.gamma"), &[heads, head_dim])?,
            k_norm: weights
                .f32_shaped(&format!("{prefix}.k_rms_norm.gamma"), &[heads, head_dim])?,
            to_out: Lin::new(
                device,
                &weights.f32_shaped(&format!("{prefix}.to_out.weight"), &[channels, channels])?,
                channels,
                channels,
                Some(&weights.f32_shaped(&format!("{prefix}.to_out.bias"), &[channels])?),
            )?,
        })
    }

    fn forward(&self, x: &Ten, heads: usize, channels: usize) -> Result<Ten> {
        let head_dim = channels / heads;
        let scale = 1.0 / (head_dim as f32).sqrt();
        let qkv = linear(x, &self.to_qkv)?;
        let q = slice_cols(&qkv, 0, channels)?;
        let k = slice_cols(&qkv, channels, channels)?;
        let v = slice_cols(&qkv, 2 * channels, channels)?;
        drop(qkv);
        let q = rms_norm_per_head(
            &q,
            heads,
            head_dim,
            &self.q_norm,
            SPLAT_DEC_NAMESPACE,
            &format!("{}.q_rms_norm", self.name),
        )?;
        let k = rms_norm_per_head(
            &k,
            heads,
            head_dim,
            &self.k_norm,
            SPLAT_DEC_NAMESPACE,
            &format!("{}.k_rms_norm", self.name),
        )?;
        let attn = attention(&q, &k, &v, heads, scale)?;
        drop((q, k, v));
        linear(&attn, &self.to_out)
    }
}

struct Mlp {
    fc0: Lin,
    fc2: Lin,
}

impl Mlp {
    fn load(device: Device, weights: &SplatWeights, prefix: &str, channels: usize, inner: usize) -> Result<Self> {
        Ok(Self {
            fc0: Lin::new(
                device,
                &weights.f32_shaped(&format!("{prefix}.mlp.0.weight"), &[inner, channels])?,
                inner,
                channels,
                Some(&weights.f32_shaped(&format!("{prefix}.mlp.0.bias"), &[inner])?),
            )?,
            fc2: Lin::new(
                device,
                &weights.f32_shaped(&format!("{prefix}.mlp.2.weight"), &[channels, inner])?,
                channels,
                inner,
                Some(&weights.f32_shaped(&format!("{prefix}.mlp.2.bias"), &[channels])?),
            )?,
        })
    }

    fn forward(&self, x: &Ten) -> Result<Ten> {
        linear(&gelu_tanh(&linear(x, &self.fc0)?)?, &self.fc2)
    }
}

const BLOCK_NORM_EPS: f32 = 1e-6;
const FINAL_NORM_EPS: f32 = 1e-5;

// ---------------------------------------------------------------------------
// Octree probability decoder
// ---------------------------------------------------------------------------

struct OctreeBlock {
    cross: CrossAttn,
    mlp: Mlp,
}

pub struct SplatOctree {
    device: Device,
    in_proj: Lin,
    input_layer: Lin,
    l_mlp0_w: Vec<f32>,
    l_mlp0_b: Vec<f32>,
    l_mlp2_w: Vec<f32>,
    l_mlp2_b: Vec<f32>,
    admod_w: Vec<f32>,
    admod_b: Vec<f32>,
    blocks: Vec<OctreeBlock>,
    out_proj: Lin,
    pos_freqs: Vec<f32>,
}

impl SplatOctree {
    pub fn prepare(device: Device, weights: &SplatWeights) -> Result<Self> {
        let c = OCT_MODEL_CHANNELS;
        let mut blocks = Vec::with_capacity(OCT_BLOCKS);
        for i in 0..OCT_BLOCKS {
            let p = format!("octree.blocks.{i}");
            blocks.push(OctreeBlock {
                cross: CrossAttn::load(
                    device,
                    weights,
                    &format!("{p}.cross_attn"),
                    c,
                    OCT_COND_CHANNELS,
                    OCT_HEADS,
                )?,
                mlp: Mlp::load(device, weights, &format!("{p}.mlp"), c, OCT_FFN)?,
            });
        }
        Ok(Self {
            device,
            in_proj: Lin::new(
                device,
                &weights.f32_shaped("octree.in_proj.weight", &[c, 3])?,
                c,
                3,
                Some(&weights.f32_shaped("octree.in_proj.bias", &[c])?),
            )?,
            input_layer: Lin::new(
                device,
                &weights.f32_shaped("octree.input_layer.weight", &[c, c])?,
                c,
                c,
                Some(&weights.f32_shaped("octree.input_layer.bias", &[c])?),
            )?,
            l_mlp0_w: weights.f32_shaped("octree.l_embedder.mlp.0.weight", &[c, T_FREQ_DIM])?,
            l_mlp0_b: weights.f32_shaped("octree.l_embedder.mlp.0.bias", &[c])?,
            l_mlp2_w: weights.f32_shaped("octree.l_embedder.mlp.2.weight", &[c, c])?,
            l_mlp2_b: weights.f32_shaped("octree.l_embedder.mlp.2.bias", &[c])?,
            admod_w: weights.f32_shaped("octree.adaLN_modulation.1.weight", &[6 * c, c])?,
            admod_b: weights.f32_shaped("octree.adaLN_modulation.1.bias", &[6 * c])?,
            blocks,
            out_proj: Lin::new(
                device,
                &weights.f32_shaped("octree.out_proj.weight", &[OCT_OUT, c])?,
                OCT_OUT,
                c,
                Some(&weights.f32_shaped("octree.out_proj.bias", &[OCT_OUT])?),
            )?,
            pos_freqs: pcd_position_embed_v2_freqs(c, 3, POS_EMBED_V2_MAX_RES),
        })
    }

    /// Cache `to_kv(latent)` per block — the latent is constant for the whole
    /// decode, and the octree calls this model once per level.
    pub fn project_latent(&self, latent: &Ten) -> Result<Vec<(Ten, Ten)>> {
        self.blocks
            .iter()
            .map(|block| {
                block
                    .cross
                    .project_context(latent, OCT_HEADS, OCT_MODEL_CHANNELS)
            })
            .collect()
    }

    /// `(logits, probs)` for one point list at resolution `res`.
    fn forward(&self, points: &[f32], res: f32, kv: &[(Ten, Ten)]) -> Result<Vec<f32>> {
        let c = OCT_MODEL_CHANNELS;
        let count = points.len() / 3;
        let pos = pcd_position_embed(points, 3, &self.pos_freqs, std::f32::consts::PI, c);
        let x = Ten::upload(self.device, points, count, 3)?;
        let pos = Ten::upload(self.device, &pos, count, c)?;
        let mut h = add(&linear(&x, &self.in_proj)?, &pos)?;
        drop((x, pos));
        h = linear(&h, &self.input_layer)?;

        let mods = self.level_modulation(res);
        let mods = Ten::upload(self.device, &mods, 1, 6 * c)?;
        for (block, kv) in self.blocks.iter().zip(kv) {
            let normed = layer_norm_mod(&h, &mods, c, 0, BLOCK_NORM_EPS)?;
            let attn = block.cross.forward(&normed, kv, OCT_HEADS, c)?;
            drop(normed);
            h = gated_residual_mod(&h, &attn, &mods, 2 * c)?;
            drop(attn);
            let normed = layer_norm_mod(&h, &mods, 4 * c, 3 * c, BLOCK_NORM_EPS)?;
            let ff = block.mlp.forward(&normed)?;
            drop(normed);
            h = gated_residual_mod(&h, &ff, &mods, 5 * c)?;
        }
        let h = layer_norm(&h, None, None, FINAL_NORM_EPS)?;
        linear(&h, &self.out_proj)?.to_host()
    }

    fn level_modulation(&self, res: f32) -> Vec<f32> {
        let c = OCT_MODEL_CHANNELS;
        let embedding = level_embedding(res, T_FREQ_DIM, 1024.0);
        let mut hidden = host_linear(
            &embedding,
            T_FREQ_DIM,
            &self.l_mlp0_w,
            c,
            Some(&self.l_mlp0_b),
        );
        host_silu(&mut hidden);
        let l_emb = host_linear(&hidden, c, &self.l_mlp2_w, c, Some(&self.l_mlp2_b));
        let mut silu_l = l_emb;
        host_silu(&mut silu_l);
        host_linear(&silu_l, c, &self.admod_w, 6 * c, Some(&self.admod_b))
    }

    /// `OctreeProbabilityFixedlenDecoder.sample`: descend `OCT_LEVEL` levels,
    /// splitting each node's sample budget over its 8 children, then place
    /// one anchor per remaining sample uniformly inside its leaf voxel.
    /// Returns `(anchors, count)` with anchors as `(count, 3)` in `[0, 1)`.
    pub fn sample(
        &self,
        latent: &Ten,
        num_points: usize,
        rng: &mut SplatRng,
        mut progress: Option<ProgressHook>,
        cancel: Option<crate::splat::SplatCancel<'_>>,
    ) -> Result<Vec<f32>> {
        // (i, j, k) for k in 0..2 for j in 0..2 for i in 0..2 — the child
        // slot order the reference's softmax columns are trained against.
        let mut child_offset = [[0i64; 3]; 8];
        let mut slot = 0;
        for k in 0..2i64 {
            for j in 0..2i64 {
                for i in 0..2i64 {
                    child_offset[slot] = [i, j, k];
                    slot += 1;
                }
            }
        }

        let kv = self.project_latent(latent)?;
        let mut coords: Vec<[i64; 3]> = vec![[0, 0, 0]];
        let mut counts: Vec<usize> = vec![num_points];

        for level in 1..=OCT_LEVEL {
            crate::splat::check_cancel(cancel)?;
            crate::emit_progress(
                &mut progress,
                &format!("octree level {level}/{OCT_LEVEL}"),
                level as f64 / OCT_LEVEL as f64,
            )?;
            let res_parent = (1u64 << (level - 1)) as f32;
            let res = (1u64 << level) as f32;
            let mut points = Vec::with_capacity(coords.len() * 3);
            for coord in &coords {
                for axis in coord {
                    points.push((*axis as f32 + 0.5) / res_parent);
                }
            }
            let logits = self.forward(&points, res, &kv)?;

            let mut next_coords = Vec::new();
            let mut next_counts = Vec::new();
            let mut sampled = [0i64; 8];
            for (node, coord) in coords.iter().enumerate() {
                let row = &logits[node * OCT_OUT..(node + 1) * OCT_OUT];
                let probs = softmax(row);
                sample_probs_row(&probs, counts[node], rng, &mut sampled);
                for (slot, offset) in child_offset.iter().enumerate() {
                    if sampled[slot] > 0 {
                        next_coords.push([
                            coord[0] * 2 + offset[0],
                            coord[1] * 2 + offset[1],
                            coord[2] * 2 + offset[2],
                        ]);
                        next_counts.push(sampled[slot] as usize);
                    }
                }
            }
            coords = next_coords;
            counts = next_counts;
        }

        // repeat_interleave by count, then jitter uniformly inside the voxel.
        let res = (1u64 << OCT_LEVEL) as f32;
        let mut anchors = Vec::with_capacity(num_points * 3);
        for (coord, count) in coords.iter().zip(&counts) {
            for _ in 0..*count {
                for axis in coord {
                    anchors.push((*axis as f32 + rng.uniform()) / res);
                }
            }
        }
        if anchors.len() != num_points * 3 {
            return Err(DiffusionError::model(format!(
                "octree sampled {} anchors, expected {num_points}",
                anchors.len() / 3
            )));
        }
        Ok(anchors)
    }
}

fn softmax(logits: &[f32]) -> Vec<f32> {
    let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mut out: Vec<f32> = logits.iter().map(|v| (v - max).exp()).collect();
    let sum: f32 = out.iter().sum();
    if sum > 0.0 {
        out.iter_mut().for_each(|v| *v /= sum);
    }
    out
}

// ---------------------------------------------------------------------------
// Elastic gaussian decoder
// ---------------------------------------------------------------------------

struct GsBlock {
    self_attn: SelfAttn,
    cross: CrossAttn,
    /// `norm2` is the only affine LayerNorm in `TransformerCrossBlock`.
    norm2_w: Vec<f32>,
    norm2_b: Vec<f32>,
    mlp: Mlp,
}

pub struct SplatGaussianDecoder {
    device: Device,
    in_proj: Lin,
    input_layer: Lin,
    blocks: Vec<GsBlock>,
    out_proj: Lin,
    pos_freqs: Vec<f32>,
    /// `(32, 3)` Hammersley jitter, loaded from the checkpoint buffer.
    perturbation: Vec<f32>,
    /// `log(exp(0.05) - 1)`, the inverse-softplus of the base offset scale.
    base_offset_scale: f32,
}

impl SplatGaussianDecoder {
    pub fn prepare(device: Device, weights: &SplatWeights) -> Result<Self> {
        let c = GS_MODEL_CHANNELS;
        let mut blocks = Vec::with_capacity(GS_BLOCKS);
        for i in 0..GS_BLOCKS {
            let p = format!("gs.blocks.{i}");
            blocks.push(GsBlock {
                self_attn: SelfAttn::load(device, weights, &format!("{p}.self_attn"), c, GS_HEADS)?,
                cross: CrossAttn::load(
                    device,
                    weights,
                    &format!("{p}.cross_attn"),
                    c,
                    GS_COND_CHANNELS,
                    GS_HEADS,
                )?,
                norm2_w: weights.f32_shaped(&format!("{p}.norm2.weight"), &[c])?,
                norm2_b: weights.f32_shaped(&format!("{p}.norm2.bias"), &[c])?,
                mlp: Mlp::load(device, weights, &format!("{p}.mlp"), c, GS_FFN)?,
            });
        }
        let perturbation =
            weights.f32_shaped("gs.points_offset_perturbation", &[GS_PER_POINT, 3])?;
        let base_offset_scale = weights.f32("gs.base_offset_scale")?;
        Ok(Self {
            device,
            in_proj: Lin::new(
                device,
                &weights.f32_shaped("gs.in_proj.weight", &[c, GS_IN_CHANNELS])?,
                c,
                GS_IN_CHANNELS,
                Some(&weights.f32_shaped("gs.in_proj.bias", &[c])?),
            )?,
            input_layer: Lin::new(
                device,
                &weights.f32_shaped("gs.input_layer.weight", &[c, c])?,
                c,
                c,
                Some(&weights.f32_shaped("gs.input_layer.bias", &[c])?),
            )?,
            blocks,
            out_proj: Lin::new(
                device,
                &weights.f32_shaped("gs.out_proj.weight", &[GS_OUT_CHANNELS, c])?,
                GS_OUT_CHANNELS,
                c,
                Some(&weights.f32_shaped("gs.out_proj.bias", &[GS_OUT_CHANNELS])?),
            )?,
            pos_freqs: pcd_position_embed_v2_freqs(c, 3, POS_EMBED_V2_MAX_RES),
            perturbation,
            base_offset_scale: *base_offset_scale
                .first()
                .ok_or_else(|| DiffusionError::model("gs.base_offset_scale is empty"))?,
        })
    }

    pub fn perturbation(&self) -> &[f32] {
        &self.perturbation
    }

    pub fn base_offset_scale(&self) -> f32 {
        self.base_offset_scale
    }

    /// `(anchors, 480)` raw features for the given anchor list.
    pub fn forward(
        &self,
        anchors: &[f32],
        latent: &Ten,
        mut progress: Option<ProgressHook>,
        cancel: Option<crate::splat::SplatCancel<'_>>,
    ) -> Result<Vec<f32>> {
        let c = GS_MODEL_CHANNELS;
        let count = anchors.len() / 3;
        let pos = pcd_position_embed(anchors, 3, &self.pos_freqs, std::f32::consts::PI, c);
        let x = Ten::upload(self.device, anchors, count, GS_IN_CHANNELS)?;
        let pos = Ten::upload(self.device, &pos, count, c)?;
        let mut h = add(&linear(&x, &self.in_proj)?, &pos)?;
        drop((x, pos));
        h = linear(&h, &self.input_layer)?;

        for (i, block) in self.blocks.iter().enumerate() {
            crate::splat::check_cancel(cancel)?;
            crate::emit_progress(
                &mut progress,
                &format!("gaussian block {}/{GS_BLOCKS}", i + 1),
                i as f64 / GS_BLOCKS as f64,
            )?;
            let normed = layer_norm(&h, None, None, BLOCK_NORM_EPS)?;
            let attn = block.self_attn.forward(&normed, GS_HEADS, c)?;
            drop(normed);
            h = add(&h, &attn)?;
            drop(attn);

            let normed = layer_norm(&h, Some(&block.norm2_w), Some(&block.norm2_b), BLOCK_NORM_EPS)?;
            let kv = block.cross.project_context(latent, GS_HEADS, c)?;
            let cross = block.cross.forward(&normed, &kv, GS_HEADS, c)?;
            drop((normed, kv));
            h = add(&h, &cross)?;
            drop(cross);

            let normed = layer_norm(&h, None, None, BLOCK_NORM_EPS)?;
            let ff = block.mlp.forward(&normed)?;
            drop(normed);
            h = add(&h, &ff)?;
        }
        let h = layer_norm(&h, None, None, FINAL_NORM_EPS)?;
        linear(&h, &self.out_proj)?.to_host()
    }
}

/// `ElasticGaussianFixedlenDecoder._get_offset` for one anchor's 32 gaussians.
/// `features` is one 480-wide row; the result is `(32, 3)`.
pub fn gaussian_offsets(features: &[f32], perturbation: &[f32], base_offset_scale: f32) -> Vec<f32> {
    let (xyz0, _) = crate::splat::gs_layout_range("_xyz").expect("layout");
    let (scale0, _) = crate::splat::gs_layout_range("_offset_scale").expect("layout");
    let mut out = vec![0.0f32; GS_PER_POINT * 3];
    for g in 0..GS_PER_POINT {
        // softplus(raw + base_offset_scale), broadcast over the 3 axes.
        let offset_scale = softplus(features[scale0 + g] + base_offset_scale);
        for axis in 0..3 {
            // lr['_xyz'] is 1.0, so the raw value passes through unscaled.
            let raw = features[xyz0 + g * 3 + axis] + perturbation[g * 3 + axis];
            out[g * 3 + axis] = raw.tanh() * 0.5 * GS_PERTURB_SIZE * offset_scale;
        }
    }
    out
}

pub fn softplus(x: f32) -> f32 {
    // Numerically stable log1p(exp(x)).
    if x > 20.0 {
        x
    } else {
        x.exp().ln_1p()
    }
}

/// Every tensor name the decoder reads.
pub fn decoder_expected_tensors() -> Vec<(String, Vec<usize>)> {
    let c = OCT_MODEL_CHANNELS;
    let mut out = vec![
        ("octree.in_proj.weight".to_string(), vec![c, 3]),
        ("octree.in_proj.bias".to_string(), vec![c]),
        ("octree.input_layer.weight".to_string(), vec![c, c]),
        ("octree.input_layer.bias".to_string(), vec![c]),
        ("octree.l_embedder.mlp.0.weight".to_string(), vec![c, T_FREQ_DIM]),
        ("octree.l_embedder.mlp.0.bias".to_string(), vec![c]),
        ("octree.l_embedder.mlp.2.weight".to_string(), vec![c, c]),
        ("octree.l_embedder.mlp.2.bias".to_string(), vec![c]),
        ("octree.adaLN_modulation.1.weight".to_string(), vec![6 * c, c]),
        ("octree.adaLN_modulation.1.bias".to_string(), vec![6 * c]),
        ("octree.out_proj.weight".to_string(), vec![OCT_OUT, c]),
        ("octree.out_proj.bias".to_string(), vec![OCT_OUT]),
        ("gs.in_proj.weight".to_string(), vec![c, GS_IN_CHANNELS]),
        ("gs.in_proj.bias".to_string(), vec![c]),
        ("gs.input_layer.weight".to_string(), vec![c, c]),
        ("gs.input_layer.bias".to_string(), vec![c]),
        ("gs.out_proj.weight".to_string(), vec![GS_OUT_CHANNELS, c]),
        ("gs.out_proj.bias".to_string(), vec![GS_OUT_CHANNELS]),
        ("gs.points_offset_perturbation".to_string(), vec![GS_PER_POINT, 3]),
        ("gs.base_offset_scale".to_string(), vec![]),
    ];
    let mut cross = |prefix: String, ctx: usize, heads: usize| {
        out.extend([
            (format!("{prefix}.to_q.weight"), vec![c, c]),
            (format!("{prefix}.to_q.bias"), vec![c]),
            (format!("{prefix}.to_kv.weight"), vec![2 * c, ctx]),
            (format!("{prefix}.to_kv.bias"), vec![2 * c]),
            (format!("{prefix}.q_rms_norm.gamma"), vec![heads, c / heads]),
            (format!("{prefix}.k_rms_norm.gamma"), vec![heads, c / heads]),
            (format!("{prefix}.to_out.weight"), vec![c, c]),
            (format!("{prefix}.to_out.bias"), vec![c]),
        ]);
    };
    for i in 0..OCT_BLOCKS {
        cross(format!("octree.blocks.{i}.cross_attn"), OCT_COND_CHANNELS, OCT_HEADS);
    }
    for i in 0..GS_BLOCKS {
        cross(format!("gs.blocks.{i}.cross_attn"), GS_COND_CHANNELS, GS_HEADS);
    }
    for i in 0..GS_BLOCKS {
        let p = format!("gs.blocks.{i}.self_attn");
        out.extend([
            (format!("{p}.to_qkv.weight"), vec![3 * c, c]),
            (format!("{p}.to_qkv.bias"), vec![3 * c]),
            (format!("{p}.q_rms_norm.gamma"), vec![GS_HEADS, GS_HEAD_DIM]),
            (format!("{p}.k_rms_norm.gamma"), vec![GS_HEADS, GS_HEAD_DIM]),
            (format!("{p}.to_out.weight"), vec![c, c]),
            (format!("{p}.to_out.bias"), vec![c]),
        ]);
        out.extend([
            (format!("gs.blocks.{i}.norm2.weight"), vec![c]),
            (format!("gs.blocks.{i}.norm2.bias"), vec![c]),
        ]);
    }
    for (prefix, inner) in [("octree.blocks", OCT_FFN), ("gs.blocks", GS_FFN)] {
        let count = if prefix.starts_with("octree") { OCT_BLOCKS } else { GS_BLOCKS };
        for i in 0..count {
            out.extend([
                (format!("{prefix}.{i}.mlp.mlp.0.weight"), vec![inner, c]),
                (format!("{prefix}.{i}.mlp.mlp.0.bias"), vec![inner]),
                (format!("{prefix}.{i}.mlp.mlp.2.weight"), vec![c, inner]),
                (format!("{prefix}.{i}.mlp.mlp.2.bias"), vec![c]),
            ]);
        }
    }
    let _ = (OCT_HEAD_DIM, GS_LAYOUT);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_dict_contract_matches_the_released_decoder() {
        let expected = decoder_expected_tensors();
        let names: Vec<&str> = expected.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"gs.blocks.15.cross_attn.to_kv.weight"));
        assert!(names.contains(&"octree.blocks.3.mlp.mlp.2.bias"));
        assert!(names.contains(&"gs.points_offset_perturbation"));
        // The released file has 388 tensors; every one of them is read.
        assert_eq!(expected.len(), 388);
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "duplicate tensor name");
    }

    #[test]
    fn softplus_is_stable_and_matches_log1p_exp() {
        assert!((softplus(0.0) - 2.0f32.ln()).abs() < 1e-6);
        assert!((softplus(-5.0) - (-5.0f32).exp().ln_1p()).abs() < 1e-6);
        assert_eq!(softplus(50.0), 50.0);
        assert!(softplus(-50.0) > 0.0);
    }

    #[test]
    fn softmax_normalizes_and_is_shift_invariant() {
        let a = softmax(&[0.0, 0.0, 0.0, 0.0]);
        assert!(a.iter().all(|v| (*v - 0.25).abs() < 1e-6));
        let b = softmax(&[100.0, 100.0, 100.0, 100.0]);
        assert!(b.iter().all(|v| (*v - 0.25).abs() < 1e-6));
        let c = softmax(&[0.0, 1.0]);
        assert!((c[0] + c[1] - 1.0).abs() < 1e-6);
        assert!(c[1] > c[0]);
    }

    #[test]
    fn gaussian_offsets_apply_perturbation_then_tanh_then_scale() {
        let mut features = vec![0.0f32; GS_OUT_CHANNELS];
        let (scale0, _) = crate::splat::gs_layout_range("_offset_scale").unwrap();
        // A very negative offset-scale logit collapses the offsets to ~0.
        for g in 0..GS_PER_POINT {
            features[scale0 + g] = -40.0;
        }
        let perturbation = vec![0.5f32; GS_PER_POINT * 3];
        let out = gaussian_offsets(&features, &perturbation, -40.0);
        assert!(out.iter().all(|v| v.abs() < 1e-8));
        // With offset_scale = softplus(0) = ln 2 and raw = 0 + 0.5:
        // tanh(0.5) * 0.5 * 1.5 * ln2
        let features = vec![0.0f32; GS_OUT_CHANNELS];
        let out = gaussian_offsets(&features, &perturbation, 0.0);
        let want = 0.5f32.tanh() * 0.5 * GS_PERTURB_SIZE * 2.0f32.ln();
        assert!((out[0] - want).abs() < 1e-6, "{} vs {want}", out[0]);
    }
}
