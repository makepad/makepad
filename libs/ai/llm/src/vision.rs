// Qwen3-VL family vision tower (clip arch, projector "qwen3vl_merger"), on
// Metal or CUDA. Serves Qwen3.5-9B and Qwen3.8-27B from the same code.
//
// Loads the separate mmproj GGUF, preprocesses an RGB image (smart-resize +
// normalize + block-major patch unfold), runs the 27-block SigLIP-style ViT
// with 2D vision rope and full bidirectional flash attention, and returns the
// projected [n_tokens x proj_dim] embeddings ready for LLM prefill injection.
//
// Reference implementation: llama.cpp tools/mtmd (clip.cpp models/qwen3vl.cpp,
// mtmd-image.cpp mtmd_image_preprocessor_dyn_size). Preprocessing here is an
// exact port; the graph mirrors the reference node-for-node. Parity is
// validated by the vlm-vision-probe bin against dumps from
// tools/vlm_oracle/clip_dump.
//
// The tower runs on Metal or CUDA from one graph builder. The only
// backend-conditional detail is the K/V dtype fed to the one `flash_attn_ext`
// node: Metal casts K/V to f16, CUDA keeps them f32 because its maskless
// (non-causal) kernel is the f32 roformer path. Both are gated against the
// same CPU oracle; f32 K/V scores marginally closer to it.
//
// Config, not fork: Qwen3.5-9B and Qwen3.8-27B share this tower exactly (27
// blocks, n_embd 1152, ffn 4304, 16 heads, patch 16, merge 2, no deepstack);
// they differ only in `clip.vision.projection_dim` (4096 vs 5120), which is
// read from the GGUF.

use std::collections::BTreeMap;

use crate::metal_compiled::{prepare_graph, MetalGraphSession, MetalGraphTensorWrite};
use crate::{
    BufferUsage, Context, CudaExecRuntime, CudaRawGraphSession, Graph, Op, Prec, ScaleMode,
    TensorId, TensorType, UnaryOp, GGML_ROPE_TYPE_VISION,
};
use makepad_ai_metal::{BufferStorageMode, MetalRuntime};

/// Which device the tower runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VisionBackend {
    Metal,
    Cuda,
}

enum VisionRuntime {
    Metal(MetalRuntime),
    Cuda(Box<CudaExecRuntime>),
}

enum VisionSession {
    Metal(MetalGraphSession),
    Cuda(Box<CudaRawGraphSession>),
}

use crate::error::{LlamaError, Result};
use crate::gguf::{GgufArray, GgufFile, GgufValue};
use crate::weights::{GgufWeightLayout, LoadedGgufWeights};

#[derive(Clone, Debug)]
pub struct VisionConfig {
    pub image_size: usize,
    pub patch_size: usize,
    pub n_embd: usize,
    pub n_ffn: usize,
    pub n_layers: usize,
    pub n_heads: usize,
    pub proj_dim: usize,
    pub n_merge: usize,
    pub eps: f32,
    pub image_mean: [f32; 3],
    pub image_std: [f32; 3],
    pub min_pixels: usize,
    pub max_pixels: usize,
}

impl VisionConfig {
    pub fn from_gguf(gguf: &GgufFile) -> Result<Self> {
        let arch = required_string(gguf, "general.architecture")?;
        if arch != "clip" {
            return Err(LlamaError::unsupported(format!(
                "mmproj general.architecture must be 'clip', got '{arch}'"
            )));
        }
        let projector = required_string(gguf, "clip.projector_type")?;
        if projector != "qwen3vl_merger" {
            return Err(LlamaError::unsupported(format!(
                "unsupported clip.projector_type '{projector}' (expected qwen3vl_merger)"
            )));
        }
        let patch_size = required_usize(gguf, "clip.vision.patch_size")?;
        let n_merge = optional_usize(gguf, "clip.vision.spatial_merge_size")?.unwrap_or(2);
        // reference defaults: set_limit_image_tokens(8, 4096) — one output token
        // covers (patch_size * n_merge)^2 pixels
        let token_pixels = (patch_size * n_merge) * (patch_size * n_merge);
        let min_pixels =
            optional_usize(gguf, "clip.vision.image_min_pixels")?.unwrap_or(8 * token_pixels);
        let max_pixels =
            optional_usize(gguf, "clip.vision.image_max_pixels")?.unwrap_or(4096 * token_pixels);
        Ok(Self {
            image_size: required_usize(gguf, "clip.vision.image_size")?,
            patch_size,
            n_embd: required_usize(gguf, "clip.vision.embedding_length")?,
            n_ffn: required_usize(gguf, "clip.vision.feed_forward_length")?,
            n_layers: required_usize(gguf, "clip.vision.block_count")?,
            n_heads: required_usize(gguf, "clip.vision.attention.head_count")?,
            proj_dim: required_usize(gguf, "clip.vision.projection_dim")?,
            n_merge,
            eps: optional_f32(gguf, "clip.vision.attention.layer_norm_epsilon")?.unwrap_or(1e-6),
            image_mean: required_f32_3(gguf, "clip.vision.image_mean")?,
            image_std: required_f32_3(gguf, "clip.vision.image_std")?,
            min_pixels,
            max_pixels,
        })
    }

