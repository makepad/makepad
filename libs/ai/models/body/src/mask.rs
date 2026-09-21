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
    use crate::fixture::{self, OracleRoot};
    use crate::preprocess::{crop_geometry_at, crop_normalized};

    fn mask_fixture(name: &str) -> Option<(Vec<usize>, Vec<f32>)> {
        fixture::load_from(OracleRoot::Mask, name)
    }

    fn max_and_mean_abs(actual: &[f32], expected: &[f32]) -> (f32, f32, usize) {
        assert_eq!(actual.len(), expected.len());
        let mut maximum = 0.0f32;
        let mut maximum_at = 0usize;
        let mut sum = 0.0f32;
        for (index, (&actual, &expected)) in actual.iter().zip(expected).enumerate() {
            let error = (actual - expected).abs();
            sum += error;
            if error > maximum {
                maximum = error;
                maximum_at = index;
            }
        }
        (maximum, sum / actual.len() as f32, maximum_at)
    }

    #[test]
    fn oracle_mask_crop_geometry_image_and_warp() {
        let Some((image_shape, image)) = mask_fixture("input_rgb_u8") else {
            eprintln!("SKIP oracle_mask_crop_geometry_image_and_warp: fixtures absent");
            return;
        };
        let (h, w) = (image_shape[0], image_shape[1]);
        let box_values = mask_fixture("box_xyxy").expect("box_xyxy").1;
        let bbox = [box_values[0], box_values[1], box_values[2], box_values[3]];
        let geo = crop_geometry_at(bbox, w, h, None, IMAGE_SIZE, 1.25);

        let center = mask_fixture("batch_bbox_center").expect("bbox center").1;
        let scale = mask_fixture("batch_bbox_scale").expect("bbox scale").1;
        let affine = mask_fixture("batch_affine_trans").expect("affine").1;
        let center_error = max_and_mean_abs(&geo.center, &center).0;
        let scale_error = max_and_mean_abs(&[geo.side, geo.side], &scale).0;
        let affine_error = max_and_mean_abs(&geo.affine, &affine).0;
        eprintln!(
            "body mask crop geometry: center {center_error:.7}, scale {scale_error:.7}, affine {affine_error:.7} max abs"
        );
        assert!(center_error <= 1e-4);
        assert!(scale_error <= 1e-4);
        assert!(affine_error <= 1e-4);

        let rgb: Vec<u8> = image.into_iter().map(|value| value as u8).collect();
        let mut crop = crop_normalized(&rgb, w, h, &geo);
        let plane = IMAGE_SIZE * IMAGE_SIZE;
        let mean = [0.485f32, 0.456, 0.406];
        let std = [0.229f32, 0.224, 0.225];
        for channel in 0..3 {
            for value in &mut crop[channel * plane..(channel + 1) * plane] {
                *value = *value * std[channel] + mean[channel];
            }
        }
        let expected_crop = mask_fixture("batch_img").expect("batch_img").1;
        let (crop_max, crop_mean, crop_at) = max_and_mean_abs(&crop, &expected_crop);
        eprintln!(
            "body mask image crop: max {crop_max:.7} mean {crop_mean:.7} at {crop_at}"
        );
        assert!(crop_max <= 2e-2);

        let full_mask: Vec<u8> = mask_fixture("mask_full_u8")
            .expect("mask_full_u8")
            .1
            .into_iter()
            .map(|value| value as u8)
            .collect();
        let actual_mask = warp_mask(&full_mask, w, h, &geo);
        let expected_mask = mask_fixture("batch_mask").expect("batch_mask").1;
        let (mask_max, mask_mean, mask_at) =
            max_and_mean_abs(&actual_mask, &expected_mask);
        eprintln!(
            "body mask warp: max {mask_max:.7} mean {mask_mean:.7} at {mask_at}"
        );
        if mask_max > 2e-2 {
            let mismatches: Vec<_> = actual_mask
                .iter()
                .zip(&expected_mask)
                .enumerate()
                .filter(|(_, (actual, expected))| (*actual - *expected).abs() > 2e-2)
                .take(32)
                .map(|(index, (actual, expected))| {
                    (index % IMAGE_SIZE, index / IMAGE_SIZE, *actual, *expected)
                })
                .collect();
            eprintln!("body mask warp mismatches: {mismatches:?}");
        }
        assert!(mask_max <= 2e-2);
    }

    #[test]
    fn oracle_mask_downscaling_stages() {
        if fixture::oracle_dir_for(OracleRoot::Mask).is_none() {
            eprintln!("SKIP oracle_mask_downscaling_stages: fixtures absent");
            return;
        }
        let Some(weights_path) = fixture::weights_path_from(OracleRoot::Mask).or_else(fixture::weights_path) else {
            eprintln!("SKIP oracle_mask_downscaling_stages: weights absent");
            return;
        };
        let weights = BodyWeights::load(weights_path).expect("load body weights");
        let encoder = MaskEmbed::load(&weights).expect("load mask encoder");
        let input = mask_fixture("mask_prompt_in").expect("mask_prompt_in").1;
        let mut captures = Vec::new();
        let planar = encoder.embed_planar(&input, |index, values| {
            captures.push((index, values.to_vec()));
        });
        for (index, actual) in captures {
            let field = format!("maskds{index}_out_0");
            let expected = mask_fixture(&field).unwrap_or_else(|| panic!("missing {field}")).1;
            let (max, mean, at) = max_and_mean_abs(&actual, &expected);
            eprintln!("body {field}: max {max:.7} mean {mean:.7} at {at}");
            // The last two convolutions sum 256 and 1024 products in a
            // different order than the reference: f32 accumulation noise.
            let tolerance = if index >= 9 { 5e-3 } else { 1e-4 };
            assert!(max <= tolerance, "{field} max abs {max} at {at}");
        }
        let expected = mask_fixture("mask_embeddings_raw")
            .expect("mask_embeddings_raw")
            .1;
        let (max, mean, at) = max_and_mean_abs(&planar, &expected);
        eprintln!("body mask_embeddings_raw: max {max:.7} mean {mean:.7} at {at}");
        assert!(max <= 1e-4, "mask_embeddings_raw max abs {max} at {at}");

        let no_mask = mask_fixture("no_mask_embeddings")
            .expect("no_mask_embeddings")
            .1;
        let mut no_mask_max = 0.0f32;
        for channel in 0..DINO_DIM {
            for token in 0..NUM_PATCHES {
                no_mask_max = no_mask_max.max(
                    (encoder.no_mask_embed[channel]
                        - no_mask[channel * NUM_PATCHES + token])
                        .abs(),
                );
            }
        }
        eprintln!("body no_mask_embeddings: max {no_mask_max:.7}");
        assert!(no_mask_max <= 1e-7);
    }

    #[test]
    fn gpu_oracle_infer_masked_end_to_end() {
        use crate::backend::gpu_device_available;
        use crate::model::BodyModel;

        if fixture::oracle_dir_for(OracleRoot::Mask).is_none() {
            eprintln!("SKIP gpu_oracle_infer_masked_end_to_end: fixtures absent");
            return;
        }
        if !gpu_device_available() || !fixture::gpu_required_ops_available() {
            eprintln!("SKIP gpu_oracle_infer_masked_end_to_end: no GPU");
            return;
        }
        let Some(weights_path) = fixture::weights_path_from(OracleRoot::Mask).or_else(fixture::weights_path) else {
            eprintln!("SKIP gpu_oracle_infer_masked_end_to_end: weights absent");
            return;
        };
        let (image_shape, image) = mask_fixture("input_rgb_u8").expect("input image");
        let (h, w) = (image_shape[0], image_shape[1]);
        let rgb: Vec<u8> = image.into_iter().map(|value| value as u8).collect();
        let mask: Vec<u8> = mask_fixture("mask_full_u8")
            .expect("full mask")
            .1
            .into_iter()
            .map(|value| value as u8)
            .collect();
        let box_values = mask_fixture("box_xyxy").expect("box_xyxy").1;
        let bbox = [box_values[0], box_values[1], box_values[2], box_values[3]];
        let score = mask_fixture("batch_mask_score").expect("mask score").1[0];
        let mut model = BodyModel::load(&weights_path).expect("load body model");
        model.correctives_every_step = true;
        let packet = model
            .infer_masked(&rgb, w as u32, h as u32, bbox, Some((&mask, score)))
            .expect("masked inference");
        let person = &packet.people[0];
        let kp3d = mask_fixture("final_pred_keypoints_3d")
            .expect("final 3d keypoints")
            .1;
        let kp2d = mask_fixture("final_pred_keypoints_2d")
            .expect("final 2d keypoints")
            .1;
        let (error3d, mean3d, at3d) = max_and_mean_abs(&person.kp3d, &kp3d);
        let (error2d, mean2d, at2d) = max_and_mean_abs(&person.kp2d, &kp2d);
        eprintln!(
            "body infer_masked: kp3d max {error3d:.7} m mean {mean3d:.7} at {at3d}; kp2d max {error2d:.4} px mean {mean2d:.4} at {at2d}"
        );
        // The same bf16 backbone noise budget as the full-mode tests.
        assert!(error3d <= 4e-3, "masked kp3d max abs {error3d} m at {at3d}");
        assert!(error2d <= 1.0, "masked kp2d max abs {error2d} px at {at2d}");
    }

    #[test]
    fn gelu_is_erf_form() {
        let mut values = [-1.0, 0.0, 1.0];
        gelu_erf_in_place(&mut values);
        assert!((values[0] + 0.158_655_26).abs() < 2e-7);
        assert_eq!(values[1], 0.0);
        assert!((values[2] - 0.841_344_7).abs() < 2e-7);
    }
}
