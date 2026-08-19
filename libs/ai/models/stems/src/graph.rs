//! The BS-RoFormer forward graph, expressed once in the ggml IR so the same
//! description runs on the Metal store (client) and the CUDA store (fleet).
//!
//! Scope: **spectrum in, masks out**. The STFT and the inverse STFT stay on the
//! CPU (`stft.rs`) — they are ~0.5% of the arithmetic and there is no FFT op in
//! the runtime, so putting them on the GPU would mean a DFT-as-GEMM that costs
//! more than it saves.
//!
//! Input  `features`: `[4100, 1101]` f32 — one chunk's spectrum, feature-major.
//!        Feature `f` at frame `t` is `((bin*2 + channel) * 2 + re_im)`, i.e.
//!        the reference's `rearrange('b s f t c -> b t ((f s) c)')`.
//! Output `mask[stem]`: `[4100, 1101]` f32, same layout — the complex ratio
//!        mask the caller multiplies into the spectrum before the inverse STFT.
//!
//! Two axis layouts alternate through the trunk, exactly as in the reference's
//! axial attention:
//!   TIME layout `[384, 1101, 62]` — sequence over frames, batch over bands.
//!   FREQ layout `[384, 62, 1101]` — sequence over bands, batch over frames.
//! The transition is one `permute` + `cont`.

use crate::config::*;
use crate::weights::{
    attn_name, bs_bias, bs_gamma, bs_weight, ff_name, mask_name, StemsWeights, FINAL_NORM,
};
use makepad_ai_common::{
    BufferUsage, Context, DiffusionError, Graph, Op, Result, TensorId, TensorType, UnaryOp,
};

/// RMSNorm here is `F.normalize(x) * sqrt(dim) * gamma`, which is ggml's
/// `rms_norm` with a vanishing epsilon. A true zero would turn an all-silent
/// band into NaN, so use the smallest epsilon that cannot perturb real
/// spectrogram magnitudes (torch's `F.normalize` guards the same way at 1e-12).
const NORM_EPS: f32 = 1e-12;

const ACT: BufferUsage = BufferUsage::Activations;
/// 1 / sqrt(dim_head).
const ATTN_SCALE: f32 = 0.125;
/// `GGML_ROPE_TYPE_NORMAL` — interleaved adjacent pairs (GPT-J style).
/// `rotary_embedding_torch` rotates `(x[2i], x[2i+1])`, verified numerically
/// against the library; it is NOT the NeoX split-half convention.
const ROPE_MODE_NORMAL: i32 = 0;

const F32_SIZE: usize = 4;

pub struct StemsGraph {
    pub graph: Graph,
    /// Write one chunk's spectrum here before each execution.
    pub features: TensorId,
    /// One complex ratio mask per stem, in `Stem::ALL` order.
    pub masks: [TensorId; NUM_STEMS],
}