    pub fn head_dim(&self) -> usize {
        self.n_embd / self.n_heads
    }

    /// Pixels per merged output token edge (32 for qwen3.5).
    pub fn align_size(&self) -> usize {
        self.patch_size * self.n_merge
    }
}

/// A preprocessed image ready for the vision tower.
pub struct PreparedImage {
    /// Resized width/height in pixels (multiples of align_size).
    pub width: usize,
    pub height: usize,
    /// Normalized resized image, interleaved RGB f32 — layout identical to the
    /// reference clip_image_f32 buffer, kept for parity testing.
    pub pixels: Vec<f32>,
    /// Patch matrix [n_patches rows x (3*patch*patch) cols] in block-major
    /// token order; each row is (channel, ky, kx) with kx fastest, matching
    /// the conv weight layout.
    pub patches: Vec<f32>,
    /// Grid size in patches.
    pub grid_w: usize,
    pub grid_h: usize,
}

impl PreparedImage {
    pub fn n_patches(&self) -> usize {
        self.grid_w * self.grid_h
    }

    /// Output tokens after 2x2 spatial merge.
    pub fn n_tokens(&self) -> usize {
        self.n_patches() / 4
    }

    pub fn tokens_w(&self) -> usize {
        self.grid_w / 2
    }

    pub fn tokens_h(&self) -> usize {
        self.grid_h / 2
    }
}

/// Port of mtmd-image.cpp img_tool::calc_size_preserved_ratio.
pub fn calc_size_preserved_ratio(
    width: usize,
    height: usize,
    align_size: usize,
    min_pixels: usize,
    max_pixels: usize,
) -> (usize, usize) {
    let f = align_size as f32;
    let round_by = |x: f32| ((x / f).round() as usize) * align_size;
    let ceil_by = |x: f32| ((x / f).ceil() as usize) * align_size;
    let floor_by = |x: f32| ((x / f).floor() as usize) * align_size;

    let mut w_bar = round_by(width as f32).max(align_size);
    let mut h_bar = round_by(height as f32).max(align_size);

    if h_bar * w_bar > max_pixels {
        let beta = ((height * width) as f32 / max_pixels as f32).sqrt();
        h_bar = floor_by(height as f32 / beta).max(align_size);
        w_bar = floor_by(width as f32 / beta).max(align_size);
    } else if h_bar * w_bar < min_pixels {
        let beta = (min_pixels as f32 / (height * width) as f32).sqrt();
        h_bar = ceil_by(height as f32 * beta);
        w_bar = ceil_by(width as f32 * beta);
    }
    (w_bar, h_bar)
}

/// Port of mtmd-image.cpp img_tool::resize_bilinear with the (src-1)/target
/// ratio convention. Interior arithmetic is f64: the reference binary promotes
/// to double, and matching it keeps u8 truncation boundaries identical
/// (validated: 7 of 589824 values off by one step on the resize test image,
/// vs 1131 with f32).
fn resize_bilinear_u8(src: &[u8], sw: usize, sh: usize, tw: usize, th: usize) -> Vec<u8> {
    assert!(sw >= 2 && sh >= 2);
    let mut dst = vec![0u8; tw * th * 3];
    let x_ratio = (sw - 1) as f64 / tw as f64;
    let y_ratio = (sh - 1) as f64 / th as f64;
    for y in 0..th {
        for x in 0..tw {
            let px = x_ratio * x as f64;
            let py = y_ratio * y as f64;
            let x0 = (px as usize).min(sw - 2);
            let y0 = (py as usize).min(sh - 2);
            let xl = px - x0 as f64;
            let yl = py - y0 as f64;
            for c in 0..3 {
                let s00 = src[3 * (y0 * sw + x0) + c] as f64;
                let s01 = src[3 * (y0 * sw + x0 + 1) + c] as f64;
                let s10 = src[3 * ((y0 + 1) * sw + x0) + c] as f64;
                let s11 = src[3 * ((y0 + 1) * sw + x0 + 1) + c] as f64;
                let top = s00 + (s01 - s00) * xl;
                let bottom = s10 + (s11 - s10) * xl;
                dst[3 * (y * tw + x) + c] = (top + (bottom - top) * yl) as u8;
            }
        }
    }
    dst
}

