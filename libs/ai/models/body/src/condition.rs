//! Dense image positional encoding and ray-conditioned decoder context.

use crate::backend::{
    gpu_add, gpu_layer_norm_mul_add, gpu_linear_f32_resident, gpu_upload, GpuTensor,
};
use crate::weights::BodyWeights;
use crate::{DiffusionError, Result, DEC_NORM_EPS, DINO_DIM, PATCHES_SIDE};

const PE_HALF: usize = DINO_DIM / 2;
const RAY_FEATURES: usize = 99;
const RAY_FREQUENCIES: usize = 16;

/// Apply a supplied mask prompt before the existing ray-conditioning seam.
/// `None` performs no operation so the v1 no-mask path remains byte-for-byte
/// unchanged. For a mask, subtract the no-mask embedding because
/// [`RayCond::apply`] adds its projection back through the folded bias.
pub fn apply_mask_prompt(
    x: &GpuTensor,
    mask: Option<(&[f32], f32)>,
    no_mask_embed: &[f32; DINO_DIM],
) -> Result<Option<GpuTensor>> {
    let Some((mask_tokens, mask_score)) = mask else {
        return Ok(None);
    };
    if x.cols() != DINO_DIM || mask_tokens.len() != x.rows() * DINO_DIM {
        return Err(DiffusionError::workflow(format!(
            "body mask conditioning shapes are x={}x{} mask={}, expected {} values",
            x.rows(),
            x.cols(),
            mask_tokens.len(),
            x.rows() * DINO_DIM,
        )));
    }
    let mut adjustment = vec![0.0f32; mask_tokens.len()];
    for (token, row) in adjustment.chunks_exact_mut(DINO_DIM).enumerate() {
        let mask_row = &mask_tokens[token * DINO_DIM..(token + 1) * DINO_DIM];
        for channel in 0..DINO_DIM {
            row[channel] = mask_score * mask_row[channel] - no_mask_embed[channel];
        }
    }
    let adjustment = gpu_upload(&adjustment, x.rows(), DINO_DIM)
        .map_err(DiffusionError::model)?;
    gpu_add(x, &adjustment)
        .map(Some)
        .map_err(DiffusionError::model)
}

pub fn dense_pe(g: &[f32]) -> Vec<f32> {
    dense_pe_at(g, PATCHES_SIDE)
}

/// The dense positional encoding of a `side x side` patch grid.
pub fn dense_pe_at(g: &[f32], side: usize) -> Vec<f32> {
    assert_eq!(g.len(), 2 * PE_HALF, "dense PE matrix must be 2x640");
    let mut output = vec![0.0f32; side * side * DINO_DIM];
    for gy in 0..side {
        let y = 2.0 * ((gy as f32 + 0.5) / side as f32) - 1.0;
        for gx in 0..side {
            let x = 2.0 * ((gx as f32 + 0.5) / side as f32) - 1.0;
            let row = gy * side + gx;
            for k in 0..PE_HALF {
                let angle = 2.0 * std::f32::consts::PI * (x * g[k] + y * g[PE_HALF + k]);
                output[row * DINO_DIM + k] = angle.sin();
                output[row * DINO_DIM + PE_HALF + k] = angle.cos();
            }
        }
    }
    output
}

pub fn ray_features(rays: &[f32]) -> Vec<f32> {
    assert_eq!(rays.len() % 2, 0, "patch rays must be (x, y) pairs");
    let tokens = rays.len() / 2;
    let mut output = vec![0.0f32; tokens * RAY_FEATURES];
    for token in 0..tokens {
        let ray = [rays[token * 2], rays[token * 2 + 1], 1.0];
        let row = &mut output[token * RAY_FEATURES..(token + 1) * RAY_FEATURES];
        row[..3].copy_from_slice(&ray);
        for d in 0..3 {
            for k in 0..RAY_FREQUENCIES {
                let frequency = 1.0 + 31.0 * k as f32 / (RAY_FREQUENCIES - 1) as f32;
                let angle = std::f32::consts::PI * ray[d] * frequency;
                row[3 + d * RAY_FREQUENCIES + k] = angle.sin();
                row[3 + 3 * RAY_FREQUENCIES + d * RAY_FREQUENCIES + k] = angle.cos();
            }
        }
    }
    output
}

