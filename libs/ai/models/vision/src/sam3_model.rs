//! Device graph for SAM 3.1 image PCS (Comfy-Org multiplex header).
//!
//! Every layer shape, constant and block ordering below follows the
//! Apache-2.0 Hugging Face `transformers` SAM 3 implementation
//! (`src/transformers/models/sam3/modeling_sam3.py` and
//! `configuration_sam3.py`, "Copyright 2025 The Meta AI Authors and The
//! HuggingFace Team"); the interactive/tracker mask decoder follows the same
//! project's Apache-2.0 `models/sam2`. Where our numerics deliberately
//! diverge from the HF reference to reproduce the reference dumps of the
//! target checkpoint pipeline, the site says so inline. See
//! `../THIRD_PARTY_NOTICES.md`.
//!
//! Numerics that dumps must still pin: PE RoPE (no learned rope tensors in
//! this pack — interpolated absolute pos only), CLIP causal vs bidirectional
//! text attention (paper says causal), and pixel-decoder fusion order.

use crate::backend::{
    gpu_add, gpu_attention_packed, gpu_attention_packed_cross,
    gpu_attention_packed_cross_bias,
    gpu_attention_packed_flash2_d64, gpu_birefnet_image_to_patches,
    gpu_rpb_expand,
    gpu_birefnet_relu,
    gpu_birefnet_resize_bilinear, gpu_birefnet_tokens_to_planar, gpu_concat_cols,
    gpu_concat_rows, gpu_conv2d_planar_cached, gpu_device_available, gpu_download, gpu_gelu,
    gpu_copy_into, gpu_gather_cols, gpu_gather_rows_colblock, gpu_gelu_erf,
    gpu_graph_capture, gpu_graph_launch, gpu_group_norm_planar,
    gpu_layer_norm_mod, gpu_sam3_refine_boxes, gpu_sam3_rpb_axial, gpu_sam3_sine_embed,
    gpu_upload_into, GpuStepGraph,
    gpu_linear_f32_resident, gpu_linear_nt_cached, gpu_linear_nt_cached_bf16_f32acc, gpu_mul,
    gpu_pixel_shuffle_planar_cached, gpu_reshape, gpu_silu,
    gpu_rope_interleaved, gpu_slice_cols, gpu_slice_rows, gpu_upload, gpu_upload_u32,
    gpu_upsample_nearest2x, GpuLinearPart, GpuTensor,
};
use makepad_ai_flux::clip::ClipTokenizer;
use crate::sam3::{
    Sam3Cancel, Sam3Mask, Sam3Preprocessed, Sam3Prompt, Sam3Weights,
    SAM3_CACHE_NAMESPACE, SAM3_DETECTOR_DIM, SAM3_DETECTOR_HEADS, SAM3_DECODER_DEPTH,
    SAM3_FUSION_DEPTH, SAM3_GLOBAL_LAYERS, SAM3_INPUT_SIZE, SAM3_LN_EPS, SAM3_NUM_QUERIES,
    SAM3_PAD_ID, SAM3_PATCH, SAM3_POS_BASE, SAM3_REFINE_ITERS, SAM3_SCORE_THRESH, SAM3_TEXT_CTX,
    SAM3_TEXT_DEPTH, SAM3_TEXT_DIM, SAM3_TEXT_HEADS, SAM3_TEXT_MLP, SAM3_VISION_DEPTH,
    SAM3_VISION_DIM, SAM3_VISION_HEADS, SAM3_VISION_MLP, SAM3_VISION_WINDOW,
};
use crate::{emit_progress, DiffusionError, ProgressHook, Result};
use makepad_ai_common::quant::GGML_TYPE_BF16;
use std::collections::HashMap;

const VISION_HEAD_DIM: usize = SAM3_VISION_DIM / SAM3_VISION_HEADS;
const TEXT_HEAD_DIM: usize = SAM3_TEXT_DIM / SAM3_TEXT_HEADS;
const DET_HEAD_DIM: usize = SAM3_DETECTOR_DIM / SAM3_DETECTOR_HEADS;
const GRID: usize = SAM3_INPUT_SIZE / SAM3_PATCH;

fn check_cancel(cancel: Option<Sam3Cancel<'_>>) -> Result<()> {
    if cancel.is_some_and(|is_cancelled| is_cancelled()) {
        Err(DiffusionError::Cancelled)
    } else {
        Ok(())
    }
}

fn quick_gelu_tensor(x: &GpuTensor) -> Result<GpuTensor> {
    // quick_gelu(x) = silu(1.702 x) / 1.702. Stay on device to avoid 24 D2H syncs.
    let n = x.rows() * x.cols();
    let scale = gpu_upload(&vec![1.702f32; n], x.rows(), x.cols()).map_err(DiffusionError::model)?;
    let scaled = gpu_mul(x, &scale).map_err(DiffusionError::model)?;
    let silu = gpu_silu(&scaled).map_err(DiffusionError::model)?;
    let inv = gpu_upload(&vec![1.0 / 1.702; n], x.rows(), x.cols()).map_err(DiffusionError::model)?;
    gpu_mul(&silu, &inv).map_err(DiffusionError::model)
}

fn f32_to_bf16_bytes(values: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 2);
    for &value in values {
        let bits = value.to_bits();
        let rounded = ((bits.wrapping_add(0x7fff + ((bits >> 16) & 1))) >> 16) as u16;
        out.extend_from_slice(&rounded.to_le_bytes());
    }
    out
}

fn norm_mods(weights: &Sam3Weights, prefix: &str, dim: usize) -> Result<GpuTensor> {
    let weight = weights.f32_shaped(&format!("{prefix}.weight"), &[dim])?;
    let bias = weights.f32_shaped(&format!("{prefix}.bias"), &[dim])?;
    let mut packed = Vec::with_capacity(2 * dim);
    packed.extend(weight.iter().map(|value| value - 1.0));
    packed.extend_from_slice(&bias);
    gpu_upload(&packed, 1, 2 * dim).map_err(DiffusionError::model)
}

fn linear(
    x: &GpuTensor,
    bytes: &[u8],
    key: &str,
    n: usize,
    bias: &[f32],
) -> Result<GpuTensor> {
    gpu_linear_nt_cached_bf16_f32acc(
        x,
        SAM3_CACHE_NAMESPACE,
        &[GpuLinearPart {
            bt_ggml_type: GGML_TYPE_BF16,
            n,
            cache_key: key,
            bytes,
        }],
        bias,
    )
    .map_err(DiffusionError::model)
}

/// f16-operand, f16-accumulate GEMM — the reduced-precision reduction torch
/// enables by default for f16 checkpoints (the oracle's arithmetic class).
/// Twice the f32-accumulate tensor-op rate. Used only where it measurably
/// helps without costing dump parity: the ViT trunk (05 IoU improved) —
/// spreading it to the decoder/score path cost IoU for ~2ms and was
/// rejected. The generic model default deliberately selects this policy.
fn linear_fast(
    x: &GpuTensor,
    bytes: &[u8],
    key: &str,
    n: usize,
    bias: &[f32],
) -> Result<GpuTensor> {
    gpu_linear_nt_cached(
        x,
        SAM3_CACHE_NAMESPACE,
        &[GpuLinearPart {
            bt_ggml_type: GGML_TYPE_BF16,
            n,
            cache_key: key,
            bytes,
        }],
        bias,
    )
    .map_err(DiffusionError::model)
}

struct Linear {
    bytes: Vec<u8>,
    key: String,
    n: usize,
    bias: Vec<f32>,
}

impl Linear {
    fn load(weights: &Sam3Weights, name: &str, out: usize, inn: usize) -> Result<Self> {
        Self::load_named(
            weights,
            name,
            &format!("{name}.weight"),
            Some(&format!("{name}.bias")),
            out,
            inn,
        )
    }

    /// CLIP / DETR packed QKV lives at `*.in_proj_weight`, not `*.in_proj.weight`.
    fn load_in_proj(weights: &Sam3Weights, prefix: &str, dim: usize) -> Result<Self> {
        Self::load_named(
            weights,
            &format!("{prefix}.in_proj"),
            &format!("{prefix}.in_proj_weight"),
            Some(&format!("{prefix}.in_proj_bias")),
            3 * dim,
            dim,
        )
    }

    fn load_named(
        weights: &Sam3Weights,
        key: &str,
        weight_name: &str,
        bias_name: Option<&str>,
        out: usize,
        inn: usize,
    ) -> Result<Self> {
        let w = weights.f32_shaped(weight_name, &[out, inn])?;
        let bias = match bias_name {
            Some(name) if weights.has(name) => weights.f32_shaped(name, &[out])?,
            _ => vec![0.0; out],
        };
        Ok(Self {
            bytes: f32_to_bf16_bytes(&w),
            key: key.to_string(),
            n: out,
            bias,
        })
    }

    fn forward(&self, x: &GpuTensor) -> Result<GpuTensor> {
        linear(x, &self.bytes, &self.key, self.n, &self.bias)
    }

    fn forward_fast(&self, x: &GpuTensor) -> Result<GpuTensor> {
        linear_fast(x, &self.bytes, &self.key, self.n, &self.bias)
    }
}

/// Packed `in_proj` split into separate q/k/v projections so cross-attention
/// only runs the thirds it needs on each input (the memory side is 5184
/// tokens; a packed forward would compute 3x the required columns).
struct SplitProj {
    q: Linear,
    k: Linear,
    v: Linear,
}

impl SplitProj {
    fn load_in_proj(weights: &Sam3Weights, prefix: &str, dim: usize) -> Result<Self> {
        let w = weights.f32_shaped(&format!("{prefix}.in_proj_weight"), &[3 * dim, dim])?;
        let bias_name = format!("{prefix}.in_proj_bias");
        let b = if weights.has(&bias_name) {
            weights.f32_shaped(&bias_name, &[3 * dim])?
        } else {
            vec![0.0; 3 * dim]
        };
        let part = |index: usize, name: &str| -> Linear {
            Linear {
                bytes: f32_to_bf16_bytes(&w[index * dim * dim..(index + 1) * dim * dim]),
                key: format!("{prefix}.in_proj::{name}"),
                n: dim,
                bias: b[index * dim..(index + 1) * dim].to_vec(),
            }
        };
        Ok(Self {
            q: part(0, "q"),
            k: part(1, "k"),
            v: part(2, "v"),
        })
    }
}

struct Conv2d {
    key: String,
    weights: Vec<f32>,
    bias: Vec<f32>,
    out_channels: usize,
    kw: usize,
    kh: usize,
    pad_x: usize,
    pad_y: usize,
}

impl Conv2d {
    fn load(
        weights: &Sam3Weights,
        name: &str,
        in_channels: usize,
        out_channels: usize,
        k: usize,
        bias: bool,
    ) -> Result<Self> {
        let values =
            weights.f32_shaped(&format!("{name}.weight"), &[out_channels, in_channels, k, k])?;
        let bias_values = if bias {
            weights.f32_shaped(&format!("{name}.bias"), &[out_channels])?
        } else {
            vec![0.0; out_channels]
        };
        Ok(Self {
            key: name.to_string(),
            weights: values,
            bias: bias_values,
            out_channels,
            kw: k,
            kh: k,
            pad_x: k / 2,
            pad_y: k / 2,
        })
    }

    fn forward(&self, input: &Planar) -> Result<Planar> {
        let tensor = gpu_conv2d_planar_cached(
            &input.tensor,
            input.width,
            input.height,
            SAM3_CACHE_NAMESPACE,
            &self.key,
            &self.weights,
            &self.bias,
            self.out_channels,
            self.kw,
            self.kh,
            self.pad_x,
            self.pad_y,
        )
        .map_err(DiffusionError::model)?;
        Ok(Planar {
            tensor,
            width: input.width,
            height: input.height,
        })
    }
}

struct ConvTransposeNoOverlap {
    weight: GpuTensor,
    bias: Vec<f32>,
    bias_key: String,
    out_channels: usize,
    scale: usize,
}

impl ConvTransposeNoOverlap {
    fn load(
        weights: &Sam3Weights,
        name: &str,
        in_channels: usize,
        out_channels: usize,
        scale: usize,
    ) -> Result<Self> {
        let source = weights.f32_shaped(
            &format!("{name}.weight"),
            &[in_channels, out_channels, scale, scale],
        )?;
        let mut reordered = vec![0.0f32; source.len()];
        for output in 0..out_channels {
            for ky in 0..scale {
                for kx in 0..scale {
                    let feature = (output * scale + ky) * scale + kx;
                    for input in 0..in_channels {
                        let src = ((input * out_channels + output) * scale + ky) * scale + kx;
                        reordered[feature * in_channels + input] = source[src];
                    }
                }
            }
        }
        let bias = if weights.has(&format!("{name}.bias")) {
            weights.f32_shaped(&format!("{name}.bias"), &[out_channels])?
        } else {
            vec![0.0; out_channels]
        };
        Ok(Self {
            weight: gpu_upload(&reordered, out_channels * scale * scale, in_channels)
                .map_err(DiffusionError::model)?,
            bias,
            bias_key: name.to_string(),
            out_channels,
            scale,
        })
    }

    fn forward(&self, input: Planar) -> Result<Planar> {
        let tokens =
            gpu_birefnet_tokens_to_planar(&input.tensor).map_err(DiffusionError::model)?;
        let expanded = crate::backend::gpu_linear_f32_resident(&tokens, &self.weight, None)
            .map_err(DiffusionError::model)?;
        let expanded =
            gpu_birefnet_tokens_to_planar(&expanded).map_err(DiffusionError::model)?;
        let tensor = gpu_pixel_shuffle_planar_cached(
            &expanded,
            input.width,
            input.height,
            self.out_channels,
            self.scale,
            SAM3_CACHE_NAMESPACE,
            &self.bias_key,
            &self.bias,
        )
        .map_err(DiffusionError::model)?;
        Ok(Planar {
            tensor,
            width: input.width * self.scale,
            height: input.height * self.scale,
        })
    }
}

struct Planar {
    tensor: GpuTensor,
    width: usize,
    height: usize,
}

struct VisionBlock {
    norm1: GpuTensor,
    norm2: GpuTensor,
    qkv: Linear,
    proj: Linear,
    fc1: Linear,
    fc2: Linear,
    global: bool,
}

struct TextBlock {
    ln1: GpuTensor,
    ln2: GpuTensor,
    qkv: Linear,
    proj: Linear,
    fc: Linear,
    proj_out: Linear,
}

struct FusionLayer {
    self_attn: SplitProj,
    self_out: Linear,
    cross: SplitProj,
    cross_out: Linear,
    linear1: Linear,
    linear2: Linear,
    norm1: GpuTensor,
    norm2: GpuTensor,
    norm3: GpuTensor,
}

struct DecoderLayer {
    self_attn: SplitProj,
    self_out: Linear,
    ca_text: SplitProj,
    ca_text_out: Linear,
    cross: SplitProj,
    cross_out: Linear,
    linear1: Linear,
    linear2: Linear,
    norm1: GpuTensor,
    norm2: GpuTensor,
    norm3: GpuTensor,
    catext_norm: GpuTensor,
}

struct InteractiveFpn {
    conv0_up0: ConvTransposeNoOverlap,
    conv0_up1: ConvTransposeNoOverlap,
    conv0_1x1: Conv2d,
    conv0_3x3: Conv2d,
    conv1_up: ConvTransposeNoOverlap,
    conv1_1x1: Conv2d,
    conv1_3x3: Conv2d,
    conv2_1x1: Conv2d,
    conv2_3x3: Conv2d,
}

struct SamAttn {
    q: Linear,
    k: Linear,
    v: Linear,
    out: Linear,
    heads: usize,
}

impl SamAttn {
    fn load(
        weights: &Sam3Weights,
        prefix: &str,
        in_dim: usize,
        inner_dim: usize,
        heads: usize,
    ) -> Result<Self> {
        Ok(Self {
            q: Linear::load(weights, &format!("{prefix}.q_proj"), inner_dim, in_dim)?,
            k: Linear::load(weights, &format!("{prefix}.k_proj"), inner_dim, in_dim)?,
            v: Linear::load(weights, &format!("{prefix}.v_proj"), inner_dim, in_dim)?,
            out: Linear::load(weights, &format!("{prefix}.out_proj"), in_dim, inner_dim)?,
            heads,
        })
    }

    fn forward(
        &self,
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        scale: f32,
    ) -> Result<GpuTensor> {
        let q = self.q.forward(q)?;
        let k = self.k.forward(k)?;
        let v = self.v.forward(v)?;
        let attended = gpu_attention_packed_cross(&q, &k, &v, self.heads, scale)
            .map_err(DiffusionError::model)?;
        self.out.forward(&attended)
    }
}

