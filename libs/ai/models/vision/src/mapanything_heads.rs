//! The MapAnything dense head and pose head.
//!
//! Both heads are implemented from the published papers and from nothing
//! else: the dense head from Ranftl, Bochkovskiy and Koltun, "Vision
//! Transformers for Dense Prediction" (arXiv 2103.13413: the reassemble
//! stages, the RefineNet-style fusion blocks with their residual convolutional
//! units, and the convolutional output head), and the pose head from Chen et
//! al., "Map-Relative Pose Regression for Visual Re-Localization" (arXiv
//! 2404.09884, the residual 1x1 convolution block, global average pooling and
//! small MLP) as MapAnything (Keetha et al., arXiv 2509.13414) and Reloc3r
//! (Dong et al., arXiv 2412.08376) use it. Tensor names and shapes come from
//! the checkpoint header, the interface from the lane's interface contract.
//!
//! Everything runs in f32. Neither head normalises or squashes its output: the
//! dense head returns six raw channels, the pose head a raw translation and a
//! raw `x, y, z, w` quaternion.

use crate::backend::{
    gpu_add, gpu_gather_cols, gpu_birefnet_global_avg_pool, gpu_birefnet_relu, gpu_birefnet_tokens_to_planar,
    gpu_download, GpuTensor,
};
use crate::da3::{add_planar, relu, resize, stride_two, CacheScope, ConvTransposeNoOverlap, Planar, StrideCache, TensorSource};
use crate::mapanything::{Conv3, HostLinear, Linear};
use crate::{DiffusionError, Result};
use std::sync::Mutex;

/// Patch grid side and the image side the heads work at.
const SIDE: usize = 37;
const IMAGE: usize = 518;
/// Token width and tokens per view.
const HIDDEN: usize = 1536;
const TOKENS: usize = SIDE * SIDE;
/// DPT feature width and the output head's inner width.
const FEATURES: usize = 256;
pub(crate) const OUTPUT_INNER: usize = 128;
pub(crate) const OUTPUT_CHANNELS: usize = 6;
/// Reassemble stage widths, shallow to deep.
const STAGE_WIDTHS: [usize; 4] = [96, 192, 384, 768];
const POSE_WIDTH: usize = 784;

fn transpose(tensor: &GpuTensor) -> Result<GpuTensor> {
    gpu_birefnet_tokens_to_planar(tensor).map_err(DiffusionError::model)
}

fn relu_tokens(tensor: &GpuTensor) -> Result<GpuTensor> {
    gpu_birefnet_relu(tensor).map_err(DiffusionError::model)
}

/// Keeps rows and columns `0..out` of a planar map: one column-index table,
/// gathered for every channel at once.
fn crop(input: &Planar, out: usize) -> Result<Planar> {
    let indices: Vec<u32> = (0..out)
        .flat_map(|y| (0..out).map(move |x| (y * input.width + x) as u32))
        .collect();
    Ok(Planar {
        tensor: gpu_gather_cols(&input.tensor, &indices).map_err(DiffusionError::model)?,
        width: out,
        height: out,
    })
}

/// How a reassemble stage changes the resolution of its 37 x 37 projection.
enum Resample {
    /// Transposed convolution, stride equal to the kernel.
    Up(ConvTransposeNoOverlap),
    /// Kept at the patch resolution.
    Keep,
    /// 3x3 convolution with stride 2 and padding 1 (37 to 19).
    Down(Conv3),
}

/// One reassemble stage: 1x1 projection, resampling, bias-free 3x3 convolution
/// to the feature width. No activation.
struct Stage {
    project: Linear,
    resample: Resample,
    to_features: Conv3,
}

impl Stage {
    fn load(scope: &CacheScope, weights: &dyn TensorSource, index: usize) -> Result<Self> {
        let width = STAGE_WIDTHS[index];
        let prefix = format!("dense_head.0.input_process.{index}");
        let resample = match index {
            0 => Resample::Up(ConvTransposeNoOverlap::load_in(scope, weights, &format!("{prefix}.0.1"), width, 4)?),
            1 => Resample::Up(ConvTransposeNoOverlap::load_in(scope, weights, &format!("{prefix}.0.1"), width, 2)?),
            2 => Resample::Keep,
            _ => Resample::Down(Conv3::load(scope, weights, &format!("{prefix}.0.1"), width, width, true)?),
        };
        Ok(Self {
            project: Linear::load(weights, &format!("{prefix}.0.0"), HIDDEN, width)?,
            resample,
            to_features: Conv3::load(scope, weights, &format!("{prefix}.1"), width, FEATURES, false)?,
        })
    }

