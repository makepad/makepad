//! One fixed-shape Beat This! forward graph: `[1500,128]` log-mel frames to
//! `[1500,2]` beat/downbeat logits.
//!
//! The graph uses the same RMSNorm, rotary gated attention and axial
//! time/frequency batching pattern as the stems RoFormer. Convolutions are a
//! compact graph-native im2col made from strided views plus `mul_mat`; this is
//! supported by both compiled stores (the CUDA raw-graph executor does not
//! currently expose `Op::Im2col`/`Op::Conv2d`).

use crate::config::*;
use crate::weights::{
    block_bn, block_conv, transformer_name, BeatsWeights, FINAL_NORM, FRONT_LINEAR_B,
    FRONT_LINEAR_W, HEAD_B, HEAD_W, INPUT_BN_BIAS, INPUT_BN_SCALE, STEM_BN_BIAS,
    STEM_BN_SCALE, STEM_CONV,
};
use makepad_ai_common::{
    BufferUsage, Context, DiffusionError, Graph, Op, Prec, Result, TensorId, TensorType, UnaryOp,
};

const ACT: BufferUsage = BufferUsage::Activations;
const F32_BYTES: usize = 4;

pub struct BeatsGraph {
    pub graph: Graph,
    pub mel: TensorId,
    /// `[2, CHUNK_FRAMES]`, with beat (already SumHead-combined) at row 0.
    pub logits: TensorId,
}

pub fn build_graph(weights: &mut BeatsWeights) -> Result<BeatsGraph> {
    let config = weights.config;
    let ctx = &mut weights.ctx;
    let mel = ctx
        .new_named_tensor(
            "mel",
            TensorType::F32,
            2,
            &[MEL_BINS as i64, CHUNK_FRAMES as i64],
            ACT,
        )
        .map_err(DiffusionError::model)?;
    let pos_time = positions(ctx, "position.time", CHUNK_FRAMES)?;
    let pos_freq = [
        positions(ctx, "position.freq32", 32)?,
        positions(ctx, "position.freq16", 16)?,
        positions(ctx, "position.freq8", 8)?,
    ];
    let pads = [
        zeros(ctx, "pad.stem", &[1, 1, MEL_BINS])?,
        zeros(ctx, "pad.front0", &[32, 1, 32])?,
        zeros(ctx, "pad.front1", &[64, 1, 16])?,
        zeros(ctx, "pad.front2", &[128, 1, 8])?,
    ];

    ctx.set_no_alloc(true);

    // BatchNorm over each mel bin, then `[freq,time] -> [1,time,freq]`.
    let x = affine(
        ctx,
        mel,
        weight(ctx, INPUT_BN_SCALE)?,
        weight(ctx, INPUT_BN_BIAS)?,
    )?;
    let x = ctx.transpose(x).map_err(DiffusionError::model)?;
    let x = ctx
        .cont_2d(x, CHUNK_FRAMES as i64, MEL_BINS as i64)
        .map_err(DiffusionError::model)?;
    let x = ctx
        .reshape(x, &[1, CHUNK_FRAMES as i64, MEL_BINS as i64])
        .map_err(DiffusionError::model)?;
    let x = conv2d(
        ctx,
        x,
        weight(ctx, STEM_CONV)?,
        pads[0],
        1,
        MEL_BINS,
        STEM_DIM,
        4,
        3,
        4,
    )?;
    let mut x = affine(
        ctx,
        x,
        weight(ctx, STEM_BN_SCALE)?,
        weight(ctx, STEM_BN_BIAS)?,
    )?;
    x = ctx
        .unary(x, UnaryOp::GeluErf, ACT)
        .map_err(DiffusionError::model)?;

    for block in 0..STEM_BLOCKS {
        let dim = STEM_CHANNELS[block];
        let freq = STEM_FREQS[block];
        // `[channel,time,freq] -> [channel,freq,time]`: frequency sequence,
        // one batch per time frame.
        x = swap12_cont(ctx, x)?;
        x = transformer(
            ctx,
            x,
            pos_freq[block],
            dim,
            &format!("front{block}.freq"),
        )?;
        // Time sequence, one batch per frequency band.
        x = swap12_cont(ctx, x)?;
        x = transformer(
            ctx,
            x,
            pos_time,
            dim,
            &format!("front{block}.time"),
        )?;
        x = conv2d(
            ctx,
            x,
            weight(ctx, &block_conv(block))?,
            pads[block + 1],
            dim,
            freq,
            STEM_CHANNELS[block + 1],
            2,
            3,
            2,
        )?;
        x = affine(
            ctx,
            x,
            weight(ctx, &block_bn(block, "scale"))?,
            weight(ctx, &block_bn(block, "bias"))?,
        )?;
        x = ctx
            .unary(x, UnaryOp::GeluErf, ACT)
            .map_err(DiffusionError::model)?;
    }

    // PyTorch `rearrange('b c f t -> b t (c f)')` flattens with frequency
    // fastest inside channel. `[c,t,f] -> [f,c,t]`, make contiguous, flatten.
    let x = ctx.permute(x, [1, 2, 0, 3]).map_err(DiffusionError::model)?;
    let x = ctx
        .cont_3d(x, 4, 256, CHUNK_FRAMES as i64)
        .map_err(DiffusionError::model)?;
    let x = ctx
        .reshape(x, &[FRONTEND_FEATURES as i64, CHUNK_FRAMES as i64])
        .map_err(DiffusionError::model)?;
    let mut x = ctx
        .mul_mat(weight(ctx, FRONT_LINEAR_W)?, x, ACT)
        .map_err(DiffusionError::model)?;
    x = add(ctx, x, weight(ctx, FRONT_LINEAR_B)?)?;

    for layer in 0..MAIN_LAYERS {
        x = transformer(
            ctx,
            x,
            pos_time,
            config.transformer_dim,
            &format!("main{layer}"),
        )?;
    }
    x = norm_scale(ctx, x, weight(ctx, FINAL_NORM)?)?;
    let raw = ctx
        .mul_mat(weight(ctx, HEAD_W)?, x, ACT)
        .map_err(DiffusionError::model)?;
    let raw = add(ctx, raw, weight(ctx, HEAD_B)?)?;

    // SumHead: beat = beat_channel + downbeat_channel; downbeat unchanged.
    let beat = ctx
        .view(
            raw,
            TensorType::F32,
            &[1, CHUNK_FRAMES as i64],
            &[F32_BYTES, 2 * F32_BYTES],
            0,
        )
        .map_err(DiffusionError::model)?;
    let downbeat = ctx
        .view(
            raw,
            TensorType::F32,
            &[1, CHUNK_FRAMES as i64],
            &[F32_BYTES, 2 * F32_BYTES],
            F32_BYTES,
        )
        .map_err(DiffusionError::model)?;
    let beat = ctx
        .binary_like_a(Op::Add, beat, downbeat, ACT)
        .map_err(DiffusionError::model)?;
    let logits = ctx
        .concat(beat, downbeat, 0, ACT)
        .map_err(DiffusionError::model)?;

    ctx.set_no_alloc(false);
    let mut graph = Graph::new();
    graph
        .build_forward_expand(ctx, logits)
        .map_err(DiffusionError::model)?;
    Ok(BeatsGraph { graph, mel, logits })
}