struct TwoWayBlock {
    self_attn: SamAttn,
    t2i: SamAttn,
    i2t: SamAttn,
    mlp1: Linear,
    mlp2: Linear,
    norm1: GpuTensor,
    norm2: GpuTensor,
    norm3: GpuTensor,
    norm4: GpuTensor,
    skip_first_pe: bool,
}

struct HyperMlp {
    l0: Linear,
    l1: Linear,
    l2: Linear,
}

struct InteractiveSam {
    fpn: InteractiveFpn,
    no_mem: GpuTensor,
    tokens: GpuTensor,
    dense_pe: GpuTensor,
    mask_ds0_w: Vec<f32>,
    mask_ds0_b: Vec<f32>,
    mask_ds0_ln: GpuTensor,
    mask_ds1_w: Vec<f32>,
    mask_ds1_b: Vec<f32>,
    mask_ds1_ln: GpuTensor,
    mask_ds2: Conv2d,
    layers: [TwoWayBlock; 2],
    final_attn: SamAttn,
    final_norm: GpuTensor,
    up0: ConvTransposeNoOverlap,
    up0_ln: GpuTensor,
    up1: ConvTransposeNoOverlap,
    conv_s0: Conv2d,
    conv_s1: Conv2d,
    hyper0: HyperMlp,
}

pub struct Sam3Model {
    tokenizer: ClipTokenizer,
    patch: Linear,
    pos_spatial: GpuTensor,
    ln_pre: GpuTensor,
    /// Process-unique id keying the captured trunk graphs (an address key
    /// would replay a stale graph after drop + reload).
    generation: usize,
    vision: Vec<VisionBlock>,
    /// Row gather that both window-orders the sequence and folds the 9
    /// windows into the column axis: `[5184, 1024]` → `[576, 9*1024]`.
    /// One flash call with 9*16 heads then runs all windows block-diagonally.
    window_fold: GpuTensor,
    /// Inverse gather back to raster row order.
    window_unfold: GpuTensor,
    rope_global_cos: GpuTensor,
    rope_global_sin: GpuTensor,
    /// Window-local RoPE tables for the folded layout (`[576, 32]`); every
    /// window shares the same local coordinates.
    rope_window_cos: GpuTensor,
    rope_window_sin: GpuTensor,
    text_embed: Vec<f32>,
    text_pos: GpuTensor,
    text_blocks: Vec<TextBlock>,
    text_ln: GpuTensor,
    text_resize: Linear,
    conv0_up0: ConvTransposeNoOverlap,
    conv0_up1: ConvTransposeNoOverlap,
    conv0_1x1: Conv2d,
    conv0_3x3: Conv2d,
    conv1_up: ConvTransposeNoOverlap,
    conv1_1x1: Conv2d,
    conv1_3x3: Conv2d,
    conv2_1x1: Conv2d,
    conv2_3x3: Conv2d,
    fusion: Vec<FusionLayer>,
    decoder: Vec<DecoderLayer>,
    decoder_norm: GpuTensor,
    query_embed: GpuTensor,
    presence_token: GpuTensor,
    presence_out_norm: GpuTensor,
    presence_h0: Linear,
    presence_h1: Linear,
    presence_h2: Linear,
    reference_points_sig: GpuTensor,
    zero_pos: GpuTensor,
    /// Zero columns plus gather maps that pad the 8x32 detector heads to
    /// 8x64 so the d64 flash kernel can run fusion self-attention.
    pad_zeros: GpuTensor,
    pad_idx: Vec<u32>,
    unpad_idx: Vec<u32>,
    ref_point_h0: Linear,
    ref_point_h1: Linear,
    rpb_x0: Linear,
    rpb_x1: Linear,
    rpb_y0: Linear,
    rpb_y1: Linear,
    geo_cls: GpuTensor,
    geo_final: Linear,
    geo_norm: GpuTensor,
    geo_encode_norm: GpuTensor,
    geo_layers: Vec<FusionLayer>,
    bbox0: Linear,
    bbox1: Linear,
    bbox2: Linear,
    hs_proj: Linear,
    prompt_mlp0: Linear,
    prompt_mlp1: Linear,
    prompt_mlp_norm: GpuTensor,
    prompt_proj: Linear,
    mask_embed0: Linear,
    mask_embed1: Linear,
    mask_embed2: Linear,
    pixel_conv: [Conv2d; 3],
    pixel_gn_gamma: [Vec<f32>; 3],
    pixel_gn_beta: [Vec<f32>; 3],
    seg_cross: SplitProj,
    seg_cross_out: Linear,
    seg_cross_norm: GpuTensor,
    instance_head: Conv2d,
    #[allow(dead_code)]
    semantic_head: Conv2d,
    interactive: InteractiveSam,
    /// PositionEmbeddingSine over the 72×72 memory grid, resident on device.
    det_pos: GpuTensor,
    /// Per-phrase resized text embeddings (`[rows, 256]`). The text tower is
    /// deterministic per phrase, so warm runs re-upload the cached rows
    /// instead of re-running 24 CLIP blocks (Comfy encodes conditioning once,
    /// outside the detect node, for the same reason).
    text_cache: std::sync::Mutex<HashMap<String, (usize, Vec<f32>)>>,
}

impl InteractiveSam {
    fn load(weights: &Sam3Weights) -> Result<Self> {
        const PE: &str = "tracker.model.interactive_sam_prompt_encoder";
        const MD: &str = "tracker.model.interactive_sam_mask_decoder";
        const IC: &str = "detector.backbone.vision_backbone.interactive_convs";
        let fpn = InteractiveFpn {
            conv0_up0: ConvTransposeNoOverlap::load(weights, &format!("{IC}.0.dconv_2x2_0"), SAM3_VISION_DIM, 512, 2)?,
            conv0_up1: ConvTransposeNoOverlap::load(weights, &format!("{IC}.0.dconv_2x2_1"), 512, SAM3_DETECTOR_DIM, 2)?,
            conv0_1x1: Conv2d::load(weights, &format!("{IC}.0.conv_1x1"), SAM3_DETECTOR_DIM, SAM3_DETECTOR_DIM, 1, true)?,
            conv0_3x3: Conv2d::load(weights, &format!("{IC}.0.conv_3x3"), SAM3_DETECTOR_DIM, SAM3_DETECTOR_DIM, 3, true)?,
            conv1_up: ConvTransposeNoOverlap::load(weights, &format!("{IC}.1.dconv_2x2"), SAM3_VISION_DIM, 512, 2)?,
            conv1_1x1: Conv2d::load(weights, &format!("{IC}.1.conv_1x1"), 512, SAM3_DETECTOR_DIM, 1, true)?,
            conv1_3x3: Conv2d::load(weights, &format!("{IC}.1.conv_3x3"), SAM3_DETECTOR_DIM, SAM3_DETECTOR_DIM, 3, true)?,
            conv2_1x1: Conv2d::load(weights, &format!("{IC}.2.conv_1x1"), SAM3_VISION_DIM, SAM3_DETECTOR_DIM, 1, true)?,
            conv2_3x3: Conv2d::load(weights, &format!("{IC}.2.conv_3x3"), SAM3_DETECTOR_DIM, SAM3_DETECTOR_DIM, 3, true)?,
        };
        let no_mem_row =
            weights.f32_shaped("tracker.model.interactivity_no_mem_embed", &[1, 1, SAM3_DETECTOR_DIM])?;
        let mut no_mem_exp = Vec::with_capacity(GRID * GRID * SAM3_DETECTOR_DIM);
        for _ in 0..(GRID * GRID) {
            no_mem_exp.extend_from_slice(&no_mem_row);
        }
        let no_mem = gpu_upload(&no_mem_exp, GRID * GRID, SAM3_DETECTOR_DIM)
            .map_err(DiffusionError::model)?;
        let obj = weights.f32_shaped(&format!("{MD}.obj_score_token.weight"), &[1, SAM3_DETECTOR_DIM])?;
        let iou = weights.f32_shaped(&format!("{MD}.iou_token.weight"), &[1, SAM3_DETECTOR_DIM])?;
        let masks = weights.f32_shaped(&format!("{MD}.mask_tokens.weight"), &[4, SAM3_DETECTOR_DIM])?;
        let nap = weights.f32_shaped(&format!("{PE}.not_a_point_embed.weight"), &[1, SAM3_DETECTOR_DIM])?;
        let mut tok = Vec::with_capacity(8 * SAM3_DETECTOR_DIM);
        tok.extend_from_slice(&obj);
        tok.extend_from_slice(&iou);
        tok.extend_from_slice(&masks);
        tok.extend_from_slice(&nap);
        tok.extend_from_slice(&nap);
        let tokens = gpu_upload(&tok, 8, SAM3_DETECTOR_DIM).map_err(DiffusionError::model)?;
        let pe_matrix = weights.f32_shaped(
            &format!("{PE}.pe_layer.positional_encoding_gaussian_matrix"),
            &[2, 128],
        )?;
        let dense_pe = gpu_upload(&random_pe_grid(&pe_matrix, GRID, GRID), GRID * GRID, SAM3_DETECTOR_DIM)
            .map_err(DiffusionError::model)?;
        let load_twoway = |i: usize, skip: bool| -> Result<TwoWayBlock> {
            let p = format!("{MD}.transformer.layers.{i}");
            Ok(TwoWayBlock {
                self_attn: SamAttn::load(weights, &format!("{p}.self_attn"), SAM3_DETECTOR_DIM, SAM3_DETECTOR_DIM, 8)?,
                t2i: SamAttn::load(weights, &format!("{p}.cross_attn_token_to_image"), SAM3_DETECTOR_DIM, 128, 8)?,
                i2t: SamAttn::load(weights, &format!("{p}.cross_attn_image_to_token"), SAM3_DETECTOR_DIM, 128, 8)?,
                mlp1: Linear::load(weights, &format!("{p}.mlp.lin1"), 2048, SAM3_DETECTOR_DIM)?,
                mlp2: Linear::load(weights, &format!("{p}.mlp.lin2"), SAM3_DETECTOR_DIM, 2048)?,
                norm1: norm_mods(weights, &format!("{p}.norm1"), SAM3_DETECTOR_DIM)?,
                norm2: norm_mods(weights, &format!("{p}.norm2"), SAM3_DETECTOR_DIM)?,
                norm3: norm_mods(weights, &format!("{p}.norm3"), SAM3_DETECTOR_DIM)?,
                norm4: norm_mods(weights, &format!("{p}.norm4"), SAM3_DETECTOR_DIM)?,
                skip_first_pe: skip,
            })
        };
        Ok(Self {
            fpn,
            no_mem,
            tokens,
            dense_pe,
            mask_ds0_w: weights.f32_shaped(&format!("{PE}.mask_downscaling.0.weight"), &[4, 1, 2, 2])?,
            mask_ds0_b: weights.f32_shaped(&format!("{PE}.mask_downscaling.0.bias"), &[4])?,
            mask_ds0_ln: norm_mods(weights, &format!("{PE}.mask_downscaling.1"), 4)?,
            mask_ds1_w: weights.f32_shaped(&format!("{PE}.mask_downscaling.3.weight"), &[16, 4, 2, 2])?,
            mask_ds1_b: weights.f32_shaped(&format!("{PE}.mask_downscaling.3.bias"), &[16])?,
            mask_ds1_ln: norm_mods(weights, &format!("{PE}.mask_downscaling.4"), 16)?,
            mask_ds2: Conv2d::load(weights, &format!("{PE}.mask_downscaling.6"), 16, SAM3_DETECTOR_DIM, 1, true)?,
            layers: [load_twoway(0, true)?, load_twoway(1, false)?],
            final_attn: SamAttn::load(
                weights,
                &format!("{MD}.transformer.final_attn_token_to_image"),
                SAM3_DETECTOR_DIM,
                128,
                8,
            )?,
            final_norm: norm_mods(weights, &format!("{MD}.transformer.norm_final_attn"), SAM3_DETECTOR_DIM)?,
            up0: ConvTransposeNoOverlap::load(weights, &format!("{MD}.output_upscaling.0"), SAM3_DETECTOR_DIM, 64, 2)?,
            up0_ln: norm_mods(weights, &format!("{MD}.output_upscaling.1"), 64)?,
            up1: ConvTransposeNoOverlap::load(weights, &format!("{MD}.output_upscaling.3"), 64, 32, 2)?,
            conv_s0: Conv2d::load(weights, &format!("{MD}.conv_s0"), SAM3_DETECTOR_DIM, 32, 1, true)?,
            conv_s1: Conv2d::load(weights, &format!("{MD}.conv_s1"), SAM3_DETECTOR_DIM, 64, 1, true)?,
            hyper0: HyperMlp {
                l0: Linear::load(weights, &format!("{MD}.output_hypernetworks_mlps.0.layers.0"), SAM3_DETECTOR_DIM, SAM3_DETECTOR_DIM)?,
                l1: Linear::load(weights, &format!("{MD}.output_hypernetworks_mlps.0.layers.1"), SAM3_DETECTOR_DIM, SAM3_DETECTOR_DIM)?,
                l2: Linear::load(weights, &format!("{MD}.output_hypernetworks_mlps.0.layers.2"), 32, SAM3_DETECTOR_DIM)?,
            },
        })
    }
}