    fn forward(&self, tokens: &GpuTensor, stride: &StrideCache) -> Result<Planar> {
        let map = Planar {
            tensor: transpose(&self.project.forward(tokens)?)?,
            width: SIDE,
            height: SIDE,
        };
        let map = match &self.resample {
            Resample::Up(deconv) => deconv.forward(map)?,
            Resample::Keep => map,
            Resample::Down(conv) => stride_two(stride, conv.forward(&map)?)?,
        };
        self.to_features.forward(&map)
    }
}

/// Residual convolutional unit without normalisation:
/// `x + conv2(relu(conv1(relu(x))))`.
struct Rcu {
    conv1: Conv3,
    conv2: Conv3,
}

impl Rcu {
    fn load(scope: &CacheScope, weights: &dyn TensorSource, prefix: &str) -> Result<Self> {
        Ok(Self {
            conv1: Conv3::load(scope, weights, &format!("{prefix}.conv1"), FEATURES, FEATURES, true)?,
            conv2: Conv3::load(scope, weights, &format!("{prefix}.conv2"), FEATURES, FEATURES, true)?,
        })
    }

    fn forward(&self, x: &Planar) -> Result<Planar> {
        let hidden = self.conv1.forward(&relu(x)?)?;
        let hidden = self.conv2.forward(&relu(&hidden)?)?;
        add_planar(x, &hidden)
    }
}

/// One fusion block. Blocks 1 to 3 pass the reassembled map through a first
/// unit and add the path from the deeper block; block 4 has only the second
/// unit. Then a second unit, a 1x1 projection and a bilinear enlargement.
struct Fusion {
    lateral: Option<Rcu>,
    main: Rcu,
    out: Linear,
}

impl Fusion {
    fn load(scope: &CacheScope, weights: &dyn TensorSource, index: usize) -> Result<Self> {
        let prefix = format!("dense_head.0.scratch.refinenet{index}");
        Ok(Self {
            lateral: if index == 4 {
                None
            } else {
                Some(Rcu::load(scope, weights, &format!("{prefix}.resConfUnit1"))?)
            },
            main: Rcu::load(scope, weights, &format!("{prefix}.resConfUnit2"))?,
            out: Linear::load(weights, &format!("{prefix}.out_conv"), FEATURES, FEATURES)?,
        })
    }

    /// `reassembled` is this block's stage map, `path` the deeper block's
    /// output (absent for block 4). The projection runs before the resize: a
    /// 1x1 convolution with bias commutes with bilinear interpolation.
    fn forward(&self, reassembled: &Planar, path: Option<&Planar>, size: usize) -> Result<Planar> {
        let hidden = match (&self.lateral, path) {
            (Some(unit), Some(path)) => self.main.forward(&add_planar(&unit.forward(reassembled)?, path)?)?,
            (None, None) => self.main.forward(reassembled)?,
            _ => return Err(DiffusionError::workflow("MapAnything fusion block: path and unit disagree")),
        };
        let projected = self.out.forward_planar(&hidden)?;
        drop(hidden);
        let doubled = resize(&projected, projected.width * 2, projected.height * 2, true)?;
        if doubled.width == size {
            Ok(doubled)
        } else {
            crop(&doubled, size)
        }
    }
}

/// DPT dense head: reassemble four token sets, fuse deepest first, then the
/// convolutional output head. Output: raw `[6, 518 * 518]`.
pub(crate) struct DenseHead {
    stages: [Stage; 4],
    fusion: [Fusion; 4],
    conv1: Conv3,
    conv2: Conv3,
    conv3: Linear,
    stride: StrideCache,
}

impl DenseHead {
    pub(crate) fn load(scope: &CacheScope, weights: &dyn TensorSource) -> Result<Self> {
        Ok(Self {
            stages: [
                Stage::load(scope, weights, 0)?,
                Stage::load(scope, weights, 1)?,
                Stage::load(scope, weights, 2)?,
                Stage::load(scope, weights, 3)?,
            ],
            fusion: [
                Fusion::load(scope, weights, 1)?,
                Fusion::load(scope, weights, 2)?,
                Fusion::load(scope, weights, 3)?,
                Fusion::load(scope, weights, 4)?,
            ],
            conv1: Conv3::load(scope, weights, "dense_head.1.conv1", FEATURES, OUTPUT_INNER, true)?,
            conv2: Conv3::load(scope, weights, "dense_head.1.conv2.0", OUTPUT_INNER, OUTPUT_INNER, true)?,
            conv3: Linear::load(weights, "dense_head.1.conv2.2", OUTPUT_INNER, OUTPUT_CHANNELS)?,
            stride: Mutex::new(None),
        })
    }

