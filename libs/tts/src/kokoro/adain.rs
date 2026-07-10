//! `AdaIN1d` and `AdainResBlk1d` — the residual block used by both the prosody
//! predictor's F0/noise branches and the decoder.
//!
//! Shape of the block, read straight out of the ONNX export:
//!
//! ```text
//! residual: norm1(x,s) -> LeakyReLU(0.2) -> [pool] -> conv1
//!                      -> norm2(·,s) -> LeakyReLU(0.2) -> conv2
//! shortcut: [nearest x2] -> [conv1x1]
//! out     : (residual + shortcut) / sqrt(2)
//! ```

use std::f32::consts::FRAC_1_SQRT_2;

use super::ops::{
    conv1d, conv_transpose1d_depthwise, instance_norm_time, leaky_relu, linear, nearest_upsample2,
    Mat,
};
use super::weights::Weights;
use crate::TtsError;

const STYLE: usize = 128;
const LEAKY_SLOPE: f32 = 0.2;
const EPS: f32 = 1e-5;

/// `(1 + gamma) * instance_norm(x) + beta`, with gamma and beta produced from
/// the style vector. Gamma is the *first* half of the `fc` output.
pub struct AdaIn1d {
    fc_weight: Vec<f32>,
    fc_bias: Vec<f32>,
    channels: usize,
}

impl AdaIn1d {
    pub(crate) fn load(weights: &Weights, prefix: &str, channels: usize) -> Result<Self, TtsError> {
        Ok(Self {
            fc_weight: need(weights, &format!("{prefix}.fc.weight"))?,
            fc_bias: need(weights, &format!("{prefix}.fc.bias"))?,
            channels,
        })
    }

    pub fn forward(&self, x: &Mat, style: &[f32]) -> Mat {
        let style_row = Mat::from_vec(1, STYLE, style.to_vec());
        let h = linear(&style_row, &self.fc_weight, &self.fc_bias, 2 * self.channels);
        let (gamma, beta) = h.data.split_at(self.channels);

        let mut out = instance_norm_time(x, EPS);
        for channel in 0..self.channels {
            let scale = 1.0 + gamma[channel];
            let shift = beta[channel];
            for value in out.row_mut(channel) {
                *value = *value * scale + shift;
            }
        }
        out
    }
}

pub struct AdainResBlk1d {
    norm1: AdaIn1d,
    norm2: AdaIn1d,
    conv1: (Vec<f32>, Vec<f32>),
    conv2: (Vec<f32>, Vec<f32>),
    /// Present only when `dim_in != dim_out`. Weight-normed, no bias.
    conv1x1: Option<Vec<f32>>,
    /// Depthwise transposed conv, present only in upsampling blocks.
    pool: Option<(Vec<f32>, Vec<f32>)>,
    dim_out: usize,
    upsample: bool,
}

fn need(weights: &Weights, name: &str) -> Result<Vec<f32>, TtsError> {
    weights
        .get(name)
        .map(<[f32]>::to_vec)
        .ok_or_else(|| TtsError::Backend(format!("missing tensor {name}")))
}

impl AdainResBlk1d {
    pub fn load(
        weights: &Weights,
        prefix: &str,
        dim_in: usize,
        dim_out: usize,
        upsample: bool,
    ) -> Result<Self, TtsError> {
        let conv1x1 = (dim_in != dim_out)
            .then(|| weights.weight_norm(&format!("{prefix}.conv1x1")))
            .transpose()?;

        let pool = upsample
            .then(|| -> Result<_, TtsError> {
                Ok((
                    weights.weight_norm(&format!("{prefix}.pool"))?,
                    need(weights, &format!("{prefix}.pool.bias"))?,
                ))
            })
            .transpose()?;

        Ok(Self {
            norm1: AdaIn1d::load(weights, &format!("{prefix}.norm1"), dim_in)?,
            norm2: AdaIn1d::load(weights, &format!("{prefix}.norm2"), dim_out)?,
            conv1: (
                weights.weight_norm(&format!("{prefix}.conv1"))?,
                need(weights, &format!("{prefix}.conv1.bias"))?,
            ),
            conv2: (
                weights.weight_norm(&format!("{prefix}.conv2"))?,
                need(weights, &format!("{prefix}.conv2.bias"))?,
            ),
            conv1x1,
            pool,
            dim_out,
            upsample,
        })
    }

    pub fn forward(&self, x: &Mat, style: &[f32]) -> Mat {
        // Residual path.
        let mut h = self.norm1.forward(x, style);
        leaky_relu(&mut h, LEAKY_SLOPE);
        if let Some((weight, bias)) = &self.pool {
            h = conv_transpose1d_depthwise(&h, weight, bias, 2, 1, 1);
        }
        h = conv1d(&h, &self.conv1.0, &self.conv1.1, self.dim_out, 1);

        let mut h = self.norm2.forward(&h, style);
        leaky_relu(&mut h, LEAKY_SLOPE);
        let residual = conv1d(&h, &self.conv2.0, &self.conv2.1, self.dim_out, 1);

        // Shortcut: nearest-neighbour upsample, unlike the residual's learned
        // transposed convolution. Both must be right; they differ per sample.
        let mut shortcut = if self.upsample {
            nearest_upsample2(x)
        } else {
            x.clone()
        };
        if let Some(weight) = &self.conv1x1 {
            let no_bias = vec![0.0; self.dim_out];
            shortcut = conv1d(&shortcut, weight, &no_bias, self.dim_out, 0);
        }

        let mut out = residual;
        for (value, add) in out.data.iter_mut().zip(&shortcut.data) {
            *value = (*value + add) * FRAC_1_SQRT_2;
        }
        out
    }
}