#[allow(clippy::too_many_arguments)]
fn conv2d(
    ctx: &mut Context,
    input: TensorId,
    kernel: TensorId,
    zero_pad: TensorId,
    in_channels: usize,
    in_freq: usize,
    out_channels: usize,
    kernel_h: usize,
    kernel_w: usize,
    freq_stride: usize,
) -> Result<TensorId> {
    let time = CHUNK_FRAMES;
    let padded = ctx
        .concat(zero_pad, input, 1, ACT)
        .map_err(DiffusionError::model)?;
    let padded = ctx
        .concat(padded, zero_pad, 1, ACT)
        .map_err(DiffusionError::model)?;
    let padded_time = time + 2;
    let out_freq = (in_freq - kernel_h) / freq_stride + 1;
    let mut patches = Vec::with_capacity(kernel_h * kernel_w);
    for ky in 0..kernel_h {
        for kx in 0..kernel_w {
            let view = ctx
                .view(
                    padded,
                    TensorType::F32,
                    &[in_channels as i64, time as i64, out_freq as i64],
                    &[
                        F32_BYTES,
                        in_channels * F32_BYTES,
                        in_channels * padded_time * freq_stride * F32_BYTES,
                    ],
                    (ky * in_channels * padded_time + kx * in_channels) * F32_BYTES,
                )
                .map_err(DiffusionError::model)?;
            let contiguous = ctx
                .cont_3d(
                    view,
                    in_channels as i64,
                    time as i64,
                    out_freq as i64,
                )
                .map_err(DiffusionError::model)?;
            patches.push(
                ctx.reshape(
                    contiguous,
                    &[in_channels as i64, (time * out_freq) as i64],
                )
                .map_err(DiffusionError::model)?,
            );
        }
    }
    let columns = concat_all(ctx, &patches, 0)?;
    let output = ctx
        .mul_mat(kernel, columns, ACT)
        .map_err(DiffusionError::model)?;
    let output = ctx
        .reshape(
            output,
            &[out_channels as i64, time as i64, out_freq as i64],
        )
        .map_err(DiffusionError::model)?;
    Ok(output)
}

