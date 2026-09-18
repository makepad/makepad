//! The Mel-Band RoFormer forward graph: **spectrum in, band masks out**.
//!
//! Input  `features`: `[4100, frames]` f32 — exactly the four-stem graph's
//!        input, packed by the same code.
//! Output `[7916, frames]` f32 — the 60 band masks laid end to end along the
//!        feature axis, band `b` at [`band_mask_offset`]`(b)`, each in the
//!        band's own `((bin - first) * 2 + channel) * 2 + re_im` order.
//!
//! The bands overlap, so the reference adds each band's mask into the bins it
//! covers and divides by how many bands cover each bin. The runtime has no
//! scatter-add on either store, and the sum is some eight thousand additions
//! per frame, so that one step is left to the CPU after readback
//! (`model.rs`); the graph stops at the band masks.
//!
//! The trunk is the four-stem [`transformer`](crate::graph) builder, called
//! unchanged. The reference's Mel-Band transformer differs from the four-stem
//! one in a single respect — it normalises its output — so each call is
//! followed by that transformer's own norm, and there is no norm after the
//! last block.
//!
//! Every per-band weight is 2-D (see `weights.rs`), so the band split and the
//! mask head are 60 small chains rather than a few batched ones. A band's
//! input is a contiguous run of the feature axis and a band's trunk output is
//! a contiguous run of the TIME layout, so both are views; the band-split
//! view strides over the features it skips and is copied once, because the
//! CUDA store's GEMM and unary kernels read contiguous tensors only.

use super::config::*;
use super::weights::{band_bias, band_gamma, band_weight, mask_name, norm_name};
use crate::config::{DIM, FEATURES};
use crate::graph::{
    add, debug_assert_extents, norm_scale, norm_scale_eps, positions, swap12_cont, transformer,
    weights_id,
};
use crate::weights::StemsWeights;
use makepad_ai_common::{
    BufferUsage, Context, DiffusionError, Graph, Op, Result, TensorId, TensorType, UnaryOp,
};

const ACT: BufferUsage = BufferUsage::Activations;
const F32_SIZE: usize = 4;
/// The epsilon of the band-split norm, the one norm here that sees the raw
/// spectrogram. The reference divides by the band's norm and only clamps
/// that at 1e-12, which floors nothing a recording holds: a band with next
/// to no energy in it is brought up to unit level like any other, and the
/// network was trained on exactly that. The runtime adds its epsilon under
/// the root of the MEAN square instead, where the shared 1e-12 would hold
/// every band quieter than -120 dBFS below unit level. 1e-30 keeps an
/// all-zero band finite and floors nothing either.
const BAND_NORM_EPS: f32 = 1e-30;

/// How far into the forward a graph is built.
///
/// A separator always runs to [`Masks`](Stage::Masks). The earlier stages
/// exist to find a fault: a graph that stops at one returns that stage's
/// tensor as its output, which can be held against the same stage of a
/// reference forward, and the first stage that disagrees names the part that
/// is wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// The band split, TIME layout `[384, frames, 60]`.
    BandSplit,
    /// After block `i`'s time and then frequency transformer, each with its
    /// output norm: FREQ layout `[384, 60, frames]`.
    Layer(usize),
    /// The whole forward: `[7916, frames]`.
    Masks,
}

pub struct VocalsGraph {
    pub graph: Graph,
    /// Write one chunk's spectrum here before each execution.
    pub features: TensorId,
    /// What the graph was built to return.
    pub output: TensorId,
    pub stage: Stage,
}

/// Builds the forward graph for chunks of `frames` STFT frames.
pub fn build_graph(weights: &mut StemsWeights, frames: usize) -> Result<VocalsGraph> {
    build_graph_until(weights, frames, Stage::Masks)
}

/// As [`build_graph`], stopping at `stage`.
pub fn build_graph_until(
    weights: &mut StemsWeights,
    frames: usize,
    stage: Stage,
) -> Result<VocalsGraph> {
    if let Stage::Layer(block) = stage {
        if block >= DEPTH {
            return Err(DiffusionError::model(format!(
                "vocals graph: no block {block}, the trunk has {DEPTH}"
            )));
        }
    }
    let ctx = &mut weights.ctx;

    let features = ctx
        .new_named_tensor(
            "features",
            TensorType::F32,
            2,
            &[FEATURES as i64, frames as i64],
            ACT,
        )
        .map_err(DiffusionError::model)?;
    let pos_time = positions(ctx, "pos.time", frames)?;
    let pos_freq = positions(ctx, "pos.freq", NUM_BANDS)?;

    // The leaves above hold real arena bytes; everything below is an
    // intermediate the device planner places (see the four-stem builder).
    ctx.set_no_alloc(true);
    let output = forward(ctx, features, pos_time, pos_freq, frames, stage)?;
    ctx.set_no_alloc(false);

    let mut graph = Graph::new();
    graph
        .build_forward_expand(ctx, output)
        .map_err(DiffusionError::model)?;

    Ok(VocalsGraph {
        graph,
        features,
        output,
        stage,
    })
}

