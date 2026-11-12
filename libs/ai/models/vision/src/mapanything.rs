//! Native MapAnything inference: metric multi-view geometry from N images.
//!
//! One forward takes N views (518x518 images, each with optional calibration
//! rays, depth along those rays and a camera pose) and returns per view the
//! ray directions, depth along each ray, a confidence and an ambiguity mask,
//! the camera pose in the frame of view 0, plus one metric scale for the set.
//!
//! The graph, stage by stage:
//! - DINOv2 ViT-g/14 encoder (24 blocks, SwiGLU MLP, no final norm): patch
//!   tokens plus the class token as the view's extra token.
//! - Geometric inputs, each encoded and added to the patch tokens: rays and
//!   depth by pixel-unshuffle conv encoders, depth scale, rotation,
//!   translation and translation scale by small MLPs; then one LayerNorm.
//! - A 16-block alternating-attention transformer over all views' tokens and
//!   a scale token: even blocks attend over everything, odd blocks within each
//!   view (the scale token sits those out).
//! - The dense and pose heads (`mapanything_heads.rs`) on [fused
//!   tokens, blocks 7, 11, 15] and on block 15, and a scale head on the scale
//!   token; then the output adaptors.
//!
//! [`MapAnythingPrecision::StrictF32`] runs every op in f32 (GEMMs without
//! TF32, cuDNN convolutions pinned to FMA math). [`MapAnythingPrecision::Bf16`]
//! gives the encoder and the multi-view transformer bf16 GEMM operands (f32
//! accumulation) and bf16 flash attention; the geometric encoders and every
//! head stay f32.

use crate::backend::WeightOwner;
use std::sync::Arc;
use crate::backend::{
    gpu_add, gpu_attention_packed_f32, gpu_attention_packed_flash_bf16,
    gpu_birefnet_image_to_patches, gpu_birefnet_tokens_to_planar, gpu_concat_cols,
    gpu_concat_rows_many, gpu_download, gpu_gelu_erf, gpu_layer_norm_mod,
    gpu_linear_f32_resident, gpu_realesrgan_bias_lrelu_f32, gpu_realesrgan_conv3x3_f32,
    gpu_slice_cols, gpu_slice_rows, gpu_swiglu_gate_first, GpuTensor,
};
use crate::da3::{
    f32_to_bf16_bytes, norm_mods, tensor, upload, CacheScope, Da3WeightFile, DinoLayer,
    DinoLayerBf16, DinoLinears, Planar, TensorSource,
};
use crate::mapanything_heads::{DenseHead, PoseHead};
use crate::mapanything_prep::pose_in_frame_of;
use crate::{emit_progress, DiffusionError, ProgressHook, Result};
use std::path::Path;

/// Input side in pixels (both axes).
pub const MAPANYTHING_SIZE: usize = 518;
pub const MAPANYTHING_PATCH: usize = 14;
/// Patch grid side: 518 / 14.
pub const MAPANYTHING_SIDE: usize = 37;
const PIXELS: usize = MAPANYTHING_SIZE * MAPANYTHING_SIZE;
const PATCHES: usize = MAPANYTHING_SIDE * MAPANYTHING_SIDE;
/// A view's tokens in the transformers: its patches, then its class token.
const VIEW_TOKENS: usize = PATCHES + 1;
const HIDDEN: usize = 1536;
const HEADS: usize = 24;
const HEAD_DIM: usize = 64;
/// SwiGLU hidden width: `w12` makes two of these, `w3` takes one.
const SWIGLU: usize = 4096;
const ENCODER_BLOCKS: usize = 24;
const SHARE_BLOCKS: usize = 16;
/// Multi-view transformer blocks whose normalised output feeds the dense head
/// (the last block's output is the third).
const SHARE_TAPS: [usize; 3] = [7, 11, 15];
const NORM_EPS: f32 = 1e-6;
const SCALE_FEATURES: usize = 196;
const UNSHUFFLE: usize = MAPANYTHING_PATCH * MAPANYTHING_PATCH;

/// Peak live device bytes over the forward's stages, each tensor at the size
/// the backend allocates (`alloc`); weights are excluded (resident before a forward).
pub fn forward_activation_bytes(views: usize, alloc: impl Fn(usize) -> usize) -> u64 {
    // Every activation in the forward is f32.
    let t = |rows, cols| alloc(rows * cols * 4);
    // Encoder token rows across all views.
    let v = views * VIEW_TOKENS;
    // Multi-view transformer rows include the scale token.
    let r = v + 1;
    // Forward keeps fused patches, registers and reference view rows.
    let retained = views * t(PATCHES, HIDDEN) + views * t(1, HIDDEN) + t(VIEW_TOKENS, HIDDEN);
    // Block keeps per-view attention outputs alongside their concatenation.
    let parts = if views == 1 { 0 } else { views * t(VIEW_TOKENS, HIDDEN) };
    // Block's swiglu statement retains the inputs and intermediate tensors.
    let block_mlp = |rows, parts| {
        5 * t(rows, HIDDEN) + t(rows, 3 * HIDDEN) + parts
            + t(rows, 2 * SWIGLU) + t(rows, SWIGLU)
    };
    // Encoder block at the swiglu statement.
    let encoder = block_mlp(v, parts);
    // Odd shared block holds two SHARE_TAPS, caller hidden and scale row beside its body copy.
    let odd_block = retained + 2 * t(r, HIDDEN) + t(r, HIDDEN) + t(1, HIDDEN)
        + block_mlp(v, parts);
    // Even shared block holds two SHARE_TAPS and attends over the scale row too.
    let even_block = retained + 2 * t(r, HIDDEN) + block_mlp(r, 0);
    // Forward copies per-view features while all three taps are alive.
    let features = retained + 3 * t(r, HIDDEN) + 3 * views * t(PATCHES, HIDDEN);
    // DenseHead retains conv1, resized, conv2, relu, transposed and both conv3 layouts.
    let dense = retained + 3 * views * t(PATCHES, HIDDEN)
        + t(crate::mapanything_heads::OUTPUT_INNER, (MAPANYTHING_SIDE * 8).pow(2))
        + 4 * t(crate::mapanything_heads::OUTPUT_INNER, PIXELS)
        + 2 * t(crate::mapanything_heads::OUTPUT_CHANNELS, PIXELS);
    encoder.max(odd_block).max(even_block).max(features).max(dense) as u64
}

/// Explicit compute mode, chosen at load: it decides which weight copies exist
/// on the device, so a loaded model runs in exactly one mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapAnythingPrecision {
    /// Encoder and multi-view transformer: bf16 GEMM operands with f32
    /// accumulation and bf16 flash attention. Everything else f32.
    Bf16,
    /// Every op in f32.
    StrictF32,
}

/// One input view. Images are ImageNet-normalised planar RGB, `3 x 518 x 518`
/// (`mapanything_prep::normalize_rgb8`). Camera frames are OpenCV (x right,
/// y down, z forward); poses are camera-to-world.
#[derive(Clone, Copy, Debug)]
pub struct MapAnythingView<'a> {
    pub image: &'a [f32],
    /// Unit ray directions in the camera frame, `x, y, z` per pixel.
    pub rays: Option<&'a [f32]>,
    /// Depth along each ray, one per pixel; 0 where unknown.
    pub depth_along_ray: Option<&'a [f32]>,
    /// Camera-to-world rotation (quaternion `x, y, z, w`) and translation. If
    /// any view has a pose, view 0 must have one: poses enter in its frame.
    pub pose: Option<([f32; 4], [f32; 3])>,
    /// The depth and pose inputs are in metres.
    pub metric: bool,
}

