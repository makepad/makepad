//! The detector as data: an ordered list of operations over channel-major
//! feature maps, plus the weights they name. The converter writes it, both
//! executors (CPU reference, Vulkan) run it. Batch norms are folded into the
//! convolutions by the converter, so none appear here.
//!
//! Values are numbered: value 0 is the input picture `[3, H, W]`; value
//! `i + 1` is the output of `ops[i]`. Every value is a feature map
//! `[channels, height, width]`, stored row-major per channel.

/// An activation fused after an operation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Act {
    None,
    Relu,
    /// `x * relu6(x + 3) / 6`
    HardSwish,
    /// `clamp(slope * x + offset, 0, 1)`.
    HardSigmoid { slope: f32, offset: f32 },
    Sigmoid,
    /// Hard-swish followed by a learnt scalar affine `y * scale + bias`.
    HardSwishAffine { scale: f32, bias: f32 },
}

impl Act {
    pub fn apply(self, x: f32) -> f32 {
        match self {
            Act::None => x,
            Act::Relu => x.max(0.0),
            Act::HardSwish => x * (x + 3.0).clamp(0.0, 6.0) / 6.0,
            Act::HardSigmoid { slope, offset } => (x * slope + offset).clamp(0.0, 1.0),
            Act::Sigmoid => 1.0 / (1.0 + (-x).exp()),
            Act::HardSwishAffine { scale, bias } => x * (x + 3.0).clamp(0.0, 6.0) / 6.0 * scale + bias,
        }
    }

    /// The activation as a small code plus two parameters (kernels and the file).
    pub fn code(self) -> (u32, f32, f32) {
        match self {
            Act::None => (0, 0.0, 0.0),
            Act::Relu => (1, 0.0, 0.0),
            Act::HardSwish => (2, 0.0, 0.0),
            Act::HardSigmoid { slope, offset } => (3, slope, offset),
            Act::Sigmoid => (4, 0.0, 0.0),
            Act::HardSwishAffine { scale, bias } => (5, scale, bias),
        }
    }

    pub fn from_code(code: u32, a: f32, b: f32) -> Option<Act> {
        Some(match code {
            0 => Act::None,
            1 => Act::Relu,
            2 => Act::HardSwish,
            3 => Act::HardSigmoid { slope: a, offset: b },
            4 => Act::Sigmoid,
            5 => Act::HardSwishAffine { scale: a, bias: b },
            _ => return None,
        })
    }
}

/// One operation. Value indices refer to earlier values (see the module doc);
/// weight indices refer to [`Graph::weights`].
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// 2D convolution, weights `[out, in / groups, kh, kw]`, bias `[out]`.
    /// `groups` is 1 (dense) or equal to the channel count (depthwise).
    Conv {
        input: usize,
        weight: usize,
        bias: Option<usize>,
        out_channels: usize,
        kernel: (usize, usize),
        stride: (usize, usize),
        /// top, left, bottom, right
        pad: [usize; 4],
        groups: usize,
        act: Act,
    },
    /// 2D transposed convolution, weights `[in, out, kh, kw]` (Paddle/PyTorch
    /// layout), bias `[out]`, no padding (the DB head uses k=2, s=2).
    ConvTranspose {
        input: usize,
        weight: usize,
        bias: Option<usize>,
        out_channels: usize,
        kernel: (usize, usize),
        stride: (usize, usize),
        act: Act,
    },
    /// Elementwise `a + b` (same shape), then `act`.
    Add { a: usize, b: usize, act: Act },
    /// `x * s` where `s` is `[C, 1, 1]` (squeeze-excitation) or the same shape as `x`.
    Mul { x: usize, s: usize },
    /// Mean over each channel: `[C, H, W] -> [C, 1, 1]`.
    GlobalAvgPool { input: usize },
    /// Average or max pooling with a square window (no padding counted).
    Pool { input: usize, kernel: usize, stride: usize, pad: usize, max: bool },
    /// Nearest-neighbour resize to the size of value `like` (FPN upsampling).
    UpsampleLike { input: usize, like: usize },
    /// Nearest-neighbour upsampling by an integer factor.
    Upsample { input: usize, scale: usize },
    /// Channel concatenation of same-sized maps.
    Concat { inputs: Vec<usize> },
    /// Activation alone.
    Activate { input: usize, act: Act },
}

