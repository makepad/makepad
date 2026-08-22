//! Gaussian assembly and the 3DGS PLY writer.
//!
//! `_build_gaussians` in triposplat.py turns one decoder feature row into 32
//! gaussians, applying the representation's activations; `Gaussian._get_ply_data`
//! then UNDOES them, because a 3DGS PLY stores pre-activation parameters:
//! the opacity logit, log scales, and a wxyz quaternion. Both halves are
//! folded together here so no value makes a pointless round trip through its
//! own activation.
//!
//! The output is the standard binary-little-endian layout every 3DGS viewer
//! (including the in-repo `makepad_splat::load_ply_from_bytes`) expects:
//! `x y z nx ny nz f_dc_0..2 opacity scale_0..2 rot_0..3`, sh_degree 0 so
//! there are no `f_rest_*` columns.
//!
//! One deliberate deviation from the reference: the opacity column is written
//! as `raw + inverse_sigmoid(opacity_bias)` directly instead of
//! `inverse_sigmoid(sigmoid(raw + bias))`. The two are the same value, but
//! the reference's round trip saturates to +/-inf in f32 once |raw| exceeds
//! ~17 and would put a non-finite number in the artifact.

use crate::splat::{
    gs_layout_range, GS_FILTER_KERNEL_3D, GS_LR_ROTATION, GS_OPACITY_BIAS, GS_OUT_CHANNELS,
    GS_PER_POINT, GS_SCALING_BIAS,
};
use crate::splat_decoder::{gaussian_offsets, softplus};
use crate::{DiffusionError, Result};

/// `Gaussian._DEFAULT_TRANSFORM` — the Y-up rotation every saved artifact is
/// written through.
pub const DEFAULT_TRANSFORM: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]];

/// `aabb = [-0.5, -0.5, -0.5, 1.0, 1.0, 1.0]`: `xyz = raw * size + origin`.
const AABB_ORIGIN: f32 = -0.5;
const AABB_SIZE: f32 = 1.0;

/// One finished splat in PLY (pre-activation) units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlySplat {
    pub position: [f32; 3],
    pub f_dc: [f32; 3],
    /// Opacity LOGIT.
    pub opacity: f32,
    /// LOG scales.
    pub scale: [f32; 3],
    /// wxyz quaternion.
    pub rotation: [f32; 4],
}

/// `inverse_sigmoid(opacity_bias)`.
pub fn opacity_bias_logit() -> f32 {
    (GS_OPACITY_BIAS / (1.0 - GS_OPACITY_BIAS)).ln()
}

/// `inverse_softplus(scaling_bias)` = `x + log(-expm1(-x))`.
pub fn scaling_bias_preactivation() -> f32 {
    GS_SCALING_BIAS + (-((-GS_SCALING_BIAS).exp_m1())).ln()
}