/// One view's prediction. Maps are row-major `y * 518 + x`.
#[derive(Clone, Debug)]
pub struct MapAnythingViewOutput {
    /// Unit ray directions in the camera frame, `x, y, z` per pixel.
    pub rays: Vec<f32>,
    /// Metric depth along each ray.
    pub depth_along_ray: Vec<f32>,
    /// `1 + exp(logit)`: 1 is least confident.
    pub confidence: Vec<f32>,
    /// Ambiguity logits; a pixel is non-ambiguous where `sigmoid(logit) > 0.5`.
    pub mask_logits: Vec<f32>,
    /// Camera-to-world pose in view 0's frame: unit quaternion `x, y, z, w`
    /// and metric translation.
    pub quat: [f32; 4],
    pub trans: [f32; 3],
}

impl MapAnythingViewOutput {
    /// Camera-frame points, `x, y, z` per pixel.
    pub fn points_camera(&self) -> Vec<f32> {
        let mut out = Vec::with_capacity(self.rays.len());
        for (ray, &depth) in self.rays.chunks_exact(3).zip(&self.depth_along_ray) {
            out.extend(ray.iter().map(|value| value * depth));
        }
        out
    }

    /// Points in view 0's frame, `x, y, z` per pixel.
    pub fn points_world(&self) -> Vec<f32> {
        let rotation = rotation(self.quat);
        let mut out = Vec::with_capacity(self.rays.len());
        for (ray, &depth) in self.rays.chunks_exact(3).zip(&self.depth_along_ray) {
            let local = [ray[0] * depth, ray[1] * depth, ray[2] * depth];
            for (row, offset) in rotation.iter().zip(self.trans) {
                out.push(row[0] * local[0] + row[1] * local[1] + row[2] * local[2] + offset);
            }
        }
        out
    }
}

#[derive(Clone, Debug)]
pub struct MapAnythingOutput {
    pub views: Vec<MapAnythingViewOutput>,
    /// The metric scale the predictions were multiplied by.
    pub metric_scale: f32,
}

/// Validation tap sink: `(reference module name, values in the reference
/// tensor's own layout)`. Taps download, so timed runs pass none.
pub type MapAnythingTap<'a> = &'a mut dyn FnMut(&str, Vec<f32>);

struct Tap<'a, 'b> {
    sink: Option<&'a mut (dyn FnMut(&str, Vec<f32>) + 'b)>,
}

impl Tap<'_, '_> {
    fn on(&self) -> bool {
        self.sink.is_some()
    }

    fn put(&mut self, name: &str, values: Vec<f32>) {
        if let Some(sink) = self.sink.as_mut() {
            sink(name, values);
        }
    }

    /// Tensors stacked as they are (planar views give NCHW, token views NLC).
    fn stacked(&mut self, name: &str, parts: &[&GpuTensor]) -> Result<()> {
        if !self.on() {
            return Ok(());
        }
        let mut values = Vec::new();
        for part in parts {
            values.extend(download(part)?);
        }
        self.put(name, values);
        Ok(())
    }

    /// Token tensors `[tokens, channels]`, each transposed: NCHW.
    fn channels_first(&mut self, name: &str, parts: &[&GpuTensor]) -> Result<()> {
        if !self.on() {
            return Ok(());
        }
        let mut values = Vec::new();
        for part in parts {
            let data = download(part)?;
            let (rows, cols) = (part.rows(), part.cols());
            for col in 0..cols {
                values.extend((0..rows).map(|row| data[row * cols + col]));
            }
        }
        self.put(name, values);
        Ok(())
    }
}

fn model_error(error: String) -> DiffusionError {
    DiffusionError::model(error)
}

fn download(tensor: &GpuTensor) -> Result<Vec<f32>> {
    gpu_download(tensor).map_err(model_error)
}

fn transpose(tensor: &GpuTensor) -> Result<GpuTensor> {
    gpu_birefnet_tokens_to_planar(tensor).map_err(model_error)
}

fn add(left: &GpuTensor, right: &GpuTensor) -> Result<GpuTensor> {
    gpu_add(left, right).map_err(model_error)
}

fn rows(tensor: &GpuTensor, start: usize, len: usize) -> Result<GpuTensor> {
    gpu_slice_rows(tensor, start, len).map_err(model_error)
}

/// A device copy (tensors are not `Clone`).
fn copy(tensor: &GpuTensor) -> Result<GpuTensor> {
    rows(tensor, 0, tensor.rows())
}

fn concat_rows(parts: &[&GpuTensor]) -> Result<GpuTensor> {
    gpu_concat_rows_many(parts).map_err(model_error)
}

fn gelu(tensor: &GpuTensor) -> Result<GpuTensor> {
    gpu_gelu_erf(tensor).map_err(model_error)
}

fn layer_norm(tensor: &GpuTensor, mods: &GpuTensor, dim: usize) -> Result<GpuTensor> {
    gpu_layer_norm_mod(tensor, mods, 0, dim, NORM_EPS).map_err(model_error)
}

/// Rotation matrix of a (normalised) quaternion `x, y, z, w`, row-major.
fn rotation(quat: [f32; 4]) -> [[f32; 3]; 3] {
    let norm = quat.iter().map(|value| value * value).sum::<f32>().sqrt();
    let [x, y, z, w] = quat.map(|value| value / norm);
    [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - w * z), 2.0 * (x * z + w * y)],
        [2.0 * (x * y + w * z), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - w * x)],
        [2.0 * (x * z - w * y), 2.0 * (y * z + w * x), 1.0 - 2.0 * (x * x + y * y)],
    ]
}

/// A resident f32 linear (also every 1x1 convolution, on token rows).
pub(crate) struct Linear {
    weight: GpuTensor,
    bias: Option<GpuTensor>,
}

impl Linear {
    pub(crate) fn load(weights: &dyn TensorSource, name: &str, inputs: usize, outputs: usize) -> Result<Self> {
        Ok(Self {
            weight: upload(&tensor(weights, &format!("{name}.weight"), outputs * inputs)?, outputs, inputs)?,
            bias: Some(upload(&tensor(weights, &format!("{name}.bias"), outputs)?, 1, outputs)?),
        })
    }

    pub(crate) fn forward(&self, x: &GpuTensor) -> Result<GpuTensor> {
        gpu_linear_f32_resident(x, &self.weight, self.bias.as_ref()).map_err(model_error)
    }

    /// As a 1x1 convolution over a planar map.
    pub(crate) fn forward_planar(&self, x: &Planar) -> Result<Planar> {
        Ok(Planar {
            tensor: transpose(&self.forward(&transpose(&x.tensor)?)?)?,
            width: x.width,
            height: x.height,
        })
    }
}

/// A small host-side linear (the pose and scale MLPs), f64 accumulation.
pub(crate) struct HostLinear {
    weight: Vec<f32>,
    bias: Vec<f32>,
    inputs: usize,
}