pub fn build_graph(weights: &mut StemsWeights) -> Result<StemsGraph> {
    let groups = band_groups();
    let ctx = &mut weights.ctx;

    let features = ctx
        .new_named_tensor(
            "features",
            TensorType::F32,
            2,
            &[FEATURES as i64, CHUNK_FRAMES as i64],
            ACT,
        )
        .map_err(DiffusionError::model)?;

    let pos_time = positions(ctx, "pos.time", CHUNK_FRAMES)?;
    let pos_freq = positions(ctx, "pos.freq", NUM_BANDS)?;

    // Everything above needs real arena bytes (weights, the input the caller
    // writes, the position constants). Everything BELOW is intermediate: hand
    // it to the graph planner instead, which assigns offsets in the device
    // buffer and recycles dead ranges. Allocating intermediates in the arena
    // too would cost ~40 GB for this chunk size AND make the planner start
    // above them, i.e. pay twice.
    ctx.set_no_alloc(true);

    // ---- band split: 7 batched (rms_norm, scale, linear) over band groups --
    let mut band_outputs = Vec::with_capacity(groups.len());
    for (g, group) in groups.iter().enumerate() {
        let w = group.width as i64;
        let n = group.count as i64;
        // (feature, band, frame) view into the 4100-wide spectrum.
        let slice = ctx
            .view(
                features,
                TensorType::F32,
                &[w, n, CHUNK_FRAMES as i64],
                &[F32_SIZE, group.width * F32_SIZE, FEATURES * F32_SIZE],
                group.feature_offset * F32_SIZE,
            )
            .map_err(DiffusionError::model)?;
        // -> (feature, frame, band) so the batch axis is the band.
        let x = swap12_cont(ctx, slice)?;
        let x = norm_scale(ctx, x, weights_id(ctx, &bs_gamma(g))?)?;
        let x = ctx
            .mul_mat(weights_id(ctx, &bs_weight(g))?, x, ACT)
            .map_err(DiffusionError::model)?;
        let x = add(ctx, x, weights_id(ctx, &bs_bias(g))?)?;
        debug_assert_extents(ctx, x, &[DIM as i64, CHUNK_FRAMES as i64, n], "band split")?;
        band_outputs.push(x);
    }
    // [384, 1101, 62] — TIME layout.
    let mut x = concat_all(ctx, &band_outputs, 2)?;

    // ---- 8 blocks of (time transformer, freq transformer) ----
    for block in 0..DEPTH {
        x = transformer(ctx, x, pos_time, block, 0)?;
        // TIME -> FREQ.
        x = swap12_cont(ctx, x)?;
        x = transformer(ctx, x, pos_freq, block, 1)?;
        if block + 1 < DEPTH {
            // FREQ -> TIME for the next block's time transformer.
            x = swap12_cont(ctx, x)?;
        }
    }
    // After the last freq transformer x is FREQ layout: [384, 62, 1101].
    let x = norm_scale(ctx, x, weights_id(ctx, FINAL_NORM)?)?;

    // ---- mask estimators ----
    // The per-group (feature, frame, band) regrouping is shared by all four
    // stems, so it happens once here rather than four times.
    let mut group_features = Vec::with_capacity(groups.len());
    for group in &groups {
        let n = group.count as i64;
        let slice = ctx
            .view(
                x,
                TensorType::F32,
                &[DIM as i64, n, CHUNK_FRAMES as i64],
                &[
                    F32_SIZE,
                    DIM * F32_SIZE,
                    DIM * NUM_BANDS * F32_SIZE,
                ],
                group.first_band * DIM * F32_SIZE,
            )
            .map_err(DiffusionError::model)?;
        group_features.push(swap12_cont(ctx, slice)?);
    }

    let mut masks = [0usize; NUM_STEMS];
    for stem in 0..NUM_STEMS {
        let mut parts = Vec::with_capacity(groups.len());
        for (g, group) in groups.iter().enumerate() {
            let n = group.count as i64;
            let w = group.width as i64;
            let xg = group_features[g];
            let h = ctx
                .mul_mat(weights_id(ctx, &mask_name(stem, g, "w1"))?, xg, ACT)
                .map_err(DiffusionError::model)?;
            let h = add(ctx, h, weights_id(ctx, &mask_name(stem, g, "b1"))?)?;
            let h = ctx
                .unary(h, UnaryOp::Tanh, ACT)
                .map_err(DiffusionError::model)?;
            // nn.GLU: value * sigmoid(gate), with the halves pre-split at load.
            let value = ctx
                .mul_mat(weights_id(ctx, &mask_name(stem, g, "wv"))?, h, ACT)
                .map_err(DiffusionError::model)?;
            let value = add(ctx, value, weights_id(ctx, &mask_name(stem, g, "bv"))?)?;
            let gate = ctx
                .mul_mat(weights_id(ctx, &mask_name(stem, g, "wg"))?, h, ACT)
                .map_err(DiffusionError::model)?;
            let gate = add(ctx, gate, weights_id(ctx, &mask_name(stem, g, "bg"))?)?;
            let gate = ctx
                .unary(gate, UnaryOp::Sigmoid, ACT)
                .map_err(DiffusionError::model)?;
            let out = ctx
                .binary_like_a(Op::Mul, value, gate, ACT)
                .map_err(DiffusionError::model)?;
            debug_assert_extents(ctx, out, &[w, CHUNK_FRAMES as i64, n], "mask estimator")?;
            // (feature, frame, band) -> (feature, band, frame), then flatten
            // the band-major feature block for this group.
            let out = swap12_cont(ctx, out)?;
            let out = ctx
                .reshape(out, &[w * n, CHUNK_FRAMES as i64])
                .map_err(DiffusionError::model)?;
            parts.push(out);
        }
        let mask = concat_all(ctx, &parts, 0)?;
        debug_assert_extents(
            ctx,
            mask,
            &[FEATURES as i64, CHUNK_FRAMES as i64],
            "stem mask",
        )?;
        masks[stem] = mask;
    }

    ctx.set_no_alloc(false);

    let mut graph = Graph::new();
    for mask in masks {
        graph
            .build_forward_expand(ctx, mask)
            .map_err(DiffusionError::model)?;
    }

    Ok(StemsGraph {
        graph,
        features,
        masks,
    })
}

// ---------------------------------------------------------------------------
// One `Transformer(depth=1, norm_output=False)`: attention block then
// feed-forward block, both residual.
// ---------------------------------------------------------------------------