/// `_build_gaussians` + `_get_ply_data`, for one anchor's decoder feature row
/// and its anchor position. Appends 32 splats.
pub fn build_anchor_splats(
    features: &[f32],
    anchor: [f32; 3],
    perturbation: &[f32],
    base_offset_scale: f32,
    transform: &[[f32; 3]; 3],
    out: &mut Vec<PlySplat>,
) -> Result<()> {
    if features.len() != GS_OUT_CHANNELS {
        return Err(DiffusionError::workflow(format!(
            "gaussian feature row is {} wide, expected {GS_OUT_CHANNELS}",
            features.len()
        )));
    }
    let offsets = gaussian_offsets(features, perturbation, base_offset_scale);
    let (dc0, _) = gs_layout_range("_features_dc").expect("layout");
    let (scaling0, _) = gs_layout_range("_scaling").expect("layout");
    let (rotation0, _) = gs_layout_range("_rotation").expect("layout");
    let (opacity0, _) = gs_layout_range("_opacity").expect("layout");
    let scale_bias = scaling_bias_preactivation();
    let opacity_bias = opacity_bias_logit();

    for g in 0..GS_PER_POINT {
        let mut position = [0.0f32; 3];
        for axis in 0..3 {
            let raw = offsets[g * 3 + axis] + anchor[axis];
            position[axis] = raw * AABB_SIZE + AABB_ORIGIN;
        }
        let mut scale = [0.0f32; 3];
        for axis in 0..3 {
            // get_scaling = sqrt(softplus(raw + bias)^2 + kernel^2); the PLY
            // column is its log.
            let activated = softplus(features[scaling0 + g * 3 + axis] + scale_bias);
            let squared = activated * activated + GS_FILTER_KERNEL_3D * GS_FILTER_KERNEL_3D;
            scale[axis] = 0.5 * squared.ln();
        }
        // lr['_rotation'] = 0.1, and rots_bias = [1, 0, 0, 0].
        let mut rotation = [0.0f32; 4];
        for c in 0..4 {
            rotation[c] = features[rotation0 + g * 4 + c] * GS_LR_ROTATION;
        }
        rotation[0] += 1.0;

        let (position, rotation) = apply_transform(position, rotation, transform);
        out.push(PlySplat {
            position,
            f_dc: [
                features[dc0 + g * 3],
                features[dc0 + g * 3 + 1],
                features[dc0 + g * 3 + 2],
            ],
            opacity: features[opacity0 + g] + opacity_bias,
            scale,
            rotation,
        });
    }
    Ok(())
}

/// `xyz @ T^T` and `quat(T @ R(quat))`.
pub fn apply_transform(
    position: [f32; 3],
    rotation: [f32; 4],
    transform: &[[f32; 3]; 3],
) -> ([f32; 3], [f32; 4]) {
    let mut moved = [0.0f32; 3];
    for (i, row) in transform.iter().enumerate() {
        moved[i] = row[0] * position[0] + row[1] * position[1] + row[2] * position[2];
    }
    let r = quat_to_matrix(rotation);
    let mut rotated = [[0.0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            rotated[i][j] = (0..3).map(|k| transform[i][k] * r[k][j]).sum();
        }
    }
    (moved, matrix_to_quat(rotated))
}

/// `_quat_to_matrix` (wxyz, normalized first).
pub fn quat_to_matrix(q: [f32; 4]) -> [[f32; 3]; 3] {
    let norm = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    let inv = if norm > 0.0 { 1.0 / norm } else { 0.0 };
    let (w, x, y, z) = (q[0] * inv, q[1] * inv, q[2] * inv, q[3] * inv);
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - w * z),
            2.0 * (x * z + w * y),
        ],
        [
            2.0 * (x * y + w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - w * x),
        ],
        [
            2.0 * (x * z - w * y),
            2.0 * (y * z + w * x),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ]
}

/// `_matrix_to_quat`, including the reference's `s == 0` fallbacks for the
/// trace <= -1 case.
pub fn matrix_to_quat(r: [[f32; 3]; 3]) -> [f32; 4] {
    let trace = r[0][0] + r[1][1] + r[2][2];
    let s = (trace + 1.0).max(0.0).sqrt() * 2.0;
    let mut q = if s != 0.0 {
        [
            0.25 * s,
            (r[2][1] - r[1][2]) / s,
            (r[0][2] - r[2][0]) / s,
            (r[1][0] - r[0][1]) / s,
        ]
    } else if r[0][0] >= r[1][1] && r[0][0] >= r[2][2] {
        let s1 = (1.0 + r[0][0] - r[1][1] - r[2][2]).max(0.0).sqrt() * 2.0;
        [
            (r[2][1] - r[1][2]) / s1,
            0.25 * s1,
            (r[0][1] + r[1][0]) / s1,
            (r[0][2] + r[2][0]) / s1,
        ]
    } else if r[1][1] > r[0][0] && r[1][1] >= r[2][2] {
        let s2 = (1.0 + r[1][1] - r[0][0] - r[2][2]).max(0.0).sqrt() * 2.0;
        [
            (r[0][2] - r[2][0]) / s2,
            (r[0][1] + r[1][0]) / s2,
            0.25 * s2,
            (r[1][2] + r[2][1]) / s2,
        ]
    } else {
        let s3 = (1.0 + r[2][2] - r[0][0] - r[1][1]).max(0.0).sqrt() * 2.0;
        [
            (r[1][0] - r[0][1]) / s3,
            (r[0][2] + r[2][0]) / s3,
            (r[1][2] + r[2][1]) / s3,
            0.25 * s3,
        ]
    };
    let norm = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if norm > 0.0 {
        for value in &mut q {
            *value /= norm;
        }
    } else {
        q = [1.0, 0.0, 0.0, 0.0];
    }
    q
}