/// The 1x1 conv over `[E + no_mask | ray features]` split into its two
/// column blocks, `W_e` (1280 wide) and `W_f` (99 wide), so the context is
/// `W_e E + (W_f feats + W_e no_mask)`: two resident linears and an add,
/// no host round trip of the 1024 x 1280 embedding. The `W_e no_mask` term
/// is a constant and lives in the second linear's bias.
pub struct RayCond {
    image_w: GpuTensor,
    ray_w: GpuTensor,
    image_w_host: Vec<f32>,
    pub norm_w: Vec<f32>,
    pub norm_b: Vec<f32>,
}

impl RayCond {
    pub fn prepare(weights: &BodyWeights) -> Result<Self> {
        Self::prepare_named(weights, "ray_cond_emb")
    }

    pub fn prepare_named(weights: &BodyWeights, prefix: &str) -> Result<Self> {
        let conv = weights.f32_shaped(
            &format!("{prefix}.conv.weight"),
            &[DINO_DIM, DINO_DIM + RAY_FEATURES, 1, 1],
        )?;
        let cols = DINO_DIM + RAY_FEATURES;
        let mut image_w = vec![0.0f32; DINO_DIM * DINO_DIM];
        let mut ray_w = vec![0.0f32; DINO_DIM * RAY_FEATURES];
        for out in 0..DINO_DIM {
            image_w[out * DINO_DIM..(out + 1) * DINO_DIM]
                .copy_from_slice(&conv[out * cols..out * cols + DINO_DIM]);
            ray_w[out * RAY_FEATURES..(out + 1) * RAY_FEATURES]
                .copy_from_slice(&conv[out * cols + DINO_DIM..(out + 1) * cols]);
        }
        Ok(Self {
            image_w: gpu_upload(&image_w, DINO_DIM, DINO_DIM).map_err(DiffusionError::model)?,
            ray_w: gpu_upload(&ray_w, DINO_DIM, RAY_FEATURES).map_err(DiffusionError::model)?,
            image_w_host: image_w,
            norm_w: weights.f32_shaped(&format!("{prefix}.norm.weight"), &[DINO_DIM])?,
            norm_b: weights.f32_shaped(&format!("{prefix}.norm.bias"), &[DINO_DIM])?,
        })
    }

    pub fn apply(
        &self,
        e: &GpuTensor,
        no_mask_embed: &[f32; DINO_DIM],
        feats: &[f32],
    ) -> Result<GpuTensor> {
        let tokens = e.rows();
        if e.cols() != DINO_DIM {
            return Err(DiffusionError::workflow(format!(
                "ray conditioning image shape is {}x{}, expected {tokens}x{DINO_DIM}",
                e.rows(),
                e.cols()
            )));
        }
        if feats.len() != tokens * RAY_FEATURES {
            return Err(DiffusionError::workflow(format!(
                "ray conditioning features have {} values, expected {}",
                feats.len(),
                tokens * RAY_FEATURES
            )));
        }
        // W_e no_mask: 1.6M multiply-adds on the host, once per frame.
        let mut bias = vec![0.0f32; DINO_DIM];
        for (out, value) in bias.iter_mut().enumerate() {
            let row = &self.image_w_host[out * DINO_DIM..(out + 1) * DINO_DIM];
            *value = row.iter().zip(no_mask_embed).map(|(w, m)| w * m).sum();
        }
        let bias = gpu_upload(&bias, 1, DINO_DIM).map_err(DiffusionError::model)?;
        let feats = gpu_upload(feats, tokens, RAY_FEATURES).map_err(DiffusionError::model)?;
        let from_image = gpu_linear_f32_resident(e, &self.image_w, None).map_err(DiffusionError::model)?;
        let from_rays =
            gpu_linear_f32_resident(&feats, &self.ray_w, Some(&bias)).map_err(DiffusionError::model)?;
        let projected = gpu_add(&from_image, &from_rays).map_err(DiffusionError::model)?;
        gpu_layer_norm_mul_add(&projected, &self.norm_w, &self.norm_b, DEC_NORM_EPS)
            .map_err(DiffusionError::model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{gpu_device_available, gpu_download, gpu_upload};
    use crate::NUM_PATCHES;

    fn assert_close(actual: f32, expected: f32, tolerance: f32) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "actual={actual} expected={expected} tolerance={tolerance}"
        );
    }

