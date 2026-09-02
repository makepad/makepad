//! MiniMax H3 video VAE: the ViT DECODER (t2va) and the single-frame
//! spatial ENCODER (fl2va keyframe conditioning; see the encoder section
//! further down).
//!
//! The decoder is a non-causal
//! ViT: every latent voxel is one token (dim 2048 = 32 heads x 64), 4 learned
//! register tokens + 1 zero cls token appended, 36 pre-norm blocks with
//! LayerScale residuals, rotate-half 3D rope (theta 100, 24 of 64 head
//! channels rotated per half), LayerNorm out, and a patch projection that
//! expands each token into a 4x16x16 pixel block.
//!
//! Orchestration mirrors autoencoder_kl_minimax_h3.py exactly: spatial tiling
//! (ON by default in the released checkpoint — 256px tiles, >=64px overlaps
//! widened latent-aligned, linear cross-fade blending) around every clip, and
//! the temporal chunk loop that decodes 5(+2 overlap) latent frames into 17
//! pixel frames per chunk with a 5-frame cross-fade between chunks.
//!
//! Every tile-clip ViT forward is independent (the temporal/spatial
//! cross-fades only consume its output), so all of them are gathered up front
//! and BATCHED through the 36 layers by folding the batch into the row axis
//! ((B, S, D) -> (B*S, D)) — gemms, norms, rope and the elementwise ops are
//! row-wise and batch-transparent. Attention runs per clip on row slices (see
//! decoder_vit_forward_group for why). `H3_VAE_BATCH` caps the fold
//! (default 42 clips, the whole 640x352x124 decode in one batch).
//!
//! Precision: the reference decodes under torch f16 AUTOCAST over f32
//! weights (norms explicitly in f32). Here: weights stream from the f32
//! shards converted to f16 into the device cache (each gemm sees f16
//! operands, like autocast); activations/residual stay f32 on device; norms
//! f32; blends and post_quant_conv host-side f32. The VAE explicitly uses
//! f16 accumulation while retaining f32 activations, matching the reference
//! autocast path without process-global precision state.

use crate::backend::{
    gpu_add, gpu_attention_packed, gpu_concat_rows, gpu_conv2d_planar_cached, gpu_download,
    gpu_gated_residual, gpu_gather_cols, gpu_group_norm_planar, gpu_layer_norm_mul_add,
    gpu_linear_nt_cached_with_precision, gpu_rms_norm_mul, gpu_rope_half, gpu_silu,
    gpu_slice_cols, gpu_slice_rows, gpu_swiglu_value_gate, gpu_upload, gpu_weight_cache_ensure,
    GemmPrecision, GpuLinearPart, GpuTensor,
};
use crate::h3::H3ShardedWeights;
use crate::{DiffusionError, Result};
use makepad_ai_common::quant::GGML_TYPE_F16;
use makepad_ai_loader::MlxDType;

pub const H3_VAE_PRECISION: GemmPrecision = GemmPrecision {
    f16_accumulate: true,
    f16_activations: false,
};

pub const H3_VAE_NAMESPACE: &str = "h3vae";
pub const H3_VAE_LATENT_CHANNELS: usize = 24;
pub const H3_VAE_DECODER_LAYERS: usize = 36;
pub const H3_VAE_HEADS: usize = 32;
pub const H3_VAE_HEAD_DIM: usize = 64;
pub const H3_VAE_DIM: usize = H3_VAE_HEADS * H3_VAE_HEAD_DIM; // 2048
pub const H3_VAE_FFN_INNER: usize = 4 * H3_VAE_DIM; // 8192 (SwiGLU inner)
pub const H3_VAE_REGISTER_TOKENS: usize = 4;
pub const H3_VAE_ROPE_THETA: f32 = 100.0;
pub const H3_VAE_ROT_HALF: usize = 24; // rope_dim_ratio 0.75 * 64 / 2
pub const H3_VAE_NORM_EPS: f32 = 1e-5;
pub const H3_VAE_PATCH: usize = 16; // spatial compression
pub const H3_VAE_PATCH_T: usize = 4; // temporal compression
pub const H3_VAE_CLIP_LENGTH: usize = 17;
pub const H3_VAE_TOKEN_DROP: usize = 3;
pub const H3_VAE_TOKENS_CHUNK: usize = 5; // ceil(17/4)
pub const H3_VAE_TOKEN_OVERLAP: usize = 2; // (-3) mod 5
pub const H3_VAE_FRAME_PRE_PAD: usize = 3; // (-17) mod 4
pub const H3_VAE_FRAME_OVERLAP: usize = 5; // 2*4 - 3
pub const H3_VAE_TILE: usize = 256;
pub const H3_VAE_TILE_OVERLAP: usize = 64;
/// Default `H3_VAE_BATCH`: max tile-clips folded into one batched ViT
/// forward. MEASURED on the 96GB box (640x352x124, f16acc): B=1 10.9s,
/// B=6 10.9s, B=14 11.9s, B=42 19.4s — the fold is compute-neutral (the
/// decode is bound by the per-clip composite-attention calls, ~2ms x 1512,
/// plus the one-time 5.4GB weight stream-in; batching only the gemms buys
/// nothing) and big B hits a multi-GB allocation cliff on the FF buffers.
/// 6 keeps transient VRAM ~1.5GB above the ~3GB fixed floor and stays off
/// the cliff on every tier. The real remaining lever is a batched composite
/// attention (or FA2 at head_dim 64) — flagged, not built.
pub const H3_VAE_DEFAULT_BATCH: usize = 6;

/// `H3_VAE_BATCH` env knob: cap on tile-clips per batched forward (>= 1).
fn h3_vae_batch_limit() -> usize {
    std::env::var("H3_VAE_BATCH")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|&value| value >= 1)
        .unwrap_or(H3_VAE_DEFAULT_BATCH)
}

/// Per-channel latent normalization (vae/config.json — checkpoint contract).
pub const H3_VAE_LATENTS_MEAN: [f32; 24] = [
    0.858090341091156, -0.9606591463088989, 1.0661640167236328, -0.5090325474739075,
    -0.2727581858634949, -1.3675414323806763, -0.2553254961967468, -0.26907554268836975,
    -0.5376840829849243, -0.0464097298681736, 0.6657370328903198, 0.19690127670764923,
    -0.5460608005523682, -0.4035342037677765, -0.23683024942874908, 0.25928452610969543,
    -0.30133944749832153, 0.211341992020607, -1.1206848621368408, 0.3581933379173279,
    -0.04225143790245056, 0.2604829967021942, 0.22864092886447906, 0.7056031823158264,
];
pub const H3_VAE_LATENTS_STD: [f32; 24] = [
    1.2223774194717407, 1.2767263650894165, 1.6831774711608887, 1.7549455165863037,
    1.5636216402053833, 2.194143533706665, 0.9653137922286987, 1.0569885969161987,
    0.841948926448822, 0.7729952931404114, 1.8955937623977661, 0.946841835975647,
    0.7996809482574463, 0.44988900423049927, 0.7197399735450745, 0.6936293244361877,
    2.961095094680786, 2.7694199085235596, 3.0496184825897217, 2.1088054180145264,
    3.276226282119751, 3.1627357006073, 2.2816812992095947, 2.6127843856811523,
];
pub const H3_PIXEL_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
pub const H3_PIXEL_STD: [f32; 3] = [0.229, 0.224, 0.225];

// ---------------------------------------------------------------------------
// A little (c, f, h, w) row-major volume for the host-side stitch/blend work.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Vol {
    pub c: usize,
    pub f: usize,
    pub h: usize,
    pub w: usize,
    pub data: Vec<f32>,
}

impl Vol {
    pub fn zeros(c: usize, f: usize, h: usize, w: usize) -> Self {
        Self { c, f, h, w, data: vec![0.0; c * f * h * w] }
    }

    #[inline]
    fn at(&self, c: usize, f: usize, y: usize, x: usize) -> f32 {
        self.data[((c * self.f + f) * self.h + y) * self.w + x]
    }

    #[inline]
    fn set(&mut self, c: usize, f: usize, y: usize, x: usize, value: f32) {
        self.data[((c * self.f + f) * self.h + y) * self.w + x] = value;
    }

    /// Frames [start, start+len) of this volume.
    fn slice_frames(&self, start: usize, len: usize) -> Vol {
        let len = len.min(self.f.saturating_sub(start));
        let mut out = Vol::zeros(self.c, len, self.h, self.w);
        for c in 0..self.c {
            for f in 0..len {
                let src = ((c * self.f + start + f) * self.h) * self.w;
                let dst = ((c * len + f) * out.h) * out.w;
                out.data[dst..dst + self.h * self.w]
                    .copy_from_slice(&self.data[src..src + self.h * self.w]);
            }
        }
        out
    }
}

/// The reference `_blend`: cross-fade `extent` slots of `a`'s tail into `b`'s
/// head along `axis` (0 = frames, 1 = height, 2 = width); the result has
/// `b`'s length on that axis.
fn blend(a: &Vol, b: &Vol, extent: usize, axis: usize) -> Vol {
    let len = |v: &Vol| match axis {
        0 => v.f,
        1 => v.h,
        _ => v.w,
    };
    let extent = extent.min(len(a)).min(len(b));
    let mut out = b.clone();
    let a_len = len(a);
    for c in 0..b.c {
        for f in 0..b.f {
            for y in 0..b.h {
                for x in 0..b.w {
                    let pos = match axis {
                        0 => f,
                        1 => y,
                        _ => x,
                    };
                    if pos >= extent {
                        continue;
                    }
                    let weight_b = pos as f32 / extent as f32;
                    let weight_a = 1.0 - weight_b;
                    let a_value = match axis {
                        0 => a.at(c, a_len - extent + pos, y, x),
                        1 => a.at(c, f, a_len - extent + pos, x),
                        _ => a.at(c, f, y, a_len - extent + pos),
                    };
                    let value = a_value * weight_a + out.at(c, f, y, x) * weight_b;
                    out.set(c, f, y, x, value);
                }
            }
        }
    }
    out
}

/// The reference `_split_tiles`: tile starts, lengths and overlaps covering
/// `length` pixels with `tile_size` tiles and >= `min_overlap` overlaps,
/// slack distributed round-robin in whole 16px (latent-aligned) steps.
pub fn h3_vae_split_tiles(length: usize, tile_size: usize, min_overlap: usize)
    -> (Vec<usize>, Vec<usize>, Vec<usize>)
{
    if tile_size >= length {
        return (vec![0], vec![length], vec![]);
    }
    let mut num_tiles = length.div_ceil(tile_size);
    loop {
        let coverage =
            (tile_size * num_tiles) as i64 - (min_overlap * (num_tiles - 1)) as i64 - length as i64;
        if coverage >= 0 {
            break;
        }
        num_tiles += 1;
    }
    let mut overlaps = vec![min_overlap; num_tiles - 1];
    let remaining = tile_size * num_tiles - overlaps.iter().sum::<usize>() - length;
    for i in 0..remaining / H3_VAE_PATCH {
        overlaps[i % (num_tiles - 1)] += H3_VAE_PATCH;
    }
    let mut starts = vec![0usize];
    for i in 0..num_tiles - 1 {
        starts.push(starts[i] + tile_size - overlaps[i]);
    }
    (starts, vec![tile_size; num_tiles], overlaps)
}

// ---------------------------------------------------------------------------
// Prepared host weights.
// ---------------------------------------------------------------------------

