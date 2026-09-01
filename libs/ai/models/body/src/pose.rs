//! Pose-head parameter decoding and camera projection.

use crate::mhr::MhrRig;

const BODY_ROTATION_IDXS: [[usize; 3]; 23] = [
    [0, 2, 4],
    [6, 8, 10],
    [12, 13, 14],
    [15, 16, 17],
    [18, 19, 20],
    [21, 22, 23],
    [24, 25, 26],
    [27, 28, 29],
    [34, 35, 36],
    [37, 38, 39],
    [44, 45, 46],
    [53, 54, 55],
    [64, 65, 66],
    [85, 69, 73],
    [86, 70, 79],
    [87, 71, 82],
    [88, 72, 76],
    [91, 92, 93],
    [112, 96, 100],
    [113, 97, 106],
    [114, 98, 109],
    [115, 99, 103],
    [130, 131, 132],
];

const BODY_HINGE_IDXS: [usize; 58] = [
    1, 3, 5, 7, 9, 11, 30, 31, 32, 33, 40, 41, 42, 43, 47, 48, 49, 50, 51, 52,
    56, 57, 58, 59, 60, 61, 62, 63, 67, 68, 74, 75, 77, 78, 80, 81, 83, 84, 89,
    90, 94, 95, 101, 102, 104, 105, 107, 108, 110, 111, 116, 117, 118, 119, 120,
    121, 122, 123,
];

const HAND_DOFS: [usize; 16] = [3, 1, 1, 3, 1, 1, 3, 1, 1, 3, 1, 1, 2, 3, 1, 1];

/// Convert two proposed rotation columns to a row-major rotation matrix.
pub fn rot6d_to_rotmat(value: [f32; 6]) -> [[f32; 3]; 3] {
    let a1 = [value[0], value[1], value[2]];
    let a2 = [value[3], value[4], value[5]];
    let b1 = normalize3(a1);
    let projection = dot3(b1, a2);
    let b2 = normalize3([
        a2[0] - projection * b1[0],
        a2[1] - projection * b1[1],
        a2[2] - projection * b1[2],
    ]);
    let b3 = cross3(b1, b2);
    [
        [b1[0], b2[0], b3[0]],
        [b1[1], b2[1], b3[1]],
        [b1[2], b2[2], b3[2]],
    ]
}

/// Extract `(rx, ry, rz)` for `R = Rz(rz) * Ry(ry) * Rx(rx)`.
pub fn rotmat_to_euler_zyx(matrix: [[f32; 3]; 3]) -> [f32; 3] {
    let cy = (matrix[0][0] * matrix[0][0] + matrix[1][0] * matrix[1][0]).sqrt();
    let ry = (-matrix[2][0]).atan2(cy);
    if cy < 1.0e-6 {
        [(-matrix[1][2]).atan2(matrix[1][1]), ry, 0.0]
    } else {
        [
            matrix[2][1].atan2(matrix[2][2]),
            ry,
            matrix[1][0].atan2(matrix[0][0]),
        ]
    }
}

/// roma's lowercase `xyz` (verified against the hand-mode rig oracle):
/// `R = Rz(z) * Ry(y) * Rx(x)`, the same order the rig applies to every
/// joint's `(rx, ry, rz)` triple.
pub fn euler_xyz_to_matrix([x, y, z]: [f32; 3]) -> [[f32; 3]; 3] {
    let (sx, cx) = x.sin_cos();
    let (sy, cy) = y.sin_cos();
    let (sz, cz) = z.sin_cos();
    [
        [cz * cy, cz * sy * sx - sz * cx, cz * sy * cx + sz * sx],
        [sz * cy, sz * sy * sx + cz * cx, sz * sy * cx - cz * sx],
        [-sy, cy * sx, cy * cx],
    ]
}

/// Inverse of [`euler_xyz_to_matrix`], returning `(x, y, z)`.
pub fn matrix_to_euler_xyz(matrix: [[f32; 3]; 3]) -> [f32; 3] {
    let cy = (matrix[0][0] * matrix[0][0] + matrix[1][0] * matrix[1][0]).sqrt();
    let y = (-matrix[2][0]).atan2(cy);
    if cy < 1.0e-6 {
        [(-matrix[1][2]).atan2(matrix[1][1]), y, 0.0]
    } else {
        [
            matrix[2][1].atan2(matrix[2][2]),
            y,
            matrix[1][0].atan2(matrix[0][0]),
        ]
    }
}