impl Sam3Model {
    pub fn prepare(
        weights: &Sam3Weights,
        cancel: Option<Sam3Cancel<'_>>,
        mut progress: Option<ProgressHook>,
    ) -> Result<Self> {
        check_cancel(cancel)?;
        if !gpu_device_available() {
            return Err(DiffusionError::model(
                "sam3 requires makepad-ggml CUDA; no device is available",
            ));
        }
        emit_progress(&mut progress, "load sam3 tokenizer", 0.0)?;
        let tokenizer = ClipTokenizer::new()?;
        emit_progress(&mut progress, "load sam3 vision", 0.05)?;
        let patch = Linear {
            bytes: f32_to_bf16_bytes(&weights.f32_shaped(
                "detector.backbone.vision_backbone.trunk.patch_embed.proj.weight",
                &[SAM3_VISION_DIM, 3, SAM3_PATCH, SAM3_PATCH],
            )?),
            key: "vision.patch".into(),
            n: SAM3_VISION_DIM,
            bias: vec![0.0; SAM3_VISION_DIM],
        };
        let base_pos = weights.f32_shaped(
            "detector.backbone.vision_backbone.trunk.pos_embed",
            &[1, 1 + SAM3_POS_BASE * SAM3_POS_BASE, SAM3_VISION_DIM],
        )?;
        let pos = tile_pos(&base_pos, GRID, GRID)?;
        let pos_spatial = gpu_upload(&pos[SAM3_VISION_DIM..], GRID * GRID, SAM3_VISION_DIM)
            .map_err(DiffusionError::model)?;
        let ln_pre = norm_mods(
            weights,
            "detector.backbone.vision_backbone.trunk.ln_pre",
            SAM3_VISION_DIM,
        )?;
        let mut vision = Vec::with_capacity(SAM3_VISION_DEPTH);
        for i in 0..SAM3_VISION_DEPTH {
            check_cancel(cancel)?;
            emit_progress(
                &mut progress,
                &format!("load sam3 vision {i}/{SAM3_VISION_DEPTH}"),
                0.05 + 0.35 * (i as f64 / SAM3_VISION_DEPTH as f64),
            )?;
            let p = format!("detector.backbone.vision_backbone.trunk.blocks.{i}");
            vision.push(VisionBlock {
                norm1: norm_mods(weights, &format!("{p}.norm1"), SAM3_VISION_DIM)?,
                norm2: norm_mods(weights, &format!("{p}.norm2"), SAM3_VISION_DIM)?,
                qkv: Linear::load(weights, &format!("{p}.attn.qkv"), 3 * SAM3_VISION_DIM, SAM3_VISION_DIM)?,
                proj: Linear::load(weights, &format!("{p}.attn.proj"), SAM3_VISION_DIM, SAM3_VISION_DIM)?,
                fc1: Linear::load(weights, &format!("{p}.mlp.fc1"), SAM3_VISION_MLP, SAM3_VISION_DIM)?,
                fc2: Linear::load(weights, &format!("{p}.mlp.fc2"), SAM3_VISION_DIM, SAM3_VISION_MLP)?,
                global: SAM3_GLOBAL_LAYERS.contains(&i),
            });
        }
        let (fwd, rev) = window_indices(GRID, SAM3_VISION_WINDOW);
        let window_tokens = SAM3_VISION_WINDOW * SAM3_VISION_WINDOW;
        let windows = (GRID / SAM3_VISION_WINDOW) * (GRID / SAM3_VISION_WINDOW);
        let mut fold = vec![0u32; GRID * GRID];
        for (j, slot) in fold.iter_mut().enumerate() {
            *slot = fwd[(j % windows) * window_tokens + j / windows];
        }
        let mut unfold = vec![0u32; GRID * GRID];
        for (r, slot) in unfold.iter_mut().enumerate() {
            let m = rev[r] as usize;
            *slot = ((m % window_tokens) * windows + m / window_tokens) as u32;
        }
        let window_fold = gpu_upload_u32(&fold).map_err(DiffusionError::model)?;
        let window_unfold = gpu_upload_u32(&unfold).map_err(DiffusionError::model)?;
        let (g_cos, g_sin) = rope2d_tables(GRID, GRID, SAM3_POS_BASE as f32 / GRID as f32);
        let (w_cos, w_sin) = rope2d_window_tables(GRID, SAM3_VISION_WINDOW);
        let rope_global_cos =
            gpu_upload(&g_cos, GRID * GRID, VISION_HEAD_DIM / 2).map_err(DiffusionError::model)?;
        let rope_global_sin =
            gpu_upload(&g_sin, GRID * GRID, VISION_HEAD_DIM / 2).map_err(DiffusionError::model)?;
        let rope_window_cos = gpu_upload(
            &w_cos[..window_tokens * VISION_HEAD_DIM / 2],
            window_tokens,
            VISION_HEAD_DIM / 2,
        )
        .map_err(DiffusionError::model)?;
        let rope_window_sin = gpu_upload(
            &w_sin[..window_tokens * VISION_HEAD_DIM / 2],
            window_tokens,
            VISION_HEAD_DIM / 2,
        )
        .map_err(DiffusionError::model)?;

        emit_progress(&mut progress, "load sam3 text", 0.42)?;
        let text_embed = weights.f32_shaped(
            "detector.backbone.language_backbone.encoder.token_embedding.weight",
            &[crate::sam3::SAM3_TEXT_VOCAB, SAM3_TEXT_DIM],
        )?;
        let text_pos = gpu_upload(
            &weights.f32_shaped(
                "detector.backbone.language_backbone.encoder.positional_embedding",
                &[SAM3_TEXT_CTX, SAM3_TEXT_DIM],
            )?,
            SAM3_TEXT_CTX,
            SAM3_TEXT_DIM,
        )
        .map_err(DiffusionError::model)?;
        let mut text_blocks = Vec::with_capacity(SAM3_TEXT_DEPTH);
        for i in 0..SAM3_TEXT_DEPTH {
            check_cancel(cancel)?;
            let p = format!(
                "detector.backbone.language_backbone.encoder.transformer.resblocks.{i}"
            );
            text_blocks.push(TextBlock {
                ln1: norm_mods(weights, &format!("{p}.ln_1"), SAM3_TEXT_DIM)?,
                ln2: norm_mods(weights, &format!("{p}.ln_2"), SAM3_TEXT_DIM)?,
                qkv: Linear::load_in_proj(weights, &format!("{p}.attn"), SAM3_TEXT_DIM)?,
                proj: Linear::load(
                    weights,
                    &format!("{p}.attn.out_proj"),
                    SAM3_TEXT_DIM,
                    SAM3_TEXT_DIM,
                )?,
                fc: Linear::load(weights, &format!("{p}.mlp.c_fc"), SAM3_TEXT_MLP, SAM3_TEXT_DIM)?,
                proj_out: Linear::load(
                    weights,
                    &format!("{p}.mlp.c_proj"),
                    SAM3_TEXT_DIM,
                    SAM3_TEXT_MLP,
                )?,
            });
        }
        let text_ln = norm_mods(
            weights,
            "detector.backbone.language_backbone.encoder.ln_final",
            SAM3_TEXT_DIM,
        )?;
        let text_resize = Linear::load(
            weights,
            "detector.backbone.language_backbone.resizer",
            SAM3_DETECTOR_DIM,
            SAM3_TEXT_DIM,
        )?;

        emit_progress(&mut progress, "load sam3 neck", 0.62)?;
        let conv0_up0 = ConvTransposeNoOverlap::load(
            weights,
            "detector.backbone.vision_backbone.convs.0.dconv_2x2_0",
            SAM3_VISION_DIM,
            512,
            2,
        )?;
        let conv0_up1 = ConvTransposeNoOverlap::load(
            weights,
            "detector.backbone.vision_backbone.convs.0.dconv_2x2_1",
            512,
            SAM3_DETECTOR_DIM,
            2,
        )?;
        let conv0_1x1 = Conv2d::load(
            weights,
            "detector.backbone.vision_backbone.convs.0.conv_1x1",
            SAM3_DETECTOR_DIM,
            SAM3_DETECTOR_DIM,
            1,
            true,
        )?;
        let conv0_3x3 = Conv2d::load(
            weights,
            "detector.backbone.vision_backbone.convs.0.conv_3x3",
            SAM3_DETECTOR_DIM,
            SAM3_DETECTOR_DIM,
            3,
            true,
        )?;
        let conv1_up = ConvTransposeNoOverlap::load(
            weights,
            "detector.backbone.vision_backbone.convs.1.dconv_2x2",
            SAM3_VISION_DIM,
            512,
            2,
        )?;
        let conv1_1x1 = Conv2d::load(
            weights,
            "detector.backbone.vision_backbone.convs.1.conv_1x1",
            512,
            SAM3_DETECTOR_DIM,
            1,
            true,
        )?;
        let conv1_3x3 = Conv2d::load(
            weights,
            "detector.backbone.vision_backbone.convs.1.conv_3x3",
            SAM3_DETECTOR_DIM,
            SAM3_DETECTOR_DIM,
            3,
            true,
        )?;
        let conv2_1x1 = Conv2d::load(
            weights,
            "detector.backbone.vision_backbone.convs.2.conv_1x1",
            SAM3_VISION_DIM,
            SAM3_DETECTOR_DIM,
            1,
            true,
        )?;
        let conv2_3x3 = Conv2d::load(
            weights,
            "detector.backbone.vision_backbone.convs.2.conv_3x3",
            SAM3_DETECTOR_DIM,
            SAM3_DETECTOR_DIM,
            3,
            true,
        )?;

        emit_progress(&mut progress, "load sam3 detector", 0.72)?;
        let mut fusion = Vec::with_capacity(SAM3_FUSION_DEPTH);
        for i in 0..SAM3_FUSION_DEPTH {
            let p = format!("detector.transformer.encoder.layers.{i}");
            fusion.push(FusionLayer {
                self_attn: SplitProj::load_in_proj(weights, &format!("{p}.self_attn"), SAM3_DETECTOR_DIM)?,
                self_out: Linear::load(
                    weights,
                    &format!("{p}.self_attn.out_proj"),
                    SAM3_DETECTOR_DIM,
                    SAM3_DETECTOR_DIM,
                )?,
                cross: SplitProj::load_in_proj(
                    weights,
                    &format!("{p}.cross_attn_image"),
                    SAM3_DETECTOR_DIM,
                )?,
                cross_out: Linear::load(
                    weights,
                    &format!("{p}.cross_attn_image.out_proj"),
                    SAM3_DETECTOR_DIM,
                    SAM3_DETECTOR_DIM,
                )?,
                linear1: Linear::load(weights, &format!("{p}.linear1"), 2048, SAM3_DETECTOR_DIM)?,
                linear2: Linear::load(weights, &format!("{p}.linear2"), SAM3_DETECTOR_DIM, 2048)?,
                norm1: norm_mods(weights, &format!("{p}.norm1"), SAM3_DETECTOR_DIM)?,
                norm2: norm_mods(weights, &format!("{p}.norm2"), SAM3_DETECTOR_DIM)?,
                norm3: norm_mods(weights, &format!("{p}.norm3"), SAM3_DETECTOR_DIM)?,
            });
        }
        let mut decoder = Vec::with_capacity(SAM3_DECODER_DEPTH);
        for i in 0..SAM3_DECODER_DEPTH {
            let p = format!("detector.transformer.decoder.layers.{i}");
            decoder.push(DecoderLayer {
                self_attn: SplitProj::load_in_proj(weights, &format!("{p}.self_attn"), SAM3_DETECTOR_DIM)?,
                self_out: Linear::load(
                    weights,
                    &format!("{p}.self_attn.out_proj"),
                    SAM3_DETECTOR_DIM,
                    SAM3_DETECTOR_DIM,
                )?,
                ca_text: SplitProj::load_in_proj(weights, &format!("{p}.ca_text"), SAM3_DETECTOR_DIM)?,
                ca_text_out: Linear::load(
                    weights,
                    &format!("{p}.ca_text.out_proj"),
                    SAM3_DETECTOR_DIM,
                    SAM3_DETECTOR_DIM,
                )?,
                cross: SplitProj::load_in_proj(weights, &format!("{p}.cross_attn"), SAM3_DETECTOR_DIM)?,
                cross_out: Linear::load(
                    weights,
                    &format!("{p}.cross_attn.out_proj"),
                    SAM3_DETECTOR_DIM,
                    SAM3_DETECTOR_DIM,
                )?,
                linear1: Linear::load(weights, &format!("{p}.linear1"), 2048, SAM3_DETECTOR_DIM)?,
                linear2: Linear::load(weights, &format!("{p}.linear2"), SAM3_DETECTOR_DIM, 2048)?,
                norm1: norm_mods(weights, &format!("{p}.norm1"), SAM3_DETECTOR_DIM)?,
                norm2: norm_mods(weights, &format!("{p}.norm2"), SAM3_DETECTOR_DIM)?,
                norm3: norm_mods(weights, &format!("{p}.norm3"), SAM3_DETECTOR_DIM)?,
                catext_norm: norm_mods(weights, &format!("{p}.catext_norm"), SAM3_DETECTOR_DIM)?,
            });
        }
        emit_progress(&mut progress, "load sam3 heads", 0.90)?;
        let interactive = InteractiveSam::load(weights)?;
        static SAM3_GENERATION: std::sync::atomic::AtomicUsize =
            std::sync::atomic::AtomicUsize::new(0);
        let result = Self {
            tokenizer,
            patch,
            pos_spatial,
            generation: SAM3_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1,
            ln_pre,
            vision,
            window_fold,
            window_unfold,
            rope_global_cos,
            rope_global_sin,
            rope_window_cos,
            rope_window_sin,
            text_embed,
            text_pos,
            text_blocks,
            text_ln,
            text_resize,
            conv0_up0,
            conv0_up1,
            conv0_1x1,
            conv0_3x3,
            conv1_up,
            conv1_1x1,
            conv1_3x3,
            conv2_1x1,
            conv2_3x3,
            fusion,
            decoder,
            decoder_norm: norm_mods(weights, "detector.transformer.decoder.norm", SAM3_DETECTOR_DIM)?,
            query_embed: gpu_upload(
                &weights.f32_shaped(
                    "detector.transformer.decoder.query_embed.weight",
                    &[SAM3_NUM_QUERIES, SAM3_DETECTOR_DIM],
                )?,
                SAM3_NUM_QUERIES,
                SAM3_DETECTOR_DIM,
            )
            .map_err(DiffusionError::model)?,
            presence_token: gpu_upload(
                &weights.f32_shaped(
                    "detector.transformer.decoder.presence_token.weight",
                    &[1, SAM3_DETECTOR_DIM],
                )?,
                1,
                SAM3_DETECTOR_DIM,
            )
            .map_err(DiffusionError::model)?,
            presence_out_norm: norm_mods(
                weights,
                "detector.transformer.decoder.presence_token_out_norm",
                SAM3_DETECTOR_DIM,
            )?,
            presence_h0: Linear::load(
                weights,
                "detector.transformer.decoder.presence_token_head.layers.0",
                SAM3_DETECTOR_DIM,
                SAM3_DETECTOR_DIM,
            )?,
            presence_h1: Linear::load(
                weights,
                "detector.transformer.decoder.presence_token_head.layers.1",
                SAM3_DETECTOR_DIM,
                SAM3_DETECTOR_DIM,
            )?,
            presence_h2: Linear::load(
                weights,
                "detector.transformer.decoder.presence_token_head.layers.2",
                1,
                SAM3_DETECTOR_DIM,
            )?,
            reference_points_sig: {
                let raw = weights.f32_shaped(
                    "detector.transformer.decoder.reference_points.weight",
                    &[SAM3_NUM_QUERIES, 4],
                )?;
                let sig: Vec<f32> = raw.iter().copied().map(sigmoid).collect();
                gpu_upload(&sig, SAM3_NUM_QUERIES, 4).map_err(DiffusionError::model)?
            },
            zero_pos: gpu_upload(&vec![0.0f32; SAM3_DETECTOR_DIM], 1, SAM3_DETECTOR_DIM)
                .map_err(DiffusionError::model)?,
            pad_zeros: gpu_upload(&vec![0.0f32; GRID * GRID * DET_HEAD_DIM], GRID * GRID, DET_HEAD_DIM)
                .map_err(DiffusionError::model)?,
            pad_idx: {
                let mut idx = vec![0u32; SAM3_DETECTOR_HEADS * 64];
                for h in 0..SAM3_DETECTOR_HEADS {
                    for d in 0..64 {
                        idx[h * 64 + d] = if d < DET_HEAD_DIM {
                            (h * DET_HEAD_DIM + d) as u32
                        } else {
                            (SAM3_DETECTOR_DIM + d - DET_HEAD_DIM) as u32
                        };
                    }
                }
                idx
            },
            unpad_idx: {
                let mut idx = vec![0u32; SAM3_DETECTOR_DIM];
                for h in 0..SAM3_DETECTOR_HEADS {
                    for d in 0..DET_HEAD_DIM {
                        idx[h * DET_HEAD_DIM + d] = (h * 64 + d) as u32;
                    }
                }
                idx
            },
            ref_point_h0: Linear::load(
                weights,
                "detector.transformer.decoder.ref_point_head.layers.0",
                SAM3_DETECTOR_DIM,
                SAM3_DETECTOR_DIM * 2,
            )?,
            ref_point_h1: Linear::load(
                weights,
                "detector.transformer.decoder.ref_point_head.layers.1",
                SAM3_DETECTOR_DIM,
                SAM3_DETECTOR_DIM,
            )?,
            rpb_x0: Linear::load(
                weights,
                "detector.transformer.decoder.boxRPB_embed_x.layers.0",
                SAM3_DETECTOR_DIM,
                2,
            )?,
            rpb_x1: Linear::load(
                weights,
                "detector.transformer.decoder.boxRPB_embed_x.layers.1",
                SAM3_DETECTOR_HEADS,
                SAM3_DETECTOR_DIM,
            )?,
            rpb_y0: Linear::load(
                weights,
                "detector.transformer.decoder.boxRPB_embed_y.layers.0",
                SAM3_DETECTOR_DIM,
                2,
            )?,
            rpb_y1: Linear::load(
                weights,
                "detector.transformer.decoder.boxRPB_embed_y.layers.1",
                SAM3_DETECTOR_HEADS,
                SAM3_DETECTOR_DIM,
            )?,
            geo_cls: gpu_upload(
                &weights.f32_shaped("detector.geometry_encoder.cls_embed.weight", &[1, SAM3_DETECTOR_DIM])?,
                1,
                SAM3_DETECTOR_DIM,
            )
            .map_err(DiffusionError::model)?,
            geo_final: Linear::load(
                weights,
                "detector.geometry_encoder.final_proj",
                SAM3_DETECTOR_DIM,
                SAM3_DETECTOR_DIM,
            )?,
            geo_norm: norm_mods(weights, "detector.geometry_encoder.norm", SAM3_DETECTOR_DIM)?,
            geo_encode_norm: norm_mods(
                weights,
                "detector.geometry_encoder.encode_norm",
                SAM3_DETECTOR_DIM,
            )?,
            geo_layers: {
                let mut layers = Vec::with_capacity(3);
                for i in 0..3 {
                    let p = format!("detector.geometry_encoder.encode.{i}");
                    layers.push(FusionLayer {
                        self_attn: SplitProj::load_in_proj(weights, &format!("{p}.self_attn"), SAM3_DETECTOR_DIM)?,
                        self_out: Linear::load(
                            weights,
                            &format!("{p}.self_attn.out_proj"),
                            SAM3_DETECTOR_DIM,
                            SAM3_DETECTOR_DIM,
                        )?,
                        cross: SplitProj::load_in_proj(
                            weights,
                            &format!("{p}.cross_attn_image"),
                            SAM3_DETECTOR_DIM,
                        )?,
                        cross_out: Linear::load(
                            weights,
                            &format!("{p}.cross_attn_image.out_proj"),
                            SAM3_DETECTOR_DIM,
                            SAM3_DETECTOR_DIM,
                        )?,
                        linear1: Linear::load(weights, &format!("{p}.linear1"), 2048, SAM3_DETECTOR_DIM)?,
                        linear2: Linear::load(weights, &format!("{p}.linear2"), SAM3_DETECTOR_DIM, 2048)?,
                        norm1: norm_mods(weights, &format!("{p}.norm1"), SAM3_DETECTOR_DIM)?,
                        norm2: norm_mods(weights, &format!("{p}.norm2"), SAM3_DETECTOR_DIM)?,
                        norm3: norm_mods(weights, &format!("{p}.norm3"), SAM3_DETECTOR_DIM)?,
                    });
                }
                layers
            },
            bbox0: Linear::load(
                weights,
                "detector.transformer.decoder.bbox_embed.layers.0",
                SAM3_DETECTOR_DIM,
                SAM3_DETECTOR_DIM,
            )?,
            bbox1: Linear::load(
                weights,
                "detector.transformer.decoder.bbox_embed.layers.1",
                SAM3_DETECTOR_DIM,
                SAM3_DETECTOR_DIM,
            )?,
            bbox2: Linear::load(
                weights,
                "detector.transformer.decoder.bbox_embed.layers.2",
                4,
                SAM3_DETECTOR_DIM,
            )?,
            hs_proj: Linear::load(
                weights,
                "detector.dot_prod_scoring.hs_proj",
                SAM3_DETECTOR_DIM,
                SAM3_DETECTOR_DIM,
            )?,
            prompt_mlp0: Linear::load(
                weights,
                "detector.dot_prod_scoring.prompt_mlp.layers.0",
                2048,
                SAM3_DETECTOR_DIM,
            )?,
            prompt_mlp1: Linear::load(
                weights,
                "detector.dot_prod_scoring.prompt_mlp.layers.1",
                SAM3_DETECTOR_DIM,
                2048,
            )?,
            prompt_mlp_norm: norm_mods(
                weights,
                "detector.dot_prod_scoring.prompt_mlp.out_norm",
                SAM3_DETECTOR_DIM,
            )?,
            prompt_proj: Linear::load(
                weights,
                "detector.dot_prod_scoring.prompt_proj",
                SAM3_DETECTOR_DIM,
                SAM3_DETECTOR_DIM,
            )?,
            mask_embed0: Linear::load(
                weights,
                "detector.segmentation_head.mask_predictor.mask_embed.layers.0",
                SAM3_DETECTOR_DIM,
                SAM3_DETECTOR_DIM,
            )?,
            mask_embed1: Linear::load(
                weights,
                "detector.segmentation_head.mask_predictor.mask_embed.layers.1",
                SAM3_DETECTOR_DIM,
                SAM3_DETECTOR_DIM,
            )?,
            mask_embed2: Linear::load(
                weights,
                "detector.segmentation_head.mask_predictor.mask_embed.layers.2",
                SAM3_DETECTOR_DIM,
                SAM3_DETECTOR_DIM,
            )?,
            pixel_conv: [
                Conv2d::load(
                    weights,
                    "detector.segmentation_head.pixel_decoder.conv_layers.0",
                    SAM3_DETECTOR_DIM,
                    SAM3_DETECTOR_DIM,
                    3,
                    true,
                )?,
                Conv2d::load(
                    weights,
                    "detector.segmentation_head.pixel_decoder.conv_layers.1",
                    SAM3_DETECTOR_DIM,
                    SAM3_DETECTOR_DIM,
                    3,
                    true,
                )?,
                Conv2d::load(
                    weights,
                    "detector.segmentation_head.pixel_decoder.conv_layers.2",
                    SAM3_DETECTOR_DIM,
                    SAM3_DETECTOR_DIM,
                    3,
                    true,
                )?,
            ],
            pixel_gn_gamma: [
                weights.f32_shaped(
                    "detector.segmentation_head.pixel_decoder.norms.0.weight",
                    &[SAM3_DETECTOR_DIM],
                )?,
                weights.f32_shaped(
                    "detector.segmentation_head.pixel_decoder.norms.1.weight",
                    &[SAM3_DETECTOR_DIM],
                )?,
                weights.f32_shaped(
                    "detector.segmentation_head.pixel_decoder.norms.2.weight",
                    &[SAM3_DETECTOR_DIM],
                )?,
            ],
            pixel_gn_beta: [
                weights.f32_shaped(
                    "detector.segmentation_head.pixel_decoder.norms.0.bias",
                    &[SAM3_DETECTOR_DIM],
                )?,
                weights.f32_shaped(
                    "detector.segmentation_head.pixel_decoder.norms.1.bias",
                    &[SAM3_DETECTOR_DIM],
                )?,
                weights.f32_shaped(
                    "detector.segmentation_head.pixel_decoder.norms.2.bias",
                    &[SAM3_DETECTOR_DIM],
                )?,
            ],
            seg_cross: SplitProj::load_in_proj(
                weights,
                "detector.segmentation_head.cross_attend_prompt",
                SAM3_DETECTOR_DIM,
            )?,
            seg_cross_out: Linear::load(
                weights,
                "detector.segmentation_head.cross_attend_prompt.out_proj",
                SAM3_DETECTOR_DIM,
                SAM3_DETECTOR_DIM,
            )?,
            seg_cross_norm: norm_mods(
                weights,
                "detector.segmentation_head.cross_attn_norm",
                SAM3_DETECTOR_DIM,
            )?,
            instance_head: Conv2d::load(
                weights,
                "detector.segmentation_head.instance_seg_head",
                SAM3_DETECTOR_DIM,
                SAM3_DETECTOR_DIM,
                1,
                true,
            )?,
            interactive,
            semantic_head: Conv2d::load(
                weights,
                "detector.segmentation_head.semantic_seg_head",
                SAM3_DETECTOR_DIM,
                1,
                1,
                true,
            )?,
            det_pos: gpu_upload(
                &sine_pos_hw(GRID, GRID, SAM3_DETECTOR_DIM),
                GRID * GRID,
                SAM3_DETECTOR_DIM,
            )
            .map_err(DiffusionError::model)?,
            text_cache: std::sync::Mutex::new(HashMap::new()),
        };
        emit_progress(&mut progress, "sam3 resident", 1.0)?;
        Ok(result)
    }