pub struct H3VaeDecoderPrepared {
    post_quant_w: Vec<f32>, // (24, 24)
    post_quant_b: Vec<f32>,
    /// (5, 2048): the 4 learned register tokens + the zero cls row.
    register_block: Vec<f32>,
    proj_in_bias: Vec<f32>,
    proj_out_bias: Vec<f32>,
    norm1_w: Vec<Vec<f32>>,
    norm2_w: Vec<Vec<f32>>,
    scale1: Vec<Vec<f32>>,
    scale2: Vec<Vec<f32>>,
    qkv_bias: Vec<Vec<f32>>, // concat [q|k|v] (6144)
    to_out_bias: Vec<Vec<f32>>,
    ff0_bias: Vec<Vec<f32>>, // (16384)
    ff2_bias: Vec<Vec<f32>>, // (2048)
    norm_out_w: Vec<f32>,
    norm_out_b: Vec<f32>,
    qk_norm_ones: Vec<f32>, // 64 ones: norm_q/norm_k are weightless RMS norms
    rope_inv_freq: [f32; 8],
}

fn host_named(weights: &H3ShardedWeights, name: &str, len: usize) -> Result<Vec<f32>> {
    let values = weights.tensor_f32(name)?;
    if values.len() != len {
        return Err(DiffusionError::model(format!(
            "h3 vae tensor '{name}' expected {len} values, got {}",
            values.len()
        )));
    }
    Ok(values)
}

impl H3VaeDecoderPrepared {
    pub fn prepare(weights: &H3ShardedWeights) -> Result<Self> {
        let dim = H3_VAE_DIM;
        let mut norm1_w = Vec::new();
        let mut norm2_w = Vec::new();
        let mut scale1 = Vec::new();
        let mut scale2 = Vec::new();
        let mut qkv_bias = Vec::new();
        let mut to_out_bias = Vec::new();
        let mut ff0_bias = Vec::new();
        let mut ff2_bias = Vec::new();
        for layer in 0..H3_VAE_DECODER_LAYERS {
            let prefix = format!("decoder.transformer_blocks.{layer}");
            norm1_w.push(host_named(weights, &format!("{prefix}.norm1.weight"), dim)?);
            norm2_w.push(host_named(weights, &format!("{prefix}.norm2.weight"), dim)?);
            scale1.push(host_named(weights, &format!("{prefix}.scale1"), dim)?);
            scale2.push(host_named(weights, &format!("{prefix}.scale2"), dim)?);
            let mut bias = host_named(weights, &format!("{prefix}.attn.to_q.bias"), dim)?;
            bias.extend(host_named(weights, &format!("{prefix}.attn.to_k.bias"), dim)?);
            bias.extend(host_named(weights, &format!("{prefix}.attn.to_v.bias"), dim)?);
            qkv_bias.push(bias);
            to_out_bias.push(host_named(weights, &format!("{prefix}.attn.to_out.0.bias"), dim)?);
            ff0_bias.push(host_named(
                weights,
                &format!("{prefix}.ff.net.0.proj.bias"),
                2 * H3_VAE_FFN_INNER,
            )?);
            ff2_bias.push(host_named(weights, &format!("{prefix}.ff.net.2.bias"), dim)?);
        }
        let registers = host_named(
            weights,
            "decoder.register_tokens",
            H3_VAE_REGISTER_TOKENS * dim,
        )?;
        let mut register_block = registers;
        register_block.extend(std::iter::repeat(0.0f32).take(dim)); // zero cls token
        let mut rope_inv_freq = [0.0f32; 8];
        for (j, value) in rope_inv_freq.iter_mut().enumerate() {
            // inv_freq = theta^(-arange(0, 1, 2*3/48)) = theta^(-j/8)
            *value = H3_VAE_ROPE_THETA.powf(-(j as f32) / 8.0);
        }
        Ok(Self {
            post_quant_w: host_named(
                weights,
                "post_quant_conv.weight",
                H3_VAE_LATENT_CHANNELS * H3_VAE_LATENT_CHANNELS,
            )?,
            post_quant_b: host_named(weights, "post_quant_conv.bias", H3_VAE_LATENT_CHANNELS)?,
            register_block,
            proj_in_bias: host_named(weights, "decoder.proj_in.bias", dim)?,
            proj_out_bias: host_named(
                weights,
                "decoder.proj_out.bias",
                3 * H3_VAE_PATCH_T * H3_VAE_PATCH * H3_VAE_PATCH,
            )?,
            norm1_w,
            norm2_w,
            scale1,
            scale2,
            qkv_bias,
            to_out_bias,
            ff0_bias,
            ff2_bias,
            norm_out_w: host_named(weights, "decoder.norm_out.weight", dim)?,
            norm_out_b: host_named(weights, "decoder.norm_out.bias", dim)?,
            qk_norm_ones: vec![1.0; H3_VAE_HEAD_DIM],
            rope_inv_freq,
        })
    }
}

/// Stream one f32 (or f16/bf16) shard tensor into the device cache as f16.
fn ensure_linear<'a>(
    weights: &H3ShardedWeights,
    name: &'a str,
    n: usize,
    k: usize,
    m: usize,
) -> Result<GpuLinearPart<'a>> {
    let want_a16 = H3_VAE_PRECISION.f16_accumulate && m > 1;
    let (dtype, _shape) = weights.tensor_dtype_shape(name)?;
    gpu_weight_cache_ensure(H3_VAE_NAMESPACE, name, GGML_TYPE_F16, n, k, want_a16, || {
        let raw = weights.tensor_bytes(name).map_err(|err| err.to_string())?;
        let mut out = vec![0u8; n * k * 2];
        match dtype {
            MlxDType::F16 => out.copy_from_slice(&raw),
            MlxDType::F32 => {
                for (i, chunk) in raw.chunks_exact(4).enumerate() {
                    let value =
                        f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                    out[i * 2..i * 2 + 2]
                        .copy_from_slice(&makepad_ai_common::quant::f32_to_f16(value).to_le_bytes());
                }
            }
            MlxDType::BF16 => {
                for (i, chunk) in raw.chunks_exact(2).enumerate() {
                    let word = u16::from_le_bytes([chunk[0], chunk[1]]);
                    let value = f32::from_bits((word as u32) << 16);
                    out[i * 2..i * 2 + 2]
                        .copy_from_slice(&makepad_ai_common::quant::f32_to_f16(value).to_le_bytes());
                }
            }
            other => return Err(format!("h3 vae tensor '{name}': unsupported dtype {other:?}")),
        }
        Ok(out)
    })
    .map_err(DiffusionError::model)?;
    Ok(GpuLinearPart { bt_ggml_type: GGML_TYPE_F16, n, cache_key: name, bytes: &[] })
}

fn linear_cached(
    weights: &H3ShardedWeights,
    x: &GpuTensor,
    name: &str,
    n: usize,
    bias: &[f32],
) -> Result<GpuTensor> {
    let part = ensure_linear(weights, name, n, x.cols(), x.rows())?;
    let parts = [part];
    gpu_linear_nt_cached_with_precision(x, H3_VAE_NAMESPACE, &parts, bias, H3_VAE_PRECISION)
        .map_err(DiffusionError::model)
}

// ---------------------------------------------------------------------------
// The batched ViT forward: B same-geometry latent tiles (24, nf, th, tw) ->
// B pixel tiles (3, nf*4, th*16, tw*16).
//
// The batch folds into the ROW axis ((B, S, D) -> (B*S, D)): every gemm,
// norm, rope and elementwise op in the block is row-wise, so folding changes
// nothing about the per-clip math while giving cublas full-size gemms.
// ATTENTION stays per clip: the composite attention path is one cublas
// strided-batched gemm pair whose batch stride is head_dim WITHIN a row
// (lda = hidden), so folding B clips into the head count would need a
// (S, B*dim) column-blocked layout round-trip per layer AND a B*heads*S^2
// scores buffer (~9.3 GB f32 + ~4.7 GB f16 probs at B=42, S=1797). The
// per-clip loop feeds the exact same (S, 2048) attention call as the
// sequential decoder, keeps scores transient at one clip (~620 MB), and
// attention is <10% of decode gemm FLOPs.
// ---------------------------------------------------------------------------

/// Concat MANY row blocks in order (gpu_concat_rows is two-tensor): pairwise
/// tree, O(N log N) copied bytes instead of a running fold's O(N^2).
fn concat_rows_all(mut parts: Vec<GpuTensor>) -> Result<GpuTensor> {
    if parts.is_empty() {
        return Err(DiffusionError::model("concat_rows_all: no parts"));
    }
    while parts.len() > 1 {
        let mut next = Vec::with_capacity(parts.len().div_ceil(2));
        let mut iter = parts.into_iter();
        while let Some(a) = iter.next() {
            match iter.next() {
                Some(b) => next.push(gpu_concat_rows(&a, &b).map_err(DiffusionError::model)?),
                None => next.push(a),
            }
        }
        parts = next;
    }
    Ok(parts.pop().expect("concat_rows_all: reduced to one"))
}