/// Port of img_tool::resize with add_padding=true (the qwen path): scale to
/// fit, center, pad with black. For aspect-matching targets this reduces to a
/// plain resize.
fn resize_pad_u8(src: &[u8], sw: usize, sh: usize, tw: usize, th: usize) -> Vec<u8> {
    if sw == tw && sh == th {
        return src.to_vec();
    }
    let scale_w = tw as f32 / sw as f32;
    let scale_h = th as f32 / sh as f32;
    let scale = scale_w.min(scale_h);
    let new_w = ((sw as f32 * scale).ceil() as usize).min(tw);
    let new_h = ((sh as f32 * scale).ceil() as usize).min(th);
    let resized = resize_bilinear_u8(src, sw, sh, new_w, new_h);
    if new_w == tw && new_h == th {
        return resized;
    }
    let mut dst = vec![0u8; tw * th * 3];
    let off_x = (tw - new_w) / 2;
    let off_y = (th - new_h) / 2;
    for y in 0..new_h {
        let src_row = &resized[3 * y * new_w..3 * (y + 1) * new_w];
        let dst_start = 3 * ((y + off_y) * tw + off_x);
        dst[dst_start..dst_start + 3 * new_w].copy_from_slice(src_row);
    }
    dst
}

/// Preprocess an interleaved RGB8 image for the vision tower.
pub fn preprocess_rgb8(
    rgb: &[u8],
    width: usize,
    height: usize,
    config: &VisionConfig,
) -> Result<PreparedImage> {
    if rgb.len() != width * height * 3 {
        return Err(LlamaError::format(format!(
            "rgb buffer size {} does not match {}x{}x3",
            rgb.len(),
            width,
            height
        )));
    }
    let align = config.align_size();
    let (tw, th) = calc_size_preserved_ratio(
        width,
        height,
        align,
        config.min_pixels,
        config.max_pixels,
    );
    let resized = resize_pad_u8(rgb, width, height, tw, th);

    // normalize: (v/255 - mean) / std, interleaved layout kept for parity checks
    let mut pixels = vec![0f32; tw * th * 3];
    for i in 0..tw * th {
        for c in 0..3 {
            let v = resized[3 * i + c] as f32 / 255.0;
            pixels[3 * i + c] = (v - config.image_mean[c]) / config.image_std[c];
        }
    }

    let patch = config.patch_size;
    let grid_w = tw / patch;
    let grid_h = th / patch;
    let patch_len = 3 * patch * patch;
    let n_patches = grid_w * grid_h;
    let mut patches = vec![0f32; n_patches * patch_len];

    // block-major token order: iterate 2x2 blocks row-major, then the 4
    // patches within each block row-major — matches the qwen3vl conv output
    // rearrangement, so 4 consecutive rows here form one merged output token.
    // Row layout is (c, ky, kx) with kx fastest, matching the flattened
    // [16,16,3,out] conv weight.
    let mut row = 0usize;
    for by in (0..grid_h).step_by(2) {
        for bx in (0..grid_w).step_by(2) {
            for dy in 0..2 {
                for dx in 0..2 {
                    let px0 = (bx + dx) * patch;
                    let py0 = (by + dy) * patch;
                    let out = &mut patches[row * patch_len..(row + 1) * patch_len];
                    let mut k = 0usize;
                    for c in 0..3 {
                        for ky in 0..patch {
                            let src_row = 3 * ((py0 + ky) * tw + px0);
                            for kx in 0..patch {
                                out[k] = pixels[src_row + 3 * kx + c];
                                k += 1;
                            }
                        }
                    }
                    row += 1;
                }
            }
        }
    }

    Ok(PreparedImage {
        width: tw,
        height: th,
        pixels,
        patches,
        grid_w,
        grid_h,
    })
}

/// M-RoPE position ids for the vision graph: 4 planes [y, x, y, x] over the
/// block-major patch sequence — port of the clip.cpp position filling.
pub fn vision_rope_positions(grid_w: usize, grid_h: usize) -> Vec<i32> {
    let n = grid_w * grid_h;
    let mut positions = vec![0i32; n * 4];
    let mut ptr = 0usize;
    for y in (0..grid_h).step_by(2) {
        for x in (0..grid_w).step_by(2) {
            for dy in 0..2 {
                for dx in 0..2 {
                    positions[ptr] = (y + dy) as i32;
                    positions[n + ptr] = (x + dx) as i32;
                    positions[2 * n + ptr] = (y + dy) as i32;
                    positions[3 * n + ptr] = (x + dx) as i32;
                    ptr += 1;
                }
            }
        }
    }
    positions
}

/// Row-major patch index for each block-major token position — used to gather
/// interpolated position embeddings into sequence order.
fn block_order_row_indices(grid_w: usize, grid_h: usize) -> Vec<i32> {
    let mut idx = Vec::with_capacity(grid_w * grid_h);
    for by in (0..grid_h).step_by(2) {
        for bx in (0..grid_w).step_by(2) {
            for dy in 0..2 {
                for dx in 0..2 {
                    idx.push(((by + dy) * grid_w + (bx + dx)) as i32);
                }
            }
        }
    }
    idx
}