    pub fn forward(
        &self,
        image: &Sam3Preprocessed,
        prompt: &Sam3Prompt,
        cancel: Option<Sam3Cancel<'_>>,
        mut progress: Option<ProgressHook>,
    ) -> Result<Sam3Mask> {
        check_cancel(cancel)?;
        emit_progress(&mut progress, "sam3 vision", 0.02)?;
        let t0 = std::time::Instant::now();
        let (tokens_256, fpn) = self.encode_vision(&image.pixels, cancel, &mut progress)?;
        let vision_s = t0.elapsed().as_secs_f64();
        emit_progress(&mut progress, "sam3 detect", 0.55)?;
        let mut union = vec![0.0f32; image.src_width * image.src_height];
        let mut boxes = Vec::new();
        let mut scores = Vec::new();
        let mut phrases = Vec::new();
        let mut raw_scores = Vec::new();
        let mut raw_boxes = Vec::new();
        let phrase_count = prompt.phrases.len().max(1);
        for (index, phrase) in prompt.phrases.iter().enumerate() {
            check_cancel(cancel)?;
            emit_progress(
                &mut progress,
                &format!("sam3 phrase {}/{phrase_count}", index + 1),
                0.55 + 0.40 * (index as f64 / phrase_count as f64),
            )?;
            let t1 = std::time::Instant::now();
            let text = self.encode_text(&phrase.text)?;
            let text_s = t1.elapsed().as_secs_f64();
            let t2 = std::time::Instant::now();
            let text = self.append_geo_cls(&text, &tokens_256, &self.det_pos)?;
            let geo_s = t2.elapsed().as_secs_f64();
            let t3 = std::time::Instant::now();
            let memory = self.fuse(&tokens_256, &text)?;
            let fuse_s = t3.elapsed().as_secs_f64();
            let t4 = std::time::Instant::now();
            let (queries, presence, pred_boxes) = self.decode(&memory, &self.det_pos, &text)?;
            let dec_s = t4.elapsed().as_secs_f64();
            let t5 = std::time::Instant::now();
            let (phrase_scores, phrase_masks) =
                self.heads(&queries, &presence, &text, &memory, &fpn, phrase.max_instances)?;
            let heads_s = t5.elapsed().as_secs_f64();
            let detect_s = vision_s + text_s + geo_s + fuse_s + dec_s + heads_s;
            eprintln!(
                "sam3 phase vision={vision_s:.3} text={text_s:.3} geo={geo_s:.3} fuse={fuse_s:.3} decode={dec_s:.3} heads={heads_s:.3} detect_only={detect_s:.3}"
            );
            if raw_scores.is_empty() {
                raw_scores = phrase_scores.clone();
                raw_boxes = pred_boxes
                    .iter()
                    .map(|&b| unletterbox_box(b, image))
                    .collect();
            }
            let mut order: Vec<usize> = (0..SAM3_NUM_QUERIES).collect();
            order.sort_by(|&a, &b| phrase_scores[b].total_cmp(&phrase_scores[a]));
            let keep = phrase.max_instances.unwrap_or(SAM3_NUM_QUERIES).min(SAM3_NUM_QUERIES);
            let t6 = std::time::Instant::now();
            for &q in order.iter().take(keep) {
                if sigmoid(phrase_scores[q]) < SAM3_SCORE_THRESH && phrase.max_instances.is_none() {
                    break;
                }
                if sigmoid(phrase_scores[q]) < SAM3_SCORE_THRESH {
                    continue;
                }
                let Some(mask288) = phrase_masks.get(&q) else {
                    continue;
                };
                let box_xyxy = unletterbox_box(pred_boxes[q], image);
                let refined = self.refine_mask(image, mask288, box_xyxy, SAM3_REFINE_ITERS)?;
                for (dst, src) in union.iter_mut().zip(refined) {
                    *dst = dst.max(src);
                }
                boxes.push(box_xyxy);
                scores.push(phrase_scores[q]);
                phrases.push(phrase.text.clone());
            }
            eprintln!("sam3 refine={:.3}", t6.elapsed().as_secs_f64());
        }
        emit_progress(&mut progress, "sam3 done", 1.0)?;
        Sam3Mask::with_raw(
            image.src_width,
            image.src_height,
            union,
            boxes,
            scores,
            phrases,
            raw_scores,
            raw_boxes,
        )
    }

    fn encode_trunk(
        &self,
        image: &GpuTensor,
        cancel: Option<Sam3Cancel<'_>>,
        progress: &mut Option<ProgressHook>,
        report: bool,
    ) -> Result<Planar> {
        let tiles = gpu_birefnet_image_to_patches(
            image,
            SAM3_INPUT_SIZE,
            SAM3_INPUT_SIZE,
            SAM3_PATCH,
            SAM3_PATCH,
        )
        .map_err(DiffusionError::model)?;
        let patch_count = GRID * GRID;
        let red = gpu_slice_rows(&tiles, 0, patch_count).map_err(DiffusionError::model)?;
        let green =
            gpu_slice_rows(&tiles, patch_count, patch_count).map_err(DiffusionError::model)?;
        let blue = gpu_slice_rows(&tiles, 2 * patch_count, patch_count)
            .map_err(DiffusionError::model)?;
        let patch_input = gpu_concat_cols(&[&red, &green, &blue]).map_err(DiffusionError::model)?;
        let patches = self.patch.forward_fast(&patch_input)?;
        let mut hidden = gpu_add(&patches, &self.pos_spatial).map_err(DiffusionError::model)?;
        hidden = gpu_layer_norm_mod(&hidden, &self.ln_pre, 0, SAM3_VISION_DIM, SAM3_LN_EPS)
            .map_err(DiffusionError::model)?;
        for (index, block) in self.vision.iter().enumerate() {
            check_cancel(cancel)?;
            if report && index % 4 == 0 {
                emit_progress(
                    progress,
                    &format!("sam3 vision {index}/{SAM3_VISION_DEPTH}"),
                    0.05 + 0.40 * (index as f64 / SAM3_VISION_DEPTH as f64),
                )?;
            }
            hidden = self.vision_block(&hidden, block)?;
        }
        Ok(Planar {
            tensor: gpu_birefnet_tokens_to_planar(&hidden).map_err(DiffusionError::model)?,
            width: GRID,
            height: GRID,
        })
    }

    fn detector_fpn(&self, trunk: &Planar) -> Result<[Planar; 3]> {
        // FPNScaleConv: gelu only after the first 4x dconv; no trailing ReLU.
        let s32 = self.conv2_3x3.forward(&self.conv2_1x1.forward(trunk)?)?;
        let up8 = self.conv1_up.forward(clone_planar(trunk)?)?;
        let s16 = self.conv1_3x3.forward(&self.conv1_1x1.forward(&up8)?)?;
        let up4a = gelu_planar(&self.conv0_up0.forward(clone_planar(trunk)?)?)?;
        let up4 = self.conv0_up1.forward(up4a)?;
        let s8 = self.conv0_3x3.forward(&self.conv0_1x1.forward(&up4)?)?;
        Ok([s8, s16, s32])
    }

    fn interactive_fpn(&self, trunk: &Planar) -> Result<[Planar; 3]> {
        let fpn = &self.interactive.fpn;
        let s32 = fpn.conv2_3x3.forward(&fpn.conv2_1x1.forward(trunk)?)?;
        let up8 = fpn.conv1_up.forward(clone_planar(trunk)?)?;
        let s16 = fpn.conv1_3x3.forward(&fpn.conv1_1x1.forward(&up8)?)?;
        let up4a = gelu_planar(&fpn.conv0_up0.forward(clone_planar(trunk)?)?)?;
        let up4 = fpn.conv0_up1.forward(up4a)?;
        let s8 = fpn.conv0_3x3.forward(&fpn.conv0_1x1.forward(&up4)?)?;
        Ok([s8, s16, s32])
    }

    fn encode_vision(
        &self,
        pixels: &[f32],
        cancel: Option<Sam3Cancel<'_>>,
        progress: &mut Option<ProgressHook>,
    ) -> Result<(GpuTensor, [Planar; 3])> {
        if let Some(result) = self.vision_graph_forward(pixels, cancel, progress)? {
            return Ok(result);
        }
        let image = gpu_upload(pixels, 3, SAM3_INPUT_SIZE * SAM3_INPUT_SIZE)
            .map_err(DiffusionError::model)?;
        self.encode_vision_eager(&image, cancel, progress)
    }

    fn encode_vision_eager(
        &self,
        image: &GpuTensor,
        cancel: Option<Sam3Cancel<'_>>,
        progress: &mut Option<ProgressHook>,
    ) -> Result<(GpuTensor, [Planar; 3])> {
        let planar = self.encode_trunk(image, cancel, progress, true)?;
        let [s8, s16, s32] = self.detector_fpn(&planar)?;
        let tokens_256 = planar_to_tokens(&s32)?;
        Ok((tokens_256, [s8, s16, s32]))
    }