/// Batch plan over tile geometries: indices grouped by identical (f, h, w) —
/// only same-geometry clips share a rope table and fold into one (B*S, D)
/// tensor — each group chunked to at most `limit` clips, original order kept
/// within groups. (In practice every tile of a decode has one geometry:
/// spatial tiles are always full 256x256 and every temporal chunk takes
/// exactly 5+2 latent frames.)
fn batch_groups(geoms: &[(usize, usize, usize)], limit: usize) -> Vec<Vec<usize>> {
    let limit = limit.max(1);
    let mut groups: Vec<((usize, usize, usize), Vec<usize>)> = Vec::new();
    for (index, geom) in geoms.iter().enumerate() {
        match groups.iter_mut().find(|(seen, _)| seen == geom) {
            Some((_, indices)) => indices.push(index),
            None => groups.push((*geom, vec![index])),
        }
    }
    groups
        .into_iter()
        .flat_map(|(_, indices)| {
            indices
                .chunks(limit)
                .map(|chunk| chunk.to_vec())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Mid-decode control for service callers: per-ViT-batch-group progress plus
/// cooperative cancellation checked between groups (a single batched group
/// forward, ~0.5-1s warm, is the granularity floor).
pub struct H3VaeCtrl<'a> {
    /// `(group, total_groups)` at the START of each batched ViT group.
    pub on_group: &'a mut dyn FnMut(usize, usize),
    /// Return true to abort with [`DiffusionError::Cancelled`].
    pub cancel: Option<&'a (dyn Fn() -> bool + 'a)>,
}

/// Decode every tile-clip, batching same-geometry clips through the ViT in
/// groups of at most `H3_VAE_BATCH`; outputs line up with `tiles`.
fn decoder_vit_forward_batch(
    weights: &H3ShardedWeights,
    prepared: &H3VaeDecoderPrepared,
    tiles: &[Vol],
    mut ctrl: Option<&mut H3VaeCtrl>,
) -> Result<Vec<Vol>> {
    let geoms: Vec<(usize, usize, usize)> =
        tiles.iter().map(|tile| (tile.f, tile.h, tile.w)).collect();
    let mut outputs: Vec<Option<Vol>> = tiles.iter().map(|_| None).collect();
    let groups = batch_groups(&geoms, h3_vae_batch_limit());
    let total_groups = groups.len();
    for (group_index, group) in groups.into_iter().enumerate() {
        if let Some(ctrl) = ctrl.as_deref_mut() {
            if ctrl.cancel.map_or(false, |cancelled| cancelled()) {
                return Err(DiffusionError::Cancelled);
            }
            (ctrl.on_group)(group_index + 1, total_groups);
        }
        let group_tiles: Vec<&Vol> = group.iter().map(|&index| &tiles[index]).collect();
        let decoded = decoder_vit_forward_group(weights, prepared, &group_tiles)?;
        for (&index, out) in group.iter().zip(decoded) {
            outputs[index] = Some(out);
        }
    }
    Ok(outputs
        .into_iter()
        .map(|out| out.expect("batch_groups covers every tile"))
        .collect())
}

fn decoder_vit_forward_group(
    weights: &H3ShardedWeights,
    prepared: &H3VaeDecoderPrepared,
    tiles: &[&Vol], // post_quant_conv already applied; c = 24, same (f, h, w)
) -> Result<Vec<Vol>> {
    let batch = tiles.len();
    let (nf, th, tw) = (tiles[0].f, tiles[0].h, tiles[0].w);
    debug_assert!(tiles
        .iter()
        .all(|tile| tile.f == nf && tile.h == th && tile.w == tw));
    let num_patches = nf * th * tw;
    let seq = num_patches + H3_VAE_REGISTER_TOKENS + 1;
    let dim = H3_VAE_DIM;

    // Token rows for every clip (t-major, h, w): (batch*num_patches, 24).
    let mut rows = vec![0.0f32; batch * num_patches * H3_VAE_LATENT_CHANNELS];
    for (clip, tile) in tiles.iter().enumerate() {
        for f in 0..nf {
            for y in 0..th {
                for x in 0..tw {
                    let row = clip * num_patches + (f * th + y) * tw + x;
                    for c in 0..H3_VAE_LATENT_CHANNELS {
                        rows[row * H3_VAE_LATENT_CHANNELS + c] = tile.at(c, f, y, x);
                    }
                }
            }
        }
    }
    let tokens_in = gpu_upload(&rows, batch * num_patches, H3_VAE_LATENT_CHANNELS)
        .map_err(DiffusionError::model)?;
    drop(rows);
    let patches = linear_cached(
        weights,
        &tokens_in,
        "decoder.proj_in.weight",
        dim,
        &prepared.proj_in_bias,
    )?;
    drop(tokens_in);
    let registers = gpu_upload(
        &prepared.register_block,
        H3_VAE_REGISTER_TOKENS + 1,
        dim,
    )
    .map_err(DiffusionError::model)?;
    // Clip-contiguous hidden rows — [clip: patches.., registers, cls] * batch
    // — so attention can slice whole clips as contiguous row ranges.
    let mut hidden = if batch == 1 {
        gpu_concat_rows(&patches, &registers).map_err(DiffusionError::model)?
    } else {
        let mut clips = Vec::with_capacity(batch);
        for clip in 0..batch {
            let patch_rows = gpu_slice_rows(&patches, clip * num_patches, num_patches)
                .map_err(DiffusionError::model)?;
            clips.push(gpu_concat_rows(&patch_rows, &registers).map_err(DiffusionError::model)?);
        }
        concat_rows_all(clips)?
    };
    drop((patches, registers));

    // Rope tables (seq, 24): axis grids 2*((i+0.5)/size)-1, angles
    // 2*pi*pos*inv_freq, layout [t*8 | h*8 | w*8]; registers/cls at 0.
    // Identical for every clip in the group (same tile geometry), repeated
    // `batch` times to line up with the folded rows — gpu_rope_half applies
    // its table strictly per row.
    let mut cos = vec![0.0f32; seq * H3_VAE_ROT_HALF];
    let mut sin = vec![0.0f32; seq * H3_VAE_ROT_HALF];
    let two_pi = 2.0 * std::f32::consts::PI;
    for f in 0..nf {
        let t_pos = 2.0 * ((f as f32 + 0.5) / nf as f32) - 1.0;
        for y in 0..th {
            let h_pos = 2.0 * ((y as f32 + 0.5) / th as f32) - 1.0;
            for x in 0..tw {
                let w_pos = 2.0 * ((x as f32 + 0.5) / tw as f32) - 1.0;
                let row = (f * th + y) * tw + x;
                for (axis, pos) in [t_pos, h_pos, w_pos].iter().enumerate() {
                    for j in 0..8 {
                        let angle = two_pi * pos * prepared.rope_inv_freq[j];
                        let col = axis * 8 + j;
                        cos[row * H3_VAE_ROT_HALF + col] = angle.cos();
                        sin[row * H3_VAE_ROT_HALF + col] = angle.sin();
                    }
                }
            }
        }
    }
    for row in num_patches..seq {
        for col in 0..H3_VAE_ROT_HALF {
            cos[row * H3_VAE_ROT_HALF + col] = 1.0;
            sin[row * H3_VAE_ROT_HALF + col] = 0.0;
        }
    }
    let (cos, sin) = if batch == 1 {
        (cos, sin)
    } else {
        let mut cos_b = Vec::with_capacity(batch * cos.len());
        let mut sin_b = Vec::with_capacity(batch * sin.len());
        for _ in 0..batch {
            cos_b.extend_from_slice(&cos);
            sin_b.extend_from_slice(&sin);
        }
        (cos_b, sin_b)
    };
    let rope_cos =
        gpu_upload(&cos, batch * seq, H3_VAE_ROT_HALF).map_err(DiffusionError::model)?;
    let rope_sin =
        gpu_upload(&sin, batch * seq, H3_VAE_ROT_HALF).map_err(DiffusionError::model)?;

    let scale = 1.0 / (H3_VAE_HEAD_DIM as f32).sqrt();
    for layer in 0..H3_VAE_DECODER_LAYERS {
        let prefix = format!("decoder.transformer_blocks.{layer}");

        let normed = gpu_rms_norm_mul(
            &hidden,
            dim,
            H3_VAE_NAMESPACE,
            &format!("{prefix}.norm1"),
            &prepared.norm1_w[layer],
            H3_VAE_NORM_EPS,
        )
        .map_err(DiffusionError::model)?;
        let q_name = format!("{prefix}.attn.to_q.weight");
        let k_name = format!("{prefix}.attn.to_k.weight");
        let v_name = format!("{prefix}.attn.to_v.weight");
        let parts = [
            ensure_linear(weights, &q_name, dim, dim, batch * seq)?,
            ensure_linear(weights, &k_name, dim, dim, batch * seq)?,
            ensure_linear(weights, &v_name, dim, dim, batch * seq)?,
        ];
        let qkv = gpu_linear_nt_cached_with_precision(
            &normed,
            H3_VAE_NAMESPACE,
            &parts,
            &prepared.qkv_bias[layer],
            H3_VAE_PRECISION,
        )
        .map_err(DiffusionError::model)?;
        drop(normed);
        let q = gpu_slice_cols(&qkv, 0, dim).map_err(DiffusionError::model)?;
        let k = gpu_slice_cols(&qkv, dim, dim).map_err(DiffusionError::model)?;
        let v = gpu_slice_cols(&qkv, dim * 2, dim).map_err(DiffusionError::model)?;
        drop(qkv);
        // Weightless per-head RMS norms (f32, like the reference's .float()).
        let q = gpu_rms_norm_mul(
            &q,
            H3_VAE_HEAD_DIM,
            H3_VAE_NAMESPACE,
            "qk_norm_ones",
            &prepared.qk_norm_ones,
            H3_VAE_NORM_EPS,
        )
        .map_err(DiffusionError::model)?;
        let k = gpu_rms_norm_mul(
            &k,
            H3_VAE_HEAD_DIM,
            H3_VAE_NAMESPACE,
            "qk_norm_ones",
            &prepared.qk_norm_ones,
            H3_VAE_NORM_EPS,
        )
        .map_err(DiffusionError::model)?;
        let q = gpu_rope_half(&q, H3_VAE_HEADS, H3_VAE_ROT_HALF, &rope_cos, &rope_sin)
            .map_err(DiffusionError::model)?;
        let k = gpu_rope_half(&k, H3_VAE_HEADS, H3_VAE_ROT_HALF, &rope_cos, &rope_sin)
            .map_err(DiffusionError::model)?;
        // head_dim 64 -> the composite (cublas + softmax) attention path.
        // Per clip on row slices (see the module comment above): every clip
        // sees the exact (S, 2048) call the sequential decoder made.
        let attn = if batch == 1 {
            gpu_attention_packed(&q, &k, &v, H3_VAE_HEADS, scale)
                .map_err(DiffusionError::model)?
        } else {
            let mut parts = Vec::with_capacity(batch);
            for clip in 0..batch {
                let q_clip =
                    gpu_slice_rows(&q, clip * seq, seq).map_err(DiffusionError::model)?;
                let k_clip =
                    gpu_slice_rows(&k, clip * seq, seq).map_err(DiffusionError::model)?;
                let v_clip =
                    gpu_slice_rows(&v, clip * seq, seq).map_err(DiffusionError::model)?;
                parts.push(
                    gpu_attention_packed(&q_clip, &k_clip, &v_clip, H3_VAE_HEADS, scale)
                        .map_err(DiffusionError::model)?,
                );
            }
            concat_rows_all(parts)?
        };
        drop((q, k, v));
        let attn = linear_cached(
            weights,
            &attn,
            &format!("{prefix}.attn.to_out.0.weight"),
            dim,
            &prepared.to_out_bias[layer],
        )?;
        hidden = gpu_gated_residual(&hidden, &attn, &prepared.scale1[layer])
            .map_err(DiffusionError::model)?;
        drop(attn);

        let normed = gpu_rms_norm_mul(
            &hidden,
            dim,
            H3_VAE_NAMESPACE,
            &format!("{prefix}.norm2"),
            &prepared.norm2_w[layer],
            H3_VAE_NORM_EPS,
        )
        .map_err(DiffusionError::model)?;
        let gateval = linear_cached(
            weights,
            &normed,
            &format!("{prefix}.ff.net.0.proj.weight"),
            2 * H3_VAE_FFN_INNER,
            &prepared.ff0_bias[layer],
        )?;
        drop(normed);
        let ff = gpu_swiglu_value_gate(&gateval).map_err(DiffusionError::model)?;
        drop(gateval);
        let ff = linear_cached(
            weights,
            &ff,
            &format!("{prefix}.ff.net.2.weight"),
            dim,
            &prepared.ff2_bias[layer],
        )?;
        hidden = gpu_gated_residual(&hidden, &ff, &prepared.scale2[layer])
            .map_err(DiffusionError::model)?;
        drop(ff);
    }

    let hidden = gpu_layer_norm_mul_add(
        &hidden,
        &prepared.norm_out_w,
        &prepared.norm_out_b,
        H3_VAE_NORM_EPS,
    )
    .map_err(DiffusionError::model)?;
    let out_cols = 3 * H3_VAE_PATCH_T * H3_VAE_PATCH * H3_VAE_PATCH; // 3072
    let projected = linear_cached(
        weights,
        &hidden,
        "decoder.proj_out.weight",
        out_cols,
        &prepared.proj_out_bias,
    )?;
    drop(hidden);
    let host = gpu_download(&projected).map_err(DiffusionError::model)?;
    drop(projected);

    // Unpatchify per clip (register/cls rows dropped): row (f, y, x) holds
    // (c, pt, py, px) c-major -> (3, nf*4, th*16, tw*16).
    let (pt, ps) = (H3_VAE_PATCH_T, H3_VAE_PATCH);
    let mut outputs = Vec::with_capacity(batch);
    for clip in 0..batch {
        let mut out = Vol::zeros(3, nf * pt, th * ps, tw * ps);
        for f in 0..nf {
            for y in 0..th {
                for x in 0..tw {
                    let row = clip * seq + (f * th + y) * tw + x;
                    let base = row * out_cols;
                    for c in 0..3 {
                        for tt in 0..pt {
                            for py in 0..ps {
                                for px in 0..ps {
                                    let value = host
                                        [base + ((c * pt + tt) * ps + py) * ps + px];
                                    out.set(c, f * pt + tt, y * ps + py, x * ps + px, value);
                                }
                            }
                        }
                    }
                }
            }
        }
        outputs.push(out);
    }
    Ok(outputs)
}

/// The spatial tile plan of one clip (pixel-space starts/lengths/overlaps;
/// identical for every temporal chunk of a decode).
struct ClipTiling {
    y_starts: Vec<usize>,
    y_lens: Vec<usize>,
    y_overlaps: Vec<usize>,
    x_starts: Vec<usize>,
    x_lens: Vec<usize>,
    x_overlaps: Vec<usize>,
}

impl ClipTiling {
    fn plan(height: usize, width: usize) -> Self {
        let (y_starts, y_lens, y_overlaps) =
            h3_vae_split_tiles(height, H3_VAE_TILE, H3_VAE_TILE_OVERLAP);
        let (x_starts, x_lens, x_overlaps) =
            h3_vae_split_tiles(width, H3_VAE_TILE, H3_VAE_TILE_OVERLAP);
        Self { y_starts, y_lens, y_overlaps, x_starts, x_lens, x_overlaps }
    }

    fn num_tiles(&self) -> usize {
        self.y_starts.len() * self.x_starts.len()
    }
}

/// Cut one clip's latents into the plan's spatial tiles, row-major (y, x).
fn split_clip_tiles(clip: &Vol, tiling: &ClipTiling) -> Vec<Vol> {
    let ratio = H3_VAE_PATCH;
    let mut tiles = Vec::with_capacity(tiling.num_tiles());
    for (y0, ylen) in tiling.y_starts.iter().zip(tiling.y_lens.iter()) {
        for (x0, xlen) in tiling.x_starts.iter().zip(tiling.x_lens.iter()) {
            let (ly, lx) = (y0 / ratio, x0 / ratio);
            let (lyl, lxl) = (ylen / ratio, xlen / ratio);
            let mut tile = Vol::zeros(clip.c, clip.f, lyl, lxl);
            for c in 0..clip.c {
                for f in 0..clip.f {
                    for y in 0..lyl {
                        for x in 0..lxl {
                            tile.set(c, f, y, x, clip.at(c, f, ly + y, lx + x));
                        }
                    }
                }
            }
            tiles.push(tile);
        }
    }
    tiles
}

/// Stitch one clip's decoded pixel tiles (row-major (y, x), mirrors
/// `_stitch_tiles`): blend against the ORIGINAL neighbours, trim the kept
/// extents, concat.
fn stitch_clip_tiles(tiling: &ClipTiling, flat_tiles: Vec<Vol>) -> Vol {
    stitch_tiles_grid(
        tiling.y_starts.len(),
        tiling.x_starts.len(),
        &tiling.y_overlaps,
        &tiling.x_overlaps,
        flat_tiles,
    )
}

/// The reference `_stitch_tiles` over an ny x nx row-major tile grid, with
/// the overlaps given in the TILES' OWN coordinate space (pixel overlaps for
/// decoded tiles, latent overlaps for encoded moments). Channel-generic.
fn stitch_tiles_grid(
    ny: usize,
    nx: usize,
    y_overlaps: &[usize],
    x_overlaps: &[usize],
    flat_tiles: Vec<Vol>,
) -> Vol {
    let out_c = flat_tiles[0].c;
    let mut tiles: Vec<Vec<Vol>> = Vec::with_capacity(ny);
    let mut flat = flat_tiles.into_iter();
    for _ in 0..ny {
        tiles.push(flat.by_ref().take(nx).collect());
    }

    let out_f = tiles[0][0].f;
    let mut result_rows: Vec<Vec<Vol>> = Vec::with_capacity(tiles.len());
    for i in 0..tiles.len() {
        let mut result_row = Vec::with_capacity(tiles[i].len());
        for j in 0..tiles[i].len() {
            let mut tile = tiles[i][j].clone();
            if i > 0 {
                tile = blend(&tiles[i - 1][j], &tile, y_overlaps[i - 1], 1);
            }
            if j > 0 {
                tile = blend(&tiles[i][j - 1], &tile, x_overlaps[j - 1], 2);
            }
            if i < tiles.len() - 1 {
                let keep = tile.h - y_overlaps[i];
                let mut trimmed = Vol::zeros(tile.c, tile.f, keep, tile.w);
                for c in 0..tile.c {
                    for f in 0..tile.f {
                        for y in 0..keep {
                            for x in 0..tile.w {
                                trimmed.set(c, f, y, x, tile.at(c, f, y, x));
                            }
                        }
                    }
                }
                tile = trimmed;
            }
            if j < tiles[i].len() - 1 {
                let keep = tile.w - x_overlaps[j];
                let mut trimmed = Vol::zeros(tile.c, tile.f, tile.h, keep);
                for c in 0..tile.c {
                    for f in 0..tile.f {
                        for y in 0..tile.h {
                            for x in 0..keep {
                                trimmed.set(c, f, y, x, tile.at(c, f, y, x));
                            }
                        }
                    }
                }
                tile = trimmed;
            }
            result_row.push(tile);
        }
        result_rows.push(result_row);
    }
    // Concat each row along width, then rows along height.
    let mut stitched_rows: Vec<Vol> = Vec::with_capacity(result_rows.len());
    for row in &result_rows {
        let total_w: usize = row.iter().map(|tile| tile.w).sum();
        let mut joined = Vol::zeros(out_c, out_f, row[0].h, total_w);
        let mut x_off = 0;
        for tile in row {
            for c in 0..out_c {
                for f in 0..out_f {
                    for y in 0..tile.h {
                        for x in 0..tile.w {
                            joined.set(c, f, y, x_off + x, tile.at(c, f, y, x));
                        }
                    }
                }
            }
            x_off += tile.w;
        }
        stitched_rows.push(joined);
    }
    let total_h: usize = stitched_rows.iter().map(|row| row.h).sum();
    let mut out = Vol::zeros(out_c, out_f, total_h, stitched_rows[0].w);
    let mut y_off = 0;
    for row in &stitched_rows {
        for c in 0..out_c {
            for f in 0..out_f {
                for y in 0..row.h {
                    for x in 0..row.w {
                        out.set(c, f, y_off + y, x, row.at(c, f, y, x));
                    }
                }
            }
        }
        y_off += row.h;
    }
    out
}

/// The temporal chunk plan of `_decode` for `num_tokens_in` latent frames:
/// (pad_tokens, num_chunks).
pub fn h3_vae_chunk_plan(num_latent_frames: usize) -> (usize, usize) {
    let num_tokens = num_latent_frames + H3_VAE_TOKEN_DROP;
    let pad_tokens = (H3_VAE_TOKENS_CHUNK - num_tokens % H3_VAE_TOKENS_CHUNK) % H3_VAE_TOKENS_CHUNK;
    let num_chunks = (num_tokens + pad_tokens) / H3_VAE_TOKENS_CHUNK - 1;
    (pad_tokens, num_chunks)
}

pub struct H3VaeDecodeRun {
    /// ImageNet-normalized RGB, (3, frames, height, width) row-major — the
    /// raw decoder output before the pixel-space revert.
    pub raw: Vol,
}

/// Decode DENORMALIZED latents (24, T, lh, lw) into raw ImageNet-normalized
/// frames. Mirrors `_decode`: post_quant_conv, temporal chunks of 5+2 latent
/// frames -> 28 pixel frames, per-chunk frame slicing and 5-frame cross-fade.
pub fn h3_vae_decode(
    weights: &H3ShardedWeights,
    prepared: &H3VaeDecoderPrepared,
    latents: &[f32],
    num_latent_frames: usize,
    latent_height: usize,
    latent_width: usize,
) -> Result<H3VaeDecodeRun> {
    h3_vae_decode_ctrl(
        weights,
        prepared,
        latents,
        num_latent_frames,
        latent_height,
        latent_width,
        None,
    )
}

/// [`h3_vae_decode`] plus an optional [`H3VaeCtrl`] for per-batch-group
/// progress and cancellation.
pub fn h3_vae_decode_ctrl(
    weights: &H3ShardedWeights,
    prepared: &H3VaeDecoderPrepared,
    latents: &[f32],
    num_latent_frames: usize,
    latent_height: usize,
    latent_width: usize,
    ctrl: Option<&mut H3VaeCtrl>,
) -> Result<H3VaeDecodeRun> {
    let (c, lh, lw) = (H3_VAE_LATENT_CHANNELS, latent_height, latent_width);
    if latents.len() != c * num_latent_frames * lh * lw {
        return Err(DiffusionError::workflow(format!(
            "h3 vae decode expected {} values, got {}",
            c * num_latent_frames * lh * lw,
            latents.len()
        )));
    }

    // post_quant_conv (1x1x1) applied once to the whole latent, host f32 —
    // identical to the reference's per-tile application (per-voxel op).
    let plane = lh * lw;
    let mut z = Vol::zeros(c, num_latent_frames, lh, lw);
    for f in 0..num_latent_frames {
        for p in 0..plane {
            let mut voxel_in = [0.0f32; H3_VAE_LATENT_CHANNELS];
            for (ci, value) in voxel_in.iter_mut().enumerate() {
                *value = latents[(ci * num_latent_frames + f) * plane + p];
            }
            for co in 0..c {
                let mut sum = prepared.post_quant_b[co];
                let wrow = &prepared.post_quant_w[co * c..(co + 1) * c];
                for ci in 0..c {
                    sum += wrow[ci] * voxel_in[ci];
                }
                z.data[(co * num_latent_frames + f) * plane + p] = sum;
            }
        }
    }

    let (pad_tokens, num_chunks) = h3_vae_chunk_plan(num_latent_frames);
    let z = if pad_tokens > 0 {
        let mut padded = Vol::zeros(c, num_latent_frames + pad_tokens, lh, lw);
        for ci in 0..c {
            for f in 0..num_latent_frames + pad_tokens {
                let src = f.min(num_latent_frames - 1);
                let from = ((ci * num_latent_frames + src) * lh) * lw;
                let to = ((ci * padded.f + f) * lh) * lw;
                padded.data[to..to + plane].copy_from_slice(&z.data[from..from + plane]);
            }
        }
        padded
    } else {
        z
    };

    // Slice every temporal chunk's clip latents up front and split them into
    // spatial tiles: all tile-clip ViT forwards are independent (the temporal
    // cross-fades below only consume their outputs), so they batch through
    // the decoder in one place — the win over sequential per-tile forwards.
    let tiling = ClipTiling::plan(lh * H3_VAE_PATCH, lw * H3_VAE_PATCH);
    let tiles_per_clip = tiling.num_tiles();
    let mut all_tiles: Vec<Vol> = Vec::with_capacity(num_chunks * tiles_per_clip);
    for i in 0..num_chunks {
        let start = i * H3_VAE_TOKENS_CHUNK;
        let take = (H3_VAE_TOKENS_CHUNK + H3_VAE_TOKEN_OVERLAP).min(z.f - start);
        let mut clip_latent = Vol::zeros(c, take, lh, lw);
        for ci in 0..c {
            for f in 0..take {
                let from = ((ci * z.f + start + f) * lh) * lw;
                let to = ((ci * take + f) * lh) * lw;
                clip_latent.data[to..to + plane].copy_from_slice(&z.data[from..from + plane]);
            }
        }
        if tiles_per_clip == 1 {
            all_tiles.push(clip_latent);
        } else {
            all_tiles.extend(split_clip_tiles(&clip_latent, &tiling));
        }
    }
    let mut tile_outputs =
        decoder_vit_forward_batch(weights, prepared, &all_tiles, ctrl)?.into_iter();
    drop(all_tiles);

    let chunk_num_frames = H3_VAE_TOKENS_CHUNK * H3_VAE_PATCH_T; // 20
    let mut decoded: Vec<Vol> = Vec::new();
    let mut overlap: Option<Vol> = None;
    for _i in 0..num_chunks {
        let clip_tiles: Vec<Vol> = tile_outputs.by_ref().take(tiles_per_clip).collect();
        let clip = if tiles_per_clip == 1 {
            clip_tiles.into_iter().next().expect("one decoded tile per clip")
        } else {
            stitch_clip_tiles(&tiling, clip_tiles)
        };
        for j in 0..2usize {
            let frame_start = j * chunk_num_frames;
            if frame_start >= clip.f {
                break;
            }
            let chunk = clip.slice_frames(frame_start, chunk_num_frames);
            let chunk = chunk.slice_frames(H3_VAE_FRAME_PRE_PAD, chunk.f);
            if j == 0 {
                let chunk = match &overlap {
                    Some(prev) => blend(prev, &chunk, H3_VAE_FRAME_OVERLAP, 0),
                    None => chunk,
                };
                decoded.push(chunk);
            } else {
                overlap = Some(chunk);
            }
        }
    }
    if let Some(prev) = overlap.take() {
        decoded.push(prev);
    }

    let total_f: usize = decoded.iter().map(|part| part.f).sum();
    let (height, width) = (lh * H3_VAE_PATCH, lw * H3_VAE_PATCH);
    let mut raw = Vol::zeros(3, total_f, height, width);
    let mut f_off = 0;
    for part in &decoded {
        for c in 0..3 {
            for f in 0..part.f {
                let from = ((c * part.f + f) * height) * width;
                let to = ((c * total_f + f_off + f) * height) * width;
                raw.data[to..to + height * width]
                    .copy_from_slice(&part.data[from..from + height * width]);
            }
        }
        f_off += part.f;
    }

    // Trim the frames produced by padded latent tokens (reference formula).
    let raw = if pad_tokens > 0 {
        let intra_tail = H3_VAE_CLIP_LENGTH % H3_VAE_PATCH_T; // 1
        let num_tokens_before_pad = z.f - pad_tokens;
        let mut pad_frames = 0usize;
        for k in 0..pad_tokens {
            pad_frames += if intra_tail != 0
                && (num_tokens_before_pad + k) % H3_VAE_TOKENS_CHUNK == 0
            {
                intra_tail
            } else {
                H3_VAE_PATCH_T
            };
        }
        let keep = raw.f - pad_frames;
        raw.slice_frames(0, keep)
    } else {
        raw
    };
    Ok(H3VaeDecodeRun { raw })
}

/// Scheduler-space latents -> the VAE's input space: `z * std + mean`.
pub fn h3_vae_denormalize_latents(
    latents: &mut [f32],
    num_latent_frames: usize,
    latent_height: usize,
    latent_width: usize,
) {
    let plane = num_latent_frames * latent_height * latent_width;
    for c in 0..H3_VAE_LATENT_CHANNELS {
        let mean = H3_VAE_LATENTS_MEAN[c];
        let std = H3_VAE_LATENTS_STD[c];
        for value in &mut latents[c * plane..(c + 1) * plane] {
            *value = *value * std + mean;
        }
    }
}

/// Raw decoder output -> u8 RGB frames (frames, height, width, 3): ImageNet
/// revert + clamp to [0, 1].
pub fn h3_vae_frames_to_u8(raw: &Vol) -> Vec<u8> {
    let mut out = vec![0u8; raw.f * raw.h * raw.w * 3];
    for f in 0..raw.f {
        for y in 0..raw.h {
            for x in 0..raw.w {
                for c in 0..3 {
                    let value = raw.at(c, f, y, x) * H3_PIXEL_STD[c] + H3_PIXEL_MEAN[c];
                    let value = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
                    out[((f * raw.h + y) * raw.w + x) * 3 + c] = value;
                }
            }
        }
    }
    out
}

// ===========================================================================
// ENCODER — the causal 3D CNN's single-frame (keyframe) spatial encode, for
// the fl2va i2v conditioning path (encoders.py encode_vae_condition +
// autoencoder_kl_minimax_h3.py MiniMaxH3VideoEncoder3d).
//
// WHY THIS IS EXACTLY A 2D CNN (the temporal-slice reduction): the keyframe
// is a SINGLE pixel frame, shape (1, 3, T=1, H, W), and `num_frames == 1`
// routes straight to `_encode_clip` (no temporal chunking, no token_drop).
// Every MiniMaxH3VideoCausalConv3d with temporal kernel 3 prepends
// `temporal_padding = 2` ZERO frames, so its temporal window over the padded
// [z, z, x] extent is a single output frame — floor((3 - 3) / stride_t) + 1
// = 1 for stride_t in {1, 2}, i.e. the temporal-stride-2 downsamplers
// included — and that one window multiplies [zero, zero, real] against
// w[:, :, 0..3]. Only w[:, :, 2, :, :] ever touches real data; the other two
// slices contribute exactly 0.0. The output is again a single frame, so the
// argument holds inductively through the whole encoder. The 1x1x1 convs
// (resnet shortcuts, quant_conv) have temporal kernel 1 and no temporal pad
// (trivially 2D), and MiniMaxH3VideoGroupNorm folds T into the batch axis,
// so on one frame it is a plain 2D GroupNorm. Hence: the single-frame
// encoder == a 2D CNN over the w[:, :, 2] (k3) / w[:, :, 0] (k1) weight
// slices, which `temporal_slice_reduction_holds` verifies numerically
// against a full causal-conv3d reference.
//
// SPATIAL padding/stride on the GPU: gpu_conv2d_planar_cached is zero-pad
// stride-1 only, so the reference's reflect padding and strided downsamples
// are composed from gpu_gather_cols (one index table re-addresses all
// channel rows of a planar tensor at once):
//   * k3 reflect pad 1 conv == gather(reflect indices, (w+2)x(h+2))
//     -> conv pad 0 (same-size out; rows/cols past the valid region hold
//     partial windows) -> gather(top-left w x h crop);
//   * downsample (asymmetric right/bottom reflect pad 1 + k3 stride 2)
//     == gather(asym pad, (w+1)x(h+1)) -> conv pad 0 -> gather(every other
//     row/column of the valid region), out = floor((w - 2) / 2) + 1 per axis
//     (torch's formula; == ceil(w / 2) for the even sizes seen here).
// The stride trick computes ~4x more conv output than needed; on a single
// 640x352 keyframe (six 256px tiles) that is milliseconds and irrelevant.
// ===========================================================================

pub const H3_VAE_MOMENT_CHANNELS: usize = 2 * H3_VAE_LATENT_CHANNELS; // 48
const H3_VAE_ENC_BLOCK_OUT: [usize; 6] = [128, 256, 256, 512, 512, 1024];
const H3_VAE_ENC_SPATIAL_DOWN: [usize; 6] = [2, 2, 2, 2, 1, 1];
const H3_VAE_ENC_TEMPORAL_DOWN: [usize; 6] = [1, 2, 2, 1, 1, 1];
const H3_VAE_ENC_GROUPS: usize = 32;
const H3_VAE_ENC_GN_EPS: f32 = 1e-6;

/// One temporally-sliced encoder conv: `weight` is the 2D (co, ci, k, k)
/// slice, `name` doubles as the device weight-cache key.
struct EncConv {
    name: String,
    weight: Vec<f32>,
    bias: Vec<f32>,
    co: usize,
    k: usize,
}

struct EncNorm {
    name: String,
    gamma: Vec<f32>,
    beta: Vec<f32>,
}

struct EncResnet {
    norm1: EncNorm,
    conv1: EncConv,
    norm2: EncNorm,
    conv2: EncConv,
    /// 1x1 projection, present iff in_channels != out_channels.
    shortcut: Option<EncConv>,
}

struct EncDownBlock {
    resnets: Vec<EncResnet>,
    /// The strided conv; blocks whose downsample factors are all 1 have none.
    downsample: Option<EncConv>,
}

/// Host-prepared single-frame encoder weights (temporal slices already
/// taken); the convs stream into the device cache on first use per key.
pub struct H3VaeEncoderPrepared {
    conv_in: EncConv,
    blocks: Vec<EncDownBlock>,
    norm_out: EncNorm,
    conv_out: EncConv,
    quant_conv: EncConv,
}

fn expect_shape(weights: &H3ShardedWeights, name: &str, expected: &[usize]) -> Result<()> {
    let (_dtype, shape) = weights.tensor_dtype_shape(name)?;
    let got: Vec<usize> = shape.iter().map(|&dim| dim as usize).collect();
    if got != expected {
        return Err(DiffusionError::model(format!(
            "h3 vae encoder tensor '{name}': expected shape {expected:?}, got {got:?}"
        )));
    }
    Ok(())
}

/// Load a CausalConv3d k3 weight (co, ci, 3, 3, 3) and slice the causal
/// temporal tap w[:, :, 2, :, :] (see the reduction argument above).
fn load_enc_conv3(weights: &H3ShardedWeights, name: &str, co: usize, ci: usize) -> Result<EncConv> {
    let weight_name = format!("{name}.weight");
    expect_shape(weights, &weight_name, &[co, ci, 3, 3, 3])?;
    let w3d = host_named(weights, &weight_name, co * ci * 27)?;
    let mut weight = vec![0.0f32; co * ci * 9];
    for oc_ic in 0..co * ci {
        let src = (oc_ic * 3 + 2) * 9;
        weight[oc_ic * 9..oc_ic * 9 + 9].copy_from_slice(&w3d[src..src + 9]);
    }
    Ok(EncConv {
        name: name.to_string(),
        weight,
        bias: host_named(weights, &format!("{name}.bias"), co)?,
        co,
        k: 3,
    })
}

/// Load a 1x1(x1) conv (resnet shortcut / quant_conv): temporal kernel 1,
/// no temporal padding — the weight is already the 2D (co, ci) matrix.
fn load_enc_conv1(weights: &H3ShardedWeights, name: &str, co: usize, ci: usize) -> Result<EncConv> {
    let weight_name = format!("{name}.weight");
    expect_shape(weights, &weight_name, &[co, ci, 1, 1, 1])?;
    Ok(EncConv {
        name: name.to_string(),
        weight: host_named(weights, &weight_name, co * ci)?,
        bias: host_named(weights, &format!("{name}.bias"), co)?,
        co,
        k: 1,
    })
}

fn load_enc_norm(weights: &H3ShardedWeights, name: &str, channels: usize) -> Result<EncNorm> {
    Ok(EncNorm {
        name: name.to_string(),
        gamma: host_named(weights, &format!("{name}.weight"), channels)?,
        beta: host_named(weights, &format!("{name}.bias"), channels)?,
    })
}

impl H3VaeEncoderPrepared {
    /// Weight names follow the diffusers module tree exactly:
    /// `encoder.conv_in`, `encoder.down_blocks.N.resnets.M.{norm1,conv1,
    /// norm2,conv2,conv_shortcut}`, `encoder.down_blocks.N.downsamplers.0.
    /// conv`, `encoder.norm_out`, `encoder.conv_out`, `quant_conv`.
    pub fn prepare(weights: &H3ShardedWeights) -> Result<Self> {
        let mut blocks = Vec::with_capacity(H3_VAE_ENC_BLOCK_OUT.len());
        let mut in_channels = H3_VAE_ENC_BLOCK_OUT[0]; // conv_in output
        for (index, &out_channels) in H3_VAE_ENC_BLOCK_OUT.iter().enumerate() {
            let prefix = format!("encoder.down_blocks.{index}");
            let mut resnets = Vec::with_capacity(2);
            for layer in 0..2 {
                let res_in = if layer == 0 { in_channels } else { out_channels };
                let res = format!("{prefix}.resnets.{layer}");
                resnets.push(EncResnet {
                    norm1: load_enc_norm(weights, &format!("{res}.norm1"), res_in)?,
                    conv1: load_enc_conv3(weights, &format!("{res}.conv1"), out_channels, res_in)?,
                    norm2: load_enc_norm(weights, &format!("{res}.norm2"), out_channels)?,
                    conv2: load_enc_conv3(
                        weights,
                        &format!("{res}.conv2"),
                        out_channels,
                        out_channels,
                    )?,
                    shortcut: if res_in != out_channels {
                        Some(load_enc_conv1(
                            weights,
                            &format!("{res}.conv_shortcut"),
                            out_channels,
                            res_in,
                        )?)
                    } else {
                        None
                    },
                });
            }
            let spatial = H3_VAE_ENC_SPATIAL_DOWN[index];
            let temporal = H3_VAE_ENC_TEMPORAL_DOWN[index];
            let downsample = if spatial * temporal > 1 {
                if spatial != 2 {
                    // Downsample3d only asym-pads when spatial_stride == 2;
                    // no released config hits the other case.
                    return Err(DiffusionError::model(format!(
                        "h3 vae encoder block {index}: unsupported spatial stride {spatial}"
                    )));
                }
                Some(load_enc_conv3(
                    weights,
                    &format!("{prefix}.downsamplers.0.conv"),
                    out_channels,
                    out_channels,
                )?)
            } else {
                None
            };
            blocks.push(EncDownBlock { resnets, downsample });
            in_channels = out_channels;
        }
        Ok(Self {
            conv_in: load_enc_conv3(weights, "encoder.conv_in", H3_VAE_ENC_BLOCK_OUT[0], 3)?,
            blocks,
            norm_out: load_enc_norm(weights, "encoder.norm_out", 1024)?,
            conv_out: load_enc_conv3(weights, "encoder.conv_out", H3_VAE_MOMENT_CHANNELS, 1024)?,
            quant_conv: load_enc_conv1(
                weights,
                "quant_conv",
                H3_VAE_MOMENT_CHANNELS,
                H3_VAE_MOMENT_CHANNELS,
            )?,
        })
    }
}

// --- index tables for the gather-composed padding/stride (host u32) --------

/// PyTorch 'reflect' (no edge repeat): -1 -> 1, len -> len - 2.
#[inline]
fn reflect_index(pos: i64, len: i64) -> usize {
    debug_assert!(len >= 2 && pos > -len && pos < 2 * len - 1);
    let pos = if pos < 0 { -pos } else { pos };
    (if pos >= len { 2 * len - 2 - pos } else { pos }) as usize
}

/// Symmetric reflect pad 1 on both axes: (w, h) -> (w + 2) x (h + 2).
fn reflect_pad1_indices(w: usize, h: usize) -> Vec<u32> {
    let mut indices = Vec::with_capacity((w + 2) * (h + 2));
    for py in 0..h + 2 {
        let sy = reflect_index(py as i64 - 1, h as i64);
        for px in 0..w + 2 {
            let sx = reflect_index(px as i64 - 1, w as i64);
            indices.push((sy * w + sx) as u32);
        }
    }
    indices
}

/// The downsampler's F.pad((0, 1, 0, 1), reflect): right/bottom pad 1 only,
/// (w, h) -> (w + 1) x (h + 1).
fn asym_pad_indices(w: usize, h: usize) -> Vec<u32> {
    let mut indices = Vec::with_capacity((w + 1) * (h + 1));
    for py in 0..h + 1 {
        let sy = reflect_index(py as i64, h as i64);
        for px in 0..w + 1 {
            let sx = reflect_index(px as i64, w as i64);
            indices.push((sy * w + sx) as u32);
        }
    }
    indices
}

/// Top-left (w, h) crop out of a plane of width `src_w`.
fn crop_indices(src_w: usize, w: usize, h: usize) -> Vec<u32> {
    let mut indices = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            indices.push((y * src_w + x) as u32);
        }
    }
    indices
}