struct VisionGraph {
    session: VisionSession,
    input_patches: TensorId,
    input_positions: TensorId,
    input_block_index: TensorId,
    output: TensorId,
    _grid_w: usize,
    _grid_h: usize,
}

pub struct VisionTower {
    pub config: VisionConfig,
    weights: LoadedGgufWeights,
    runtime: VisionRuntime,
    graphs: BTreeMap<(usize, usize), VisionGraph>,
}

impl VisionTower {
    /// Load the mmproj GGUF. `max_patches` bounds the activation arena that is
    /// reserved up front (patches = output tokens * 4).
    ///
    /// Backend: CUDA when a usable device is present, else Metal.
    /// `MAKEPAD_VISION_BACKEND=cuda|metal` pins it (and turns a missing backend
    /// into an error rather than a silent fallback).
    pub fn load(path: &str, max_patches: usize) -> Result<Self> {
        let gguf = GgufFile::open(path)?;
        let config = VisionConfig::from_gguf(&gguf)?;
        let layout = GgufWeightLayout::from_tensors(gguf.tensors.iter().cloned())?;
        let extra = Self::activation_bytes_estimate(&config, max_patches);
        let weights = layout.allocate_and_load_with_extra(&gguf, extra)?;
        let runtime = Self::select_runtime()?;
        Ok(Self {
            config,
            weights,
            runtime,
            graphs: BTreeMap::new(),
        })
    }

    fn select_runtime() -> Result<VisionRuntime> {
        let pinned = std::env::var("MAKEPAD_VISION_BACKEND").ok();
        match pinned.as_deref() {
            Some("cuda") => Ok(VisionRuntime::Cuda(Box::new(CudaExecRuntime::new()?))),
            Some("metal") => Ok(VisionRuntime::Metal(
                MetalRuntime::new().map_err(LlamaError::format)?,
            )),
            Some(other) => Err(LlamaError::unsupported(format!(
                "MAKEPAD_VISION_BACKEND must be 'cuda' or 'metal', got '{other}'"
            ))),
            None => match CudaExecRuntime::new() {
                Ok(cuda) => Ok(VisionRuntime::Cuda(Box::new(cuda))),
                Err(_) => Ok(VisionRuntime::Metal(
                    MetalRuntime::new().map_err(LlamaError::format)?,
                )),
            },
        }
    }

    /// Which device this tower is running on.
    pub fn backend(&self) -> VisionBackend {
        match self.runtime {
            VisionRuntime::Metal(_) => VisionBackend::Metal,
            VisionRuntime::Cuda(_) => VisionBackend::Cuda,
        }
    }

    /// Human-readable device description, for probe/service logs.
    pub fn device_description(&self) -> String {
        match &self.runtime {
            VisionRuntime::Metal(_) => "metal".to_string(),
            VisionRuntime::Cuda(cuda) => cuda.device_description(),
        }
    }

    fn activation_bytes_estimate(config: &VisionConfig, max_patches: usize) -> usize {
        // per patch per layer: ~18 full-width f32 nodes + 3 ffn-width nodes
        // + 2 f16 casts; generous 1.5x headroom on top
        let full = 4 * config.n_embd;
        let ffn = 4 * config.n_ffn;
        let per_patch_layer = 18 * full + 3 * ffn + config.n_embd;
        let per_patch = per_patch_layer * config.n_layers + 16 * full + 8 * config.proj_dim;
        (max_patches * per_patch) * 3 / 2 + (64 << 20)
    }

    fn tensor(&self, name: &str) -> Result<TensorId> {
        self.weights.require_tensor_id(name)
    }