    fn planar_to_tokens(values: &[f32], channels: usize) -> Vec<f32> {
        let mut output = vec![0.0f32; NUM_PATCHES * channels];
        for c in 0..channels {
            for token in 0..NUM_PATCHES {
                output[token * channels + c] = values[c * NUM_PATCHES + token];
            }
        }
        output
    }

    fn matrix_from_dense_pe(pe: &[f32]) -> Vec<f32> {
        let mut matrix = vec![0.0f32; 2 * PE_HALF];
        let coordinate = 2.0 * (0.5 / PATCHES_SIDE as f32) - 1.0;
        for k in 0..PE_HALF {
            let phase = |token: usize| {
                let row = token * DINO_DIM;
                (pe[row + k], pe[row + PE_HALF + k])
            };
            let (s00, c00) = phase(0);
            let (sx, cx) = phase(1);
            let (sy, cy) = phase(PATCHES_SIDE);
            let dx = (sx * c00 - cx * s00).atan2(cx * c00 + sx * s00);
            let dy = (sy * c00 - cy * s00).atan2(cy * c00 + sy * s00);
            let mut gx = dx * 16.0 / (2.0 * std::f32::consts::PI);
            let gy = dy * 16.0 / (2.0 * std::f32::consts::PI);
            let base = 2.0 * std::f32::consts::PI * coordinate * (gx + gy);
            if base.sin() * s00 + base.cos() * c00 < 0.0 {
                // A 16.0 frequency shift preserves adjacent phase deltas but
                // flips every value on this half-offset 32x32 grid.
                gx += 16.0;
            }
            matrix[k] = gx;
            matrix[PE_HALF + k] = gy;
        }
        matrix
    }

    #[test]
    fn ray_feature_order_is_exact() {
        let mut rays = vec![0.0f32; NUM_PATCHES * 2];
        rays[0] = 0.25;
        rays[1] = -0.5;
        let features = ray_features(&rays);
        assert_eq!(&features[..3], &[0.25, -0.5, 1.0]);
        for d in 0..3 {
            let ray = [0.25f32, -0.5, 1.0][d];
            for k in 0..16 {
                let frequency = 1.0 + 31.0 * k as f32 / 15.0;
                let expected = (std::f32::consts::PI * ray * frequency).sin();
                assert_close(features[3 + d * 16 + k], expected, 1e-7);
            }
        }
        assert_close(features[3], (std::f32::consts::PI * 0.25).sin(), 1e-7);
        assert_close(features[3 + 15], (std::f32::consts::PI * 0.25 * 32.0).sin(), 1e-6);
    }

    #[test]
    fn dense_pe_is_sin_then_cos_and_corner_symmetric() {
        let mut g = vec![0.0f32; 2 * PE_HALF];
        g[0] = 0.25;
        g[PE_HALF] = -0.5;
        g[7] = 0.125;
        let pe = dense_pe(&g);
        let coord = 2.0 * (0.5 / PATCHES_SIDE as f32) - 1.0;
        let angle = 2.0 * std::f32::consts::PI * (coord * 0.25 + coord * -0.5);
        assert_close(pe[0], angle.sin(), 1e-7);
        assert_close(pe[PE_HALF], angle.cos(), 1e-7);
        let opposite = (NUM_PATCHES - 1) * DINO_DIM;
        for k in 0..PE_HALF {
            assert_close(pe[opposite + k], -pe[k], 2e-6);
            assert_close(pe[opposite + PE_HALF + k], pe[PE_HALF + k], 2e-6);
        }
    }