/// Every other row/column (stride 2 from origin) of a plane of width `src_w`.
fn stride2_indices(src_w: usize, out_w: usize, out_h: usize) -> Vec<u32> {
    let mut indices = Vec::with_capacity(out_w * out_h);
    for y in 0..out_h {
        for x in 0..out_w {
            indices.push((2 * y * src_w + 2 * x) as u32);
        }
    }
    indices
}

// --- GPU composition of the reference convs ---------------------------------

fn enc_conv3_reflect(conv: &EncConv, x: &GpuTensor, w: usize, h: usize) -> Result<GpuTensor> {
    debug_assert_eq!(conv.k, 3);
    let padded = gpu_gather_cols(x, &reflect_pad1_indices(w, h)).map_err(DiffusionError::model)?;
    let out = gpu_conv2d_planar_cached(
        &padded,
        w + 2,
        h + 2,
        H3_VAE_NAMESPACE,
        &conv.name,
        &conv.weight,
        &conv.bias,
        conv.co,
        3,
        3,
        0,
        0,
    )
    .map_err(DiffusionError::model)?;
    gpu_gather_cols(&out, &crop_indices(w + 2, w, h)).map_err(DiffusionError::model)
}

/// The Downsample3d spatial path: asym pad + k3 stride-2 conv; returns the
/// output and its (w, h).
fn enc_conv3_down2(
    conv: &EncConv,
    x: &GpuTensor,
    w: usize,
    h: usize,
) -> Result<(GpuTensor, usize, usize)> {
    debug_assert_eq!(conv.k, 3);
    let padded = gpu_gather_cols(x, &asym_pad_indices(w, h)).map_err(DiffusionError::model)?;
    let out = gpu_conv2d_planar_cached(
        &padded,
        w + 1,
        h + 1,
        H3_VAE_NAMESPACE,
        &conv.name,
        &conv.weight,
        &conv.bias,
        conv.co,
        3,
        3,
        0,
        0,
    )
    .map_err(DiffusionError::model)?;
    // torch: out = floor((padded - kernel) / stride) + 1 = floor((w-2)/2)+1.
    let (out_w, out_h) = ((w - 2) / 2 + 1, (h - 2) / 2 + 1);
    let sampled = gpu_gather_cols(&out, &stride2_indices(w + 1, out_w, out_h))
        .map_err(DiffusionError::model)?;
    Ok((sampled, out_w, out_h))
}