    /// Warm-path CUDA graph replay for trunk + detector FPN (the DA3/flux
    /// pattern): persistent input tensor, two eager runs to settle the
    /// weight caches and activation pool, then one captured graph per model
    /// generation. Returns Ok(None) for first runs or capture failure.
    fn vision_graph_forward(
        &self,
        pixels: &[f32],
        cancel: Option<Sam3Cancel<'_>>,
        progress: &mut Option<ProgressHook>,
    ) -> Result<Option<(GpuTensor, [Planar; 3])>> {
        struct VisionGraphState {
            model: usize,
            image: GpuTensor,
            warm_runs: u32,
            capture_failed: bool,
            graph: Option<(GpuStepGraph, GpuTensor, [GpuTensor; 3])>,
        }
        thread_local! {
            static SAM3_VISION_GRAPH: std::cell::RefCell<Option<VisionGraphState>> =
                const { std::cell::RefCell::new(None) };
        }
        SAM3_VISION_GRAPH.with(|cell| {
            let mut slot = cell.borrow_mut();
            let model = self.generation;
            if slot.as_ref().is_some_and(|state| state.model == model) {
                let state = slot.as_ref().expect("matching sam3 vision graph state");
                gpu_upload_into(&state.image, pixels).map_err(DiffusionError::model)?;
            } else {
                // Drop the old graph first: its Drop unpins the pool buffers.
                *slot = None;
                *slot = Some(VisionGraphState {
                    model,
                    image: gpu_upload(pixels, 3, SAM3_INPUT_SIZE * SAM3_INPUT_SIZE)
                        .map_err(DiffusionError::model)?,
                    warm_runs: 0,
                    capture_failed: false,
                    graph: None,
                });
            }
            let state = slot.as_mut().expect("sam3 vision graph state");
            if let Some((graph, tokens, fpn)) = &state.graph {
                gpu_graph_launch(graph).map_err(DiffusionError::model)?;
                let tokens_out =
                    gpu_slice_rows(tokens, 0, tokens.rows()).map_err(DiffusionError::model)?;
                return Ok(Some((tokens_out, clone_fpn_tensors(fpn)?)));
            }
            if state.warm_runs < 2 || state.capture_failed {
                state.warm_runs = state.warm_runs.saturating_add(1);
                return self.encode_vision_eager(&state.image, cancel, progress).map(Some);
            }
            let captured = gpu_graph_capture(|| {
                let planar = self
                    .encode_trunk(&state.image, None, &mut None, false)
                    .map_err(|err| err.to_string())?;
                let [s8, s16, s32] = self.detector_fpn(&planar).map_err(|err| err.to_string())?;
                let tokens = planar_to_tokens(&s32).map_err(|err| err.to_string())?;
                Ok((tokens, [s8.tensor, s16.tensor, s32.tensor]))
            });
            match captured {
                Ok((graph, (tokens, fpn))) => {
                    gpu_graph_launch(&graph).map_err(DiffusionError::model)?;
                    let tokens_out =
                        gpu_slice_rows(&tokens, 0, tokens.rows()).map_err(DiffusionError::model)?;
                    let fpn_out = clone_fpn_tensors(&fpn)?;
                    state.graph = Some((graph, tokens, fpn));
                    Ok(Some((tokens_out, fpn_out)))
                }
                Err(err) => {
                    eprintln!("sam3 vision graph capture failed ({err}); running eager");
                    state.capture_failed = true;
                    Ok(None)
                }
            }
        })
    }

    /// Same captured-graph pattern for the refine crop trunk + interactive
    /// FPN. The input arrives as a device tensor (crop resize output), so
    /// replay feeds the persistent input with a device-to-device copy.
    fn interactive_graph_forward(&self, crop: &GpuTensor) -> Result<Option<[Planar; 3]>> {
        struct InteractiveGraphState {
            model: usize,
            image: GpuTensor,
            warm_runs: u32,
            capture_failed: bool,
            graph: Option<(GpuStepGraph, [GpuTensor; 3])>,
        }
        thread_local! {
            static SAM3_INTERACTIVE_GRAPH: std::cell::RefCell<Option<InteractiveGraphState>> =
                const { std::cell::RefCell::new(None) };
        }
        SAM3_INTERACTIVE_GRAPH.with(|cell| {
            let mut slot = cell.borrow_mut();
            let model = self.generation;
            if slot.as_ref().is_some_and(|state| state.model == model) {
                let state = slot.as_ref().expect("matching sam3 interactive graph state");
                gpu_copy_into(crop, &state.image).map_err(DiffusionError::model)?;
            } else {
                *slot = None;
                *slot = Some(InteractiveGraphState {
                    model,
                    image: gpu_slice_rows(crop, 0, crop.rows()).map_err(DiffusionError::model)?,
                    warm_runs: 0,
                    capture_failed: false,
                    graph: None,
                });
            }
            let state = slot.as_mut().expect("sam3 interactive graph state");
            if let Some((graph, fpn)) = &state.graph {
                gpu_graph_launch(graph).map_err(DiffusionError::model)?;
                return Ok(Some(clone_fpn_tensors(fpn)?));
            }
            if state.warm_runs < 2 || state.capture_failed {
                state.warm_runs = state.warm_runs.saturating_add(1);
                let trunk = self.encode_trunk(&state.image, None, &mut None, false)?;
                return self.interactive_fpn(&trunk).map(Some);
            }
            let captured = gpu_graph_capture(|| {
                let trunk = self
                    .encode_trunk(&state.image, None, &mut None, false)
                    .map_err(|err| err.to_string())?;
                let [s8, s16, s32] =
                    self.interactive_fpn(&trunk).map_err(|err| err.to_string())?;
                Ok([s8.tensor, s16.tensor, s32.tensor])
            });
            match captured {
                Ok((graph, fpn)) => {
                    gpu_graph_launch(&graph).map_err(DiffusionError::model)?;
                    let fpn_out = clone_fpn_tensors(&fpn)?;
                    state.graph = Some((graph, fpn));
                    Ok(Some(fpn_out))
                }
                Err(err) => {
                    eprintln!("sam3 interactive graph capture failed ({err}); running eager");
                    state.capture_failed = true;
                    Ok(None)
                }
            }
        })
    }

    fn vision_block(&self, hidden: &GpuTensor, block: &VisionBlock) -> Result<GpuTensor> {
        let normed = gpu_layer_norm_mod(hidden, &block.norm1, 0, SAM3_VISION_DIM, SAM3_LN_EPS)
            .map_err(DiffusionError::model)?;
        let qkv = block.qkv.forward_fast(&normed)?;
        let q = gpu_slice_cols(&qkv, 0, SAM3_VISION_DIM).map_err(DiffusionError::model)?;
        let k = gpu_slice_cols(&qkv, SAM3_VISION_DIM, SAM3_VISION_DIM)
            .map_err(DiffusionError::model)?;
        let v = gpu_slice_cols(&qkv, 2 * SAM3_VISION_DIM, SAM3_VISION_DIM)
            .map_err(DiffusionError::model)?;
        let scale = 1.0 / (VISION_HEAD_DIM as f32).sqrt();
        let attended = if block.global {
            let q = gpu_rope_interleaved(
                &q,
                SAM3_VISION_HEADS,
                &self.rope_global_cos,
                &self.rope_global_sin,
            )
            .map_err(DiffusionError::model)?;
            let k = gpu_rope_interleaved(
                &k,
                SAM3_VISION_HEADS,
                &self.rope_global_cos,
                &self.rope_global_sin,
            )
            .map_err(DiffusionError::model)?;
            gpu_attention_packed_flash2_d64(&q, &k, &v, SAM3_VISION_HEADS, scale)
                .map_err(DiffusionError::model)?
        } else {
            windowed_attention(
                &q,
                &k,
                &v,
                &self.window_fold,
                &self.window_unfold,
                &self.rope_window_cos,
                &self.rope_window_sin,
                scale,
            )?
        };
        let update = block.proj.forward_fast(&attended)?;
        let hidden = gpu_add(hidden, &update).map_err(DiffusionError::model)?;
        let normed = gpu_layer_norm_mod(&hidden, &block.norm2, 0, SAM3_VISION_DIM, SAM3_LN_EPS)
            .map_err(DiffusionError::model)?;
        let ff = gpu_gelu_erf(&block.fc1.forward_fast(&normed)?).map_err(DiffusionError::model)?;
        let ff = block.fc2.forward_fast(&ff)?;
        gpu_add(&hidden, &ff).map_err(DiffusionError::model)
    }

    fn encode_text(&self, phrase: &str) -> Result<GpuTensor> {
        if let Some((rows, data)) = self
            .text_cache
            .lock()
            .expect("sam3 text cache lock")
            .get(phrase)
            .cloned()
        {
            return gpu_upload(&data, rows, SAM3_DETECTOR_DIM).map_err(DiffusionError::model);
        }
        let resized = self.encode_text_uncached(phrase)?;
        let host = gpu_download(&resized).map_err(DiffusionError::model)?;
        self.text_cache
            .lock()
            .expect("sam3 text cache lock")
            .insert(phrase.to_string(), (resized.rows(), host));
        Ok(resized)
    }

    fn encode_text_uncached(&self, phrase: &str) -> Result<GpuTensor> {
        let mut ids = self.tokenizer.tokenize(phrase, SAM3_TEXT_CTX, false)?;
        let valid = ids.len().min(SAM3_TEXT_CTX);
        ids.resize(SAM3_TEXT_CTX, SAM3_PAD_ID);
        let mut rows = vec![0.0f32; SAM3_TEXT_CTX * SAM3_TEXT_DIM];
        for (row, &id) in ids.iter().enumerate() {
            let index = id.max(0) as usize;
            if index >= crate::sam3::SAM3_TEXT_VOCAB {
                return Err(DiffusionError::model("sam3 text token out of range"));
            }
            let src = index * SAM3_TEXT_DIM;
            rows[row * SAM3_TEXT_DIM..(row + 1) * SAM3_TEXT_DIM]
                .copy_from_slice(&self.text_embed[src..src + SAM3_TEXT_DIM]);
        }
        let embed = gpu_upload(&rows, SAM3_TEXT_CTX, SAM3_TEXT_DIM).map_err(DiffusionError::model)?;
        let mut hidden = gpu_add(&embed, &self.text_pos).map_err(DiffusionError::model)?;
        // The text tower is CLIP-L run bidirectionally (not causally) with an
        // attention mask and QuickGELU, per the SAM 3 text config. Equivalent
        // here: run the transformer over the unpadded prefix only, so pad-0
        // keys can never leak into the attention.
        hidden = gpu_slice_rows(&hidden, 0, valid.max(1)).map_err(DiffusionError::model)?;
        let scale = 1.0 / (TEXT_HEAD_DIM as f32).sqrt();
        for block in &self.text_blocks {
            let normed = gpu_layer_norm_mod(&hidden, &block.ln1, 0, SAM3_TEXT_DIM, SAM3_LN_EPS)
                .map_err(DiffusionError::model)?;
            let qkv = block.qkv.forward(&normed)?;
            let q = gpu_slice_cols(&qkv, 0, SAM3_TEXT_DIM).map_err(DiffusionError::model)?;
            let k = gpu_slice_cols(&qkv, SAM3_TEXT_DIM, SAM3_TEXT_DIM)
                .map_err(DiffusionError::model)?;
            let v = gpu_slice_cols(&qkv, 2 * SAM3_TEXT_DIM, SAM3_TEXT_DIM)
                .map_err(DiffusionError::model)?;
            let attended = gpu_attention_packed(&q, &k, &v, SAM3_TEXT_HEADS, scale)
                .map_err(DiffusionError::model)?;
            let update = block.proj.forward(&attended)?;
            hidden = gpu_add(&hidden, &update).map_err(DiffusionError::model)?;
            let normed = gpu_layer_norm_mod(&hidden, &block.ln2, 0, SAM3_TEXT_DIM, SAM3_LN_EPS)
                .map_err(DiffusionError::model)?;
            let ff = block.fc.forward(&normed)?;
            let ff = quick_gelu_tensor(&ff)?;
            let ff = block.proj_out.forward(&ff)?;
            hidden = gpu_add(&hidden, &ff).map_err(DiffusionError::model)?;
        }
        let hidden = gpu_layer_norm_mod(&hidden, &self.text_ln, 0, SAM3_TEXT_DIM, SAM3_LN_EPS)
            .map_err(DiffusionError::model)?;
        let resized = self.text_resize.forward(&hidden)?;
        Ok(resized)
    }

    fn fuse(&self, image: &GpuTensor, text: &GpuTensor) -> Result<GpuTensor> {
        let mut hidden =
            gpu_slice_rows(image, 0, image.rows()).map_err(DiffusionError::model)?;
        let pos = &self.det_pos;
        let scale = 1.0 / (DET_HEAD_DIM as f32).sqrt();
        for layer in &self.fusion {
            let normed = gpu_layer_norm_mod(&hidden, &layer.norm1, 0, SAM3_DETECTOR_DIM, SAM3_LN_EPS)
                .map_err(DiffusionError::model)?;
            let qk = gpu_add(&normed, pos).map_err(DiffusionError::model)?;
            // Self-attention over all 5184 memory tokens: pad the 32-wide
            // heads to 64 and use the flash kernel instead of materializing
            // the 5184x5184 score matrix per head.
            let q = layer.self_attn.q.forward(&qk)?;
            let k = layer.self_attn.k.forward(&qk)?;
            let v = layer.self_attn.v.forward(&normed)?;
            let attended = self.flash_d32(&q, &k, &v, scale)?;
            let update = layer.self_out.forward(&attended)?;
            hidden = gpu_add(&hidden, &update).map_err(DiffusionError::model)?;
            let normed = gpu_layer_norm_mod(&hidden, &layer.norm2, 0, SAM3_DETECTOR_DIM, SAM3_LN_EPS)
                .map_err(DiffusionError::model)?;
            hidden = residual_cross(
                &normed,
                &hidden,
                text,
                &layer.cross,
                &layer.cross_out,
                scale,
            )?;
            let normed = gpu_layer_norm_mod(&hidden, &layer.norm3, 0, SAM3_DETECTOR_DIM, SAM3_LN_EPS)
                .map_err(DiffusionError::model)?;
            let ff = gpu_birefnet_relu(&layer.linear1.forward(&normed)?)
                .map_err(DiffusionError::model)?;
            let ff = layer.linear2.forward(&ff)?;
            hidden = gpu_add(&hidden, &ff).map_err(DiffusionError::model)?;
        }
        Ok(hidden)
    }

    /// d32 heads through the d64 flash kernel: zero-pad each head's columns
    /// to 64 (zero K/Q lanes leave the scores unchanged; zero V lanes are
    /// dropped again on the way out). Uses the caller's explicit scale.
    fn flash_d32(
        &self,
        q: &GpuTensor,
        k: &GpuTensor,
        v: &GpuTensor,
        scale: f32,
    ) -> Result<GpuTensor> {
        let pad = |x: &GpuTensor| -> Result<GpuTensor> {
            let cat = gpu_concat_cols(&[x, &self.pad_zeros]).map_err(DiffusionError::model)?;
            gpu_gather_cols(&cat, &self.pad_idx).map_err(DiffusionError::model)
        };
        let attended =
            gpu_attention_packed_flash2_d64(&pad(q)?, &pad(k)?, &pad(v)?, SAM3_DETECTOR_HEADS, scale)
                .map_err(DiffusionError::model)?;
        gpu_gather_cols(&attended, &self.unpad_idx).map_err(DiffusionError::model)
    }

    fn append_geo_cls(
        &self,
        text: &GpuTensor,
        image: &GpuTensor,
        image_pos: &GpuTensor,
    ) -> Result<GpuTensor> {
        let scale = 1.0 / (DET_HEAD_DIM as f32).sqrt();
        let mut cls = self.geo_final.forward(&self.geo_cls)?;
        cls = gpu_layer_norm_mod(&cls, &self.geo_norm, 0, SAM3_DETECTOR_DIM, SAM3_LN_EPS)
            .map_err(DiffusionError::model)?;
        let memory_k = gpu_add(image, image_pos).map_err(DiffusionError::model)?;
        for layer in &self.geo_layers {
            let normed = gpu_layer_norm_mod(&cls, &layer.norm1, 0, SAM3_DETECTOR_DIM, SAM3_LN_EPS)
                .map_err(DiffusionError::model)?;
            cls = residual_self(&normed, &cls, &layer.self_attn, &layer.self_out, scale)?;
            let normed = gpu_layer_norm_mod(&cls, &layer.norm2, 0, SAM3_DETECTOR_DIM, SAM3_LN_EPS)
                .map_err(DiffusionError::model)?;
            cls = residual_cross_kv(
                &normed,
                &cls,
                &memory_k,
                image,
                &layer.cross,
                &layer.cross_out,
                scale,
            )?;
            let normed = gpu_layer_norm_mod(&cls, &layer.norm3, 0, SAM3_DETECTOR_DIM, SAM3_LN_EPS)
                .map_err(DiffusionError::model)?;
            let ff = gpu_birefnet_relu(&layer.linear1.forward(&normed)?)
                .map_err(DiffusionError::model)?;
            let ff = layer.linear2.forward(&ff)?;
            cls = gpu_add(&cls, &ff).map_err(DiffusionError::model)?;
        }
        cls = gpu_layer_norm_mod(&cls, &self.geo_encode_norm, 0, SAM3_DETECTOR_DIM, SAM3_LN_EPS)
            .map_err(DiffusionError::model)?;
        gpu_concat_rows(text, &cls).map_err(DiffusionError::model)
    }