/// Intrinsic `XZY`: input and output triples are convention ordered
/// `(x, z, y)`, and `R = Rx(x) * Rz(z) * Ry(y)`.
pub fn euler_xzy_to_matrix([x, z, y]: [f32; 3]) -> [[f32; 3]; 3] {
    let (sx, cx) = x.sin_cos();
    let (sy, cy) = y.sin_cos();
    let (sz, cz) = z.sin_cos();
    [
        [cz * cy, -sz, cz * sy],
        [cx * sz * cy + sx * sy, cx * cz, cx * sz * sy - sx * cy],
        [sx * sz * cy - cx * sy, sx * cz, sx * sz * sy + cx * cy],
    ]
}

/// Inverse of [`euler_xzy_to_matrix`], returning convention-ordered
/// `(x, z, y)`.
pub fn matrix_to_euler_xzy(matrix: [[f32; 3]; 3]) -> [f32; 3] {
    let cz = (matrix[0][0] * matrix[0][0] + matrix[0][2] * matrix[0][2]).sqrt();
    let z = (-matrix[0][1]).atan2(cz);
    if cz < 1.0e-6 {
        [(-matrix[1][2]).atan2(matrix[2][2]), z, 0.0]
    } else {
        [
            matrix[2][1].atan2(matrix[1][1]),
            z,
            matrix[0][2].atan2(matrix[0][0]),
        ]
    }
}

pub fn matrix_mul(a: [[f32; 3]; 3], b: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    std::array::from_fn(|row| {
        std::array::from_fn(|column| (0..3).map(|k| a[row][k] * b[k][column]).sum())
    })
}

pub fn matrix_transpose(value: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    std::array::from_fn(|row| std::array::from_fn(|column| value[column][row]))
}

/// Decode the 23 ball joints, 58 hinges, and six translations.
pub fn body_cont_to_model_params(value: &[f32; 260]) -> [f32; 133] {
    let mut output = [0.0; 133];
    for (joint, indices) in BODY_ROTATION_IDXS.iter().enumerate() {
        let offset = joint * 6;
        let euler = rotmat_to_euler_zyx(rot6d_to_rotmat([
            value[offset],
            value[offset + 1],
            value[offset + 2],
            value[offset + 3],
            value[offset + 4],
            value[offset + 5],
        ]));
        output[indices[0]] = euler[0];
        output[indices[1]] = euler[1];
        output[indices[2]] = euler[2];
    }
    let mut offset = 23 * 6;
    for &index in &BODY_HINGE_IDXS {
        output[index] = value[offset].atan2(value[offset + 1]);
        offset += 2;
    }
    output[124..130].copy_from_slice(&value[offset..offset + 6]);

    // Hand slots and the final decoder-owned rotation are supplied elsewhere.
    output[62..116].fill(0.0);
    output[130..133].fill(0.0);
    output
}

/// Decode one hand's continuous representation to its 27 scalar parameters.
pub fn hand_cont_to_model_params(value: &[f32; 54]) -> [f32; 27] {
    let mut output = [0.0; 27];
    let mut input_offset = 0;
    let mut output_offset = 0;
    for dofs in HAND_DOFS {
        if dofs == 3 {
            let euler = rotmat_to_euler_zyx(rot6d_to_rotmat([
                value[input_offset],
                value[input_offset + 1],
                value[input_offset + 2],
                value[input_offset + 3],
                value[input_offset + 4],
                value[input_offset + 5],
            ]));
            output[output_offset..output_offset + 3].copy_from_slice(&euler);
        } else {
            for dof in 0..dofs {
                output[output_offset + dof] =
                    value[input_offset + 2 * dof].atan2(value[input_offset + 2 * dof + 1]);
            }
        }
        input_offset += dofs * 2;
        output_offset += dofs;
    }
    debug_assert_eq!(input_offset, 54);
    debug_assert_eq!(output_offset, 27);
    output
}

#[derive(Clone, Debug)]
pub struct PoseHeadParams {
    pub global_rot: [f32; 3],
    pub body: [f32; 133],
    pub shape: [f32; 45],
    pub scale: [f32; 28],
    pub hands: [f32; 108],
    pub expr: [f32; 72],
}