fn enc_conv1x1(conv: &EncConv, x: &GpuTensor, w: usize, h: usize) -> Result<GpuTensor> {
    debug_assert_eq!(conv.k, 1);
    gpu_conv2d_planar_cached(
        x,
        w,
        h,
        H3_VAE_NAMESPACE,
        &conv.name,
        &conv.weight,
        &conv.bias,
        conv.co,
        1,
        1,
        0,
        0,
    )
    .map_err(DiffusionError::model)
}

fn enc_group_norm(norm: &EncNorm, x: &GpuTensor, w: usize, h: usize) -> Result<GpuTensor> {
    gpu_group_norm_planar(
        x,
        w,
        h,
        H3_VAE_ENC_GROUPS,
        H3_VAE_NAMESPACE,
        &norm.name,
        &norm.gamma,
        &norm.beta,
        H3_VAE_ENC_GN_EPS,
    )
    .map_err(DiffusionError::model)
}

fn enc_resnet(resnet: &EncResnet, x: &GpuTensor, w: usize, h: usize) -> Result<GpuTensor> {
    let mut hidden = enc_group_norm(&resnet.norm1, x, w, h)?;
    hidden = gpu_silu(&hidden).map_err(DiffusionError::model)?;
    hidden = enc_conv3_reflect(&resnet.conv1, &hidden, w, h)?;
    hidden = enc_group_norm(&resnet.norm2, &hidden, w, h)?;
    hidden = gpu_silu(&hidden).map_err(DiffusionError::model)?;
    hidden = enc_conv3_reflect(&resnet.conv2, &hidden, w, h)?;
    match &resnet.shortcut {
        Some(shortcut) => {
            let residual = enc_conv1x1(shortcut, x, w, h)?;
            gpu_add(&residual, &hidden).map_err(DiffusionError::model)
        }
        None => gpu_add(x, &hidden).map_err(DiffusionError::model),
    }
}