fn transformer(
    ctx: &mut Context,
    input: TensorId,
    positions: TensorId,
    dim: usize,
    prefix: &str,
) -> Result<TensorId> {
    let ne = extents(ctx, input)?;
    if ne[0] != dim as i64 {
        return Err(DiffusionError::model(format!(
            "beats transformer {prefix}: input dim {} != {dim}",
            ne[0]
        )));
    }
    let sequence = ne[1];
    let batch = ne[2];
    let heads = dim / HEAD_DIM;

    let normalized = norm_scale(
        ctx,
        input,
        weight(ctx, &transformer_name(prefix, "attn.gamma"))?,
    )?;
    let qkv = ctx
        .mul_mat(
            weight(ctx, &transformer_name(prefix, "attn.qkv"))?,
            normalized,
            ACT,
        )
        .map_err(DiffusionError::model)?;
    let mut parts = [0usize; 3];
    for (index, part) in parts.iter_mut().enumerate() {
        let view = ctx
            .view(
                qkv,
                TensorType::F32,
                &[HEAD_DIM as i64, heads as i64, sequence, batch],
                &[
                    F32_BYTES,
                    HEAD_DIM * F32_BYTES,
                    dim * 3 * F32_BYTES,
                    dim * 3 * sequence as usize * F32_BYTES,
                ],
                index * dim * F32_BYTES,
            )
            .map_err(DiffusionError::model)?;
        *part = ctx
            .cont_4d(view, HEAD_DIM as i64, heads as i64, sequence, batch)
            .map_err(DiffusionError::model)?;
    }
    let [query, key, value] = parts;
    let query = ctx
        .rope(query, positions, HEAD_DIM as i32, 0, ACT)
        .map_err(DiffusionError::model)?;
    let key = ctx
        .rope(key, positions, HEAD_DIM as i32, 0, ACT)
        .map_err(DiffusionError::model)?;
    let query = swap12(ctx, query)?;
    let key = swap12(ctx, key)?;
    let value = swap12(ctx, value)?;
    let attention = ctx
        .flash_attn_ext(
            query,
            key,
            value,
            None,
            1.0 / (HEAD_DIM as f32).sqrt(),
            0.0,
            0.0,
            ACT,
        )
        .map_err(DiffusionError::model)?;
    ctx.flash_attn_ext_set_prec(attention, Prec::F32)
        .map_err(DiffusionError::model)?;

    let gates = ctx
        .mul_mat(
            weight(ctx, &transformer_name(prefix, "attn.gates_w"))?,
            normalized,
            ACT,
        )
        .map_err(DiffusionError::model)?;
    let gates = add(
        ctx,
        gates,
        weight(ctx, &transformer_name(prefix, "attn.gates_b"))?,
    )?;
    let gates = ctx
        .unary(gates, UnaryOp::Sigmoid, ACT)
        .map_err(DiffusionError::model)?;
    let gates = ctx
        .reshape(gates, &[1, heads as i64, sequence, batch])
        .map_err(DiffusionError::model)?;
    let attention = ctx
        .binary_like_a(Op::Mul, attention, gates, ACT)
        .map_err(DiffusionError::model)?;
    let attention = ctx
        .reshape(attention, &[dim as i64, sequence, batch])
        .map_err(DiffusionError::model)?;
    let attention = ctx
        .mul_mat(
            weight(ctx, &transformer_name(prefix, "attn.out"))?,
            attention,
            ACT,
        )
        .map_err(DiffusionError::model)?;
    let residual = ctx
        .binary_like_a(Op::Add, input, attention, ACT)
        .map_err(DiffusionError::model)?;

    let hidden = norm_scale(
        ctx,
        residual,
        weight(ctx, &transformer_name(prefix, "ff.gamma"))?,
    )?;
    let hidden = ctx
        .mul_mat(
            weight(ctx, &transformer_name(prefix, "ff.w1"))?,
            hidden,
            ACT,
        )
        .map_err(DiffusionError::model)?;
    let hidden = add(
        ctx,
        hidden,
        weight(ctx, &transformer_name(prefix, "ff.b1"))?,
    )?;
    let hidden = ctx
        .unary(hidden, UnaryOp::GeluErf, ACT)
        .map_err(DiffusionError::model)?;
    let hidden = ctx
        .mul_mat(
            weight(ctx, &transformer_name(prefix, "ff.w2"))?,
            hidden,
            ACT,
        )
        .map_err(DiffusionError::model)?;
    let hidden = add(
        ctx,
        hidden,
        weight(ctx, &transformer_name(prefix, "ff.b2"))?,
    )?;
    ctx.binary_like_a(Op::Add, residual, hidden, ACT)
        .map_err(DiffusionError::model)
}