impl Op {
    pub fn inputs(&self) -> Vec<usize> {
        match self {
            Op::Conv { input, .. } | Op::ConvTranspose { input, .. } | Op::GlobalAvgPool { input }
            | Op::Pool { input, .. } | Op::Upsample { input, .. } | Op::Activate { input, .. } => vec![*input],
            Op::UpsampleLike { input, like } => vec![*input, *like],
            Op::Add { a, b, .. } => vec![*a, *b],
            Op::Mul { x, s } => vec![*x, *s],
            Op::Concat { inputs } => inputs.clone(),
        }
    }
}

/// A weight tensor (f32, row-major).
#[derive(Clone, Debug, PartialEq)]
pub struct Tensor {
    pub shape: Vec<usize>,
    pub data: Vec<f32>,
}

/// How a picture becomes the network input.
#[derive(Clone, Debug, PartialEq)]
pub struct Preprocess {
    /// The longer side is scaled to at most this (then both sides rounded to `multiple`).
    pub max_side: usize,
    pub multiple: usize,
    /// Per input channel, applied to `pixel / 255`.
    pub mean: [f32; 3],
    pub std: [f32; 3],
    /// Input channel order is B, G, R (the reference pipeline reads BGR).
    pub bgr: bool,
}

impl Default for Preprocess {
    fn default() -> Self {
        Preprocess { max_side: 960, multiple: 32, mean: [0.485, 0.456, 0.406], std: [0.229, 0.224, 0.225], bgr: true }
    }
}

/// DB post-processing thresholds (the reference pipeline's defaults).
#[derive(Clone, Debug, PartialEq)]
pub struct Postprocess {
    /// Pixels above this probability are text.
    pub thresh: f32,
    /// A region's mean probability must reach this.
    pub box_thresh: f32,
    /// Regions are grown by `area * unclip / perimeter` on each side.
    pub unclip: f32,
    /// Regions whose shorter side is below this (in network pixels) are dropped.
    pub min_size: f32,
}

impl Default for Postprocess {
    fn default() -> Self {
        Postprocess { thresh: 0.3, box_thresh: 0.6, unclip: 1.5, min_size: 3.0 }
    }
}

/// The whole detector. The last value is the probability map `[1, H, W]`.
#[derive(Clone, Debug, PartialEq)]
pub struct Graph {
    pub ops: Vec<Op>,
    pub weights: Vec<Tensor>,
    pub pre: Preprocess,
    pub post: Postprocess,
}

impl Graph {
    /// Structural checks: inputs refer to earlier values, weights exist and
    /// have the sizes the ops imply.
    pub fn validate(&self) -> Result<(), String> {
        for (i, op) in self.ops.iter().enumerate() {
            let value = i + 1;
            for input in op.inputs() {
                if input >= value {
                    return Err(format!("op {i} reads value {input}, which is not computed yet"));
                }
            }
            let weight = |w: usize| self.weights.get(w).ok_or_else(|| format!("op {i}: weight {w} missing"));
            match op {
                Op::Conv { weight: w, bias, out_channels, kernel, groups, .. } => {
                    let t = weight(*w)?;
                    if t.shape.len() != 4 || t.shape[0] != *out_channels || t.shape[2] != kernel.0 || t.shape[3] != kernel.1 {
                        return Err(format!("op {i}: conv weight shape {:?}", t.shape));
                    }
                    if *groups == 0 {
                        return Err(format!("op {i}: zero groups"));
                    }
                    if let Some(b) = bias {
                        if weight(*b)?.data.len() != *out_channels {
                            return Err(format!("op {i}: conv bias size"));
                        }
                    }
                }
                Op::ConvTranspose { weight: w, bias, out_channels, kernel, .. } => {
                    let t = weight(*w)?;
                    if t.shape.len() != 4 || t.shape[1] != *out_channels || t.shape[2] != kernel.0 || t.shape[3] != kernel.1 {
                        return Err(format!("op {i}: transposed conv weight shape {:?}", t.shape));
                    }
                    if let Some(b) = bias {
                        if weight(*b)?.data.len() != *out_channels {
                            return Err(format!("op {i}: transposed conv bias size"));
                        }
                    }
                }
                _ => {}
            }
        }
        if self.ops.is_empty() {
            return Err("empty detector graph".into());
        }
        Ok(())
    }
}