/// Encode ONE pixel tile (3, h, w planar, ImageNet-normalized) through the
/// spatial encoder + quant_conv: moments Vol (48, 1, h/16, w/16).
fn encode_tile(
    prepared: &H3VaeEncoderPrepared,
    pixels: &[f32],
    w: usize,
    h: usize,
) -> Result<Vol> {
    let mut x = gpu_upload(pixels, 3, w * h).map_err(DiffusionError::model)?;
    let (mut cur_w, mut cur_h) = (w, h);
    x = enc_conv3_reflect(&prepared.conv_in, &x, cur_w, cur_h)?;
    for block in &prepared.blocks {
        for resnet in &block.resnets {
            x = enc_resnet(resnet, &x, cur_w, cur_h)?;
        }
        if let Some(down) = &block.downsample {
            let (next, next_w, next_h) = enc_conv3_down2(down, &x, cur_w, cur_h)?;
            x = next;
            cur_w = next_w;
            cur_h = next_h;
        }
    }
    x = enc_group_norm(&prepared.norm_out, &x, cur_w, cur_h)?;
    x = gpu_silu(&x).map_err(DiffusionError::model)?;
    x = enc_conv3_reflect(&prepared.conv_out, &x, cur_w, cur_h)?;
    x = enc_conv1x1(&prepared.quant_conv, &x, cur_w, cur_h)?;
    let data = gpu_download(&x).map_err(DiffusionError::model)?;
    Ok(Vol { c: H3_VAE_MOMENT_CHANNELS, f: 1, h: cur_h, w: cur_w, data })
}

/// u8 interleaved RGB canvas (h, w, 3) -> the VAE's pixel convention:
/// f32 / 255, ImageNet-normalized, planar (3, h, w).
pub fn h3_vae_normalize_canvas(canvas_rgb: &[u8], width: usize, height: usize) -> Vec<f32> {
    assert_eq!(canvas_rgb.len(), width * height * 3, "canvas is not (h, w, 3) u8");
    let plane = width * height;
    let mut pixels = vec![0.0f32; 3 * plane];
    for c in 0..3 {
        let (mean, std) = (H3_PIXEL_MEAN[c], H3_PIXEL_STD[c]);
        for i in 0..plane {
            pixels[c * plane + i] = (canvas_rgb[i * 3 + c] as f32 / 255.0 - mean) / std;
        }
    }
    pixels
}

/// The reference `_encode_clip` for a single keyframe: u8 canvas -> ImageNet
/// normalize -> tiled spatial encode (256px tiles, >=64px overlaps widened
/// latent-aligned; per tile quant_conv(encoder(tile))) -> latent-space
/// stitch (overlap / 16, linear cross-fade). Returns the moments
/// (48, height/16, width/16) row-major f32.
pub fn h3_vae_encode_keyframe_moments(
    prepared: &H3VaeEncoderPrepared,
    canvas_rgb: &[u8],
    width: usize,
    height: usize,
) -> Result<Vec<f32>> {
    let pixels = h3_vae_normalize_canvas(canvas_rgb, width, height);
    h3_vae_encode_keyframe_moments_normalized(prepared, &pixels, width, height)
}

/// [`h3_vae_encode_keyframe_moments`] over an already-normalized planar
/// (3, height, width) input (the dump's `vae_enc_in` for validation).
pub fn h3_vae_encode_keyframe_moments_normalized(
    prepared: &H3VaeEncoderPrepared,
    pixels: &[f32],
    width: usize,
    height: usize,
) -> Result<Vec<f32>> {
    if pixels.len() != 3 * width * height {
        return Err(DiffusionError::workflow(format!(
            "h3 vae encode expected {} pixel values, got {}",
            3 * width * height,
            pixels.len()
        )));
    }
    if width % H3_VAE_PATCH != 0 || height % H3_VAE_PATCH != 0 {
        return Err(DiffusionError::workflow(format!(
            "h3 vae encode canvas {width}x{height} not a multiple of {}",
            H3_VAE_PATCH
        )));
    }
    let tiling = ClipTiling::plan(height, width);
    let mut tiles = Vec::with_capacity(tiling.num_tiles());
    for (&y0, &ylen) in tiling.y_starts.iter().zip(tiling.y_lens.iter()) {
        for (&x0, &xlen) in tiling.x_starts.iter().zip(tiling.x_lens.iter()) {
            let mut tile = vec![0.0f32; 3 * ylen * xlen];
            for c in 0..3 {
                for y in 0..ylen {
                    let src = (c * height + y0 + y) * width + x0;
                    let dst = (c * ylen + y) * xlen;
                    tile[dst..dst + xlen].copy_from_slice(&pixels[src..src + xlen]);
                }
            }
            tiles.push(encode_tile(prepared, &tile, xlen, ylen)?);
        }
    }
    // Blend/stitch in LATENT space: overlaps / spatial_compression_ratio.
    let latent_y: Vec<usize> = tiling.y_overlaps.iter().map(|o| o / H3_VAE_PATCH).collect();
    let latent_x: Vec<usize> = tiling.x_overlaps.iter().map(|o| o / H3_VAE_PATCH).collect();
    let stitched = stitch_tiles_grid(
        tiling.y_starts.len(),
        tiling.x_starts.len(),
        &latent_y,
        &latent_x,
        tiles,
    );
    let (lh, lw) = (height / H3_VAE_PATCH, width / H3_VAE_PATCH);
    if stitched.h != lh || stitched.w != lw {
        return Err(DiffusionError::model(format!(
            "h3 vae encode stitched {}x{}, expected {lh}x{lw}",
            stitched.h, stitched.w
        )));
    }
    Ok(stitched.data)
}