/// Unpack the accumulated 519-value pose prediction.
pub fn unpack_pose(pred_519: &[f32]) -> PoseHeadParams {
    assert_eq!(pred_519.len(), 519, "pose head output must contain 519 values");
    // The reference decomposes the head's rotation with a library routine
    // that returns the Z, Y, X angles in that order, and the rig then reads
    // the triple positionally as (rx, ry, rz). The network was trained through
    // that pairing, so parity means handing the rig the reversed triple
    // (oracle-verified: the straight order is 3e-2 off, the reversed 1e-4).
    let zyx = rotmat_to_euler_zyx(rot6d_to_rotmat(pred_519[0..6].try_into().unwrap()));
    let global_rot = [zyx[2], zyx[1], zyx[0]];
    let body_cont: &[f32; 260] = pred_519[6..266].try_into().unwrap();
    let mut shape = [0.0; 45];
    shape.copy_from_slice(&pred_519[266..311]);
    let mut scale = [0.0; 28];
    scale.copy_from_slice(&pred_519[311..339]);
    let mut hands = [0.0; 108];
    hands.copy_from_slice(&pred_519[339..447]);

    // The released body path disables the expression channels.
    let expr = [0.0; 72];
    PoseHeadParams {
        global_rot,
        body: body_cont_to_model_params(body_cont),
        shape,
        scale,
        hands,
        expr,
    }
}

/// Assemble 136 pose values followed by 68 scale values.
pub fn model_params(rig: &MhrRig, pose: &PoseHeadParams) -> [f32; 204] {
    let mut output = [0.0; 204];
    output[3..6].copy_from_slice(&pose.global_rot);
    output[6..136].copy_from_slice(&pose.body[..130]);

    for hand in 0..2 {
        let input = &pose.hands[hand * 54..(hand + 1) * 54];
        let mut transformed = [0.0; 54];
        for column in 0..54 {
            let mut value = rig.hand_pose_mean[column];
            for row in 0..54 {
                value += input[row] * rig.hand_pose_comps[row * 54 + column];
            }
            transformed[column] = value;
        }
        let decoded = hand_cont_to_model_params(&transformed);
        let indices = if hand == 0 {
            &rig.hand_joint_idxs_left
        } else {
            &rig.hand_joint_idxs_right
        };
        for (value, &index) in decoded.iter().zip(indices) {
            output[index as usize] = *value;
        }
    }

    for column in 0..68 {
        let mut value = rig.scale_mean[column];
        for row in 0..28 {
            value += pose.scale[row] * rig.scale_comps[row * 68 + column];
        }
        output[136 + column] = value;
    }
    output
}

pub fn camera_translation(
    pred_cam: [f32; 3],
    bbox_center: [f32; 2],
    bbox_side: f32,
    focal: f32,
    principal: [f32; 2],
) -> [f32; 3] {
    camera_translation_scaled(pred_cam, bbox_center, bbox_side, focal, principal, 1.0)
}

/// [`camera_translation`] with the camera head's `default_scale_factor`
/// multiplying the box size (1 for the body head, 10 for the hand head).
pub fn camera_translation_scaled(
    pred_cam: [f32; 3],
    bbox_center: [f32; 2],
    bbox_side: f32,
    focal: f32,
    principal: [f32; 2],
    scale_factor: f32,
) -> [f32; 3] {
    let scale = -pred_cam[0];
    let ty = -pred_cam[2];
    let bbox_scale = bbox_side * scale * scale_factor + 1.0e-8;
    [
        pred_cam[1] + 2.0 * (bbox_center[0] - principal[0]) / bbox_scale,
        ty + 2.0 * (bbox_center[1] - principal[1]) / bbox_scale,
        2.0 * focal / bbox_scale,
    ]
}