    #[test]
    fn fixture_dense_pe() {
        let Some((expected_shape, expected)) = crate::fixture::load("decoder_image_augment_in") else {
            eprintln!("body oracle fixtures absent; skipping dense-PE parity");
            return;
        };
        let expected_tokens = if expected_shape.ends_with(&[DINO_DIM, PATCHES_SIDE, PATCHES_SIDE]) {
            planar_to_tokens(&expected, DINO_DIM)
        } else {
            expected
        };
        let matrix_names = [
            "positional_encoding_gaussian_matrix",
            "prompt_encoder.pe_layer.positional_encoding_gaussian_matrix",
            "dense_pe_g",
        ];
        let mut matrix = matrix_names
            .iter()
            .find_map(|name| crate::fixture::load(name).map(|(_, values)| values));
        if matrix.is_none() {
            if let Some(path) = crate::fixture::weights_path() {
                matrix = BodyWeights::load(path).ok().and_then(|weights| {
                    weights
                        .f32_shaped(
                            "prompt_encoder.pe_layer.positional_encoding_gaussian_matrix",
                            &[2, PE_HALF],
                        )
                        .ok()
                });
            }
        }
        // The phase deltas in the oracle PE recover an equivalent matrix on
        // the half-offset 32x32 grid when the standalone matrix is omitted.
        let matrix = matrix.unwrap_or_else(|| matrix_from_dense_pe(&expected_tokens));
        let actual = dense_pe(&matrix);
        assert_eq!(actual.len(), expected_tokens.len());
        let max_error = actual
            .iter()
            .zip(&expected_tokens)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        eprintln!("body dense-PE max abs error: {max_error:.7}");
        assert!(max_error <= 1e-4);
    }

    #[test]
    fn gpu_fixture_ray_conditioning() {
        let Some((_, image_planar)) = crate::fixture::load("raycond_img_in") else {
            eprintln!("body oracle fixtures absent; skipping ray-condition GPU parity");
            return;
        };
        let Some((_, rays_planar)) = crate::fixture::load("raycond_rays_in") else {
            eprintln!("body ray fixture absent; skipping ray-condition GPU parity");
            return;
        };
        let Some((expected_shape, expected_planar)) = crate::fixture::load("raycond_out") else {
            eprintln!("body ray output fixture absent; skipping ray-condition GPU parity");
            return;
        };
        let Some(weights_path) = crate::fixture::weights_path() else {
            eprintln!("body weights path absent; skipping ray-condition GPU parity");
            return;
        };
        if !gpu_device_available() || !crate::fixture::gpu_required_ops_available() {
            eprintln!("body GPU unavailable; skipping ray-condition GPU parity");
            return;
        }
        let image = planar_to_tokens(&image_planar, DINO_DIM);
        let plane = crate::IMAGE_SIZE * crate::IMAGE_SIZE;
        assert_eq!(rays_planar.len(), 2 * plane);
        // The ray field is affine: sample it at the antialiased tap positions
        // (what the reference's shrink-by-16 amounts to), per axis.
        let sample = |c: usize, x: f32, y: f32| {
            let x0 = x.floor() as usize;
            let y0 = y.floor() as usize;
            let (x1, y1) = ((x0 + 1).min(crate::IMAGE_SIZE - 1), (y0 + 1).min(crate::IMAGE_SIZE - 1));
            let (fx, fy) = (x - x0 as f32, y - y0 as f32);
            let at = |xx: usize, yy: usize| rays_planar[c * plane + yy * crate::IMAGE_SIZE + xx];
            (at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx) * (1.0 - fy)
                + (at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx) * fy
        };
        let mut patch = vec![0.0f32; NUM_PATCHES * 2];
        for gy in 0..PATCHES_SIDE {
            for gx in 0..PATCHES_SIDE {
                let token = gy * PATCHES_SIDE + gx;
                let (x, y) = (
                    crate::preprocess::patch_sample_coord(gx),
                    crate::preprocess::patch_sample_coord(gy),
                );
                for c in 0..2 {
                    patch[token * 2 + c] = sample(c, x, y);
                }
            }
        }
        let feats = ray_features(&patch);
        let weights = BodyWeights::load(weights_path).expect("load body weights");
        let ray_cond = RayCond::prepare(&weights).expect("prepare ray conditioning");
        let image = gpu_upload(&image, NUM_PATCHES, DINO_DIM).expect("upload raycond image");
        // raycond_img_in already contains the no-mask embedding.
        let output = ray_cond
            .apply(&image, &[0.0; DINO_DIM], &feats)
            .expect("ray conditioning forward");
        let output = gpu_download(&output).expect("download ray conditioning");
        let expected = if expected_shape.ends_with(&[DINO_DIM, PATCHES_SIDE, PATCHES_SIDE]) {
            planar_to_tokens(&expected_planar, DINO_DIM)
        } else {
            expected_planar
        };
        let max_error = output
            .iter()
            .zip(&expected)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        eprintln!("body ray conditioning max abs error: {max_error:.7}");
        assert!(max_error <= 1e-3);
    }