/// f32 -> f16 bits with round-to-nearest-even (torch `.to(torch.float16)`;
/// ggml's `quant::f32_to_f16` truncates, which is NOT the checkpoint's
/// rounding). Overflow goes to +/-inf like torch.
pub fn f32_to_f16_rtne(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let frac = bits & 0x007f_ffff;
    if exp == 0xff {
        // inf or nan (nan keeps a set mantissa bit)
        return if frac == 0 { sign | 0x7c00 } else { sign | 0x7e00 };
    }
    let unbiased = exp - 127;
    // f32 subnormals (exp == 0 -> unbiased -127) and anything below half the
    // smallest f16 subnormal round to (signed) zero. 2^-25 exactly ties to
    // even = 0, so -25 itself falls through to the rounding path.
    if unbiased < -25 {
        return sign;
    }
    let mant = frac | 0x0080_0000; // implicit leading 1, 2^23 units
    // 13 dropped bits for f16 normals; subnormals drop more.
    let shift = if unbiased < -14 { 13 + (-14 - unbiased) } else { 13 } as u32;
    let halfway = 1u32 << (shift - 1);
    let rem = mant & ((1u32 << shift) - 1);
    let mut half = mant >> shift;
    if rem > halfway || (rem == halfway && (half & 1) == 1) {
        half += 1;
    }
    let out = if unbiased < -14 {
        // Subnormal mantissa; a carry into 0x400 is exactly the smallest
        // normal (exponent field 1), which the plain bit pattern encodes.
        half
    } else if half == 0x800 {
        // Mantissa carry: bump the exponent, mantissa 0.
        ((unbiased + 16) as u32) << 10
    } else {
        (((unbiased + 15) as u32) << 10) | (half & 0x3ff)
    };
    if out >= 0x7c00 {
        return sign | 0x7c00; // overflow to inf, like torch
    }
    sign | out as u16
}