impl HostLinear {
    pub(crate) fn load(weights: &dyn TensorSource, name: &str, inputs: usize, outputs: usize) -> Result<Self> {
        Ok(Self {
            weight: tensor(weights, &format!("{name}.weight"), outputs * inputs)?,
            bias: tensor(weights, &format!("{name}.bias"), outputs)?,
            inputs,
        })
    }

    pub(crate) fn forward(&self, x: &[f32]) -> Vec<f32> {
        self.weight
            .chunks_exact(self.inputs)
            .zip(&self.bias)
            .map(|(row, &bias)| {
                let sum: f64 = row.iter().zip(x).map(|(&w, &v)| f64::from(w) * f64::from(v)).sum();
                (sum + f64::from(bias)) as f32
            })
            .collect()
    }
}

fn host_relu(values: Vec<f32>) -> Vec<f32> {
    values.into_iter().map(|value| value.max(0.0)).collect()
}

/// 3x3 stride-1 "same" convolution, f32 with FMA math (no TF32).
pub(crate) struct Conv3 {
    owner: Arc<WeightOwner>,
    key: String,
    weights: Vec<f32>,
    bias: Option<Vec<f32>>,
    inputs: usize,
    outputs: usize,
}

impl Conv3 {
    pub(crate) fn load(
        scope: &CacheScope,
        weights: &dyn TensorSource,
        name: &str,
        inputs: usize,
        outputs: usize,
        bias: bool,
    ) -> Result<Self> {
        Ok(Self {
            owner: Arc::clone(&scope.owner),
            key: scope.key(name),
            weights: tensor(weights, &format!("{name}.weight"), outputs * inputs * 9)?,
            bias: if bias {
                Some(tensor(weights, &format!("{name}.bias"), outputs)?)
            } else {
                None
            },
            inputs,
            outputs,
        })
    }

    pub(crate) fn forward(&self, x: &Planar) -> Result<Planar> {
        if x.tensor.rows() != self.inputs {
            return Err(DiffusionError::workflow(format!(
                "MapAnything conv {}: {} channels in, {} expected",
                self.key, x.tensor.rows(), self.inputs
            )));
        }
        let out = gpu_realesrgan_conv3x3_f32(
            &x.tensor,
            self.inputs,
            x.width,
            x.height,
            &self.owner,
            &self.key,
            &self.weights,
            self.outputs,
        )
        .map_err(model_error)?;
        if let Some(bias) = &self.bias {
            // A leaky slope of 1 is a pure bias add.
            gpu_realesrgan_bias_lrelu_f32(&out, &self.owner, &format!("{}.bias", self.key), bias, 1.0)
                .map_err(model_error)?;
        }
        Ok(Planar {
            tensor: out,
            width: x.width,
            height: x.height,
        })
    }
}

fn add_planar(left: &Planar, right: &Planar) -> Result<Planar> {
    if (left.width, left.height) != (right.width, right.height) {
        return Err(DiffusionError::workflow("MapAnything planar add: size mismatch"));
    }
    Ok(Planar {
        tensor: add(&left.tensor, &right.tensor)?,
        width: left.width,
        height: left.height,
    })
}

/// Pixel-unshuffle encoder for a dense input (rays: 3 channels, depth: 1).
struct DenseEncoder {
    conv_in: Conv3,
    blocks: Vec<ResidualBlock>,
    out: Linear,
    norm: GpuTensor,
    name: &'static str,
}

/// `gelu(conv2(gelu(conv1 x)) + shortcut x)`, convs 3x3, shortcut 1x1.
struct ResidualBlock {
    conv1: Conv3,
    conv2: Conv3,
    shortcut: Linear,
}

impl DenseEncoder {
    fn load(scope: &CacheScope, weights: &dyn TensorSource, name: &'static str, channels: usize) -> Result<Self> {
        let unshuffled = channels * UNSHUFFLE;
        let mut blocks = Vec::new();
        for (index, (inputs, outputs)) in [(588usize, 768usize), (768, 1024)].into_iter().enumerate() {
            let prefix = format!("{name}.encoder.{index}");
            blocks.push(ResidualBlock {
                conv1: Conv3::load(scope, weights, &format!("{prefix}.conv1"), inputs, outputs, true)?,
                conv2: Conv3::load(scope, weights, &format!("{prefix}.conv2"), outputs, outputs, true)?,
                shortcut: Linear::load(weights, &format!("{prefix}.shortcut"), inputs, outputs)?,
            });
        }
        Ok(Self {
            conv_in: Conv3::load(scope, weights, &format!("{name}.conv_in"), unshuffled, 588, true)?,
            blocks,
            out: Linear::load(weights, &format!("{name}.encoder.2"), 1024, HIDDEN)?,
            norm: norm_mods(weights, &format!("{name}.norm_layer"), HIDDEN)?,
            name,
        })
    }

    /// `unshuffled`: per view `[channels * 196, 37 * 37]`. Returns token rows
    /// `[1369, 1536]` per view.
    fn forward(&self, unshuffled: Vec<GpuTensor>, tap: &mut Tap) -> Result<Vec<GpuTensor>> {
        let name = self.name;
        let parts: Vec<&GpuTensor> = unshuffled.iter().collect();
        tap.stacked(&format!("module.{name}.unshuffle"), &parts)?;
        let mut conv_in = Vec::new();
        let mut encoded = Vec::new();
        let mut normed = Vec::new();
        for input in unshuffled {
            let mut x = self.conv_in.forward(&Planar {
                tensor: input,
                width: MAPANYTHING_SIDE,
                height: MAPANYTHING_SIDE,
            })?;
            if tap.on() {
                conv_in.push(copy(&x.tensor)?);
            }
            for block in &self.blocks {
                let shortcut = block.shortcut.forward_planar(&x)?;
                let hidden = block.conv1.forward(&x)?;
                let hidden = Planar {
                    tensor: gelu(&hidden.tensor)?,
                    width: hidden.width,
                    height: hidden.height,
                };
                let hidden = block.conv2.forward(&hidden)?;
                let sum = add_planar(&hidden, &shortcut)?;
                x = Planar {
                    tensor: gelu(&sum.tensor)?,
                    width: sum.width,
                    height: sum.height,
                };
            }
            let tokens = self.out.forward(&transpose(&x.tensor)?)?;
            let tokens_normed = layer_norm(&tokens, &self.norm, HIDDEN)?;
            if tap.on() {
                encoded.push(tokens);
            }
            normed.push(tokens_normed);
        }
        let conv_in: Vec<&GpuTensor> = conv_in.iter().collect();
        tap.stacked(&format!("module.{name}.conv_in"), &conv_in)?;
        let encoded: Vec<&GpuTensor> = encoded.iter().collect();
        tap.channels_first(&format!("module.{name}.encoder"), &encoded)?;
        let parts: Vec<&GpuTensor> = normed.iter().collect();
        tap.stacked(&format!("module.{name}.norm_layer"), &parts)?;
        tap.channels_first(&format!("module.{name}.features"), &parts)?;
        Ok(normed)
    }
}

/// MLP encoder for a per-view vector: `in -> 128 -> 256 -> 512 -> 1536`
/// (GELU between), then LayerNorm.
struct GlobalEncoder {
    layers: Vec<Linear>,
    norm: GpuTensor,
    name: &'static str,
}