    #[test]
    fn gpu_oracle_mask_conditioned_context() {
        use crate::fixture::OracleRoot;

        if crate::fixture::oracle_dir_for(OracleRoot::Mask).is_none() {
            eprintln!("SKIP gpu_oracle_mask_conditioned_context: fixtures absent");
            return;
        }
        if !gpu_device_available() || !crate::fixture::gpu_required_ops_available() {
            eprintln!("SKIP gpu_oracle_mask_conditioned_context: no GPU");
            return;
        }
        let Some(weights_path) = crate::fixture::weights_path_from(OracleRoot::Mask).or_else(crate::fixture::weights_path) else {
            eprintln!("SKIP gpu_oracle_mask_conditioned_context: weights absent");
            return;
        };
        let load = |name: &str| {
            crate::fixture::load_from(OracleRoot::Mask, name)
                .unwrap_or_else(|| panic!("missing mask fixture {name}"))
                .1
        };
        let weights = BodyWeights::load(weights_path).expect("load body weights");
        let no_mask_values = weights
            .f32_shaped("prompt_encoder.no_mask_embed.weight", &[1, DINO_DIM])
            .expect("load no-mask embedding");
        let mut no_mask = [0.0f32; DINO_DIM];
        no_mask.copy_from_slice(&no_mask_values);

        let image = planar_to_tokens(&load("image_embeddings_before_mask"), DINO_DIM);
        let mask = planar_to_tokens(&load("mask_embeddings_raw"), DINO_DIM);
        let score = load("batch_mask_score")[0];
        let image = gpu_upload(&image, NUM_PATCHES, DINO_DIM).expect("upload image embedding");
        let conditioned = apply_mask_prompt(&image, Some((&mask, score)), &no_mask)
            .expect("apply mask prompt")
            .expect("mask prompt result");

        let image_shape = crate::fixture::load_from(OracleRoot::Mask, "input_rgb_u8")
            .expect("input image")
            .0;
        let bbox_values = load("box_xyxy");
        let geo = crate::preprocess::crop_geometry_at(
            [bbox_values[0], bbox_values[1], bbox_values[2], bbox_values[3]],
            image_shape[1],
            image_shape[0],
            None,
            crate::IMAGE_SIZE,
            1.25,
        );
        let feats = ray_features(&crate::preprocess::patch_rays(&geo));
        let ray_cond = RayCond::prepare(&weights).expect("prepare ray conditioning");
        let actual = ray_cond
            .apply(&conditioned, &no_mask, &feats)
            .expect("ray conditioning");
        let actual = gpu_download(&actual).expect("download ray conditioning");
        let expected = planar_to_tokens(&load("raycond_out_0"), DINO_DIM);
        let (maximum, at) = actual
            .iter()
            .zip(&expected)
            .enumerate()
            .map(|(index, (actual, expected))| ((actual - expected).abs(), index))
            .fold((0.0f32, 0usize), |best, value| {
                if value.0 > best.0 { value } else { best }
            });
        eprintln!("body masked raycond_out_0: max {maximum:.7} at {at}");
        assert!(maximum <= 5e-3, "raycond_out_0 max abs {maximum} at {at}");
    }
}
