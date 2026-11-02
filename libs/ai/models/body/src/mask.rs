//! Segmentation-mask prompt preprocessing and the mask-downscaling encoder.

use crate::preprocess::CropGeometry;
use crate::weights::BodyWeights;
use crate::{Result, DINO_DIM, IMAGE_SIZE, NUM_PATCHES, PATCHES_SIDE};

const NORM_EPS: f32 = 1e-6;

struct Conv {
    input: usize,
    output: usize,
    kernel: usize,
    weight: Vec<f32>,
    bias: Vec<f32>,
}

impl Conv {
    fn load(
        weights: &BodyWeights,
        index: usize,
        input: usize,
        output: usize,
        kernel: usize,
    ) -> Result<Self> {
        let name = format!("prompt_encoder.mask_downscaling.{index}");
        Ok(Self {
            input,
            output,
            kernel,
            weight: weights.f32_shaped(
                &format!("{name}.weight"),
                &[output, input, kernel, kernel],
            )?,
            bias: weights.f32_shaped(&format!("{name}.bias"), &[output])?,
        })
    }

    /// Valid convolution. The stride equals the kernel for the four 2x2
    /// layers, making them non-overlapping patchify + linear operations.
    fn forward(&self, input: &[f32], side: usize) -> Vec<f32> {
        assert_eq!(input.len(), self.input * side * side);
        let stride = self.kernel;
        let output_side = (side - self.kernel) / stride + 1;
        let input_plane = side * side;
        let output_plane = output_side * output_side;
        let mut output = vec![0.0f32; self.output * output_plane];
        let threads = std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(1)
            .min(self.output);
        let channels_per_thread = self.output.div_ceil(threads);
        std::thread::scope(|scope| {
            for (band, output_band) in output
                .chunks_mut(channels_per_thread * output_plane)
                .enumerate()
            {
                let output_channel_start = band * channels_per_thread;
                scope.spawn(move || {
                    for (local_channel, output_channel) in
                        output_band.chunks_mut(output_plane).enumerate()
                    {
                        let oc = output_channel_start + local_channel;
                        let weight_base = oc * self.input * self.kernel * self.kernel;
                        for oy in 0..output_side {
                            for ox in 0..output_side {
                                let mut value = self.bias[oc];
                                for ic in 0..self.input {
                                    let input_base = ic * input_plane
                                        + oy * stride * side
                                        + ox * stride;
                                    let kernel_base = weight_base
                                        + ic * self.kernel * self.kernel;
                                    for ky in 0..self.kernel {
                                        for kx in 0..self.kernel {
                                            value += input[input_base + ky * side + kx]
                                                * self.weight[
                                                    kernel_base + ky * self.kernel + kx
                                                ];
                                        }
                                    }
                                }
                                output_channel[oy * output_side + ox] = value;
                            }
                        }
                    }
                });
            }
        });
        output
    }
}

struct LayerNorm2d {
    channels: usize,
    weight: Vec<f32>,
    bias: Vec<f32>,
}

impl LayerNorm2d {
    fn load(weights: &BodyWeights, index: usize, channels: usize) -> Result<Self> {
        let name = format!("prompt_encoder.mask_downscaling.{index}");
        Ok(Self {
            channels,
            weight: weights.f32_shaped(&format!("{name}.weight"), &[channels])?,
            bias: weights.f32_shaped(&format!("{name}.bias"), &[channels])?,
        })
    }

    fn forward(&self, input: &[f32], side: usize) -> Vec<f32> {
        let plane = side * side;
        assert_eq!(input.len(), self.channels * plane);
        let mut output = vec![0.0f32; input.len()];
        for pixel in 0..plane {
            let mut mean = 0.0f32;
            for channel in 0..self.channels {
                mean += input[channel * plane + pixel];
            }
            mean /= self.channels as f32;
            let mut variance = 0.0f32;
            for channel in 0..self.channels {
                let centered = input[channel * plane + pixel] - mean;
                variance += centered * centered;
            }
            variance /= self.channels as f32;
            let inverse_std = (variance + NORM_EPS).sqrt().recip();
            for channel in 0..self.channels {
                output[channel * plane + pixel] =
                    (input[channel * plane + pixel] - mean) * inverse_std
                        * self.weight[channel]
                        + self.bias[channel];
            }
        }
        output
    }
}

/// The five mask-prompt convolutions and four per-pixel channel norms.
pub struct MaskEmbed {
    conv0: Conv,
    norm1: LayerNorm2d,
    conv3: Conv,
    norm4: LayerNorm2d,
    conv6: Conv,
    norm7: LayerNorm2d,
    conv9: Conv,
    norm10: LayerNorm2d,
    conv12: Conv,
    no_mask_embed: [f32; DINO_DIM],
}

impl MaskEmbed {
    pub fn load(weights: &BodyWeights) -> Result<Self> {
        let no_mask = weights.f32_shaped(
            "prompt_encoder.no_mask_embed.weight",
            &[1, DINO_DIM],
        )?;
        let mut no_mask_embed = [0.0f32; DINO_DIM];
        no_mask_embed.copy_from_slice(&no_mask);
        Ok(Self {
            conv0: Conv::load(weights, 0, 1, 4, 2)?,
            norm1: LayerNorm2d::load(weights, 1, 4)?,
            conv3: Conv::load(weights, 3, 4, 16, 2)?,
            norm4: LayerNorm2d::load(weights, 4, 16)?,
            conv6: Conv::load(weights, 6, 16, 64, 2)?,
            norm7: LayerNorm2d::load(weights, 7, 64)?,
            conv9: Conv::load(weights, 9, 64, 256, 2)?,
            norm10: LayerNorm2d::load(weights, 10, 256)?,
            conv12: Conv::load(weights, 12, 256, DINO_DIM, 1)?,
            no_mask_embed,
        })
    }