    fn decode(
        &self,
        memory: &GpuTensor,
        memory_pos: &GpuTensor,
        text: &GpuTensor,
    ) -> Result<(GpuTensor, GpuTensor, Vec<[f32; 4]>)> {
        let scale = 1.0 / (DET_HEAD_DIM as f32).sqrt();
        let mut tgt = gpu_slice_rows(&self.query_embed, 0, SAM3_NUM_QUERIES)
            .map_err(DiffusionError::model)?;
        let mut presence = gpu_slice_rows(&self.presence_token, 0, 1).map_err(DiffusionError::model)?;
        let mut ref_points = gpu_slice_rows(&self.reference_points_sig, 0, SAM3_NUM_QUERIES)
            .map_err(DiffusionError::model)?;
        let memory_k = gpu_add(memory, memory_pos).map_err(DiffusionError::model)?;
        for (layer_idx, layer) in self.decoder.iter().enumerate() {
            let query_pos = self.query_pos_from_refs(&ref_points)?;
            let hidden = gpu_concat_rows(&presence, &tgt).map_err(DiffusionError::model)?;
            let pos = gpu_concat_rows(&self.zero_pos, &query_pos).map_err(DiffusionError::model)?;
            let qk = gpu_add(&hidden, &pos).map_err(DiffusionError::model)?;
            let attn = residual_self_qk_out(&qk, &hidden, &layer.self_attn, &layer.self_out, scale)?;
            let hidden = gpu_layer_norm_mod(
                &gpu_add(&hidden, &attn).map_err(DiffusionError::model)?,
                &layer.norm2,
                0,
                SAM3_DETECTOR_DIM,
                SAM3_LN_EPS,
            )
            .map_err(DiffusionError::model)?;
            let q_text = gpu_add(&hidden, &pos).map_err(DiffusionError::model)?;
            let attn = residual_cross_out(&q_text, text, &layer.ca_text, &layer.ca_text_out, scale)?;
            let hidden = gpu_layer_norm_mod(
                &gpu_add(&hidden, &attn).map_err(DiffusionError::model)?,
                &layer.catext_norm,
                0,
                SAM3_DETECTOR_DIM,
                SAM3_LN_EPS,
            )
            .map_err(DiffusionError::model)?;
            let q_img = gpu_add(&hidden, &pos).map_err(DiffusionError::model)?;
            let rpb = self.box_rpb(&ref_points, GRID, GRID)?;
            let attn = residual_cross_rpb(
                &q_img,
                &memory_k,
                memory,
                &layer.cross,
                &layer.cross_out,
                scale,
                &rpb,
            )?;
            let hidden = gpu_layer_norm_mod(
                &gpu_add(&hidden, &attn).map_err(DiffusionError::model)?,
                &layer.norm1,
                0,
                SAM3_DETECTOR_DIM,
                SAM3_LN_EPS,
            )
            .map_err(DiffusionError::model)?;
            let ff = gpu_birefnet_relu(&layer.linear1.forward(&hidden)?)
                .map_err(DiffusionError::model)?;
            let ff = layer.linear2.forward(&ff)?;
            let hidden = gpu_layer_norm_mod(
                &gpu_add(&hidden, &ff).map_err(DiffusionError::model)?,
                &layer.norm3,
                0,
                SAM3_DETECTOR_DIM,
                SAM3_LN_EPS,
            )
            .map_err(DiffusionError::model)?;
            presence = gpu_slice_rows(&hidden, 0, 1).map_err(DiffusionError::model)?;
            tgt = gpu_slice_rows(&hidden, 1, SAM3_NUM_QUERIES).map_err(DiffusionError::model)?;
            if layer_idx + 1 < self.decoder.len() {
                let normed = gpu_layer_norm_mod(&tgt, &self.decoder_norm, 0, SAM3_DETECTOR_DIM, SAM3_LN_EPS)
                    .map_err(DiffusionError::model)?;
                let delta = self.bbox_delta(&normed)?;
                ref_points = gpu_sam3_refine_boxes(&ref_points, &delta)
                    .map_err(DiffusionError::model)?;
            }
        }
        let queries = gpu_layer_norm_mod(&tgt, &self.decoder_norm, 0, SAM3_DETECTOR_DIM, SAM3_LN_EPS)
            .map_err(DiffusionError::model)?;
        let delta = self.bbox_delta(&queries)?;
        let final_sig = gpu_sam3_refine_boxes(&ref_points, &delta).map_err(DiffusionError::model)?;
        let host = gpu_download(&final_sig).map_err(DiffusionError::model)?;
        let mut boxes = Vec::with_capacity(SAM3_NUM_QUERIES);
        for q in 0..SAM3_NUM_QUERIES {
            boxes.push(cxcywh_to_xyxy([
                host[q * 4],
                host[q * 4 + 1],
                host[q * 4 + 2],
                host[q * 4 + 3],
            ]));
        }
        Ok((queries, presence, boxes))
    }

    fn query_pos_from_refs(&self, ref_points: &GpuTensor) -> Result<GpuTensor> {
        let sine = gpu_sam3_sine_embed(ref_points, SAM3_DETECTOR_DIM / 2)
            .map_err(DiffusionError::model)?;
        let hidden = gpu_birefnet_relu(&self.ref_point_h0.forward(&sine)?)
            .map_err(DiffusionError::model)?;
        self.ref_point_h1.forward(&hidden)
    }

    fn bbox_delta(&self, queries: &GpuTensor) -> Result<GpuTensor> {
        let h = gpu_birefnet_relu(&self.bbox0.forward(queries)?).map_err(DiffusionError::model)?;
        let h = gpu_birefnet_relu(&self.bbox1.forward(&h)?).map_err(DiffusionError::model)?;
        self.bbox2.forward(&h)
    }

    fn box_rpb(&self, ref_points: &GpuTensor, height: usize, width: usize) -> Result<GpuTensor> {
        // [heads, Q+1, H*W] assembled on device from axial MLPs.
        let (dx_g, dy_g) =
            gpu_sam3_rpb_axial(ref_points, width, height).map_err(DiffusionError::model)?;
        let rx = gpu_birefnet_relu(&self.rpb_x0.forward(&dx_g)?).map_err(DiffusionError::model)?;
        let rx = self.rpb_x1.forward(&rx)?;
        let ry = gpu_birefnet_relu(&self.rpb_y0.forward(&dy_g)?).map_err(DiffusionError::model)?;
        let ry = self.rpb_y1.forward(&ry)?;
        gpu_rpb_expand(
            &ry,
            &rx,
            height,
            width,
            SAM3_NUM_QUERIES,
            SAM3_DETECTOR_HEADS,
        )
        .map_err(DiffusionError::model)
    }

    fn heads(
        &self,
        queries: &GpuTensor,
        presence: &GpuTensor,
        text: &GpuTensor,
        memory: &GpuTensor,
        fpn: &[Planar; 3],
        max_instances: Option<usize>,
    ) -> Result<(Vec<f32>, HashMap<usize, Vec<f32>>)> {
        let _presence = {
            let presence = gpu_layer_norm_mod(
                presence,
                &self.presence_out_norm,
                0,
                SAM3_DETECTOR_DIM,
                SAM3_LN_EPS,
            )
            .map_err(DiffusionError::model)?;
            let presence = gpu_birefnet_relu(&self.presence_h0.forward(&presence)?)
                .map_err(DiffusionError::model)?;
            let presence = gpu_birefnet_relu(&self.presence_h1.forward(&presence)?)
                .map_err(DiffusionError::model)?;
            gpu_download(&self.presence_h2.forward(&presence)?).map_err(DiffusionError::model)?
        };

        let prompt = gpu_birefnet_relu(&self.prompt_mlp0.forward(text)?)
            .map_err(DiffusionError::model)?;
        let prompt = self.prompt_mlp1.forward(&prompt)?;
        let prompt = gpu_add(&prompt, text).map_err(DiffusionError::model)?;
        let prompt = gpu_layer_norm_mod(
            &prompt,
            &self.prompt_mlp_norm,
            0,
            SAM3_DETECTOR_DIM,
            SAM3_LN_EPS,
        )
        .map_err(DiffusionError::model)?;
        let prompt_host = gpu_download(&prompt).map_err(DiffusionError::model)?;
        let prompt_rows = prompt.rows().max(1);
        let mut pooled = vec![0.0f32; SAM3_DETECTOR_DIM];
        for row in 0..prompt_rows {
            for c in 0..SAM3_DETECTOR_DIM {
                pooled[c] += prompt_host[row * SAM3_DETECTOR_DIM + c];
            }
        }
        for c in pooled.iter_mut() {
            *c /= prompt_rows as f32;
        }
        let pooled_g = gpu_upload(&pooled, 1, SAM3_DETECTOR_DIM).map_err(DiffusionError::model)?;
        let pp = gpu_download(&self.prompt_proj.forward(&pooled_g)?).map_err(DiffusionError::model)?;
        let hs = gpu_download(&self.hs_proj.forward(queries)?).map_err(DiffusionError::model)?;
        let mut scores = Vec::with_capacity(SAM3_NUM_QUERIES);
        let scale = 1.0 / (SAM3_DETECTOR_DIM as f32).sqrt();
        for q in 0..SAM3_NUM_QUERIES {
            let mut dot = 0.0f32;
            let row = q * SAM3_DETECTOR_DIM;
            for c in 0..SAM3_DETECTOR_DIM {
                dot += hs[row + c] * pp[c];
            }
            scores.push((dot * scale).clamp(-12.0, 12.0));
        }

        let mut fpn_levels = [
            clone_planar(&fpn[0])?,
            clone_planar(&fpn[1])?,
            clone_planar(&fpn[2])?,
        ];
        let enc_normed = gpu_layer_norm_mod(memory, &self.seg_cross_norm, 0, SAM3_DETECTOR_DIM, SAM3_LN_EPS)
            .map_err(DiffusionError::model)?;
        let enc_cross = residual_cross_out(&enc_normed, text, &self.seg_cross, &self.seg_cross_out, scale)?;
        let enc = gpu_add(memory, &enc_cross).map_err(DiffusionError::model)?;
        // encoder tokens are [HW, C]; planar is [C, HW] — transpose on device.
        let enc_tokens = gpu_slice_rows(&enc, 0, GRID * GRID).map_err(DiffusionError::model)?;
        fpn_levels[2] = Planar {
            tensor: gpu_birefnet_tokens_to_planar(&enc_tokens).map_err(DiffusionError::model)?,
            width: GRID,
            height: GRID,
        };

        let pixel = self.pixel_decoder(&fpn_levels)?;
        let features = self.instance_head.forward(&pixel)?;
        let feat_tokens = planar_to_tokens(&features)?;
        let embed = gpu_birefnet_relu(&self.mask_embed0.forward(queries)?)
            .map_err(DiffusionError::model)?;
        let embed = gpu_birefnet_relu(&self.mask_embed1.forward(&embed)?)
            .map_err(DiffusionError::model)?;
        let embed = self.mask_embed2.forward(&embed)?;
        let embed_t = gpu_download(&embed).map_err(DiffusionError::model)?;
        let spatial = features.width * features.height;
        let mut order: Vec<usize> = (0..SAM3_NUM_QUERIES).collect();
        order.sort_by(|&a, &b| scores[b].total_cmp(&scores[a]));
        let keep_n = max_instances.unwrap_or(4).min(SAM3_NUM_QUERIES).max(1);
        let mut kept: Vec<usize> = Vec::new();
        for &q in order.iter().take(keep_n) {
            if sigmoid(scores[q]) < SAM3_SCORE_THRESH && max_instances.is_none() && !kept.is_empty()
            {
                break;
            }
            kept.push(q);
        }
        // mask[q] = feat · embed[q] as one GEMM over the kept queries.
        let mut embed_kept = vec![0.0f32; kept.len() * SAM3_DETECTOR_DIM];
        for (i, &q) in kept.iter().enumerate() {
            embed_kept[i * SAM3_DETECTOR_DIM..(i + 1) * SAM3_DETECTOR_DIM]
                .copy_from_slice(&embed_t[q * SAM3_DETECTOR_DIM..(q + 1) * SAM3_DETECTOR_DIM]);
        }
        let embed_kept_g = gpu_upload(&embed_kept, kept.len(), SAM3_DETECTOR_DIM)
            .map_err(DiffusionError::model)?;
        let mask_all = gpu_linear_f32_resident(&feat_tokens, &embed_kept_g, None)
            .map_err(DiffusionError::model)?;
        let mask_host = gpu_download(&mask_all).map_err(DiffusionError::model)?;
        let mut masks = HashMap::new();
        for (i, &q) in kept.iter().enumerate() {
            let mut mask = vec![0.0f32; spatial];
            for pix in 0..spatial {
                mask[pix] = mask_host[pix * kept.len() + i];
            }
            masks.insert(q, mask);
        }
        Ok((scores, masks))
    }

    fn pixel_decoder(&self, fpn: &[Planar; 3]) -> Result<Planar> {
        // MaskFormer-style top-down FPN (HF `Sam3PixelDecoder`): start at the
        // coarsest level, then walk to the finest, at each step nearest-
        // upsampling what we have, adding the skip, and running conv/GN/ReLU.
        let mut prev = clone_planar(&fpn[2])?;
        for (i, feat) in [1usize, 0].into_iter().enumerate() {
            let up = nearest2x(&prev)?;
            let added = add_planar(&fpn[feat], &up)?;
            let conv = self.pixel_conv[i].forward(&added)?;
            let normed = group_norm_planar(
                &conv,
                8,
                &self.pixel_gn_gamma[i],
                &self.pixel_gn_beta[i],
                &format!("pixel_gn_{i}"),
            )?;
            prev = relu_planar(&normed)?;
        }
        Ok(prev)
    }