    /// Encode a preprocessed image; returns [n_tokens * proj_dim] f32.
    pub fn encode(&mut self, image: &PreparedImage) -> Result<Vec<f32>> {
        let key = (image.grid_w, image.grid_h);
        if !self.graphs.contains_key(&key) {
            let graph = self.build_graph(image.grid_w, image.grid_h)?;
            self.graphs.insert(key, graph);
        }
        let graph = self.graphs.get(&key).unwrap();

        let positions = vision_rope_positions(image.grid_w, image.grid_h);
        let block_index = block_order_row_indices(image.grid_w, image.grid_h);

        let patches_bytes = f32s_as_bytes(&image.patches);
        let positions_bytes = i32s_as_bytes(&positions);
        let block_index_bytes = i32s_as_bytes(&block_index);

        match &graph.session {
            VisionSession::Metal(session) => {
                let writes = vec![
                    MetalGraphTensorWrite {
                        tensor_id: graph.input_patches,
                        bytes: &patches_bytes,
                    },
                    MetalGraphTensorWrite {
                        tensor_id: graph.input_positions,
                        bytes: &positions_bytes,
                    },
                    MetalGraphTensorWrite {
                        tensor_id: graph.input_block_index,
                        bytes: &block_index_bytes,
                    },
                ];
                let execution = session
                    .execute(&self.weights.ctx, &writes, &[graph.output])
                    .map_err(LlamaError::format)?;
                let bytes = execution
                    .outputs
                    .get(&graph.output)
                    .ok_or_else(|| LlamaError::format("vision graph returned no output"))?;
                Ok(bytes_to_f32s(bytes))
            }
            VisionSession::Cuda(session) => {
                let writes: Vec<(TensorId, &[u8])> = vec![
                    (graph.input_patches, patches_bytes.as_slice()),
                    (graph.input_positions, positions_bytes.as_slice()),
                    (graph.input_block_index, block_index_bytes.as_slice()),
                ];
                let outputs = session.execute(&self.weights.ctx, &writes, &[graph.output])?;
                let bytes = outputs
                    .get(&graph.output)
                    .ok_or_else(|| LlamaError::format("vision graph returned no output"))?;
                Ok(bytes_to_f32s(bytes))
            }
        }
    }