    pub fn no_mask_embed(&self) -> &[f32; DINO_DIM] {
        &self.no_mask_embed
    }

    /// Embed one 512x512 crop-space mask. The result is `[1024, 1280]`,
    /// token-major like the backbone output.
    pub fn embed(&self, crop_mask: &[f32]) -> Vec<f32> {
        let planar = self.embed_planar(crop_mask, |_, _| {});
        let mut tokens = vec![0.0f32; NUM_PATCHES * DINO_DIM];
        for channel in 0..DINO_DIM {
            for token in 0..NUM_PATCHES {
                tokens[token * DINO_DIM + channel] =
                    planar[channel * NUM_PATCHES + token];
            }
        }
        tokens
    }

    fn embed_planar(
        &self,
        crop_mask: &[f32],
        mut stage: impl FnMut(usize, &[f32]),
    ) -> Vec<f32> {
        assert_eq!(
            crop_mask.len(),
            IMAGE_SIZE * IMAGE_SIZE,
            "mask prompt must be 512x512"
        );
        let mut side = IMAGE_SIZE;
        let mut values = self.conv0.forward(crop_mask, side);
        side /= 2;
        stage(0, &values);
        values = self.norm1.forward(&values, side);
        stage(1, &values);
        gelu_erf_in_place(&mut values);

        values = self.conv3.forward(&values, side);
        side /= 2;
        stage(3, &values);
        values = self.norm4.forward(&values, side);
        stage(4, &values);
        gelu_erf_in_place(&mut values);

        values = self.conv6.forward(&values, side);
        side /= 2;
        stage(6, &values);
        values = self.norm7.forward(&values, side);
        stage(7, &values);
        gelu_erf_in_place(&mut values);

        values = self.conv9.forward(&values, side);
        side /= 2;
        stage(9, &values);
        values = self.norm10.forward(&values, side);
        stage(10, &values);
        gelu_erf_in_place(&mut values);

        values = self.conv12.forward(&values, side);
        stage(12, &values);
        debug_assert_eq!(side, PATCHES_SIDE);
        values
    }
}

fn gelu_erf_in_place(values: &mut [f32]) {
    // Abramowitz-Stegun 7.1.26 is accurate to 1.5e-7 in erf, below f32
    // precision for this encoder, and avoids a dependency solely for erf.
    for value in values {
        let x = *value / std::f32::consts::SQRT_2;
        let sign = if x < 0.0 { -1.0 } else { 1.0 };
        let ax = x.abs();
        let t = 1.0 / (1.0 + 0.327_591_1 * ax);
        let polynomial = ((((1.061_405_4 * t - 1.453_152_1) * t + 1.421_413_8) * t
            - 0.284_496_72)
            * t
            + 0.254_829_6)
            * t;
        let erf = sign * (1.0 - polynomial * (-ax * ax).exp());
        *value = 0.5 * *value * (1.0 + erf);
    }
}

/// Warp a full-frame uint8 0/1 mask through the image crop's affine. Sampling
/// is bilinear at inverse-mapped integer crop-pixel centres with zero padding;
/// cv2 preserves the uint8 dtype, so its interpolated result is rounded back
/// to 0/1 before being widened to the returned f32 values.
pub fn warp_mask(full_mask: &[u8], w: usize, h: usize, geo: &CropGeometry) -> Vec<f32> {
    assert_eq!(full_mask.len(), w * h, "full mask dimensions do not match");
    let crop = geo.crop;
    let mut output = vec![0.0f32; crop * crop];
    // The reference obtains this affine from cv2's double-precision point
    // solve before storing it as f32. Reconstructing from the geometry avoids
    // magnifying the few last-bit differences in the simplified f32 matrix
    // when the uint8 result lands exactly on its 0.5 rounding threshold.
    let k_f64 = crop as f64 / geo.side as f64;
    let k = k_f64 as f32;
    let offset_x = (0.5 * crop as f64 - k_f64 * geo.center[0] as f64) as f32;
    let offset_y = (0.5 * crop as f64 - k_f64 * geo.center[1] as f64) as f32;
    for v in 0..crop {
        let y = (v as f32 - offset_y) / k;
        let y0 = y.floor() as isize;
        let fy = y - y0 as f32;
        for u in 0..crop {
            let x = (u as f32 - offset_x) / k;
            let x0 = x.floor() as isize;
            let fx = x - x0 as f32;
            let at = |xx: isize, yy: isize| -> f32 {
                if xx < 0 || yy < 0 || xx >= w as isize || yy >= h as isize {
                    0.0
                } else {
                    full_mask[yy as usize * w + xx as usize] as f32
                }
            };
            let top = at(x0, y0) * (1.0 - fx) + at(x0 + 1, y0) * fx;
            let bottom = at(x0, y0 + 1) * (1.0 - fx) + at(x0 + 1, y0 + 1) * fx;
            output[v * crop + u] = if top * (1.0 - fy) + bottom * fy > 0.5 {
                1.0
            } else {
                0.0
            };
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gelu_is_erf_form() {
        let mut values = [-1.0, 0.0, 1.0];
        gelu_erf_in_place(&mut values);
        assert!((values[0] + 0.158_655_26).abs() < 2e-7);
        assert_eq!(values[1], 0.0);
        assert!((values[2] - 0.841_344_7).abs() < 2e-7);
    }
}