impl GlobalEncoder {
    fn load(weights: &dyn TensorSource, name: &'static str, inputs: usize) -> Result<Self> {
        let names = ["encoder.0.0.0.0", "encoder.0.0.1", "encoder.0.1", "encoder.1"];
        let widths = [inputs, 128, 256, 512, HIDDEN];
        let mut layers = Vec::new();
        for (index, layer) in names.iter().enumerate() {
            layers.push(Linear::load(weights, &format!("{name}.{layer}"), widths[index], widths[index + 1])?);
        }
        Ok(Self {
            layers,
            norm: norm_mods(weights, &format!("{name}.norm_layer"), HIDDEN)?,
            name,
        })
    }

    /// `values`: `views x inputs`. Returns `views x 1536`.
    fn forward(&self, values: &[f32], views: usize, tap: &mut Tap) -> Result<Vec<f32>> {
        let name = self.name;
        let mut x = upload(values, views, values.len() / views)?;
        for (index, layer) in self.layers.iter().enumerate() {
            x = layer.forward(&x)?;
            if index + 1 < self.layers.len() {
                x = gelu(&x)?;
                tap.stacked(&format!("module.{name}.act_layer"), &[&x])?;
            }
        }
        tap.stacked(&format!("module.{name}.encoder"), &[&x])?;
        let normed = layer_norm(&x, &self.norm, HIDDEN)?;
        tap.stacked(&format!("module.{name}.norm_layer"), &[&normed])?;
        tap.stacked(&format!("module.{name}.features"), &[&normed])?;
        download(&normed)
    }
}

struct ScaleHead {
    proj: HostLinear,
    mlp: [HostLinear; 2],
    out: HostLinear,
}

impl ScaleHead {
    fn load(weights: &dyn TensorSource) -> Result<Self> {
        Ok(Self {
            proj: HostLinear::load(weights, "scale_head.proj", HIDDEN, SCALE_FEATURES)?,
            mlp: [
                HostLinear::load(weights, "scale_head.mlp.0.0", SCALE_FEATURES, SCALE_FEATURES)?,
                HostLinear::load(weights, "scale_head.mlp.1.0", SCALE_FEATURES, SCALE_FEATURES)?,
            ],
            out: HostLinear::load(weights, "scale_head.output_proj", SCALE_FEATURES, 1)?,
        })
    }
}

pub struct MapAnything {
    precision: MapAnythingPrecision,
    patch_w: GpuTensor,
    patch_b: GpuTensor,
    /// Class token plus position embedding, `[1370, 1536]`, and the patch
    /// position embedding alone is its rows 1..
    cls_pos: GpuTensor,
    pos: GpuTensor,
    encoder: Vec<DinoLayer>,
    ray_encoder: DenseEncoder,
    depth_encoder: DenseEncoder,
    depth_scale_encoder: GlobalEncoder,
    rot_encoder: GlobalEncoder,
    trans_encoder: GlobalEncoder,
    trans_scale_encoder: GlobalEncoder,
    fusion_norm: GpuTensor,
    scale_token: GpuTensor,
    /// The reference view's embedding, repeated over its 1370 tokens.
    ref_view_pe: GpuTensor,
    share: Vec<DinoLayer>,
    share_norm: GpuTensor,
    dense: DenseHead,
    pose: PoseHead,
    scale: ScaleHead,
}

fn dino_layer(
    owner: &Arc<WeightOwner>,
    weights: &dyn TensorSource,
    prefix: &str,
    cache_prefix: &str,
    precision: MapAnythingPrecision,
) -> Result<DinoLayer> {
    let scale1 = tensor(weights, &format!("{prefix}.ls1.gamma"), HIDDEN)?;
    let scale2 = tensor(weights, &format!("{prefix}.ls2.gamma"), HIDDEN)?;
    // LayerScale folds into the branch's last linear.
    let mut proj_w = tensor(weights, &format!("{prefix}.attn.proj.weight"), HIDDEN * HIDDEN)?;
    let mut proj_b = tensor(weights, &format!("{prefix}.attn.proj.bias"), HIDDEN)?;
    let mut w3_w = tensor(weights, &format!("{prefix}.mlp.w3.weight"), HIDDEN * SWIGLU)?;
    let mut w3_b = tensor(weights, &format!("{prefix}.mlp.w3.bias"), HIDDEN)?;
    for row in 0..HIDDEN {
        for value in &mut proj_w[row * HIDDEN..(row + 1) * HIDDEN] {
            *value *= scale1[row];
        }
        proj_b[row] *= scale1[row];
        for value in &mut w3_w[row * SWIGLU..(row + 1) * SWIGLU] {
            *value *= scale2[row];
        }
        w3_b[row] *= scale2[row];
    }
    let qkv_w = tensor(weights, &format!("{prefix}.attn.qkv.weight"), 3 * HIDDEN * HIDDEN)?;
    let qkv_b = tensor(weights, &format!("{prefix}.attn.qkv.bias"), 3 * HIDDEN)?;
    let w12_w = tensor(weights, &format!("{prefix}.mlp.w12.weight"), 2 * SWIGLU * HIDDEN)?;
    let w12_b = tensor(weights, &format!("{prefix}.mlp.w12.bias"), 2 * SWIGLU)?;
    let linears = match precision {
        MapAnythingPrecision::StrictF32 => DinoLinears::F32 {
            qkv_w: upload(&qkv_w, 3 * HIDDEN, HIDDEN)?,
            qkv_b: upload(&qkv_b, 1, 3 * HIDDEN)?,
            proj_w: upload(&proj_w, HIDDEN, HIDDEN)?,
            proj_b: upload(&proj_b, 1, HIDDEN)?,
            fc1_w: upload(&w12_w, 2 * SWIGLU, HIDDEN)?,
            fc1_b: upload(&w12_b, 1, 2 * SWIGLU)?,
            fc2_w: upload(&w3_w, HIDDEN, SWIGLU)?,
            fc2_b: upload(&w3_b, 1, HIDDEN)?,
        },
        MapAnythingPrecision::Bf16 => DinoLinears::Bf16(DinoLayerBf16 {
            qkv: f32_to_bf16_bytes(&qkv_w),
            qkv_key: format!("{cache_prefix}{prefix}.attn.qkv:bf16"),
            qkv_bias: qkv_b,
            proj: f32_to_bf16_bytes(&proj_w),
            proj_key: format!("{cache_prefix}{prefix}.attn.proj:bf16"),
            proj_bias: proj_b,
            fc1: f32_to_bf16_bytes(&w12_w),
            fc1_key: format!("{cache_prefix}{prefix}.mlp.w12:bf16"),
            fc1_bias: w12_b,
            fc2: f32_to_bf16_bytes(&w3_w),
            fc2_key: format!("{cache_prefix}{prefix}.mlp.w3:bf16"),
            fc2_bias: w3_b,
        }),
    };
    Ok(DinoLayer {
        norm1: norm_mods(weights, &format!("{prefix}.norm1"), HIDDEN)?,
        norm2: norm_mods(weights, &format!("{prefix}.norm2"), HIDDEN)?,
        linears,
        hidden: HIDDEN,
        mlp: 2 * SWIGLU,
        owner: Arc::clone(owner),
    })
}