/// The posterior sample + normalization of `encode_vae_condition`: moments
/// (48, lh, lw) -> mean ch 0..24 / logvar ch 24..48 clamped [-30, 20],
/// sample = mean + exp(0.5 * logvar) * eps (eps caller-provided, len
/// 24*lh*lw — validation injects zeros, generation brings its own RNG),
/// rounded through f16 (the reference's `.to(float16).float()`), then
/// per-channel `(x - latents_mean) / latents_std`. Returns (24, lh, lw).
pub fn h3_vae_condition_latents(
    moments: &[f32],
    lh: usize,
    lw: usize,
    eps: &[f32],
) -> Result<Vec<f32>> {
    let plane = lh * lw;
    if moments.len() != H3_VAE_MOMENT_CHANNELS * plane {
        return Err(DiffusionError::workflow(format!(
            "h3 vae condition expected {} moment values, got {}",
            H3_VAE_MOMENT_CHANNELS * plane,
            moments.len()
        )));
    }
    if eps.len() != H3_VAE_LATENT_CHANNELS * plane {
        return Err(DiffusionError::workflow(format!(
            "h3 vae condition expected {} eps values, got {}",
            H3_VAE_LATENT_CHANNELS * plane,
            eps.len()
        )));
    }
    let mut out = vec![0.0f32; H3_VAE_LATENT_CHANNELS * plane];
    for c in 0..H3_VAE_LATENT_CHANNELS {
        let (mean_c, std_c) = (H3_VAE_LATENTS_MEAN[c], H3_VAE_LATENTS_STD[c]);
        for i in 0..plane {
            let mean = moments[c * plane + i];
            let logvar = moments[(H3_VAE_LATENT_CHANNELS + c) * plane + i].clamp(-30.0, 20.0);
            let sample = mean + (0.5 * logvar).exp() * eps[c * plane + i];
            let sample = crate::f16_word_to_f32(f32_to_f16_rtne(sample));
            out[c * plane + i] = (sample - mean_c) / std_c;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_tiles_match_reference() {
        // Width 640: 3 tiles, overlaps stay 64, starts 0/192/384.
        let (starts, lens, overlaps) = h3_vae_split_tiles(640, 256, 64);
        assert_eq!(starts, vec![0, 192, 384]);
        assert_eq!(lens, vec![256, 256, 256]);
        assert_eq!(overlaps, vec![64, 64]);
        // Height 352: 2 tiles, slack 96 widens the overlap to 160.
        let (starts, _lens, overlaps) = h3_vae_split_tiles(352, 256, 64);
        assert_eq!(starts, vec![0, 96]);
        assert_eq!(overlaps, vec![160]);
        // Fits in one tile: no split.
        let (starts, lens, overlaps) = h3_vae_split_tiles(200, 256, 64);
        assert_eq!(starts, vec![0]);
        assert_eq!(lens, vec![200]);
        assert!(overlaps.is_empty());
    }

    #[test]
    fn chunk_plan_math() {
        // 37 latent frames (124 pixel frames): no padding, 7 chunks.
        assert_eq!(h3_vae_chunk_plan(37), (0, 7));
        // 7 latent frames (22 pixel frames): 10 tokens, no pad, 1 chunk.
        assert_eq!(h3_vae_chunk_plan(7), (0, 1));
        // 12 -> 15 tokens, pad 0, 2 chunks.
        assert_eq!(h3_vae_chunk_plan(12), (0, 2));
    }

    #[test]
    fn chunk_output_frames() {
        // The temporal loop yields 17 frames per chunk + the final 5-frame
        // overlap: 7 chunks -> 124 frames.
        let (pad, chunks) = h3_vae_chunk_plan(37);
        assert_eq!(pad, 0);
        let frames = chunks * H3_VAE_CLIP_LENGTH + H3_VAE_FRAME_OVERLAP;
        assert_eq!(frames, 124);
    }

    #[test]
    fn blend_crossfade() {
        // a tail [2.0], b head [0.0]: extent 2 -> weights (1, 0.5).
        let mut a = Vol::zeros(1, 4, 1, 1);
        a.data = vec![2.0; 4];
        let mut b = Vol::zeros(1, 3, 1, 1);
        b.data = vec![0.0; 3];
        let out = blend(&a, &b, 2, 0);
        assert_eq!(out.f, 3);
        assert_eq!(out.data[0], 2.0); // k=0: a*1 + b*0
        assert_eq!(out.data[1], 1.0); // k=1: a*0.5 + b*0.5
        assert_eq!(out.data[2], 0.0); // untouched tail of b
    }

    #[test]
    fn batch_groups_order_and_limits() {
        // Uniform geometry (the real decode: every tile-clip is (7, 16, 16)):
        // chunked in order to the limit.
        let geoms = vec![(7, 16, 16); 5];
        assert_eq!(batch_groups(&geoms, 2), vec![vec![0, 1], vec![2, 3], vec![4]]);
        // One batch when the limit covers everything.
        assert_eq!(batch_groups(&geoms, 42), vec![vec![0, 1, 2, 3, 4]]);
        // Mixed geometries group separately (first-seen order) so every batch
        // shares one rope table; order within a group is preserved.
        let geoms = vec![(7, 16, 16), (7, 22, 16), (7, 16, 16), (7, 22, 16)];
        assert_eq!(batch_groups(&geoms, 42), vec![vec![0, 2], vec![1, 3]]);
        // A zero limit clamps to 1 instead of looping forever.
        assert_eq!(batch_groups(&geoms[..1], 0), vec![vec![0]]);
    }

    #[test]
    fn rope_inv_freq_values() {
        let mut inv = [0.0f32; 8];
        for (j, v) in inv.iter_mut().enumerate() {
            *v = H3_VAE_ROPE_THETA.powf(-(j as f32) / 8.0);
        }
        assert_eq!(inv[0], 1.0);
        assert!((inv[4] - 0.1).abs() < 1e-6); // 100^-0.5
    }

    // --- encoder ------------------------------------------------------------

    #[test]
    fn encode_tile_geometry_640x352() {
        // The dump canvas: 3x2 tile grid, pixel overlaps [64,64]/[160],
        // latent overlaps [4,4]/[10], stitched latent 40x22.
        let tiling = ClipTiling::plan(352, 640);
        assert_eq!(tiling.y_starts, vec![0, 96]);
        assert_eq!(tiling.y_lens, vec![256, 256]);
        assert_eq!(tiling.y_overlaps, vec![160]);
        assert_eq!(tiling.x_starts, vec![0, 192, 384]);
        assert_eq!(tiling.x_overlaps, vec![64, 64]);
        assert_eq!(tiling.num_tiles(), 6);
        let latent_y: Vec<usize> = tiling.y_overlaps.iter().map(|o| o / H3_VAE_PATCH).collect();
        let latent_x: Vec<usize> = tiling.x_overlaps.iter().map(|o| o / H3_VAE_PATCH).collect();
        assert_eq!(latent_y, vec![10]);
        assert_eq!(latent_x, vec![4, 4]);
        // Stitch geometry: each 16x16 latent tile trims its trailing overlap
        // except the last -> height 6 + 16 = 22, width 12 + 12 + 16 = 40.
        let tiles: Vec<Vol> = (0..6).map(|_| Vol::zeros(48, 1, 16, 16)).collect();
        let stitched = stitch_tiles_grid(2, 3, &latent_y, &latent_x, tiles);
        assert_eq!((stitched.c, stitched.f, stitched.h, stitched.w), (48, 1, 22, 40));
    }

    #[test]
    fn split_tiles_encode_matches_decode_side() {
        // The encode side calls _split_tiles on PIXEL dims directly; verify
        // it agrees with the decode-side plan used for the same canvas.
        let (starts, lens, overlaps) = h3_vae_split_tiles(640, H3_VAE_TILE, H3_VAE_TILE_OVERLAP);
        assert_eq!((starts, lens, overlaps), (vec![0, 192, 384], vec![256; 3], vec![64, 64]));
        // One-tile canvases keep their full extent.
        let (starts, lens, overlaps) = h3_vae_split_tiles(256, H3_VAE_TILE, H3_VAE_TILE_OVERLAP);
        assert_eq!((starts, lens, overlaps), (vec![0], vec![256], vec![]));
    }

    // CPU reference: the full MiniMaxH3VideoCausalConv3d on a SINGLE frame —
    // temporal pad 2 zero frames, real spatial reflect padding, arbitrary
    // strides — against which the 2D sliced-weight + gather composition is
    // checked exactly.
    #[allow(clippy::too_many_arguments)]
    fn causal_conv3d_single_frame_ref(
        input: &[f32], // (ci, h, w)
        ci: usize,
        w: usize,
        h: usize,
        weight: &[f32], // (co, ci, 3, 3, 3)
        bias: &[f32],
        co: usize,
        stride: usize,          // spatial (and temporal, irrelevant at T=1)
        pad: (usize, usize, usize, usize), // (left, right, top, bottom) reflect
    ) -> (Vec<f32>, usize, usize) {
        let (pl, pr, pt, pb) = pad;
        let (pw, ph) = (w + pl + pr, h + pt + pb);
        // Reflect-pad the single real frame; the two causal pad frames are 0.
        let mut padded = vec![0.0f32; ci * pw * ph];
        for c in 0..ci {
            for py in 0..ph {
                let sy = reflect_index(py as i64 - pt as i64, h as i64);
                for px in 0..pw {
                    let sx = reflect_index(px as i64 - pl as i64, w as i64);
                    padded[(c * ph + py) * pw + px] = input[(c * h + sy) * w + sx];
                }
            }
        }
        let (ow, oh) = ((pw - 3) / stride + 1, (ph - 3) / stride + 1);
        let mut out = vec![0.0f32; co * ow * oh];
        for oc in 0..co {
            for oy in 0..oh {
                for ox in 0..ow {
                    let mut acc = bias[oc];
                    for icc in 0..ci {
                        for kt in 0..3usize {
                            // padded temporal frames [0, 0, real]: frame index
                            // 0 + kt over [z, z, x] — only kt == 2 is real.
                            for ky in 0..3 {
                                for kx in 0..3 {
                                    let value = if kt == 2 {
                                        padded[(icc * ph + oy * stride + ky) * pw
                                            + ox * stride
                                            + kx]
                                    } else {
                                        0.0
                                    };
                                    acc += value
                                        * weight[(((oc * ci + icc) * 3 + kt) * 3 + ky) * 3 + kx];
                                }
                            }
                        }
                    }
                    out[(oc * oh + oy) * ow + ox] = acc;
                }
            }
        }
        (out, ow, oh)
    }

    /// CPU mimic of the CUDA pad-0 conv kernel: same-size output, OOB taps
    /// skipped (partial windows past the valid region).
    fn conv2d_pad0_same_ref(
        input: &[f32],
        ci: usize,
        w: usize,
        h: usize,
        weight: &[f32], // (co, ci, 3, 3)
        bias: &[f32],
        co: usize,
    ) -> Vec<f32> {
        let mut out = vec![0.0f32; co * w * h];
        for oc in 0..co {
            for y in 0..h {
                for x in 0..w {
                    let mut acc = bias[oc];
                    for icc in 0..ci {
                        for ky in 0..3 {
                            if y + ky >= h {
                                continue;
                            }
                            for kx in 0..3 {
                                if x + kx >= w {
                                    continue;
                                }
                                acc += input[(icc * h + y + ky) * w + x + kx]
                                    * weight[((oc * ci + icc) * 3 + ky) * 3 + kx];
                            }
                        }
                    }
                    out[(oc * h + y) * w + x] = acc;
                }
            }
        }
        out
    }

    fn gather_ref(input: &[f32], rows: usize, cols: usize, indices: &[u32]) -> Vec<f32> {
        let mut out = vec![0.0f32; rows * indices.len()];
        for r in 0..rows {
            for (j, &idx) in indices.iter().enumerate() {
                out[r * indices.len() + j] = input[r * cols + idx as usize];
            }
        }
        out
    }

    fn pseudo(seed: usize) -> f32 {
        // Small deterministic values, exactly representable enough that the
        // conv accumulation orders below stay bit-identical.
        ((seed * 37 + 11) % 64) as f32 / 32.0 - 1.0
    }

    fn slice_temporal(weight3d: &[f32], co: usize, ci: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; co * ci * 9];
        for oc_ic in 0..co * ci {
            out[oc_ic * 9..oc_ic * 9 + 9]
                .copy_from_slice(&weight3d[(oc_ic * 3 + 2) * 9..(oc_ic * 3 + 2) * 9 + 9]);
        }
        out
    }

    #[test]
    fn temporal_slice_reduction_holds() {
        // Single-frame causal conv3d == 2D conv with the w[:, :, 2] slice,
        // composed exactly like the GPU path (gather pad -> pad-0 conv ->
        // gather crop/stride). Exact f32 equality: the kt < 2 taps multiply
        // literal zeros, and adding 0.0 to a finite accumulator is exact.
        let (ci, co, w, h) = (3usize, 2usize, 6usize, 5usize);
        let input: Vec<f32> = (0..ci * w * h).map(pseudo).collect();
        let weight3d: Vec<f32> = (0..co * ci * 27).map(|i| pseudo(i + 101)).collect();
        let bias: Vec<f32> = (0..co).map(|i| pseudo(i + 999)).collect();
        let weight2d = slice_temporal(&weight3d, co, ci);

        // Case A: k3 stride 1, symmetric reflect pad 1 (the resnet convs).
        let (reference, ow, oh) = causal_conv3d_single_frame_ref(
            &input, ci, w, h, &weight3d, &bias, co, 1, (1, 1, 1, 1),
        );
        assert_eq!((ow, oh), (w, h));
        let padded = gather_ref(&input, ci, w * h, &reflect_pad1_indices(w, h));
        let conv = conv2d_pad0_same_ref(&padded, ci, w + 2, h + 2, &weight2d, &bias, co);
        let ours = gather_ref(&conv, co, (w + 2) * (h + 2), &crop_indices(w + 2, w, h));
        assert_eq!(ours, reference);

        // Case B: the downsampler — asym right/bottom reflect pad 1, k3
        // stride (2, 2, 2); the temporal stride is also 2, which still
        // yields the single [z, z, x] window.
        let (reference, ow, oh) = causal_conv3d_single_frame_ref(
            &input, ci, w, h, &weight3d, &bias, co, 2, (0, 1, 0, 1),
        );
        assert_eq!((ow, oh), ((w - 2) / 2 + 1, (h - 2) / 2 + 1));
        let padded = gather_ref(&input, ci, w * h, &asym_pad_indices(w, h));
        let conv = conv2d_pad0_same_ref(&padded, ci, w + 1, h + 1, &weight2d, &bias, co);
        let ours = gather_ref(&conv, co, (w + 1) * (h + 1), &stride2_indices(w + 1, ow, oh));
        assert_eq!(ours, reference);

        // Odd width/height also agree with torch's floor formula.
        let (w, h) = (5usize, 7usize);
        let input: Vec<f32> = (0..ci * w * h).map(|i| pseudo(i + 7)).collect();
        let (reference, ow, oh) = causal_conv3d_single_frame_ref(
            &input, ci, w, h, &weight3d, &bias, co, 2, (0, 1, 0, 1),
        );
        assert_eq!((ow, oh), (2, 3));
        let padded = gather_ref(&input, ci, w * h, &asym_pad_indices(w, h));
        let conv = conv2d_pad0_same_ref(&padded, ci, w + 1, h + 1, &weight2d, &bias, co);
        let ours = gather_ref(&conv, co, (w + 1) * (h + 1), &stride2_indices(w + 1, ow, oh));
        assert_eq!(ours, reference);
    }

    #[test]
    fn pad_index_tables() {
        // Reflect (PyTorch, no edge repeat): pad 1 of [0,1,2,3] ->
        // [1, 0, 1, 2, 3, 2] per axis (reflect pad needs len >= 2, as in
        // torch).
        let indices = reflect_pad1_indices(4, 2);
        assert_eq!(indices.len(), 6 * 4);
        // Row layout (w+2 = 6): pad row = reflect of y=-1 -> y=1.
        assert_eq!(&indices[..6], &[5, 4, 5, 6, 7, 6]); // y -1 -> 1, x -1 -> 1
        assert_eq!(&indices[6..12], &[1, 0, 1, 2, 3, 2]); // y 0
        assert_eq!(&indices[12..18], &[5, 4, 5, 6, 7, 6]); // y 1
        assert_eq!(&indices[18..24], &[1, 0, 1, 2, 3, 2]); // y 2 -> reflect 1...

        // Asym right/bottom pad: [0, 1, 2, 3, 2] per row, last row = h-2.
        let indices = asym_pad_indices(4, 3);
        assert_eq!(&indices[..5], &[0, 1, 2, 3, 2]);
        assert_eq!(&indices[15..20], &[4, 5, 6, 7, 6]); // y = 3 -> reflect 1
        // Crop and stride tables.
        assert_eq!(crop_indices(5, 3, 2), vec![0, 1, 2, 5, 6, 7]);
        assert_eq!(stride2_indices(5, 2, 2), vec![0, 2, 10, 12]);
    }

    #[test]
    fn f16_rtne_matches_numpy() {
        // (f32 bits, expected f16 bits) pairs generated with numpy float16
        // (round-to-nearest-even, torch semantics).
        let cases: [(u32, u16); 20] = [
            (0x3f801000, 0x3c00), // 1 + 2^-11 ties to even -> 1.0
            (0x3f801800, 0x3c01), // 1 + 3*2^-12 rounds up
            (0x3f7ff000, 0x3c00), // 0.99975586 carries into the exponent
            (0x3dcccccd, 0x2e66), // 0.1
            (0xbdcccccd, 0xae66), // -0.1
            (0x40490fdb, 0x4248), // pi
            (0x477fefff, 0x7bff), // 65519.996 stays max normal
            (0x477ff000, 0x7c00), // 65520.0 overflows to inf
            (0x477fe000, 0x7bff), // 65504.0 = f16 max
            (0x33000000, 0x0000), // 2^-25 ties to even -> 0
            (0x33400000, 0x0001), // 1.5 * 2^-25 -> smallest subnormal
            (0x33800000, 0x0001), // 2^-24 exact smallest subnormal
            (0xb3000000, 0x8000), // -2^-25 -> -0
            (0x387fda40, 0x03ff), // 6.1e-5 -> largest subnormal
            (0x387fc000, 0x03ff), // largest subnormal exact
            (0x46ac14ee, 0x7561), // e^10
            (0x322bcc77, 0x0000), // 1e-8 underflows
            (0xc49a5225, 0xe4d3), // -1234.567
            (0x00000000, 0x0000), // +0
            (0x80000000, 0x8000), // -0
        ];
        for (f32_bits, f16_bits) in cases {
            assert_eq!(
                f32_to_f16_rtne(f32::from_bits(f32_bits)),
                f16_bits,
                "f32 bits {f32_bits:#010x}"
            );
        }
        assert_eq!(f32_to_f16_rtne(f32::INFINITY), 0x7c00);
        assert_eq!(f32_to_f16_rtne(f32::NEG_INFINITY), 0xfc00);
        assert_eq!(f32_to_f16_rtne(f32::NAN) & 0x7c00, 0x7c00);
        assert_ne!(f32_to_f16_rtne(f32::NAN) & 0x03ff, 0);
    }

    #[test]
    fn condition_latents_arithmetic() {
        // 1x1 latent grid: moments = 24 means + 24 logvars. Reference values
        // computed with numpy (f32 math, float16 rounding, vae config
        // latents_mean/std) for channels 0..4; the rest use eps 0 and a
        // benign logvar.
        let plane = 1usize;
        let mut moments = vec![0.0f32; 48 * plane];
        let mut eps = vec![0.0f32; 24 * plane];
        // (channel, mean, logvar, eps, expected out bits)
        let cases: [(usize, f32, f32, f32, u32); 4] = [
            (0, 1.2345, -40.0, 3.0, 0x3e9d9beb),  // logvar clamps to -30
            (1, -0.75, 25.0, 0.001, 0x418b5e4d),  // logvar clamps to 20
            (2, 0.3333, -1.7, -1.25, 0xbf40b756),
            (3, 2.71828, 0.0, 0.5, 0x4007f237),
        ];
        for &(c, mean, logvar, e, _bits) in &cases {
            moments[c] = mean;
            moments[24 + c] = logvar;
            eps[c] = e;
        }
        let out = h3_vae_condition_latents(&moments, 1, 1, &eps).expect("valid lengths");
        assert_eq!(out.len(), 24);
        for &(c, _mean, _logvar, _e, bits) in &cases {
            let expected = f32::from_bits(bits);
            let got = out[c];
            assert!(
                (got - expected).abs() <= 2e-6 * expected.abs().max(1.0),
                "channel {c}: got {got} expected {expected}"
            );
        }
        // eps = 0 channels reduce to f16(mean) normalized: channel 5 with
        // mean 0 -> (0 - mean_5) / std_5.
        let expected = (0.0 - H3_VAE_LATENTS_MEAN[5]) / H3_VAE_LATENTS_STD[5];
        assert!((out[5] - expected).abs() < 1e-7);
        // Length validation errors.
        assert!(h3_vae_condition_latents(&moments[..47], 1, 1, &eps).is_err());
        assert!(h3_vae_condition_latents(&moments, 1, 1, &eps[..23]).is_err());
    }
}