fn forward(
    ctx: &mut Context,
    features: TensorId,
    pos_time: TensorId,
    pos_freq: TensorId,
    frames: usize,
    stage: Stage,
) -> Result<TensorId> {
    let frames_i = frames as i64;

    // ---- band split: (rms_norm, scale, linear) per band ----
    let mut band_outputs = Vec::with_capacity(NUM_BANDS);
    for band in 0..NUM_BANDS {
        let w = band_width(band) as i64;
        // (feature, frame) view of this band's run of the spectrum.
        let slice = ctx
            .view(
                features,
                TensorType::F32,
                &[w, frames_i],
                &[F32_SIZE, FEATURES * F32_SIZE],
                band_feature_offset(band) * F32_SIZE,
            )
            .map_err(DiffusionError::model)?;
        // The band is the batch axis of the trunk, so give it one here.
        let x = ctx
            .cont_3d(slice, w, frames_i, 1)
            .map_err(DiffusionError::model)?;
        let x = norm_scale_eps(ctx, x, weights_id(ctx, &band_gamma(band))?, BAND_NORM_EPS)?;
        let x = ctx
            .mul_mat(weights_id(ctx, &band_weight(band))?, x, ACT)
            .map_err(DiffusionError::model)?;
        let x = add(ctx, x, weights_id(ctx, &band_bias(band))?)?;
        debug_assert_extents(ctx, x, &[DIM as i64, frames_i, 1], "band split")?;
        band_outputs.push(x);
    }
    // [384, frames, 60] — TIME layout.
    let mut x = concat_balanced(ctx, &band_outputs, 2)?;
    if stage == Stage::BandSplit {
        return Ok(x);
    }

    // ---- 6 blocks of (time transformer, freq transformer), each normed ----
    for block in 0..DEPTH {
        x = transformer(ctx, x, pos_time, block, 0)?;
        x = norm_scale(ctx, x, weights_id(ctx, &norm_name(block, 0))?)?;
        // TIME -> FREQ.
        x = swap12_cont(ctx, x)?;
        x = transformer(ctx, x, pos_freq, block, 1)?;
        x = norm_scale(ctx, x, weights_id(ctx, &norm_name(block, 1))?)?;
        if stage == Stage::Layer(block) {
            return Ok(x);
        }
        // FREQ -> TIME: for the next block's time transformer, and after the
        // last block because TIME is the layout a band is contiguous in.
        x = swap12_cont(ctx, x)?;
    }
    debug_assert_extents(ctx, x, &[DIM as i64, frames_i, NUM_BANDS as i64], "trunk")?;

    // ---- mask estimator: Linear, tanh, Linear, tanh, Linear, GLU per band --
    let mut band_masks = Vec::with_capacity(NUM_BANDS);
    for band in 0..NUM_BANDS {
        let w = band_width(band) as i64;
        let xb = ctx
            .view(
                x,
                TensorType::F32,
                &[DIM as i64, frames_i],
                &[F32_SIZE, DIM * F32_SIZE],
                band * frames * DIM * F32_SIZE,
            )
            .map_err(DiffusionError::model)?;
        let h = linear(ctx, xb, &mask_name(band, "w1"), &mask_name(band, "b1"))?;
        let h = ctx
            .unary(h, UnaryOp::Tanh, ACT)
            .map_err(DiffusionError::model)?;
        let h = linear(ctx, h, &mask_name(band, "w2"), &mask_name(band, "b2"))?;
        let h = ctx
            .unary(h, UnaryOp::Tanh, ACT)
            .map_err(DiffusionError::model)?;
        // nn.GLU: value * sigmoid(gate), with the halves pre-split at load.
        let value = linear(ctx, h, &mask_name(band, "wv"), &mask_name(band, "bv"))?;
        let gate = linear(ctx, h, &mask_name(band, "wg"), &mask_name(band, "bg"))?;
        let gate = ctx
            .unary(gate, UnaryOp::Sigmoid, ACT)
            .map_err(DiffusionError::model)?;
        let out = ctx
            .binary_like_a(Op::Mul, value, gate, ACT)
            .map_err(DiffusionError::model)?;
        debug_assert_extents(ctx, out, &[w, frames_i], "mask estimator")?;
        band_masks.push(out);
    }
    let masks = concat_balanced(ctx, &band_masks, 0)?;
    debug_assert_extents(ctx, masks, &[BAND_FEATURES as i64, frames_i], "band masks")?;
    Ok(masks)
}

/// `weight * x + bias`.
fn linear(ctx: &mut Context, x: TensorId, weight: &str, bias: &str) -> Result<TensorId> {
    let y = ctx
        .mul_mat(weights_id(ctx, weight)?, x, ACT)
        .map_err(DiffusionError::model)?;
    add(ctx, y, weights_id(ctx, bias)?)
}

/// Joins `parts` in order along `dim` as a balanced tree of pairwise concats.
///
/// A concat copies both of its operands. Folding 60 parts from the left
/// copies the growing head 59 times over, about thirty times the bytes of the
/// result; a balanced tree copies every part once per level, six times.
fn concat_balanced(ctx: &mut Context, parts: &[TensorId], dim: usize) -> Result<TensorId> {
    match parts {
        [] => Err(DiffusionError::model("vocals graph: empty concat")),
        [only] => Ok(*only),
        _ => {
            let (head, tail) = parts.split_at(parts.len() / 2);
            let head = concat_balanced(ctx, head, dim)?;
            let tail = concat_balanced(ctx, tail, dim)?;
            ctx.concat(head, tail, dim, ACT)
                .map_err(DiffusionError::model)
        }
    }
}