/// Pixel-unshuffle (factor 14) of a channels-last map, `channels` per pixel:
/// out channel `c * 196 + i * 14 + j` at patch `(py, px)` is input channel `c`
/// at pixel `(py * 14 + i, px * 14 + j)`.
fn unshuffle(values: &[f32], channels: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; channels * UNSHUFFLE * PATCHES];
    for py in 0..MAPANYTHING_SIDE {
        for i in 0..MAPANYTHING_PATCH {
            let y = py * MAPANYTHING_PATCH + i;
            for px in 0..MAPANYTHING_SIDE {
                for j in 0..MAPANYTHING_PATCH {
                    let pixel = y * MAPANYTHING_SIZE + px * MAPANYTHING_PATCH + j;
                    for c in 0..channels {
                        let channel = c * UNSHUFFLE + i * MAPANYTHING_PATCH + j;
                        out[channel * PATCHES + py * MAPANYTHING_SIDE + px] = values[pixel * channels + c];
                    }
                }
            }
        }
    }
    out
}

/// Depth over its mean valid (> 0) value, then `log1p`: the reference's
/// `normalize_depth_using_non_zero_pixels` and `apply_log_to_norm` on one
/// channel (whose norm is `|d|`, so it is `sign(d) * log1p(|d|)`).
fn normalized_log_depth(depth: &[f32]) -> (Vec<f32>, f32) {
    let (sum, count) = depth
        .iter()
        .filter(|&&value| value > 0.0)
        .fold((0.0f64, 0usize), |(sum, count), &value| (sum + f64::from(value), count + 1));
    let factor = ((sum / (count as f64 + 1e-8)) as f32).max(1e-8);
    let log = depth
        .iter()
        .map(|&value| {
            let scaled = value / factor;
            let norm = scaled.abs();
            scaled / norm.max(1e-8) * norm.ln_1p()
        })
        .collect();
    (log, factor)
}

impl MapAnything {
    /// Load a checkpoint. `cache_key` names it in the device weight caches
    /// (two checkpoints with the same tensor names must never share entries).
    pub fn load(
        path: impl AsRef<Path>,
        cache_key: &str,
        precision: MapAnythingPrecision,
        mut progress: Option<ProgressHook>,
    ) -> Result<Self> {
        let weights = Da3WeightFile::load(path)?;
        let weights: &dyn TensorSource = &weights;
        let scope = CacheScope {
            owner: Arc::new(WeightOwner::new("mapanything")),
            key_prefix: format!("{cache_key}/"),
        };
        let patch_dim = 3 * UNSHUFFLE;
        let cls = tensor(weights, "encoder.model.cls_token", HIDDEN)?;
        let pos = tensor(weights, "encoder.model.pos_embed", VIEW_TOKENS * HIDDEN)?;
        let mut cls_pos = pos.clone();
        for (value, cls) in cls_pos[..HIDDEN].iter_mut().zip(&cls) {
            *value += cls;
        }
        let mut encoder = Vec::with_capacity(ENCODER_BLOCKS);
        for index in 0..ENCODER_BLOCKS {
            emit_progress(&mut progress, "load MapAnything encoder", index as f64 / 48.0)?;
            encoder.push(dino_layer(&scope.owner, weights, &format!("encoder.model.blocks.{index}"), &scope.key_prefix, precision)?);
        }
        let mut share = Vec::with_capacity(SHARE_BLOCKS);
        for index in 0..SHARE_BLOCKS {
            emit_progress(&mut progress, "load MapAnything multi-view transformer", (24 + index) as f64 / 48.0)?;
            share.push(dino_layer(
                &scope.owner,
                weights,
                &format!("info_sharing.self_attention_blocks.{index}"),
                &scope.key_prefix,
                precision,
            )?);
        }
        emit_progress(&mut progress, "load MapAnything input encoders and heads", 40.0 / 48.0)?;
        let view_pe = tensor(weights, "info_sharing.view_pos_table", HIDDEN)?;
        let ref_view_pe: Vec<f32> = (0..VIEW_TOKENS).flat_map(|_| view_pe.iter().copied()).collect();
        let model = Self {
            precision,
            patch_w: upload(&tensor(weights, "encoder.model.patch_embed.proj.weight", HIDDEN * patch_dim)?, HIDDEN, patch_dim)?,
            patch_b: upload(&tensor(weights, "encoder.model.patch_embed.proj.bias", HIDDEN)?, 1, HIDDEN)?,
            cls_pos: upload(&cls_pos[..HIDDEN], 1, HIDDEN)?,
            pos: upload(&pos[HIDDEN..], PATCHES, HIDDEN)?,
            encoder,
            ray_encoder: DenseEncoder::load(&scope, weights, "ray_dirs_encoder", 3)?,
            depth_encoder: DenseEncoder::load(&scope, weights, "depth_encoder", 1)?,
            depth_scale_encoder: GlobalEncoder::load(weights, "depth_scale_encoder", 1)?,
            rot_encoder: GlobalEncoder::load(weights, "cam_rot_encoder", 4)?,
            trans_encoder: GlobalEncoder::load(weights, "cam_trans_encoder", 3)?,
            trans_scale_encoder: GlobalEncoder::load(weights, "cam_trans_scale_encoder", 1)?,
            fusion_norm: norm_mods(weights, "fusion_norm_layer", HIDDEN)?,
            scale_token: upload(&tensor(weights, "scale_token", HIDDEN)?, 1, HIDDEN)?,
            ref_view_pe: upload(&ref_view_pe, VIEW_TOKENS, HIDDEN)?,
            share,
            share_norm: norm_mods(weights, "info_sharing.norm", HIDDEN)?,
            dense: DenseHead::load(&scope, weights)?,
            pose: PoseHead::load(&scope, weights)?,
            scale: ScaleHead::load(weights)?,
        };
        emit_progress(&mut progress, "MapAnything resident", 1.0)?;
        Ok(model)
    }

    pub fn precision(&self) -> MapAnythingPrecision {
        self.precision
    }

    /// One transformer block over `hidden` (`[rows, 1536]`), attention within
    /// consecutive groups of `group` rows.
    fn block(&self, layer: &DinoLayer, hidden: &GpuTensor, group: usize) -> Result<GpuTensor> {
        if group == 0 || hidden.rows() % group != 0 {
            return Err(DiffusionError::workflow("MapAnything attention groups do not tile the tokens"));
        }
        let normed = layer_norm(hidden, &layer.norm1, HIDDEN)?;
        let qkv = layer.qkv(&normed)?;
        let scale = 1.0 / (HEAD_DIM as f32).sqrt();
        let mut parts = Vec::with_capacity(hidden.rows() / group);
        for start in (0..hidden.rows()).step_by(group) {
            let grouped;
            let part = if group == hidden.rows() {
                &qkv
            } else {
                grouped = rows(&qkv, start, group)?;
                &grouped
            };
            let q = gpu_slice_cols(part, 0, HIDDEN).map_err(model_error)?;
            let k = gpu_slice_cols(part, HIDDEN, HIDDEN).map_err(model_error)?;
            let v = gpu_slice_cols(part, 2 * HIDDEN, HIDDEN).map_err(model_error)?;
            parts.push(
                match self.precision {
                    MapAnythingPrecision::Bf16 => gpu_attention_packed_flash_bf16(&q, &k, &v, HEADS, scale),
                    MapAnythingPrecision::StrictF32 => gpu_attention_packed_f32(&q, &k, &v, HEADS, scale),
                }
                .map_err(model_error)?,
            );
        }
        let attention = if parts.len() == 1 {
            parts.pop().expect("one attention group")
        } else {
            concat_rows(&parts.iter().collect::<Vec<_>>())?
        };
        let hidden = add(hidden, &layer.proj(&attention)?)?;
        let normed = layer_norm(&hidden, &layer.norm2, HIDDEN)?;
        let gated = gpu_swiglu_gate_first(&layer.fc1(&normed)?, 0, SWIGLU).map_err(model_error)?;
        add(&hidden, &layer.fc2(&gated)?)
    }