    fn build_graph(&mut self, grid_w: usize, grid_h: usize) -> Result<VisionGraph> {
        let cfg = self.config.clone();
        // Metal's maskless flash kernel takes f16 K/V; CUDA's takes f32.
        // `MAKEPAD_VISION_KV=f32|f16` pins it, so either shape can be run on
        // either backend and gated against the same oracle.
        let kv_f16 = match std::env::var("MAKEPAD_VISION_KV").ok().as_deref() {
            Some("f32") => false,
            Some("f16") => true,
            _ => matches!(self.runtime, VisionRuntime::Metal(_)),
        };
        let n_patches = grid_w * grid_h;
        let n_embd = cfg.n_embd as i64;
        let n_heads = cfg.n_heads as i64;
        let d_head = cfg.head_dim() as i64;
        let patch_len = (3 * cfg.patch_size * cfg.patch_size) as i64;
        let n = n_patches as i64;
        let eps = cfg.eps;

        let w_patch0 = self.tensor("v.patch_embd.weight")?;
        let w_patch1 = self.tensor("v.patch_embd.weight.1")?;
        let b_patch = self.tensor("v.patch_embd.bias")?;
        let pos_embd = self.tensor("v.position_embd.weight")?;
        let post_ln_w = self.tensor("v.post_ln.weight")?;
        let post_ln_b = self.tensor("v.post_ln.bias")?;
        let mm0_w = self.tensor("mm.0.weight")?;
        let mm0_b = self.tensor("mm.0.bias")?;
        let mm2_w = self.tensor("mm.2.weight")?;
        let mm2_b = self.tensor("mm.2.bias")?;

        let ctx = &mut self.weights.ctx;
        let a = BufferUsage::Activations;
        let err = LlamaError::format;

        let input_patches = ctx
            .new_named_tensor(
                "vision.inp_patches",
                TensorType::F32,
                2,
                &[patch_len, n],
                a,
            )
            .map_err(err)?;
        let input_positions = ctx
            .new_named_tensor("vision.inp_positions", TensorType::I32, 1, &[n * 4], a)
            .map_err(err)?;
        let input_block_index = ctx
            .new_named_tensor("vision.inp_block_index", TensorType::I32, 1, &[n], a)
            .map_err(err)?;

        // patchify: the two temporal conv kernels both apply to the same still
        // frame; conv == matmul against the flattened [patch_len, n_embd] weight
        let w0 = ctx.reshape(w_patch0, &[patch_len, n_embd]).map_err(err)?;
        let w1 = ctx.reshape(w_patch1, &[patch_len, n_embd]).map_err(err)?;
        let e0 = ctx.mul_mat(w0, input_patches, a).map_err(err)?;
        let e1 = ctx.mul_mat(w1, input_patches, a).map_err(err)?;
        let mut cur = ctx.binary_like_a(Op::Add, e0, e1, a).map_err(err)?;
        cur = ctx.binary_like_a(Op::Add, cur, b_patch, a).map_err(err)?;

        // learned position embeddings, bilinearly interpolated from the native
        // grid, gathered into block-major sequence order
        {
            let n_per_side = {
                let t = ctx
                    .tensor(pos_embd)
                    .ok_or_else(|| LlamaError::format("missing position embedding tensor"))?;
                (t.ne[1] as f64).sqrt() as i64
            };
            let mut pos = pos_embd;
            if n_per_side != grid_w as i64 || n_per_side != grid_h as i64 {
                pos = ctx
                    .reshape(pos, &[n_embd, n_per_side, n_per_side])
                    .map_err(err)?;
                pos = ctx.permute(pos, [2, 0, 1, 3]).map_err(err)?;
                pos = ctx
                    .cont_3d(pos, n_per_side, n_per_side, n_embd)
                    .map_err(err)?;
                pos = ctx
                    .upscale_ext(
                        pos,
                        grid_w as i64,
                        grid_h as i64,
                        n_embd,
                        1,
                        ScaleMode::Bilinear,
                        false,
                        true,
                        a,
                    )
                    .map_err(err)?;
                pos = ctx.permute(pos, [1, 2, 0, 3]).map_err(err)?;
                pos = ctx.cont_2d(pos, n_embd, n).map_err(err)?;
            }
            let pos_seq = ctx.get_rows(pos, input_block_index, a).map_err(err)?;
            cur = ctx.binary_like_a(Op::Add, cur, pos_seq, a).map_err(err)?;
        }
        ctx.set_tensor_name(cur, "vision.inp_pos_emb").map_err(err)?;

        let scale = 1.0 / (d_head as f32).sqrt();
        let rope_sections = [(d_head / 4) as i32; 4];

        for il in 0..cfg.n_layers {
            let name = |suffix: &str| format!("v.blk.{il}.{suffix}");
            let ln1_w = self_tensor(&self.weights, &name("ln1.weight"))?;
            let ln1_b = self_tensor(&self.weights, &name("ln1.bias"))?;
            let ln2_w = self_tensor(&self.weights, &name("ln2.weight"))?;
            let ln2_b = self_tensor(&self.weights, &name("ln2.bias"))?;
            let qkv_w = self_tensor(&self.weights, &name("attn_qkv.weight"))?;
            let qkv_b = self_tensor(&self.weights, &name("attn_qkv.bias"))?;
            let out_w = self_tensor(&self.weights, &name("attn_out.weight"))?;
            let out_b = self_tensor(&self.weights, &name("attn_out.bias"))?;
            let up_w = self_tensor(&self.weights, &name("ffn_up.weight"))?;
            let up_b = self_tensor(&self.weights, &name("ffn_up.bias"))?;
            let down_w = self_tensor(&self.weights, &name("ffn_down.weight"))?;
            let down_b = self_tensor(&self.weights, &name("ffn_down.bias"))?;

            let ctx = &mut self.weights.ctx;
            let residual = cur;

            // ln1 (LayerNorm with weight + bias)
            let mut x = ctx.norm_eps(cur, eps, a).map_err(err)?;
            x = ctx.binary_like_a(Op::Mul, x, ln1_w, a).map_err(err)?;
            x = ctx.binary_like_a(Op::Add, x, ln1_b, a).map_err(err)?;

            // fused qkv, split via weight/bias row slices
            let (q, k, v) = {
                let row_bytes_w = weight_row_bytes(ctx, qkv_w)?;
                let bias_elem_bytes = 4usize; // qkv bias is f32
                let mut parts = Vec::with_capacity(3);
                for part in 0..3 {
                    let w = ctx
                        .view_2d(
                            qkv_w,
                            n_embd,
                            n_embd,
                            row_bytes_w,
                            part * n_embd as usize * row_bytes_w,
                        )
                        .map_err(err)?;
                    let b = ctx
                        .view_1d(qkv_b, n_embd, part * n_embd as usize * bias_elem_bytes)
                        .map_err(err)?;
                    let mut p = ctx.mul_mat(w, x, a).map_err(err)?;
                    p = ctx.binary_like_a(Op::Add, p, b, a).map_err(err)?;
                    parts.push(p);
                }
                (parts[0], parts[1], parts[2])
            };

            // [n_embd, n] -> [d_head, n_heads, n], 2D vision rope on q/k
            let q3 = ctx.reshape(q, &[d_head, n_heads, n]).map_err(err)?;
            let k3 = ctx.reshape(k, &[d_head, n_heads, n]).map_err(err)?;
            let v3 = ctx.reshape(v, &[d_head, n_heads, n]).map_err(err)?;
            let q3 = ctx
                .rope_multi(
                    q3,
                    input_positions,
                    None,
                    (d_head / 2) as i32,
                    rope_sections,
                    GGML_ROPE_TYPE_VISION,
                    32768,
                    10000.0,
                    1.0,
                    0.0,
                    1.0,
                    32.0,
                    1.0,
                    a,
                )
                .map_err(err)?;
            let k3 = ctx
                .rope_multi(
                    k3,
                    input_positions,
                    None,
                    (d_head / 2) as i32,
                    rope_sections,
                    GGML_ROPE_TYPE_VISION,
                    32768,
                    10000.0,
                    1.0,
                    0.0,
                    1.0,
                    32.0,
                    1.0,
                    a,
                )
                .map_err(err)?;

            // full bidirectional attention (no mask)
            let q_p = ctx.permute(q3, [0, 2, 1, 3]).map_err(err)?;
            let k_p = ctx.permute(k3, [0, 2, 1, 3]).map_err(err)?;
            let v_p = ctx.permute(v3, [0, 2, 1, 3]).map_err(err)?;
            let attn = if kv_f16 {
                // f16 K/V — the Metal path.
                let k_h = new_f16_like(ctx, k_p, &[d_head, n, n_heads, 1])?;
                let k_h = ctx.cpy(k_p, k_h, a).map_err(err)?;
                let v_h = new_f16_like(ctx, v_p, &[d_head, n, n_heads, 1])?;
                let v_h = ctx.cpy(v_p, v_h, a).map_err(err)?;
                let attn = ctx
                    .flash_attn_ext(q_p, k_h, v_h, None, scale, 0.0, 0.0, a)
                    .map_err(err)?;
                ctx.flash_attn_ext_set_prec(attn, Prec::F32).map_err(err)?;
                ctx.reshape(attn, &[n_embd, n]).map_err(err)?
            } else {
                // f32 K/V — routes to the CUDA maskless kernel.
                // Contiguous copies because that path reads plain strides.
                let k_c = ctx.cont_3d(k_p, d_head, n, n_heads).map_err(err)?;
                let v_c = ctx.cont_3d(v_p, d_head, n, n_heads).map_err(err)?;
                let attn = ctx
                    .flash_attn_ext(q_p, k_c, v_c, None, scale, 0.0, 0.0, a)
                    .map_err(err)?;
                ctx.flash_attn_ext_set_prec(attn, Prec::F32).map_err(err)?;
                ctx.reshape(attn, &[n_embd, n]).map_err(err)?
            };

            let mut o = ctx.mul_mat(out_w, attn, a).map_err(err)?;
            o = ctx.binary_like_a(Op::Add, o, out_b, a).map_err(err)?;
            cur = ctx.binary_like_a(Op::Add, o, residual, a).map_err(err)?;

            // ln2 + ffn
            let residual2 = cur;
            let mut f = ctx.norm_eps(cur, eps, a).map_err(err)?;
            f = ctx.binary_like_a(Op::Mul, f, ln2_w, a).map_err(err)?;
            f = ctx.binary_like_a(Op::Add, f, ln2_b, a).map_err(err)?;
            f = ctx.mul_mat(up_w, f, a).map_err(err)?;
            f = ctx.binary_like_a(Op::Add, f, up_b, a).map_err(err)?;
            f = ctx.unary(f, UnaryOp::Gelu, a).map_err(err)?;
            f = ctx.mul_mat(down_w, f, a).map_err(err)?;
            f = ctx.binary_like_a(Op::Add, f, down_b, a).map_err(err)?;
            cur = ctx.binary_like_a(Op::Add, f, residual2, a).map_err(err)?;
            ctx.set_tensor_name(cur, &format!("vision.blk{il}.out"))
                .map_err(err)?;
        }

        let ctx = &mut self.weights.ctx;

        // post layernorm
        cur = ctx.norm_eps(cur, eps, a).map_err(err)?;
        cur = ctx.binary_like_a(Op::Mul, cur, post_ln_w, a).map_err(err)?;
        cur = ctx.binary_like_a(Op::Add, cur, post_ln_b, a).map_err(err)?;

        // merger: concat each 2x2 block (4 consecutive tokens) then 2-layer MLP
        let merge = (cfg.n_merge * cfg.n_merge) as i64;
        cur = ctx.reshape(cur, &[n_embd * merge, n / merge]).map_err(err)?;
        cur = ctx.mul_mat(mm0_w, cur, a).map_err(err)?;
        cur = ctx.binary_like_a(Op::Add, cur, mm0_b, a).map_err(err)?;
        cur = ctx.unary(cur, UnaryOp::Gelu, a).map_err(err)?;
        cur = ctx.mul_mat(mm2_w, cur, a).map_err(err)?;
        cur = ctx.binary_like_a(Op::Add, cur, mm2_b, a).map_err(err)?;
        ctx.set_tensor_name(cur, "vision.output").map_err(err)?;

        let mut graph = Graph::new();
        graph
            .build_forward_expand(&self.weights.ctx, cur)
            .map_err(err)?;
        let session = match &self.runtime {
            VisionRuntime::Metal(runtime) => {
                let prepared =
                    prepare_graph(&self.weights.ctx, &graph, runtime.features()).map_err(err)?;
                VisionSession::Metal(
                    MetalGraphSession::from_runtime(
                        runtime.clone(),
                        &self.weights.ctx,
                        &prepared,
                        BufferStorageMode::Shared,
                        BufferStorageMode::Shared,
                    )
                    .map_err(err)?,
                )
            }
            VisionRuntime::Cuda(runtime) => VisionSession::Cuda(Box::new(
                runtime.create_raw_graph_session(&self.weights.ctx, &graph, &[cur])?,
            )),
        };

        Ok(VisionGraph {
            session,
            input_patches,
            input_positions,
            input_block_index,
            output: cur,
            _grid_w: grid_w,
            _grid_h: grid_h,
        })
    }
}