    /// `features`: F0, F1, F2, F3 of one view, each `[1369, 1536]`.
    pub(crate) fn forward(&self, features: [&GpuTensor; 4]) -> Result<Planar> {
        for tokens in features {
            if tokens.rows() != TOKENS || tokens.cols() != HIDDEN {
                return Err(DiffusionError::workflow(format!(
                    "MapAnything dense head: features are {}x{}, expected {TOKENS}x{HIDDEN}",
                    tokens.rows(),
                    tokens.cols()
                )));
            }
        }
        // Deepest first; each stage map is made just before its block.
        let mut path: Option<Planar> = None;
        // Block 4 enlarges 19 to 37; every other block doubles.
        for (block, size) in [(3, SIDE), (2, SIDE * 2), (1, SIDE * 4), (0, SIDE * 8)] {
            let reassembled = self.stages[block].forward(features[block], &self.stride)?;
            let next = self.fusion[block].forward(&reassembled, path.as_ref(), size)?;
            path = Some(next);
        }
        let fused = path.ok_or_else(|| DiffusionError::workflow("MapAnything dense head: no fusion output"))?;
        let hidden = self.conv1.forward(&fused)?;
        drop(fused);
        let hidden = resize(&hidden, IMAGE, IMAGE, true)?;
        let hidden = self.conv2.forward(&hidden)?;
        let hidden = relu(&hidden)?;
        self.conv3.forward_planar(&hidden)
    }
}

/// Pose head: per-token projection, two residual 1x1 blocks, mean over the
/// tokens, a two-layer MLP, then the translation and rotation outputs.
pub(crate) struct PoseHead {
    project: Linear,
    blocks: [[Linear; 3]; 2],
    hidden: [HostLinear; 2],
    translation: HostLinear,
    rotation: HostLinear,
}

impl PoseHead {
    pub(crate) fn load(_scope: &CacheScope, weights: &dyn TensorSource) -> Result<Self> {
        let block = |index: usize| -> Result<[Linear; 3]> {
            let name = |part: usize| format!("pose_head.res_conv.{index}.res_conv{part}");
            Ok([
                Linear::load(weights, &name(1), POSE_WIDTH, POSE_WIDTH)?,
                Linear::load(weights, &name(2), POSE_WIDTH, POSE_WIDTH)?,
                Linear::load(weights, &name(3), POSE_WIDTH, POSE_WIDTH)?,
            ])
        };
        Ok(Self {
            project: Linear::load(weights, "pose_head.proj", HIDDEN, POSE_WIDTH)?,
            blocks: [block(0)?, block(1)?],
            hidden: [
                HostLinear::load(weights, "pose_head.more_mlps.0", POSE_WIDTH, POSE_WIDTH)?,
                HostLinear::load(weights, "pose_head.more_mlps.2", POSE_WIDTH, POSE_WIDTH)?,
            ],
            translation: HostLinear::load(weights, "pose_head.fc_t", POSE_WIDTH, 3)?,
            rotation: HostLinear::load(weights, "pose_head.fc_rot", POSE_WIDTH, 4)?,
        })
    }

    /// `tokens`: F3 of one view, `[1369, 1536]`. Returns the raw translation
    /// and the raw `x, y, z, w` quaternion.
    pub(crate) fn forward(&self, tokens: &GpuTensor) -> Result<([f32; 3], [f32; 4])> {
        if tokens.rows() != TOKENS || tokens.cols() != HIDDEN {
            return Err(DiffusionError::workflow(format!(
                "MapAnything pose head: tokens are {}x{}, expected {TOKENS}x{HIDDEN}",
                tokens.rows(),
                tokens.cols()
            )));
        }
        let mut x = self.project.forward(tokens)?;
        for [first, second, third] in &self.blocks {
            let hidden = relu_tokens(&first.forward(&x)?)?;
            let hidden = relu_tokens(&second.forward(&hidden)?)?;
            let hidden = relu_tokens(&third.forward(&hidden)?)?;
            x = gpu_add(&x, &hidden).map_err(DiffusionError::model)?;
        }
        let pooled = gpu_birefnet_global_avg_pool(&transpose(&x)?).map_err(DiffusionError::model)?;
        drop(x);
        let mut embedding = gpu_download(&pooled).map_err(DiffusionError::model)?;
        let relu_host = |values: &mut Vec<f32>| values.iter_mut().for_each(|value| *value = value.max(0.0));
        for layer in &self.hidden {
            embedding = layer.forward(&embedding);
            relu_host(&mut embedding);
        }
        let translation = self.translation.forward(&embedding);
        let rotation = self.rotation.forward(&embedding);
        let translation: [f32; 3] = translation
            .try_into()
            .map_err(|_| DiffusionError::workflow("MapAnything pose head: translation size"))?;
        let rotation: [f32; 4] = rotation
            .try_into()
            .map_err(|_| DiffusionError::workflow("MapAnything pose head: rotation size"))?;
        Ok((translation, rotation))
    }
}