/// The 17 property names of a sh_degree-0 3DGS PLY, in file order.
pub const PLY_PROPERTIES: [&str; 17] = [
    "x", "y", "z", "nx", "ny", "nz", "f_dc_0", "f_dc_1", "f_dc_2", "opacity", "scale_0", "scale_1",
    "scale_2", "rot_0", "rot_1", "rot_2", "rot_3",
];

/// Serialize to a binary-little-endian 3DGS PLY.
pub fn write_ply(splats: &[PlySplat]) -> Vec<u8> {
    let mut header = String::from("ply\nformat binary_little_endian 1.0\n");
    header.push_str(&format!("element vertex {}\n", splats.len()));
    for name in PLY_PROPERTIES {
        header.push_str(&format!("property float {name}\n"));
    }
    header.push_str("end_header\n");

    let mut bytes = Vec::with_capacity(header.len() + splats.len() * PLY_PROPERTIES.len() * 4);
    bytes.extend_from_slice(header.as_bytes());
    for splat in splats {
        let row = [
            splat.position[0],
            splat.position[1],
            splat.position[2],
            0.0,
            0.0,
            0.0,
            splat.f_dc[0],
            splat.f_dc[1],
            splat.f_dc[2],
            splat.opacity,
            splat.scale[0],
            splat.scale[1],
            splat.scale[2],
            splat.rotation[0],
            splat.rotation[1],
            splat.rotation[2],
            splat.rotation[3],
        ];
        for value in row {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bias_constants_match_the_reference_representation_config() {
        // inverse_sigmoid(0.1) = ln(1/9)
        assert!((opacity_bias_logit() - (1.0f32 / 9.0).ln()).abs() < 1e-6);
        // inverse_softplus(0.004): softplus of it must return 0.004.
        let pre = scaling_bias_preactivation();
        assert!((softplus(pre) - GS_SCALING_BIAS).abs() < 1e-7, "{pre}");
        assert!(pre < -5.0 && pre > -6.0, "{pre}");
    }

    #[test]
    fn identity_transform_round_trips_a_quaternion() {
        let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let q = [0.5f32, 0.5, 0.5, 0.5];
        let (position, rotation) = apply_transform([1.0, 2.0, 3.0], q, &identity);
        assert_eq!(position, [1.0, 2.0, 3.0]);
        for (a, b) in rotation.iter().zip(&q) {
            assert!((a - b).abs() < 1e-6, "{rotation:?} vs {q:?}");
        }
    }

    #[test]
    fn default_transform_is_the_y_up_rotation() {
        // (x, y, z) -> (x, -z, y)
        let (position, _) = apply_transform([1.0, 2.0, 3.0], [1.0, 0.0, 0.0, 0.0], &DEFAULT_TRANSFORM);
        assert_eq!(position, [1.0, -3.0, 2.0]);
        // The identity rotation becomes the transform's own quaternion: a
        // +90 degree turn about X.
        let (_, rotation) = apply_transform([0.0; 3], [1.0, 0.0, 0.0, 0.0], &DEFAULT_TRANSFORM);
        let half = (std::f32::consts::FRAC_PI_4).cos();
        assert!((rotation[0] - half).abs() < 1e-5, "{rotation:?}");
        assert!((rotation[1] - half).abs() < 1e-5, "{rotation:?}");
        assert!(rotation[2].abs() < 1e-6 && rotation[3].abs() < 1e-6);
        // Round-tripping through the matrix must land back on a unit quat.
        let norm: f32 = rotation.iter().map(|v| v * v).sum();
        assert!((norm - 1.0).abs() < 1e-5);
    }

    #[test]
    fn quaternion_matrix_round_trip_over_many_rotations() {
        let mut rng = crate::splat_rand::SplatRng::new(9);
        for _ in 0..64 {
            let mut q = [rng.normal(), rng.normal(), rng.normal(), rng.normal()];
            let norm: f32 = q.iter().map(|v| v * v).sum::<f32>().sqrt();
            if norm < 1e-3 {
                continue;
            }
            for value in &mut q {
                *value /= norm;
            }
            if q[0] < 0.0 {
                // The trace branch always returns the positive-w hemisphere.
                for value in &mut q {
                    *value = -*value;
                }
            }
            let back = matrix_to_quat(quat_to_matrix(q));
            for (a, b) in back.iter().zip(&q) {
                assert!((a - b).abs() < 1e-4, "{back:?} vs {q:?}");
            }
        }
    }

    #[test]
    fn build_anchor_splats_emits_32_gaussians_in_pre_activation_units() {
        let features = vec![0.0f32; GS_OUT_CHANNELS];
        let perturbation = vec![0.0f32; GS_PER_POINT * 3];
        let mut out = Vec::new();
        build_anchor_splats(
            &features,
            [0.5, 0.5, 0.5],
            &perturbation,
            0.0,
            &DEFAULT_TRANSFORM,
            &mut out,
        )
        .unwrap();
        assert_eq!(out.len(), GS_PER_POINT);
        // All-zero features: offset 0, anchor 0.5 -> aabb 0.0 -> transform 0.
        assert_eq!(out[0].position, [0.0, 0.0, 0.0]);
        // opacity column is the raw logit plus the representation's bias.
        assert!((out[0].opacity - (1.0f32 / 9.0).ln()).abs() < 1e-6);
        // scale column is ln(sqrt(softplus(bias)^2 + k^2)) = ln of ~0.004.
        let want = 0.5
            * (GS_SCALING_BIAS * GS_SCALING_BIAS
                + GS_FILTER_KERNEL_3D * GS_FILTER_KERNEL_3D)
                .ln();
        assert!((out[0].scale[0] - want).abs() < 1e-4, "{}", out[0].scale[0]);
        // rotation is rots_bias transformed by the Y-up rotation.
        let half = (std::f32::consts::FRAC_PI_4).cos();
        assert!((out[0].rotation[0] - half).abs() < 1e-5);
        // A wrong-width feature row is a workflow error, not a panic.
        assert!(build_anchor_splats(
            &[0.0; 4],
            [0.0; 3],
            &perturbation,
            0.0,
            &DEFAULT_TRANSFORM,
            &mut out
        )
        .is_err());
    }

    #[test]
    fn ply_header_and_stride_are_the_3dgs_contract() {
        let splat = PlySplat {
            position: [1.0, 2.0, 3.0],
            f_dc: [0.1, 0.2, 0.3],
            opacity: -1.0,
            scale: [-5.0, -5.0, -5.0],
            rotation: [1.0, 0.0, 0.0, 0.0],
        };
        let bytes = write_ply(&[splat, splat]);
        let header_end = bytes
            .windows(11)
            .position(|w| w == b"end_header\n")
            .unwrap()
            + 11;
        let text = String::from_utf8_lossy(&bytes[..header_end]);
        assert!(text.starts_with("ply\nformat binary_little_endian 1.0\nelement vertex 2\n"));
        assert!(text.contains("property float f_dc_2\n"));
        assert!(text.contains("property float rot_3\n"));
        assert!(!text.contains("f_rest"));
        assert_eq!(bytes.len() - header_end, 2 * 17 * 4);
    }
}