    /// Sharpen one detector mask by re-running the interactive SAM head on a
    /// zoomed crop around the detection.
    ///
    /// The detector only ever emits logits on the 288² grid, which is coarse
    /// at source resolution. So: cut a window around the box, blow it up to
    /// the 1008² trunk input, seed the interactive head with the slice of the
    /// coarse logits that covers the same window, and let it iterate. What
    /// comes back is pasted into a source-sized canvas and merged with the
    /// upscaled coarse mask, both taken at their `> 0` sign.
    ///
    /// `iterations == 0` (and every degenerate-window bail-out) falls back to
    /// the plain upscaled coarse mask.
    fn refine_mask(
        &self,
        image: &Sam3Preprocessed,
        coarse_288: &[f32],
        box_xyxy: [f32; 4],
        iterations: usize,
    ) -> Result<Vec<f32>> {
        let src_w = image.src_width;
        let src_h = image.src_height;
        let logit_side = GRID * 4;

        let coarse_dev =
            gpu_upload(coarse_288, 1, logit_side * logit_side).map_err(DiffusionError::model)?;
        let coarse_at_src =
            gpu_birefnet_resize_bilinear(&coarse_dev, logit_side, logit_side, src_w, src_h, false)
                .map_err(DiffusionError::model)?;
        let coarse_at_src = gpu_download(&coarse_at_src).map_err(DiffusionError::model)?;
        // Only the bail-out paths materialize a binary mask of their own; the
        // refined path folds the sign test into the closing merge.
        let sign_of = |logits: &[f32]| -> Vec<f32> {
            logits
                .iter()
                .map(|&value| f32::from(u8::from(value > 0.0)))
                .collect()
        };
        if iterations == 0 {
            return Ok(sign_of(&coarse_at_src));
        }

        // Both windows are host-only arithmetic, so resolve them before any
        // device work: a degenerate one means there is nothing to refine.
        let Some(src_win) = zoom_window(box_xyxy, src_w, src_h) else {
            return Ok(sign_of(&coarse_at_src));
        };
        let Some(logit_win) = rescale_window(src_win, src_w, src_h, logit_side, logit_side) else {
            return Ok(sign_of(&coarse_at_src));
        };
        let (zoom_w, zoom_h) = (src_win.width(), src_win.height());

        // Gather the window straight into planar RGB and let the device
        // bilinear do the enlargement; it matches stretch_hwc_to_planar
        // (half-pixel centres, clamped to the window) exactly.
        let plane = zoom_w * zoom_h;
        let mut zoom_rgb = vec![0.0f32; 3 * plane];
        for row in 0..zoom_h {
            let src = ((src_win.top + row) * src_w + src_win.left) * 3;
            let dst = row * zoom_w;
            for column in 0..zoom_w {
                let src = src + column * 3;
                let dst = dst + column;
                zoom_rgb[dst] = image.src_rgb[src];
                zoom_rgb[plane + dst] = image.src_rgb[src + 1];
                zoom_rgb[2 * plane + dst] = image.src_rgb[src + 2];
            }
        }
        let zoom_dev = gpu_upload(&zoom_rgb, 3, plane).map_err(DiffusionError::model)?;
        let zoom_1008 = gpu_birefnet_resize_bilinear(
            &zoom_dev,
            zoom_w,
            zoom_h,
            SAM3_INPUT_SIZE,
            SAM3_INPUT_SIZE,
            false,
        )
        .map_err(DiffusionError::model)?;

        // Seed logits: the same window read out of the coarse 288² grid.
        let (seed_w, seed_h) = (logit_win.width(), logit_win.height());
        let mut seed = vec![0.0f32; seed_w * seed_h];
        for (row, dst) in seed.chunks_exact_mut(seed_w).enumerate() {
            let src = (logit_win.top + row) * logit_side + logit_win.left;
            dst.copy_from_slice(&coarse_288[src..src + seed_w]);
        }

        let zoom_fpn = match self.interactive_graph_forward(&zoom_1008)? {
            Some(fpn) => fpn,
            None => {
                let trunk = self.encode_trunk(&zoom_1008, None, &mut None, false)?;
                self.interactive_fpn(&trunk)?
            }
        };

        // Each pass hands the head a 288² logit plane; the head answers at the
        // same resolution, so only the first pass has to grow the seed.
        let mut logits = gpu_upload(&seed, 1, seed_w * seed_h).map_err(DiffusionError::model)?;
        let mut logits_w = seed_w;
        let mut logits_h = seed_h;
        for _ in 0..iterations {
            let at_1008 = gpu_birefnet_resize_bilinear(
                &logits,
                logits_w,
                logits_h,
                SAM3_INPUT_SIZE,
                SAM3_INPUT_SIZE,
                false,
            )
            .map_err(DiffusionError::model)?;
            let at_288 = gpu_birefnet_resize_bilinear(
                &at_1008,
                SAM3_INPUT_SIZE,
                SAM3_INPUT_SIZE,
                logit_side,
                logit_side,
                false,
            )
            .map_err(DiffusionError::model)?;
            logits = self.forward_segment(&zoom_fpn, &at_288)?;
            logits_w = logit_side;
            logits_h = logit_side;
        }

        // Back down the same ladder the input came up, to window pixels.
        let at_1008 = gpu_birefnet_resize_bilinear(
            &logits,
            logits_w,
            logits_h,
            SAM3_INPUT_SIZE,
            SAM3_INPUT_SIZE,
            false,
        )
        .map_err(DiffusionError::model)?;
        let at_zoom = gpu_birefnet_resize_bilinear(
            &at_1008,
            SAM3_INPUT_SIZE,
            SAM3_INPUT_SIZE,
            zoom_w,
            zoom_h,
            false,
        )
        .map_err(DiffusionError::model)?;
        let at_zoom = gpu_download(&at_zoom).map_err(DiffusionError::model)?;

        let mut merged = vec![0.0f32; src_h * src_w];
        for (row, refined) in at_zoom.chunks_exact(zoom_w).take(zoom_h).enumerate() {
            let dst = (src_win.top + row) * src_w + src_win.left;
            merged[dst..dst + zoom_w].copy_from_slice(refined);
        }
        for (slot, &coarse) in merged.iter_mut().zip(coarse_at_src.iter()) {
            *slot = f32::from(u8::from(*slot > 0.0 || coarse > 0.0));
        }
        Ok(merged)
    }

    /// `mask288` is a planar `[1, 288*288]` device tensor; the refined mask
    /// logits come back in the same layout.
    fn forward_segment(&self, ifpn: &[Planar; 3], mask288: &GpuTensor) -> Result<GpuTensor> {
        let sam = &self.interactive;
        let dense = self.mask_downscale(mask288)?;
        let s32_tokens = planar_to_tokens(&ifpn[2])?;
        let s32_tokens = gpu_add(&s32_tokens, &sam.no_mem).map_err(DiffusionError::model)?;
        let dense_tokens = planar_to_tokens(&dense)?;
        let src = gpu_add(&s32_tokens, &dense_tokens).map_err(DiffusionError::model)?;
        let mut queries = gpu_slice_rows(&sam.tokens, 0, 8).map_err(DiffusionError::model)?;
        let mut keys = src;
        let q_pe = &sam.tokens;
        let k_pe = &sam.dense_pe;
        for block in &sam.layers {
            let (q, k) = self.twoway_block(&queries, &keys, q_pe, k_pe, block)?;
            queries = q;
            keys = k;
        }
        let scale_cross = 1.0 / 16f32.sqrt();
        let q = gpu_add(&queries, q_pe).map_err(DiffusionError::model)?;
        let k = gpu_add(&keys, k_pe).map_err(DiffusionError::model)?;
        let att = sam.final_attn.forward(&q, &k, &keys, scale_cross)?;
        let queries = gpu_add(&queries, &att).map_err(DiffusionError::model)?;
        let queries = gpu_layer_norm_mod(&queries, &sam.final_norm, 0, SAM3_DETECTOR_DIM, SAM3_LN_EPS)
            .map_err(DiffusionError::model)?;
        let src_out = Planar {
            tensor: gpu_birefnet_tokens_to_planar(&keys).map_err(DiffusionError::model)?,
            width: GRID,
            height: GRID,
        };
        let d1 = sam.up0.forward(src_out)?;
        let s1 = sam.conv_s1.forward(&ifpn[1])?;
        let up1 = gelu_planar(&norm_planar_dim(&add_planar(&d1, &s1)?, &sam.up0_ln, 64)?)?;
        let d2 = sam.up1.forward(up1)?;
        let s0 = sam.conv_s0.forward(&ifpn[0])?;
        let up2 = gelu_planar(&add_planar(&d2, &s0)?)?;
        let mt0 = gpu_slice_rows(&queries, 2, 1).map_err(DiffusionError::model)?;
        let h = gpu_birefnet_relu(&sam.hyper0.l0.forward(&mt0)?).map_err(DiffusionError::model)?;
        let h = gpu_birefnet_relu(&sam.hyper0.l1.forward(&h)?).map_err(DiffusionError::model)?;
        let h = sam.hyper0.l2.forward(&h)?;
        let feat = planar_to_tokens(&up2)?;
        // mask[pix] = feat[pix] · h — one GEMV, planarized back to [1, HW].
        let mask_tokens = gpu_linear_f32_resident(&feat, &h, None).map_err(DiffusionError::model)?;
        gpu_birefnet_tokens_to_planar(&mask_tokens).map_err(DiffusionError::model)
    }

    fn mask_downscale(&self, mask288: &GpuTensor) -> Result<Planar> {
        let sam = &self.interactive;
        let side = GRID * 4;
        let c0 = crate::backend::gpu_conv2d_planar_strided(
            mask288,
            side,
            side,
            side / 2,
            side / 2,
            SAM3_CACHE_NAMESPACE,
            "interactive.mask_ds0",
            &sam.mask_ds0_w,
            &sam.mask_ds0_b,
            4,
            2,
            2,
            0,
            0,
            2,
            2,
        )
        .map_err(DiffusionError::model)?;
        let p0 = Planar {
            tensor: c0,
            width: side / 2,
            height: side / 2,
        };
        let p0 = gelu_planar(&norm_planar_dim(&p0, &sam.mask_ds0_ln, 4)?)?;
        let c1 = crate::backend::gpu_conv2d_planar_strided(
            &p0.tensor,
            side / 2,
            side / 2,
            side / 4,
            side / 4,
            SAM3_CACHE_NAMESPACE,
            "interactive.mask_ds1",
            &sam.mask_ds1_w,
            &sam.mask_ds1_b,
            16,
            2,
            2,
            0,
            0,
            2,
            2,
        )
        .map_err(DiffusionError::model)?;
        let p1 = Planar {
            tensor: c1,
            width: side / 4,
            height: side / 4,
        };
        let p1 = gelu_planar(&norm_planar_dim(&p1, &sam.mask_ds1_ln, 16)?)?;
        sam.mask_ds2.forward(&p1)
    }

    fn twoway_block(
        &self,
        queries: &GpuTensor,
        keys: &GpuTensor,
        q_pe: &GpuTensor,
        k_pe: &GpuTensor,
        block: &TwoWayBlock,
    ) -> Result<(GpuTensor, GpuTensor)> {
        let scale_self = 1.0 / 32f32.sqrt();
        let scale_cross = 1.0 / 16f32.sqrt();
        let queries = if block.skip_first_pe {
            let att = block.self_attn.forward(queries, queries, queries, scale_self)?;
            gpu_layer_norm_mod(&att, &block.norm1, 0, SAM3_DETECTOR_DIM, SAM3_LN_EPS)
                .map_err(DiffusionError::model)?
        } else {
            let q = gpu_add(queries, q_pe).map_err(DiffusionError::model)?;
            let att = block.self_attn.forward(&q, &q, queries, scale_self)?;
            let added = gpu_add(queries, &att).map_err(DiffusionError::model)?;
            gpu_layer_norm_mod(&added, &block.norm1, 0, SAM3_DETECTOR_DIM, SAM3_LN_EPS)
                .map_err(DiffusionError::model)?
        };
        let q = gpu_add(&queries, q_pe).map_err(DiffusionError::model)?;
        let k = gpu_add(keys, k_pe).map_err(DiffusionError::model)?;
        let att = block.t2i.forward(&q, &k, keys, scale_cross)?;
        let queries = gpu_add(&queries, &att).map_err(DiffusionError::model)?;
        let queries = gpu_layer_norm_mod(&queries, &block.norm2, 0, SAM3_DETECTOR_DIM, SAM3_LN_EPS)
            .map_err(DiffusionError::model)?;
        let mlp = gpu_birefnet_relu(&block.mlp1.forward(&queries)?).map_err(DiffusionError::model)?;
        let mlp = block.mlp2.forward(&mlp)?;
        let queries = gpu_add(&queries, &mlp).map_err(DiffusionError::model)?;
        let queries = gpu_layer_norm_mod(&queries, &block.norm3, 0, SAM3_DETECTOR_DIM, SAM3_LN_EPS)
            .map_err(DiffusionError::model)?;
        let q = gpu_add(&queries, q_pe).map_err(DiffusionError::model)?;
        let k = gpu_add(keys, k_pe).map_err(DiffusionError::model)?;
        let att = block.i2t.forward(&k, &q, &queries, scale_cross)?;
        let keys = gpu_add(keys, &att).map_err(DiffusionError::model)?;
        let keys = gpu_layer_norm_mod(&keys, &block.norm4, 0, SAM3_DETECTOR_DIM, SAM3_LN_EPS)
            .map_err(DiffusionError::model)?;
        Ok((queries, keys))
    }
}

fn residual_self(
    normed: &GpuTensor,
    residual: &GpuTensor,
    proj: &SplitProj,
    out_proj: &Linear,
    scale: f32,
) -> Result<GpuTensor> {
    residual_self_qk(normed, normed, residual, proj, out_proj, scale)
}

fn residual_self_qk(
    qk: &GpuTensor,
    value: &GpuTensor,
    residual: &GpuTensor,
    proj: &SplitProj,
    out_proj: &Linear,
    scale: f32,
) -> Result<GpuTensor> {
    let update = residual_self_qk_out(qk, value, proj, out_proj, scale)?;
    gpu_add(residual, &update).map_err(DiffusionError::model)
}

fn residual_self_qk_out(
    qk: &GpuTensor,
    value: &GpuTensor,
    proj: &SplitProj,
    out_proj: &Linear,
    scale: f32,
) -> Result<GpuTensor> {
    let q = proj.q.forward(qk)?;
    let k = proj.k.forward(qk)?;
    let v = proj.v.forward(value)?;
    let attended = gpu_attention_packed_cross(&q, &k, &v, SAM3_DETECTOR_HEADS, scale)
        .map_err(DiffusionError::model)?;
    out_proj.forward(&attended)
}

fn residual_cross_out(
    query: &GpuTensor,
    memory: &GpuTensor,
    proj: &SplitProj,
    out_proj: &Linear,
    scale: f32,
) -> Result<GpuTensor> {
    residual_cross_kv_out(query, memory, memory, proj, out_proj, scale)
}

fn residual_cross_kv(
    query: &GpuTensor,
    residual: &GpuTensor,
    key_src: &GpuTensor,
    value_src: &GpuTensor,
    proj: &SplitProj,
    out_proj: &Linear,
    scale: f32,
) -> Result<GpuTensor> {
    let update = residual_cross_kv_out(query, key_src, value_src, proj, out_proj, scale)?;
    gpu_add(residual, &update).map_err(DiffusionError::model)
}

fn residual_cross_kv_out(
    query: &GpuTensor,
    key_src: &GpuTensor,
    value_src: &GpuTensor,
    proj: &SplitProj,
    out_proj: &Linear,
    scale: f32,
) -> Result<GpuTensor> {
    let q = proj.q.forward(query)?;
    let k = proj.k.forward(key_src)?;
    let v = proj.v.forward(value_src)?;
    let attended = gpu_attention_packed_cross(&q, &k, &v, SAM3_DETECTOR_HEADS, scale)
        .map_err(DiffusionError::model)?;
    out_proj.forward(&attended)
}

fn residual_cross_rpb(
    query: &GpuTensor,
    key_src: &GpuTensor,
    value_src: &GpuTensor,
    proj: &SplitProj,
    out_proj: &Linear,
    scale: f32,
    bias: &GpuTensor,
) -> Result<GpuTensor> {
    let q = proj.q.forward(query)?;
    let k = proj.k.forward(key_src)?;
    let v = proj.v.forward(value_src)?;
    let attended = gpu_attention_packed_cross_bias(&q, &k, &v, SAM3_DETECTOR_HEADS, scale, bias)
        .map_err(DiffusionError::model)?;
    out_proj.forward(&attended)
}

fn attention_with_bias(
    q: &[f32],
    k: &[f32],
    v: &[f32],
    q_rows: usize,
    k_rows: usize,
    heads: usize,
    scale: f32,
    bias: &[f32],
) -> Vec<f32> {
    let head_dim = SAM3_DETECTOR_DIM / heads;
    let mut out = vec![0.0f32; q_rows * SAM3_DETECTOR_DIM];
    for h in 0..heads {
        for qi in 0..q_rows {
            let mut scores = vec![0.0f32; k_rows];
            let mut max = f32::NEG_INFINITY;
            for ki in 0..k_rows {
                let mut dot = 0.0f32;
                for d in 0..head_dim {
                    let qv = q[qi * SAM3_DETECTOR_DIM + h * head_dim + d];
                    let kv = k[ki * SAM3_DETECTOR_DIM + h * head_dim + d];
                    dot += qv * kv;
                }
                let s = dot * scale + bias[(h * q_rows + qi) * k_rows + ki];
                scores[ki] = s;
                if s > max {
                    max = s;
                }
            }
            let mut sum = 0.0f32;
            for s in &mut scores {
                *s = (*s - max).exp();
                sum += *s;
            }
            let inv = 1.0 / sum.max(1e-20);
            for d in 0..head_dim {
                let mut acc = 0.0f32;
                for ki in 0..k_rows {
                    acc += scores[ki] * inv * v[ki * SAM3_DETECTOR_DIM + h * head_dim + d];
                }
                out[qi * SAM3_DETECTOR_DIM + h * head_dim + d] = acc;
            }
        }
    }
    out
}

/// How far past the detection box the refine crop reaches, as a fraction of
/// the box's own size on each axis. Matched to the reference dumps.
const ZOOM_MARGIN: f32 = 0.1;

/// Half-open pixel window `[left, right) x [top, bottom)` on some grid.
#[derive(Clone, Copy)]
struct Window {
    left: usize,
    top: usize,
    right: usize,
    bottom: usize,
}

impl Window {
    fn width(&self) -> usize {
        self.right - self.left
    }

    fn height(&self) -> usize {
        self.bottom - self.top
    }
}

/// Grow `box_xyxy` by [`ZOOM_MARGIN`] of its own extent on every side and clip
/// the result to a `width` x `height` image. `None` once the clip leaves
/// nothing behind — an empty window has no crop to refine.
fn zoom_window(box_xyxy: [f32; 4], width: usize, height: usize) -> Option<Window> {
    let margin_x = (box_xyxy[2] - box_xyxy[0]) * ZOOM_MARGIN;
    let margin_y = (box_xyxy[3] - box_xyxy[1]) * ZOOM_MARGIN;
    let window = Window {
        left: ((box_xyxy[0] - margin_x) as i32).max(0) as usize,
        top: ((box_xyxy[1] - margin_y) as i32).max(0) as usize,
        right: ((box_xyxy[2] + margin_x) as i32 as usize).min(width),
        bottom: ((box_xyxy[3] + margin_y) as i32 as usize).min(height),
    };
    (window.right > window.left && window.bottom > window.top).then_some(window)
}