    /// The DINOv2 encoder over all views at once (attention per view).
    /// Returns the `[views * 1370, 1536]` block-23 tokens.
    fn encode(&self, views: &[MapAnythingView], tap: &mut Tap, progress: &mut Option<ProgressHook>) -> Result<GpuTensor> {
        let mut per_view = Vec::with_capacity(views.len());
        for view in views {
            let image = upload(view.image, 3, PIXELS)?;
            let tiles = gpu_birefnet_image_to_patches(&image, MAPANYTHING_SIZE, MAPANYTHING_SIZE, MAPANYTHING_PATCH, MAPANYTHING_PATCH)
                .map_err(model_error)?;
            let planes = [rows(&tiles, 0, PATCHES)?, rows(&tiles, PATCHES, PATCHES)?, rows(&tiles, 2 * PATCHES, PATCHES)?];
            let patch_input = gpu_concat_cols(&[&planes[0], &planes[1], &planes[2]]).map_err(model_error)?;
            let patches = gpu_linear_f32_resident(&patch_input, &self.patch_w, Some(&self.patch_b)).map_err(model_error)?;
            let patches = add(&patches, &self.pos)?;
            per_view.push(concat_rows(&[&self.cls_pos, &patches])?);
        }
        let mut hidden = concat_rows(&per_view.iter().collect::<Vec<_>>())?;
        drop(per_view);
        for (index, layer) in self.encoder.iter().enumerate() {
            emit_progress(progress, "MapAnything encoder", index as f64 / 48.0)?;
            hidden = self.block(layer, &hidden, VIEW_TOKENS)?;
            tap.stacked(&format!("module.encoder.model.blocks.{index}"), &[&hidden])?;
        }
        Ok(hidden)
    }