fn self_tensor(weights: &LoadedGgufWeights, name: &str) -> Result<TensorId> {
    weights.require_tensor_id(name)
}

fn weight_row_bytes(ctx: &Context, id: TensorId) -> Result<usize> {
    let t = ctx
        .tensor(id)
        .ok_or_else(|| LlamaError::format("missing qkv weight tensor"))?;
    Ok(t.nb[1])
}

fn new_f16_like(ctx: &mut Context, _src: TensorId, ne: &[i64]) -> Result<TensorId> {
    ctx.new_tensor(TensorType::F16, ne.len(), ne, BufferUsage::Activations)
        .map_err(LlamaError::format)
}

fn f32s_as_bytes(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for v in values {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    bytes
}

fn i32s_as_bytes(values: &[i32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for v in values {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    bytes
}

fn bytes_to_f32s(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn required_string(gguf: &GgufFile, key: &str) -> Result<String> {
    match gguf.get_value(key) {
        Some(GgufValue::String(s)) => Ok(String::from_utf8_lossy(s.as_bytes()).into_owned()),
        Some(_) => Err(LlamaError::format(format!("key '{key}' is not a string"))),
        None => Err(LlamaError::format(format!("missing key '{key}'"))),
    }
}

fn required_usize(gguf: &GgufFile, key: &str) -> Result<usize> {
    optional_usize(gguf, key)?
        .ok_or_else(|| LlamaError::format(format!("missing key '{key}'")))
}

fn optional_usize(gguf: &GgufFile, key: &str) -> Result<Option<usize>> {
    match gguf.get_value(key) {
        Some(GgufValue::Uint32(v)) => Ok(Some(*v as usize)),
        Some(GgufValue::Int32(v)) => Ok(Some(*v as usize)),
        Some(GgufValue::Uint64(v)) => Ok(Some(*v as usize)),
        Some(_) => Err(LlamaError::format(format!("key '{key}' is not an integer"))),
        None => Ok(None),
    }
}

fn optional_f32(gguf: &GgufFile, key: &str) -> Result<Option<f32>> {
    match gguf.get_value(key) {
        Some(GgufValue::Float32(v)) => Ok(Some(*v)),
        Some(_) => Err(LlamaError::format(format!("key '{key}' is not a float"))),
        None => Ok(None),
    }
}

fn required_f32_3(gguf: &GgufFile, key: &str) -> Result<[f32; 3]> {
    match gguf.get_value(key) {
        Some(GgufValue::Array(GgufArray::Float32(v))) if v.len() >= 3 => Ok([v[0], v[1], v[2]]),
        Some(_) => Err(LlamaError::format(format!(
            "key '{key}' is not a float array of 3"
        ))),
        None => Err(LlamaError::format(format!("missing key '{key}'"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calc_size_matches_reference_examples() {
        // 512x384 already aligned -> unchanged
        assert_eq!(calc_size_preserved_ratio(512, 384, 32, 8192, 4194304), (512, 384));
        // 500x375 rounds to nearest multiple of 32 (both round up here)
        assert_eq!(calc_size_preserved_ratio(500, 375, 32, 8192, 4194304), (512, 384));
        // huge image scales down under max_pixels
        let (w, h) = calc_size_preserved_ratio(3840, 2160, 32, 8192, 4194304);
        assert!(w * h <= 4194304);
        assert_eq!(w % 32, 0);
        assert_eq!(h % 32, 0);
    }

    #[test]
    fn block_order_covers_all_patches_once() {
        let idx = block_order_row_indices(6, 4);
        let mut seen = vec![false; 24];
        for &i in &idx {
            assert!(!seen[i as usize]);
            seen[i as usize] = true;
        }
        assert!(seen.iter().all(|&s| s));
        // first block: (0,0) (0,1) (1,0) (1,1) in row-major patch coords
        assert_eq!(&idx[0..4], &[0, 1, 6, 7]);
    }

    #[test]
    fn rope_positions_follow_block_order() {
        let pos = vision_rope_positions(4, 2);
        let n = 8;
        // token 0..3 = block at (0,0): (y,x) = (0,0) (0,1) (1,0) (1,1)
        assert_eq!(&pos[0..4], &[0, 0, 1, 1]); // y plane
        assert_eq!(&pos[n..n + 4], &[0, 1, 0, 1]); // x plane
        assert_eq!(pos[2 * n], pos[0]);
        assert_eq!(pos[3 * n], pos[n]);
    }
}