/// Project row-major XYZ keypoints and return row-major UV plus camera depth.
pub fn project(
    kp3d: &[f32],
    cam_t: [f32; 3],
    focal: f32,
    principal: [f32; 2],
) -> (Vec<f32>, Vec<f32>) {
    assert_eq!(kp3d.len() % 3, 0, "3D keypoints must be XYZ triples");
    let count = kp3d.len() / 3;
    let mut kp2d = Vec::with_capacity(count * 2);
    let mut depth = Vec::with_capacity(count);
    for point in kp3d.chunks_exact(3) {
        let x = point[0] + cam_t[0];
        let y = point[1] + cam_t[1];
        let z = point[2] + cam_t[2];
        kp2d.push(focal * x / z + principal[0]);
        kp2d.push(focal * y / z + principal[1]);
        depth.push(z);
    }
    (kp2d, depth)
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize3(value: [f32; 3]) -> [f32; 3] {
    let inverse = 1.0 / dot3(value, value).sqrt().max(1.0e-12);
    [value[0] * inverse, value[1] * inverse, value[2] * inverse]
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture;
    use crate::weights::BodyWeights;
    use std::f32::consts::FRAC_PI_2;

    fn euler_matrix([rx, ry, rz]: [f32; 3]) -> [[f32; 3]; 3] {
        let (sx, cx) = rx.sin_cos();
        let (sy, cy) = ry.sin_cos();
        let (sz, cz) = rz.sin_cos();
        [
            [cz * cy, cz * sy * sx - sz * cx, cz * sy * cx + sz * sx],
            [sz * cy, sz * sy * sx + cz * cx, sz * sy * cx - cz * sx],
            [-sy, cy * sx, cy * cx],
        ]
    }

    fn rot6d_from_euler(euler: [f32; 3]) -> [f32; 6] {
        let matrix = euler_matrix(euler);
        [
            matrix[0][0],
            matrix[1][0],
            matrix[2][0],
            matrix[0][1],
            matrix[1][1],
            matrix[2][1],
        ]
    }

    fn assert_near(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < 2.0e-5, "{actual} != {expected}");
    }

    #[test]
    fn rotation_6d_and_zyx_euler_round_trip() {
        let identity = rot6d_to_rotmat([1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        assert_eq!(identity, [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);

        let expected = [0.31, -0.42, 0.73];
        let matrix = rot6d_to_rotmat(rot6d_from_euler(expected));
        let actual = rotmat_to_euler_zyx(matrix);
        for axis in 0..3 {
            assert_near(actual[axis], expected[axis]);
        }
    }

    #[test]
    fn zyx_extraction_uses_singular_guard() {
        let matrix = euler_matrix([0.35, FRAC_PI_2, -0.2]);
        let euler = rotmat_to_euler_zyx(matrix);
        assert_near(euler[1], FRAC_PI_2);
        assert_eq!(euler[2], 0.0);
        let rebuilt = euler_matrix(euler);
        for row in 0..3 {
            for column in 0..3 {
                assert_near(rebuilt[row][column], matrix[row][column]);
            }
        }
    }

    #[test]
    fn body_continuous_values_land_at_documented_indices() {
        let mut value = [0.0; 260];
        for joint in 0..23 {
            let euler = [0.01 * joint as f32, -0.02 * joint as f32, 0.03 * joint as f32];
            value[joint * 6..joint * 6 + 6].copy_from_slice(&rot6d_from_euler(euler));
        }
        for hinge in 0..58 {
            let angle = 0.005 * (hinge + 1) as f32;
            let (sin, cos) = angle.sin_cos();
            value[138 + hinge * 2] = sin;
            value[139 + hinge * 2] = cos;
        }
        value[254..260].copy_from_slice(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);

        let output = body_cont_to_model_params(&value);
        assert_near(output[0], 0.0);
        assert_near(output[34], 0.08);
        assert_near(output[35], -0.16);
        assert_near(output[36], 0.24);
        assert_near(output[1], 0.005);
        assert_eq!(&output[62..116], &[0.0; 54]);
        assert_near(output[116], 0.005 * 51.0);
        assert_near(output[123], 0.005 * 58.0);
        assert_eq!(&output[124..130], &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(&output[130..133], &[0.0; 3]);
    }

    #[test]
    fn hand_continuous_mixed_dofs_decode_in_order() {
        let mut value = [0.0; 54];
        let mut input = 0;
        for dofs in HAND_DOFS {
            if dofs == 3 {
                value[input..input + 6]
                    .copy_from_slice(&rot6d_from_euler([0.1, -0.2, 0.3]));
            } else {
                for dof in 0..dofs {
                    let angle = 0.04 * (dof + 1) as f32;
                    let (sin, cos) = angle.sin_cos();
                    value[input + 2 * dof] = sin;
                    value[input + 2 * dof + 1] = cos;
                }
            }
            input += 2 * dofs;
        }
        let output = hand_cont_to_model_params(&value);
        assert_near(output[0], 0.1);
        assert_near(output[1], -0.2);
        assert_near(output[2], 0.3);
        assert_near(output[3], 0.04);
        assert_near(output[20], 0.04);
        assert_near(output[21], 0.08);
        assert_near(output[22], 0.1);
    }

    #[test]
    fn camera_and_projection_follow_full_image_convention() {
        let translation = camera_translation([-2.0, 0.5, -0.25], [300.0, 220.0], 100.0, 500.0, [250.0, 200.0]);
        assert_near(translation[0], 1.0);
        assert_near(translation[1], 0.45);
        assert_near(translation[2], 5.0);
        let (points, depth) = project(&[1.0, 2.0, 5.0], translation, 500.0, [250.0, 200.0]);
        assert_near(points[0], 350.0);
        assert_near(points[1], 322.5);
        assert_near(depth[0], 10.0);
    }

    fn max_abs(left: &[f32], right: &[f32]) -> f32 {
        assert_eq!(left.len(), right.len());
        left.iter()
            .zip(right)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn oracle_pose_unpack_and_model_assembly() {
        let Some((_, projected)) = fixture::load("head_pose_proj_out_0") else {
            eprintln!("SKIP oracle_pose_unpack_and_model_assembly: pose fixture absent");
            return;
        };
        let Some((_, init_pose_fixture)) = fixture::load("init_pose") else {
            eprintln!("SKIP oracle_pose_unpack_and_model_assembly: init_pose.f32 absent");
            return;
        };
        let Some(weights_path) = fixture::weights_path() else {
            eprintln!("SKIP oracle_pose_unpack_and_model_assembly: weights absent");
            return;
        };
        let weights = BodyWeights::load(weights_path).expect("oracle weights must load");
        let init_pose = weights
            .f32_shaped("init_pose.weight", &[1, 519])
            .expect("init_pose.weight must load");
        assert!(
            max_abs(&init_pose, &init_pose_fixture) <= 1.0e-6,
            "oracle init_pose differs from weights"
        );
        assert_eq!(projected.len(), 519);
        let pred: Vec<f32> = projected
            .iter()
            .zip(&init_pose)
            .map(|(value, initial)| value + initial)
            .collect();
        let pose = unpack_pose(&pred);

        let Some((_, expected_global)) = fixture::load("mhr_in_global_rot_0") else {
            eprintln!("SKIP oracle_pose_unpack_and_model_assembly: MHR inputs absent");
            return;
        };
        let Some((_, expected_body)) = fixture::load("mhr_in_body_pose_params_0") else {
            eprintln!("SKIP oracle_pose_unpack_and_model_assembly: MHR inputs absent");
            return;
        };
        let Some((_, expected_scale)) = fixture::load("mhr_in_scale_params_0") else {
            eprintln!("SKIP oracle_pose_unpack_and_model_assembly: MHR inputs absent");
            return;
        };
        let Some((_, expected_shape)) = fixture::load("mhr_in_shape_params_0") else {
            eprintln!("SKIP oracle_pose_unpack_and_model_assembly: MHR inputs absent");
            return;
        };
        let Some((_, expected_hands)) = fixture::load("mhr_in_hand_pose_params_0") else {
            eprintln!("SKIP oracle_pose_unpack_and_model_assembly: MHR inputs absent");
            return;
        };
        let Some((_, expected_params)) = fixture::load("mhrjit_in_params_0") else {
            eprintln!("SKIP oracle_pose_unpack_and_model_assembly: MHR JIT input absent");
            return;
        };

        let global_error = max_abs(&pose.global_rot, &expected_global);
        eprintln!("pose oracle global rotation max abs error {global_error:.7}");
        assert!(global_error <= 1.0e-4, "global rotation triple order (see unpack_pose)");
        assert!(max_abs(&pose.body, &expected_body) <= 1.0e-4);
        assert!(max_abs(&pose.scale, &expected_scale) <= 1.0e-4);
        assert!(max_abs(&pose.shape, &expected_shape) <= 1.0e-4);
        assert!(max_abs(&pose.hands, &expected_hands) <= 1.0e-4);

        let rig = fixture::rig().expect("oracle rig must load after weights check");
        let model = model_params(&rig, &pose);
        let model_error = max_abs(&model, &expected_params);
        eprintln!("pose oracle MHR parameter max abs error {model_error:.7}");
        assert!(model_error <= 1.0e-4);
    }
}