fn norm_scale(ctx: &mut Context, input: TensorId, gamma: TensorId) -> Result<TensorId> {
    let normalized = ctx
        .rms_norm_eps(input, NORM_EPS, ACT)
        .map_err(DiffusionError::model)?;
    ctx.binary_like_a(Op::Mul, normalized, gamma, ACT)
        .map_err(DiffusionError::model)
}

fn affine(
    ctx: &mut Context,
    input: TensorId,
    scale: TensorId,
    bias: TensorId,
) -> Result<TensorId> {
    let scaled = ctx
        .binary_like_a(Op::Mul, input, scale, ACT)
        .map_err(DiffusionError::model)?;
    add(ctx, scaled, bias)
}

fn add(ctx: &mut Context, input: TensorId, bias: TensorId) -> Result<TensorId> {
    ctx.binary_like_a(Op::Add, input, bias, ACT)
        .map_err(DiffusionError::model)
}

fn swap12(ctx: &mut Context, input: TensorId) -> Result<TensorId> {
    ctx.permute(input, [0, 2, 1, 3])
        .map_err(DiffusionError::model)
}

fn swap12_cont(ctx: &mut Context, input: TensorId) -> Result<TensorId> {
    let swapped = swap12(ctx, input)?;
    let ne = extents(ctx, swapped)?;
    ctx.cont_3d(swapped, ne[0], ne[1], ne[2])
        .map_err(DiffusionError::model)
}

fn concat_all(ctx: &mut Context, parts: &[TensorId], axis: usize) -> Result<TensorId> {
    let mut iter = parts.iter().copied();
    let mut output = iter
        .next()
        .ok_or_else(|| DiffusionError::model("beats graph: empty concat"))?;
    for part in iter {
        output = ctx
            .concat(output, part, axis, ACT)
            .map_err(DiffusionError::model)?;
    }
    Ok(output)
}

fn weight(ctx: &Context, name: &str) -> Result<TensorId> {
    ctx.get_tensor(name)
        .ok_or_else(|| DiffusionError::model(format!("beats graph: missing tensor '{name}'")))
}

fn extents(ctx: &Context, input: TensorId) -> Result<[i64; 4]> {
    Ok(ctx
        .tensor(input)
        .ok_or_else(|| DiffusionError::model("beats graph: dangling tensor id"))?
        .ne)
}

fn positions(ctx: &mut Context, name: &str, count: usize) -> Result<TensorId> {
    let tensor = ctx
        .new_named_tensor(
            name,
            TensorType::I32,
            1,
            &[count as i64],
            BufferUsage::Weights,
        )
        .map_err(DiffusionError::model)?;
    let values: Vec<i32> = (0..count as i32).collect();
    let bytes = unsafe {
        std::slice::from_raw_parts(values.as_ptr().cast::<u8>(), values.len() * F32_BYTES)
    };
    ctx.write_tensor_data(tensor, bytes)
        .map_err(DiffusionError::model)?;
    Ok(tensor)
}

fn zeros(ctx: &mut Context, name: &str, extents: &[usize]) -> Result<TensorId> {
    let shape: Vec<i64> = extents.iter().map(|&value| value as i64).collect();
    let tensor = ctx
        .new_named_tensor(
            name,
            TensorType::F32,
            shape.len(),
            &shape,
            BufferUsage::Weights,
        )
        .map_err(DiffusionError::model)?;
    let values = vec![0.0f32; extents.iter().product()];
    let bytes = unsafe {
        std::slice::from_raw_parts(values.as_ptr().cast::<u8>(), values.len() * F32_BYTES)
    };
    ctx.write_tensor_data(tensor, bytes)
        .map_err(DiffusionError::model)?;
    Ok(tensor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::weights::DEFAULT_GRAPH_EXTRA_BYTES;
    use std::path::Path;

    #[test]
    fn small_checkpoint_builds_the_complete_graph() {
        let checkpoint = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../../local/models/weights/beat_this/small0.ckpt");
        if !checkpoint.is_file() {
            eprintln!("beats graph build: SKIP, {} is not seeded", checkpoint.display());
            return;
        }
        let mut weights =
            BeatsWeights::load_with_options(&checkpoint, DEFAULT_GRAPH_EXTRA_BYTES, false).unwrap();
        let graph = build_graph(&mut weights).unwrap();
        assert_eq!(weights.ctx.tensor(graph.mel).unwrap().ne, [128, 1500, 1, 1]);
        assert_eq!(weights.ctx.tensor(graph.logits).unwrap().ne, [2, 1500, 1, 1]);
        assert!(!graph.graph.nodes.is_empty());
    }
}