    /// Run all views. Every view is 518x518; geometric inputs may be given for
    /// some views and not others.
    pub fn forward(
        &self,
        views: &[MapAnythingView],
        mut progress: Option<ProgressHook>,
        tap: Option<MapAnythingTap>,
    ) -> Result<MapAnythingOutput> {
        let mut tap = Tap { sink: tap };
        let count = views.len();
        if count == 0 {
            return Err(DiffusionError::workflow("MapAnything needs at least one view"));
        }
        for (index, view) in views.iter().enumerate() {
            let bad = view.image.len() != 3 * PIXELS
                || view.rays.is_some_and(|rays| rays.len() != 3 * PIXELS)
                || view.depth_along_ray.is_some_and(|depth| depth.len() != PIXELS);
            if bad {
                return Err(DiffusionError::workflow(format!(
                    "MapAnything view {index}: image, rays or depth is not {MAPANYTHING_SIZE}x{MAPANYTHING_SIZE}"
                )));
            }
        }
        if views[0].pose.is_none() && views.iter().any(|view| view.pose.is_some()) {
            return Err(DiffusionError::workflow("MapAnything: poses enter in view 0's frame, and view 0 has none"));
        }

        // 1. Image encoder.
        let encoded = self.encode(views, &mut tap, &mut progress)?;
        let mut patches = Vec::with_capacity(count);
        let mut registers = Vec::with_capacity(count);
        for view in 0..count {
            registers.push(rows(&encoded, view * VIEW_TOKENS, 1)?);
            patches.push(rows(&encoded, view * VIEW_TOKENS + 1, PATCHES)?);
        }
        drop(encoded);
        tap.channels_first("module.encoder.features", &patches.iter().collect::<Vec<_>>())?;
        tap.stacked("module.encoder.registers", &registers.iter().collect::<Vec<_>>())?;

        // 2. Geometric inputs, added to the patch tokens. A missing input
        // adds exactly zero in the reference (its encoder output times a zero
        // mask), so it is skipped.
        emit_progress(&mut progress, "MapAnything geometric inputs", 24.0 / 48.0)?;
        let mut fused = patches;
        if views.iter().any(|view| view.rays.is_some()) {
            let mut inputs = Vec::new();
            for view in views {
                let rays = view.rays.map(|rays| unshuffle(rays, 3)).unwrap_or_else(|| vec![0.0; 3 * UNSHUFFLE * PATCHES]);
                inputs.push(upload(&rays, 3 * UNSHUFFLE, PATCHES)?);
            }
            let encoded = self.ray_encoder.forward(inputs, &mut tap)?;
            for (view, encoded) in encoded.iter().enumerate() {
                if views[view].rays.is_some() {
                    fused[view] = add(&fused[view], encoded)?;
                }
            }
        }
        let mut global = vec![0.0f32; count * HIDDEN];
        let mut add_global = |values: &[f32], masks: &[bool]| {
            for (view, &on) in masks.iter().enumerate() {
                if on {
                    for (sum, value) in global[view * HIDDEN..(view + 1) * HIDDEN].iter_mut().zip(&values[view * HIDDEN..]) {
                        *sum += value;
                    }
                }
            }
        };
        if views.iter().any(|view| view.depth_along_ray.is_some()) {
            let mut inputs = Vec::new();
            let mut log_factors = Vec::new();
            for view in views {
                let (log_depth, factor) = match view.depth_along_ray {
                    Some(depth) => normalized_log_depth(depth),
                    None => (vec![0.0; PIXELS], 0.0),
                };
                inputs.push(upload(&unshuffle(&log_depth, 1), UNSHUFFLE, PATCHES)?);
                log_factors.push((factor + 1e-8).ln());
            }
            let encoded = self.depth_encoder.forward(inputs, &mut tap)?;
            for (view, encoded) in encoded.iter().enumerate() {
                if views[view].depth_along_ray.is_some() {
                    fused[view] = add(&fused[view], encoded)?;
                }
            }
            let scale = self.depth_scale_encoder.forward(&log_factors, count, &mut tap)?;
            let masks: Vec<bool> = views.iter().map(|view| view.depth_along_ray.is_some() && view.metric).collect();
            add_global(&scale, &masks);
        }
        if let Some((ref_quat, ref_trans)) = views[0].pose {
            let mut quats = Vec::with_capacity(count * 4);
            let mut trans = Vec::with_capacity(count);
            for view in views {
                let (quat, offset) = match view.pose {
                    Some((quat, offset)) => pose_in_frame_of(ref_quat, ref_trans, quat, offset),
                    None => ([0.0, 0.0, 0.0, 1.0], [0.0; 3]),
                };
                quats.extend(quat);
                trans.push(offset);
            }
            // normalize_pose_translations: over the views with a nonzero offset.
            let distances: Vec<f32> = trans.iter().map(|t| (t[0] * t[0] + t[1] * t[1] + t[2] * t[2]).sqrt()).collect();
            let moved = distances.iter().filter(|&&distance| distance > 0.0).count();
            let factor = (distances.iter().sum::<f32>() / (moved as f32 + 1e-8)).max(1e-8);
            let scaled: Vec<f32> = trans.iter().flat_map(|t| t.map(|value| value / factor)).collect();
            let posed: Vec<bool> = views.iter().map(|view| view.pose.is_some()).collect();
            let rot = self.rot_encoder.forward(&quats, count, &mut tap)?;
            add_global(&rot, &posed);
            let offset = self.trans_encoder.forward(&scaled, count, &mut tap)?;
            add_global(&offset, &posed);
            let log_factor = vec![(factor + 1e-8).ln(); count];
            let offset_scale = self.trans_scale_encoder.forward(&log_factor, count, &mut tap)?;
            let metric: Vec<bool> = views.iter().map(|view| view.pose.is_some() && view.metric).collect();
            add_global(&offset_scale, &metric);
        }
        for (view, fused) in fused.iter_mut().enumerate() {
            let row = &global[view * HIDDEN..(view + 1) * HIDDEN];
            if row.iter().any(|&value| value != 0.0) {
                let expanded: Vec<f32> = (0..PATCHES).flat_map(|_| row.iter().copied()).collect();
                *fused = add(fused, &upload(&expanded, PATCHES, HIDDEN)?)?;
            }
            *fused = layer_norm(fused, &self.fusion_norm, HIDDEN)?;
        }
        tap.stacked("module.fusion_norm_layer", &fused.iter().collect::<Vec<_>>())?;

        // 3. Multi-view transformer: per view [patches, class token], then
        // the scale token; the reference view's embedding on view 0.
        let mut parts: Vec<&GpuTensor> = Vec::with_capacity(2 * count + 1);
        for view in 0..count {
            parts.push(&fused[view]);
            parts.push(&registers[view]);
        }
        parts.push(&self.scale_token);
        let mut hidden = concat_rows(&parts)?;
        tap.stacked("module.info_sharing.proj_embed", &[&hidden])?;
        let reference = add(&rows(&hidden, 0, VIEW_TOKENS)?, &self.ref_view_pe)?;
        hidden = if count == 1 {
            concat_rows(&[&reference, &self.scale_token])?
        } else {
            concat_rows(&[&reference, &rows(&hidden, VIEW_TOKENS, hidden.rows() - VIEW_TOKENS)?])?
        };
        let view_rows = count * VIEW_TOKENS;
        let mut normed = Vec::with_capacity(SHARE_TAPS.len());
        for (index, layer) in self.share.iter().enumerate() {
            emit_progress(&mut progress, "MapAnything multi-view transformer", (25 + index) as f64 / 48.0)?;
            if index % 2 == 0 {
                hidden = self.block(layer, &hidden, hidden.rows())?;
                tap.stacked(&format!("module.info_sharing.self_attention_blocks.{index}"), &[&hidden])?;
            } else {
                let scale_token = rows(&hidden, view_rows, 1)?;
                let body = self.block(layer, &rows(&hidden, 0, view_rows)?, VIEW_TOKENS)?;
                tap.stacked(&format!("module.info_sharing.self_attention_blocks.{index}"), &[&body])?;
                hidden = concat_rows(&[&body, &scale_token])?;
            }
            if SHARE_TAPS.contains(&index) {
                let out = layer_norm(&hidden, &self.share_norm, HIDDEN)?;
                tap.stacked("module.info_sharing.norm", &[&out])?;
                normed.push(out);
            }
        }
        drop(hidden);
        let mut features: Vec<Vec<GpuTensor>> = Vec::with_capacity(SHARE_TAPS.len());
        for out in &normed {
            let mut per_view = Vec::with_capacity(count);
            for view in 0..count {
                per_view.push(rows(out, view * VIEW_TOKENS, PATCHES)?);
            }
            features.push(per_view);
        }
        if tap.on() {
            let names = ["module.info_sharing[1][0]", "module.info_sharing[1][1]", "module.info_sharing[0]"];
            for ((name, out), per_view) in names.iter().zip(&normed).zip(&features) {
                for (view, tokens) in per_view.iter().enumerate() {
                    tap.channels_first(&format!("{name}.features[{view}]"), &[tokens])?;
                }
                tap.stacked(&format!("{name}.additional_token_features"), &[&rows(out, view_rows, 1)?])?;
                for view in 0..count {
                    tap.stacked(
                        &format!("{name}.additional_token_features_per_view[{view}]"),
                        &[&rows(out, view * VIEW_TOKENS + PATCHES, 1)?],
                    )?;
                }
            }
        }
        let scale_token = download(&rows(&normed[2], view_rows, 1)?)?;
        drop(normed);

        // 4. Heads, one view at a time.
        let mut dense = Vec::with_capacity(count);
        let mut decoded = Vec::new();
        for view in 0..count {
            emit_progress(&mut progress, "MapAnything dense head", (41.0 + 6.0 * view as f64 / count as f64) / 48.0)?;
            let out = self.dense.forward([&fused[view], &features[0][view], &features[1][view], &features[2][view]])?;
            if tap.on() {
                decoded.push(copy(&out.tensor)?);
            }
            dense.push(download(&out.tensor)?);
        }
        tap.stacked("module.dpt_regressor_head.decoded_channels", &decoded.iter().collect::<Vec<_>>())?;
        tap.stacked("module.dense_head.decoded_channels", &decoded.iter().collect::<Vec<_>>())?;
        drop(decoded);

        let mut poses = Vec::with_capacity(count);
        let mut pose_taps: [Vec<f32>; 3] = Default::default();
        for view in 0..count {
            let (offset, quat) = self.pose.forward(&features[2][view])?;
            let norm = quat.iter().map(|value| value * value).sum::<f32>().sqrt().max(1e-8);
            let unit = quat.map(|value| value / norm);
            if tap.on() {
                pose_taps[0].extend(offset.iter().chain(&quat));
                pose_taps[1].extend(unit);
                pose_taps[2].extend(offset.iter().chain(&unit));
            }
            poses.push((unit, offset));
        }
        let [raw_pose, unit_quats, pose_value] = pose_taps;
        tap.put("module.pose_head.decoded_channels", raw_pose);
        tap.put("module.pose_adaptor.cam_trans_adaptor.value", poses.iter().flat_map(|(_, t)| *t).collect());
        tap.put("module.pose_adaptor.quaternions_adaptor.value", unit_quats);
        tap.put("module.pose_adaptor.value", pose_value);

        let projected = self.scale.proj.forward(&scale_token);
        let hidden = host_relu(self.scale.mlp[0].forward(&projected));
        let hidden = host_relu(self.scale.mlp[1].forward(&hidden));
        let logit = self.scale.out.forward(&hidden)[0];
        let metric_scale = logit.exp().max(1e-8);
        tap.put("module.scale_head.proj", projected);
        tap.put("module.scale_head.output_proj", vec![logit]);
        tap.put("module.scale_head.decoded_channels", vec![logit]);
        tap.put("module.scale_adaptor.value", vec![metric_scale]);

        // 5. Adaptors and the output scene representation.
        let mut outputs = Vec::with_capacity(count);
        let mut adaptor_taps: [Vec<f32>; 4] = Default::default();
        for (view, channels) in dense.into_iter().enumerate() {
            let plane = |index: usize| &channels[index * PIXELS..(index + 1) * PIXELS];
            let mut rays = Vec::with_capacity(3 * PIXELS);
            let mut depth = Vec::with_capacity(PIXELS);
            for pixel in 0..PIXELS {
                let ray = [plane(0)[pixel], plane(1)[pixel], plane(2)[pixel]];
                let norm = (ray[0] * ray[0] + ray[1] * ray[1] + ray[2] * ray[2]).sqrt().max(1e-8);
                rays.extend(ray.map(|value| value / norm));
                depth.push(plane(3)[pixel].exp().max(0.0));
            }
            let confidence: Vec<f32> = plane(4).iter().map(|value| 1.0 + value.exp()).collect();
            let mask_logits = plane(5).to_vec();
            if tap.on() {
                for channel in 0..3 {
                    adaptor_taps[0].extend(rays.iter().skip(channel).step_by(3));
                }
                adaptor_taps[0].extend(&depth);
                adaptor_taps[1].extend(&confidence);
                adaptor_taps[2].extend(&mask_logits);
                adaptor_taps[3].extend(mask_logits.iter().map(|value| 1.0 / (1.0 + (-value).exp())));
            }
            let (quat, offset) = poses[view];
            outputs.push(MapAnythingViewOutput {
                rays,
                depth_along_ray: depth.iter().map(|value| value * metric_scale).collect(),
                confidence,
                mask_logits,
                quat,
                trans: offset.map(|value| value * metric_scale),
            });
        }
        if tap.on() {
            let [value, confidence, logits, mask] = adaptor_taps;
            tap.put("module.dense_adaptor.value_adaptor.value", value.clone());
            tap.put("module.dense_adaptor.confidence_adaptor.value", confidence.clone());
            tap.put("module.dense_adaptor.mask_adaptor.logits", logits.clone());
            tap.put("module.dense_adaptor.mask_adaptor.mask", mask.clone());
            tap.put("module.dense_adaptor.value", value);
            tap.put("module.dense_adaptor.confidence", confidence);
            tap.put("module.dense_adaptor.logits", logits);
            tap.put("module.dense_adaptor.mask", mask);
            for (view, out) in outputs.iter().enumerate() {
                let camera = out.points_camera();
                tap.put(&format!("output[{view}].pts3d"), out.points_world());
                tap.put(&format!("output[{view}].depth_z"), camera.iter().skip(2).step_by(3).copied().collect());
                tap.put(&format!("output[{view}].pts3d_cam"), camera);
                tap.put(&format!("output[{view}].ray_directions"), out.rays.clone());
                tap.put(&format!("output[{view}].depth_along_ray"), out.depth_along_ray.clone());
                tap.put(&format!("output[{view}].cam_trans"), out.trans.to_vec());
                tap.put(&format!("output[{view}].cam_quats"), out.quat.to_vec());
                tap.put(&format!("output[{view}].metric_scaling_factor"), vec![metric_scale]);
                tap.put(&format!("output[{view}].conf"), out.confidence.clone());
                tap.put(
                    &format!("output[{view}].non_ambiguous_mask"),
                    out.mask_logits.iter().map(|&value| if value > 0.0 { 1.0 } else { 0.0 }).collect(),
                );
                tap.put(&format!("output[{view}].non_ambiguous_mask_logits"), out.mask_logits.clone());
                let rotation = rotation(out.quat);
                let mut pose = vec![0.0f32; 16];
                for row in 0..3 {
                    pose[row * 4..row * 4 + 3].copy_from_slice(&rotation[row]);
                    pose[row * 4 + 3] = out.trans[row];
                }
                pose[15] = 1.0;
                tap.put(&format!("output[{view}].camera_poses"), pose);
                let [fx, fy, cx, cy] = crate::mapanything_prep::pinhole_from_rays(&out.rays, MAPANYTHING_SIZE, MAPANYTHING_SIZE)
                    .map_err(DiffusionError::model)?;
                tap.put(&format!("output[{view}].intrinsics"), vec![fx, 0.0, cx, 0.0, fy, cy, 0.0, 0.0, 1.0]);
            }
        }
        emit_progress(&mut progress, "MapAnything done", 1.0)?;
        Ok(MapAnythingOutput { views: outputs, metric_scale })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapanything_activation_estimate_tracks_allocated_stage_peaks() {
        let cuda_alloc = |bytes: usize| {
            let bytes = bytes.max(256);
            let step = 256 * 1024 * 1024;
            if bytes <= step {
                bytes.next_power_of_two()
            } else {
                bytes.div_ceil(step) * step
            }
        };
        let mut previous_exact = 0;
        let mut previous_rounded = 0;
        for views in 1..=40 {
            let exact = forward_activation_bytes(views, |bytes| bytes);
            let rounded = forward_activation_bytes(views, cuda_alloc);
            assert!(exact >= previous_exact, "exact estimate decreased at {views} views");
            assert!(rounded >= previous_rounded, "rounded estimate decreased at {views} views");
            assert!(rounded >= exact, "rounded estimate below exact at {views} views");
            previous_exact = exact;
            previous_rounded = rounded;
        }
        // Measured-and-modelled points; these move if the forward's live set changes.
        assert_eq!(forward_activation_bytes(12, cuda_alloc) / (1 << 20), 3216);
        assert_eq!(forward_activation_bytes(32, cuda_alloc) / (1 << 20), 8464);
    }

    #[test]
    fn mapanything_unshuffle_puts_each_pixel_in_its_patch_channel() {
        let mut values = vec![0.0f32; PIXELS * 3];
        // Pixel (y 15, x 30) is patch (1, 2), offset (1, 2); channel 2.
        values[(15 * MAPANYTHING_SIZE + 30) * 3 + 2] = 7.0;
        let out = unshuffle(&values, 3);
        let channel = 2 * UNSHUFFLE + MAPANYTHING_PATCH + 2;
        assert_eq!(out[channel * PATCHES + MAPANYTHING_SIDE + 2], 7.0);
        assert_eq!(out.iter().filter(|&&value| value != 0.0).count(), 1);
    }

    #[test]
    fn mapanything_depth_normalises_by_the_mean_valid_depth_then_log1p() {
        let mut depth = vec![0.0f32; 4];
        depth[1] = 1.0;
        depth[2] = 3.0;
        let (log, factor) = normalized_log_depth(&depth);
        assert!((factor - 2.0).abs() < 1e-6);
        assert_eq!(log[0], 0.0);
        assert!((log[1] - 0.5f32.ln_1p()).abs() < 1e-6);
        assert!((log[2] - 1.5f32.ln_1p()).abs() < 1e-6);
    }

    #[test]
    fn mapanything_rotation_of_a_quarter_turn_about_z() {
        let half = std::f32::consts::FRAC_PI_4;
        let r = rotation([0.0, 0.0, half.sin(), half.cos()]);
        let expect = [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
        for (row, want) in r.iter().zip(expect) {
            for (got, want) in row.iter().zip(want) {
                assert!((got - want).abs() < 1e-6, "{r:?}");
            }
        }
    }
}