fn transformer(
    ctx: &mut Context,
    x: TensorId,
    positions: TensorId,
    block: usize,
    axis: usize,
) -> Result<TensorId> {
    let (seq, batch) = seq_batch(ctx, x)?;

    // -- attention --
    let h = norm_scale(ctx, x, weights_id(ctx, &attn_name(block, axis, "gamma"))?)?;
    let qkv = ctx
        .mul_mat(weights_id(ctx, &attn_name(block, axis, "qkv"))?, h, ACT)
        .map_err(DiffusionError::model)?;

    let mut parts = [0usize; 3];
    for (i, part) in parts.iter_mut().enumerate() {
        // [dim_head, heads, seq, batch] view of one third of the qkv rows.
        let v = ctx
            .view(
                qkv,
                TensorType::F32,
                &[DIM_HEAD as i64, HEADS as i64, seq, batch],
                &[
                    F32_SIZE,
                    DIM_HEAD * F32_SIZE,
                    DIM_INNER * 3 * F32_SIZE,
                    DIM_INNER * 3 * seq as usize * F32_SIZE,
                ],
                i * DIM_INNER * F32_SIZE,
            )
            .map_err(DiffusionError::model)?;
        *part = contiguous_4d(ctx, v)?;
    }
    let [q, k, v] = parts;
    // RoPE over the sequence axis (ne[2]); values pass through unrotated
    // beyond n_dims, which is exactly dim_head here.
    let q = ctx
        .rope(q, positions, DIM_HEAD as i32, ROPE_MODE_NORMAL, ACT)
        .map_err(DiffusionError::model)?;
    let k = ctx
        .rope(k, positions, DIM_HEAD as i32, ROPE_MODE_NORMAL, ACT)
        .map_err(DiffusionError::model)?;

    // flash_attn_ext wants [dim_head, seq, heads, batch].
    let qa = swap12(ctx, q)?;
    let ka = swap12(ctx, k)?;
    let va = swap12(ctx, v)?;
    let attn = ctx
        .flash_attn_ext(qa, ka, va, None, ATTN_SCALE, 0.0, 0.0, ACT)
        .map_err(DiffusionError::model)?;
    ctx.flash_attn_ext_set_prec(attn, makepad_ai_common::Prec::F32)
        .map_err(DiffusionError::model)?;
    // -> [dim_head, heads, seq, batch]

    // Per-head output gate: `out * sigmoid(to_gates(x))`.
    let gates = ctx
        .mul_mat(weights_id(ctx, &attn_name(block, axis, "gates_w"))?, h, ACT)
        .map_err(DiffusionError::model)?;
    let gates = add(ctx, gates, weights_id(ctx, &attn_name(block, axis, "gates_b"))?)?;
    let gates = ctx
        .unary(gates, UnaryOp::Sigmoid, ACT)
        .map_err(DiffusionError::model)?;
    // [heads, seq, batch] -> [1, heads, seq, batch]; ggml broadcasts ne0.
    let gates = ctx
        .reshape(gates, &[1, HEADS as i64, seq, batch])
        .map_err(DiffusionError::model)?;
    let attn = ctx
        .binary_like_a(Op::Mul, attn, gates, ACT)
        .map_err(DiffusionError::model)?;

    // `rearrange('b h n d -> b n (h d)')` is a plain reshape here: the gate
    // multiply already produced a contiguous (dim_head, heads, seq, batch)
    // tensor with dim_head fastest, so flattening ne0/ne1 IS the rearrange.
    // (A `cont` in between would copy 175 MB per layer to no effect.)
    let attn = ctx
        .reshape(attn, &[DIM_INNER as i64, seq, batch])
        .map_err(DiffusionError::model)?;
    let attn = ctx
        .mul_mat(weights_id(ctx, &attn_name(block, axis, "out"))?, attn, ACT)
        .map_err(DiffusionError::model)?;
    let x = ctx
        .binary_like_a(Op::Add, x, attn, ACT)
        .map_err(DiffusionError::model)?;

    // -- feed forward --
    let h = norm_scale(ctx, x, weights_id(ctx, &ff_name(block, axis, "gamma"))?)?;
    let h = ctx
        .mul_mat(weights_id(ctx, &ff_name(block, axis, "w1"))?, h, ACT)
        .map_err(DiffusionError::model)?;
    let h = add(ctx, h, weights_id(ctx, &ff_name(block, axis, "b1"))?)?;
    // torch `nn.GELU()` defaults to approximate='none' — the exact erf form.
    let h = ctx
        .unary(h, UnaryOp::GeluErf, ACT)
        .map_err(DiffusionError::model)?;
    let h = ctx
        .mul_mat(weights_id(ctx, &ff_name(block, axis, "w2"))?, h, ACT)
        .map_err(DiffusionError::model)?;
    let h = add(ctx, h, weights_id(ctx, &ff_name(block, axis, "b2"))?)?;
    ctx.binary_like_a(Op::Add, x, h, ACT)
        .map_err(DiffusionError::model)
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn weights_id(ctx: &Context, name: &str) -> Result<TensorId> {
    ctx.get_tensor(name)
        .ok_or_else(|| DiffusionError::model(format!("stems graph: no tensor '{name}'")))
}

/// `rms_norm(x) * gamma`; the Metal planner fuses the pair into one kernel.
fn norm_scale(ctx: &mut Context, x: TensorId, gamma: TensorId) -> Result<TensorId> {
    let n = ctx
        .rms_norm_eps(x, NORM_EPS, ACT)
        .map_err(DiffusionError::model)?;
    ctx.binary_like_a(Op::Mul, n, gamma, ACT)
        .map_err(DiffusionError::model)
}

fn add(ctx: &mut Context, x: TensorId, bias: TensorId) -> Result<TensorId> {
    ctx.binary_like_a(Op::Add, x, bias, ACT)
        .map_err(DiffusionError::model)
}

/// Exchange ne[1] and ne[2] — the TIME<->FREQ layout flip, and the
/// heads<->sequence flip attention needs.
fn swap12(ctx: &mut Context, x: TensorId) -> Result<TensorId> {
    ctx.permute(x, [0, 2, 1, 3]).map_err(DiffusionError::model)
}

/// `swap12` followed by `cont` — the layout flip as one call, so the borrow
/// checker sees a single sequential use of the context.
fn swap12_cont(ctx: &mut Context, x: TensorId) -> Result<TensorId> {
    let permuted = swap12(ctx, x)?;
    contiguous(ctx, permuted)
}

fn contiguous(ctx: &mut Context, x: TensorId) -> Result<TensorId> {
    let ne = extents(ctx, x)?;
    ctx.cont_3d(x, ne[0], ne[1], ne[2])
        .map_err(DiffusionError::model)
}

fn contiguous_4d(ctx: &mut Context, x: TensorId) -> Result<TensorId> {
    let ne = extents(ctx, x)?;
    ctx.cont_4d(x, ne[0], ne[1], ne[2], ne[3])
        .map_err(DiffusionError::model)
}

fn extents(ctx: &Context, x: TensorId) -> Result<[i64; 4]> {
    Ok(ctx
        .tensor(x)
        .ok_or_else(|| DiffusionError::model("stems graph: dangling tensor id"))?
        .ne)
}

/// Sequence length and batch count of a `[dim, seq, batch]` trunk tensor.
fn seq_batch(ctx: &Context, x: TensorId) -> Result<(i64, i64)> {
    let ne = extents(ctx, x)?;
    if ne[0] != DIM as i64 {
        return Err(DiffusionError::model(format!(
            "stems trunk tensor has ne0 {}, expected {DIM}",
            ne[0]
        )));
    }
    Ok((ne[1], ne[2]))
}

fn concat_all(ctx: &mut Context, parts: &[TensorId], dim: usize) -> Result<TensorId> {
    let mut iter = parts.iter().copied();
    let mut acc = iter
        .next()
        .ok_or_else(|| DiffusionError::model("stems graph: empty concat"))?;
    for part in iter {
        acc = ctx
            .concat(acc, part, dim, ACT)
            .map_err(DiffusionError::model)?;
    }
    Ok(acc)
}

/// Position indices 0..n as the I32 tensor RoPE reads.
fn positions(ctx: &mut Context, name: &str, n: usize) -> Result<TensorId> {
    let id = ctx
        .new_named_tensor(name, TensorType::I32, 1, &[n as i64], BufferUsage::Weights)
        .map_err(DiffusionError::model)?;
    let values: Vec<i32> = (0..n as i32).collect();
    let bytes =
        unsafe { std::slice::from_raw_parts(values.as_ptr() as *const u8, values.len() * 4) };
    ctx.write_tensor_data(id, bytes)
        .map_err(DiffusionError::model)?;
    Ok(id)
}

fn debug_assert_extents(
    ctx: &Context,
    id: TensorId,
    want: &[i64],
    what: &str,
) -> Result<()> {
    let ne = extents(ctx, id)?;
    for (i, &w) in want.iter().enumerate() {
        if ne[i] != w {
            return Err(DiffusionError::model(format!(
                "stems graph {what}: ne{i} is {} but should be {w} (full shape {ne:?})",
                ne[i]
            )));
        }
    }
    Ok(())
}