/// Restate a window measured on `src_w` x `src_h` in the coordinates of a
/// `dst_w` x `dst_h` grid. `None` when it rounds away to nothing.
fn rescale_window(
    window: Window,
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
) -> Option<Window> {
    let project = |value: usize, from: usize, to: usize| -> usize {
        (value as f32 / from as f32 * to as f32) as usize
    };
    let scaled = Window {
        left: project(window.left, src_w, dst_w),
        top: project(window.top, src_h, dst_h),
        right: project(window.right, src_w, dst_w),
        bottom: project(window.bottom, src_h, dst_h),
    };
    (scaled.right > scaled.left && scaled.bottom > scaled.top).then_some(scaled)
}

fn cxcywh_to_xyxy(b: [f32; 4]) -> [f32; 4] {
    [
        b[0] - 0.5 * b[2],
        b[1] - 0.5 * b[3],
        b[0] + 0.5 * b[2],
        b[1] + 0.5 * b[3],
    ]
}

fn nearest2x(p: &Planar) -> Result<Planar> {
    Ok(Planar {
        tensor: gpu_upsample_nearest2x(&p.tensor, p.width, p.height).map_err(DiffusionError::model)?,
        width: p.width * 2,
        height: p.height * 2,
    })
}

fn relu_planar(p: &Planar) -> Result<Planar> {
    Ok(Planar {
        tensor: gpu_birefnet_relu(&p.tensor).map_err(DiffusionError::model)?,
        width: p.width,
        height: p.height,
    })
}

fn group_norm_planar(
    p: &Planar,
    groups: usize,
    gamma: &[f32],
    beta: &[f32],
    key: &str,
) -> Result<Planar> {
    Ok(Planar {
        tensor: gpu_group_norm_planar(
            &p.tensor,
            p.width,
            p.height,
            groups,
            SAM3_CACHE_NAMESPACE,
            key,
            gamma,
            beta,
            SAM3_LN_EPS,
        )
        .map_err(DiffusionError::model)?,
        width: p.width,
        height: p.height,
    })
}

/// DETR-style sine position encoding (HF `Sam3SinePositionEmbedding`,
/// normalize=True, temperature=10000, feats=256). Returns tokens
/// `[H*W, dim]` in row-major y,x order (cat(sincos(y), sincos(x))).
///
/// NOTE the normalizer: `pos / (N - 1 + 1e-6)`, which is what the oracle our
/// dumps are diffed against uses. The HF and upstream-paper form is
/// `(pos + 1) / (N + 1e-6)`. Changing it moves every detector score, so it
/// stays pinned to the oracle.
fn sine_pos_hw(height: usize, width: usize, dim: usize) -> Vec<f32> {
    let half = dim / 2;
    let mut out = vec![0.0f32; height * width * dim];
    for y in 0..height {
        let yn = y as f32 / (height.saturating_sub(1) as f32 + 1e-6);
        for x in 0..width {
            let xn = x as f32 / (width.saturating_sub(1) as f32 + 1e-6);
            let row = (y * width + x) * dim;
            sine_interleaved(yn, half, &mut out[row..row + half]);
            sine_interleaved(xn, half, &mut out[row + half..row + dim]);
        }
    }
    out
}

fn sine_interleaved(value: f32, half: usize, dest: &mut [f32]) {
    let scale = 2.0 * std::f32::consts::PI;
    let pairs = half / 2;
    for i in 0..pairs {
        let freq_i = (2 * i) as f32;
        let freq = 10000f32.powf(freq_i / half as f32);
        let raw = value * scale / freq;
        dest[2 * i] = raw.sin();
        dest[2 * i + 1] = raw.cos();
    }
}

fn residual_cross(
    query: &GpuTensor,
    residual: &GpuTensor,
    memory: &GpuTensor,
    proj: &SplitProj,
    out_proj: &Linear,
    scale: f32,
) -> Result<GpuTensor> {
    let update = residual_cross_kv_out(query, memory, memory, proj, out_proj, scale)?;
    gpu_add(residual, &update).map_err(DiffusionError::model)
}

fn windowed_attention(
    q: &GpuTensor,
    k: &GpuTensor,
    v: &GpuTensor,
    fold: &GpuTensor,
    unfold: &GpuTensor,
    rope_cos: &GpuTensor,
    rope_sin: &GpuTensor,
    scale: f32,
) -> Result<GpuTensor> {
    // Fold the 9 windows into the column axis and run one flash call with
    // 9*16 heads: attention is computed per 64-wide head block, so the
    // windows stay block-diagonal — identical math to 9 separate calls.
    let window_tokens = SAM3_VISION_WINDOW * SAM3_VISION_WINDOW;
    let windows = (GRID / SAM3_VISION_WINDOW) * (GRID / SAM3_VISION_WINDOW);
    let wide = windows * SAM3_VISION_DIM;
    let fold_tensor = |x: &GpuTensor| -> Result<GpuTensor> {
        let gathered =
            gpu_gather_rows_colblock(x, fold, None, SAM3_VISION_DIM).map_err(DiffusionError::model)?;
        gpu_reshape(gathered, window_tokens, wide).map_err(DiffusionError::model)
    };
    let q = fold_tensor(q)?;
    let k = fold_tensor(k)?;
    let v = fold_tensor(v)?;
    let heads = windows * SAM3_VISION_HEADS;
    let q = gpu_rope_interleaved(&q, heads, rope_cos, rope_sin).map_err(DiffusionError::model)?;
    let k = gpu_rope_interleaved(&k, heads, rope_cos, rope_sin).map_err(DiffusionError::model)?;
    let attended = gpu_attention_packed_flash2_d64(&q, &k, &v, heads, scale)
        .map_err(DiffusionError::model)?;
    let flat = gpu_reshape(attended, GRID * GRID, SAM3_VISION_DIM).map_err(DiffusionError::model)?;
    gpu_gather_rows_colblock(&flat, unfold, None, SAM3_VISION_DIM).map_err(DiffusionError::model)
}

fn rope2d_pair(x: f32, y: f32, rot_half: usize, theta: f32, cos: &mut [f32], sin: &mut [f32]) {
    // Flux EmbedND axes_dim = [rot_half, rot_half] mapped onto one head:
    // first rot_half/2 pairs use X, remaining pairs use Y.
    let axis_pairs = rot_half / 2;
    for i in 0..rot_half {
        let axis_pos = if i < axis_pairs { x } else { y };
        let freq_i = (i % axis_pairs) as f32;
        let freq = 1.0 / theta.powf(2.0 * freq_i / rot_half as f32);
        let angle = axis_pos * freq;
        cos[i] = angle.cos();
        sin[i] = angle.sin();
    }
}

fn rope2d_tables(width: usize, height: usize, scale_pos: f32) -> (Vec<f32>, Vec<f32>) {
    let rot_half = VISION_HEAD_DIM / 2;
    let n = width * height;
    let mut cos = vec![0.0f32; n * rot_half];
    let mut sin = vec![0.0f32; n * rot_half];
    for y in 0..height {
        for x in 0..width {
            let row = y * width + x;
            rope2d_pair(
                x as f32 * scale_pos,
                y as f32 * scale_pos,
                rot_half,
                10000.0,
                &mut cos[row * rot_half..(row + 1) * rot_half],
                &mut sin[row * rot_half..(row + 1) * rot_half],
            );
        }
    }
    (cos, sin)
}

fn rope2d_window_tables(grid: usize, window: usize) -> (Vec<f32>, Vec<f32>) {
    let rot_half = VISION_HEAD_DIM / 2;
    let n = grid * grid;
    let nw = grid / window;
    let mut cos = vec![0.0f32; n * rot_half];
    let mut sin = vec![0.0f32; n * rot_half];
    let mut dest = 0usize;
    for _wy in 0..nw {
        for _wx in 0..nw {
            for ly in 0..window {
                for lx in 0..window {
                    rope2d_pair(
                        lx as f32,
                        ly as f32,
                        rot_half,
                        10000.0,
                        &mut cos[dest * rot_half..(dest + 1) * rot_half],
                        &mut sin[dest * rot_half..(dest + 1) * rot_half],
                    );
                    dest += 1;
                }
            }
        }
    }
    (cos, sin)
}

fn window_indices(grid: usize, window: usize) -> (Vec<u32>, Vec<u32>) {
    let nw = grid / window;
    let n = grid * grid;
    let mut fwd = vec![0u32; n];
    let mut rev = vec![0u32; n];
    let mut dest = 0usize;
    for wy in 0..nw {
        for wx in 0..nw {
            for ly in 0..window {
                for lx in 0..window {
                    let src = (wy * window + ly) * grid + wx * window + lx;
                    fwd[dest] = src as u32;
                    rev[src] = dest as u32;
                    dest += 1;
                }
            }
        }
    }
    (fwd, rev)
}

/// Tile the 24×24 pretrained spatial grid across the 72×72 input grid rather
/// than bilinear-interpolating it (HF `Sam3ViTEmbeddings::_tile_position_embeddings`).
fn tile_pos(base: &[f32], width: usize, height: usize) -> Result<Vec<f32>> {
    let expected = (1 + SAM3_POS_BASE * SAM3_POS_BASE) * SAM3_VISION_DIM;
    if base.len() != expected {
        return Err(DiffusionError::workflow("sam3 pos embed shape mismatch"));
    }
    let mut output = vec![0.0f32; (1 + width * height) * SAM3_VISION_DIM];
    output[..SAM3_VISION_DIM].copy_from_slice(&base[..SAM3_VISION_DIM]);
    let source = &base[SAM3_VISION_DIM..];
    for oy in 0..height {
        let sy = oy % SAM3_POS_BASE;
        for ox in 0..width {
            let sx = ox % SAM3_POS_BASE;
            let dst = (1 + oy * width + ox) * SAM3_VISION_DIM;
            let src = (sy * SAM3_POS_BASE + sx) * SAM3_VISION_DIM;
            output[dst..dst + SAM3_VISION_DIM].copy_from_slice(&source[src..src + SAM3_VISION_DIM]);
        }
    }
    Ok(output)
}

fn planar_to_tokens(planar: &Planar) -> Result<GpuTensor> {
    gpu_birefnet_tokens_to_planar(&planar.tensor).map_err(DiffusionError::model)
}

/// Copy captured-graph FPN outputs into caller-owned tensors (the graph's
/// own buffers stay pinned inside the thread-local state).
fn clone_fpn_tensors(fpn: &[GpuTensor; 3]) -> Result<[Planar; 3]> {
    let sides = [GRID * 4, GRID * 2, GRID];
    let mut out = Vec::with_capacity(3);
    for (tensor, side) in fpn.iter().zip(sides) {
        out.push(Planar {
            tensor: gpu_slice_rows(tensor, 0, tensor.rows()).map_err(DiffusionError::model)?,
            width: side,
            height: side,
        });
    }
    let mut it = out.into_iter();
    Ok([
        it.next().expect("fpn0"),
        it.next().expect("fpn1"),
        it.next().expect("fpn2"),
    ])
}

fn clone_planar(p: &Planar) -> Result<Planar> {
    Ok(Planar {
        tensor: gpu_slice_rows(&p.tensor, 0, p.tensor.rows()).map_err(DiffusionError::model)?,
        width: p.width,
        height: p.height,
    })
}

fn gelu_planar(p: &Planar) -> Result<Planar> {
    Ok(Planar {
        tensor: gpu_gelu(&p.tensor).map_err(DiffusionError::model)?,
        width: p.width,
        height: p.height,
    })
}

fn norm_planar_dim(p: &Planar, mods: &GpuTensor, dim: usize) -> Result<Planar> {
    let tokens = planar_to_tokens(p)?;
    let normed = gpu_layer_norm_mod(&tokens, mods, 0, dim, SAM3_LN_EPS).map_err(DiffusionError::model)?;
    Ok(Planar {
        tensor: gpu_birefnet_tokens_to_planar(&normed).map_err(DiffusionError::model)?,
        width: p.width,
        height: p.height,
    })
}

fn random_pe_grid(matrix: &[f32], height: usize, width: usize) -> Vec<f32> {
    let feats = 128usize;
    let mut out = vec![0.0f32; height * width * feats * 2];
    for y in 0..height {
        let yn = (y as f32 + 0.5) / height as f32;
        for x in 0..width {
            let xn = (x as f32 + 0.5) / width as f32;
            let coords = [xn, yn];
            let row = (y * width + x) * (feats * 2);
            for f in 0..feats {
                let mut dot = 0.0f32;
                for c in 0..2 {
                    let n = 2.0 * coords[c] - 1.0;
                    dot += n * matrix[c * feats + f];
                }
                let projected = 2.0 * std::f32::consts::PI * dot;
                out[row + f] = projected.sin();
                out[row + feats + f] = projected.cos();
            }
        }
    }
    out
}

fn add_planar(a: &Planar, b: &Planar) -> Result<Planar> {
    if a.width != b.width || a.height != b.height {
        return Err(DiffusionError::workflow("sam3 planar add size mismatch"));
    }
    Ok(Planar {
        tensor: gpu_add(&a.tensor, &b.tensor).map_err(DiffusionError::model)?,
        width: a.width,
        height: a.height,
    })
}

fn norm_planar(p: &Planar, mods: &GpuTensor) -> Result<Planar> {
    let tokens = planar_to_tokens(p)?;
    let normed = gpu_layer_norm_mod(&tokens, mods, 0, SAM3_DETECTOR_DIM, SAM3_LN_EPS)
        .map_err(DiffusionError::model)?;
    Ok(Planar {
        tensor: gpu_birefnet_tokens_to_planar(&normed).map_err(DiffusionError::model)?,
        width: p.width,
        height: p.height,
    })
}

fn upsample_to(p: &Planar, width: usize, height: usize) -> Result<Planar> {
    if p.width == width && p.height == height {
        return clone_planar(p);
    }
    Ok(Planar {
        tensor: gpu_birefnet_resize_bilinear(&p.tensor, p.width, p.height, width, height, false)
            .map_err(DiffusionError::model)?,
        width,
        height,
    })
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

fn unletterbox_box(box_xyxy: [f32; 4], image: &Sam3Preprocessed) -> [f32; 4] {
    let map = |v: f32, src: usize| -> f32 { (v * src as f32).clamp(0.0, src as f32) };
    [
        map(box_xyxy[0], image.src_width),
        map(box_xyxy[1], image.src_height),
        map(box_xyxy[2], image.src_width),
        map(box_xyxy[3], image.src_height),
    ]
}

fn unletterbox_mask(
    mask: &[f32],
    mask_w: usize,
    mask_h: usize,
    image: &Sam3Preprocessed,
) -> Result<Vec<f32>> {
    if mask.len() != mask_w * mask_h {
        return Err(DiffusionError::workflow("sam3 mask size mismatch"));
    }
    let mut out = vec![0.0f32; image.src_width * image.src_height];
    for y in 0..image.src_height {
        let sy = (y as f32 + 0.5) * mask_h as f32 / image.src_height as f32 - 0.5;
        let y0 = sy.floor().clamp(0.0, (mask_h - 1) as f32) as usize;
        let y1 = (y0 + 1).min(mask_h - 1);
        let fy = (sy - y0 as f32).clamp(0.0, 1.0);
        for x in 0..image.src_width {
            let sx = (x as f32 + 0.5) * mask_w as f32 / image.src_width as f32 - 0.5;
            let x0 = sx.floor().clamp(0.0, (mask_w - 1) as f32) as usize;
            let x1 = (x0 + 1).min(mask_w - 1);
            let fx = (sx - x0 as f32).clamp(0.0, 1.0);
            let v00 = mask[y0 * mask_w + x0];
            let v10 = mask[y0 * mask_w + x1];
            let v01 = mask[y1 * mask_w + x0];
            let v11 = mask[y1 * mask_w + x1];
            out[y * image.src_width + x] = (v00 * (1.0 - fx) * (1.0 - fy)
                + v10 * fx * (1.0 - fy)
                + v01 * (1.0 - fx) * fy
                + v11 * fx * fy)
                .clamp(0.0, 1.0);
        }
    }
    Ok(out)
}
