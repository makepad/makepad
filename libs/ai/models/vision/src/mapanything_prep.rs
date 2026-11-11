//! Host-side MapAnything input preparation and output geometry.
//! Mirrors the Apache-2.0 MapAnything geometry.py and inference.py reference.
//! Cameras use OpenCV right/down/forward axes, camera-to-world poses, and xyzw quaternions.

fn image_len(width: usize, height: usize, channels: usize) -> Result<usize, String> {
    width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(channels))
        .ok_or_else(|| "image dimensions overflow usize".to_string())
}

fn check_ray_depth_lengths(rays: &[f32], depth: &[f32]) -> Result<(), String> {
    if depth.len().checked_mul(3) != Some(rays.len()) {
        return Err("ray map must contain exactly three components per depth value".to_string());
    }
    Ok(())
}

/// Produces planar ImageNet-normalized RGB expected by `preprocess_input_views_for_inference`.
pub fn normalize_rgb8(rgb: &[u8], width: usize, height: usize) -> Result<Vec<f32>, String> {
    let len = image_len(width, height, 3)?;
    if rgb.len() != len {
        return Err(format!("RGB length {} does not match expected {len}", rgb.len()));
    }
    let pixels = len / 3;
    let mean = [0.485, 0.456, 0.406];
    let std = [0.229, 0.224, 0.225];
    let mut result = vec![0.0; len];
    for (i, pixel) in rgb.chunks_exact(3).enumerate() {
        for c in 0..3 {
            result[c * pixels + i] = ((f64::from(pixel[c]) / 255.0 - mean[c]) / std[c]) as f32;
        }
    }
    Ok(result)
}

/// Generates unit rays at integer pixels, mirroring `get_rays_in_camera_frame` with sphere normalization.
pub fn rays_from_pinhole(
    fx: f32,
    fy: f32,
    cx: f32,
    cy: f32,
    width: usize,
    height: usize,
) -> Vec<f32> {
    let [fx, fy, cx, cy] = [fx, fy, cx, cy].map(f64::from);
    let mut rays = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let dx = (x as f64 - cx) / fx;
            let dy = (y as f64 - cy) / fy;
            let norm = (dx * dx + dy * dy + 1.0).sqrt();
            rays.extend_from_slice(&[(dx / norm) as f32, (dy / norm) as f32, (1.0 / norm) as f32]);
        }
    }
    rays
}

/// Normalizes each complete xyz ray as in `preprocess_input_views_for_inference`; trailing values stay unchanged.
pub fn normalize_rays(rays: &mut [f32]) {
    for ray in rays.chunks_exact_mut(3) {
        let norm = ray.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>().sqrt() + 1e-8;
        for v in ray {
            *v = (f64::from(*v) / norm) as f32;
        }
    }
}

/// Converts z depth to nonnegative ray distance, mirroring `preprocess_input_views_for_inference`.
pub fn depth_z_to_along_ray(depth_z: &[f32], rays: &[f32]) -> Result<Vec<f32>, String> {
    check_ray_depth_lengths(rays, depth_z)?;
    Ok(depth_z
        .iter()
        .zip(rays.chunks_exact(3))
        .map(|(&depth, ray)| {
            ray.iter()
                .map(|&v| (f64::from(depth) * (f64::from(v) / f64::from(ray[2]))).powi(2))
                .sum::<f64>()
                .sqrt() as f32
        })
        .collect())
}

/// Converts a rotation to xyzw using `rotation_matrix_to_quaternion` and `standardize_quaternion`.
pub fn rotation_to_quaternion(m: [[f32; 3]; 3]) -> [f32; 4] {
    let [[m00, m01, m02], [m10, m11, m12], [m20, m21, m22]] = m.map(|row| row.map(f64::from));
    let q_abs = [
        1.0 + m00 + m11 + m22,
        1.0 + m00 - m11 - m22,
        1.0 - m00 + m11 - m22,
        1.0 - m00 - m11 + m22,
    ]
    .map(|v| if v > 0.0 { v.sqrt() } else { 0.0 });
    let candidates = [
        [q_abs[0].powi(2), m21 - m12, m02 - m20, m10 - m01],
        [m21 - m12, q_abs[1].powi(2), m10 + m01, m02 + m20],
        [m02 - m20, m10 + m01, q_abs[2].powi(2), m12 + m21],
        [m10 - m01, m20 + m02, m21 + m12, q_abs[3].powi(2)],
    ];
    // Strict comparison preserves torch.argmax's first-candidate tie break.
    let mut best = 0;
    for i in 1..4 {
        if q_abs[i] > q_abs[best] {
            best = i;
        }
    }
    let [w, x, y, z] = candidates[best].map(|v| v / (2.0 * q_abs[best].max(0.1)));
    let sign = if w < 0.0 { -1.0 } else { 1.0 };
    [x, y, z, w].map(|v| (sign * v) as f32)
}

fn rotation_f64(q: [f64; 4]) -> [[f64; 3]; 3] {
    let norm = q.iter().map(|v| v * v).sum::<f64>().sqrt();
    let [x, y, z, w] = q.map(|v| v / norm);
    [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - w * z), 2.0 * (x * z + w * y)],
        [2.0 * (x * y + w * z), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - w * x)],
        [2.0 * (x * z - w * y), 2.0 * (y * z + w * x), 1.0 - 2.0 * (x * x + y * y)],
    ]
}

/// Normalizes an xyzw quaternion and converts it as in `quaternion_to_rotation_matrix`.
pub fn quaternion_to_rotation(q: [f32; 4]) -> [[f32; 3]; 3] {
    rotation_f64(q.map(f64::from)).map(|row| row.map(|v| v as f32))
}

fn multiply_f64(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    let [x1, y1, z1, w1] = a;
    let [x2, y2, z2, w2] = b;
    [
        w1 * x2 + x1 * w2 + y1 * z2 - z1 * y2,
        w1 * y2 - x1 * z2 + y1 * w2 + z1 * x2,
        w1 * z2 + x1 * y2 - y1 * x2 + z1 * w2,
        w1 * w2 - x1 * x2 - y1 * y2 - z1 * z2,
    ]
}

/// Multiplies xyzw quaternions in order, mirroring `quaternion_multiply` without normalization.
pub fn quaternion_multiply(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    multiply_f64(a.map(f64::from), b.map(f64::from)).map(|v| v as f32)
}

fn inverse_f64(q: [f64; 4]) -> [f64; 4] {
    let norm_squared = q.iter().map(|v| v * v).sum::<f64>();
    let [x, y, z, w] = q;
    [-x, -y, -z, w].map(|v| v / norm_squared)
}

/// Computes conjugate divided by squared norm, mirroring `quaternion_inverse`.
pub fn quaternion_inverse(q: [f32; 4]) -> [f32; 4] {
    inverse_f64(q.map(f64::from)).map(|v| v as f32)
}

fn rotate_f64(m: [[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    m.map(|row| row[0] * v[0] + row[1] * v[1] + row[2] * v[2])
}

/// Expresses pose 2 in pose 1's frame, mirroring `transform_pose_using_quats_and_trans_2_to_1`.
pub fn pose_in_frame_of(
    q1: [f32; 4],
    t1: [f32; 3],
    q2: [f32; 4],
    t2: [f32; 3],
) -> ([f32; 4], [f32; 3]) {
    let inv_q1 = inverse_f64(q1.map(f64::from));
    let rotation = rotation_f64(inv_q1);
    let rotated_t1 = rotate_f64(rotation, t1.map(f64::from));
    let rotated_t2 = rotate_f64(rotation, t2.map(f64::from));
    let q = multiply_f64(inv_q1, q2.map(f64::from)).map(|v| v as f32);
    let t = std::array::from_fn(|i| (rotated_t2[i] - rotated_t1[i]) as f32);
    (q, t)
}

/// Builds world points as in `convert_ray_dirs_depth_along_ray_pose_trans_quats_to_pointmap`.
pub fn rays_depth_pose_to_points(
    rays: &[f32],
    depth_along_ray: &[f32],
    q: [f32; 4],
    t: [f32; 3],
) -> Result<Vec<f32>, String> {
    check_ray_depth_lengths(rays, depth_along_ray)?;
    let q = q.map(f64::from);
    let norm = q.iter().map(|v| v * v).sum::<f64>().sqrt();
    let rotation = rotation_f64(q.map(|v| v / norm));
    let mut points = Vec::with_capacity(rays.len());
    for (ray, &depth) in rays.chunks_exact(3).zip(depth_along_ray) {
        let local = std::array::from_fn(|i| f64::from(ray[i]) * f64::from(depth));
        let world = rotate_f64(rotation, local);
        points.extend((0..3).map(|i| (world[i] + f64::from(t[i])) as f32));
    }
    Ok(points)
}

/// Recovers (fx, fy, cx, cy) using `recover_pinhole_intrinsics_from_ray_directions`' sampled normal equations.
/// Rejects maps above 1,000,000 pixels and singular or nonfinite sampled systems.
pub fn pinhole_from_rays(rays: &[f32], width: usize, height: usize) -> Result<[f32; 4], String> {
    let pixels = image_len(width, height, 1)?;
    if pixels > 1_000_000 {
        return Err("pinhole recovery supports at most 1,000,000 pixels (least-squares branch only)".to_string());
    }
    let len = image_len(width, height, 3)?;
    if rays.len() != len {
        return Err(format!("ray length {} does not match expected {len}", rays.len()));
    }
    // A = [1, ray_axis / ray_z]; accumulate A^T A and A^T pixel per axis.
    let mut count = 0.0;
    let mut sum_ratio = [0.0; 2];
    let mut sum_squared = [0.0; 2];
    let mut sum_pixel = [0.0; 2];
    let mut sum_product = [0.0; 2];
    for y in (0..height).step_by((height / 50).max(1)) {
        for x in (0..width).step_by((width / 50).max(1)) {
            let ray = &rays[3 * (y * width + x)..][..3];
            let pixel = [x as f64, y as f64];
            count += 1.0;
            for axis in 0..2 {
                let ratio = f64::from(ray[axis]) / f64::from(ray[2]);
                sum_ratio[axis] += ratio;
                sum_squared[axis] += ratio * ratio;
                sum_pixel[axis] += pixel[axis];
                sum_product[axis] += ratio * pixel[axis];
            }
        }
    }
    let mut result = [0.0; 4];
    for axis in 0..2 {
        let determinant = count * sum_squared[axis] - sum_ratio[axis] * sum_ratio[axis];
        if determinant == 0.0 || !determinant.is_finite() {
            return Err(format!("singular or nonfinite pinhole least-squares system on axis {axis}"));
        }
        let focal = (count * sum_product[axis] - sum_ratio[axis] * sum_pixel[axis]) / determinant;
        let center = (sum_squared[axis] * sum_pixel[axis] - sum_ratio[axis] * sum_product[axis]) / determinant;
        result[axis] = focal as f32;
        result[axis + 2] = center as f32;
    }
    if !result.iter().all(|v| v.is_finite()) {
        return Err("pinhole least-squares solution is not finite in f32".to_string());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(actual: &[f32], expected: &[f32], tolerance: f32) {
        assert_eq!(actual.len(), expected.len());
        for (i, (&a, &e)) in actual.iter().zip(expected).enumerate() {
            assert!((a - e).abs() <= tolerance, "component {i}: {a} != {e}, tolerance {tolerance}");
        }
    }

    fn assert_rotation(actual: [[f32; 3]; 3], expected: [[f32; 3]; 3], tolerance: f32) {
        for (a, e) in actual.iter().zip(expected) {
            assert_close(a, &e, tolerance);
        }
    }

    #[test]
    fn rgb_black_and_white_match_imagenet_in_planar_order() {
        let output = normalize_rgb8(&[0, 0, 0, 255, 255, 255], 2, 1).expect("valid RGB");
        assert_close(&output, &[-2.117904, 2.248908, -2.035714, 2.428571, -1.804444, 2.64], 2e-6);
    }

    #[test]
    fn rgb_rejects_length_mismatch_and_dimension_overflow() {
        assert!(normalize_rgb8(&[0; 5], 2, 1).is_err());
        assert!(normalize_rgb8(&[0; 7], 2, 1).is_err());
        assert!(normalize_rgb8(&[], usize::MAX, 2).is_err());
        assert!(normalize_rgb8(&[], usize::MAX / 2, 1).is_err());
        assert_eq!(normalize_rgb8(&[], 0, 1), Ok(vec![]));
    }

    #[test]
    fn pinhole_rays_match_reference_at_integer_pixels() {
        let rays = rays_from_pinhole(400.0, 400.0, 259.0, 259.0, 518, 518);
        assert_eq!(rays.len(), 3 * 518 * 518);
        for (pixel, expected) in [
            (0, [-0.47753638, -0.47753638, 0.73750794]),
            (1, [-0.4761112, -0.4779566, 0.7381569]),
            (300 * 518 + 300, [0.10143978, 0.10143978, 0.98965645]),
        ] {
            assert_close(&rays[3 * pixel..3 * pixel + 3], &expected, 2e-6);
        }
    }

    #[test]
    fn provided_rays_use_additive_epsilon_and_preserve_zero() {
        let mut rays = [3.0, 4.0, 0.0, 0.0, 0.0, 0.0, 1e-8, 0.0, 0.0];
        normalize_rays(&mut rays);
        assert_close(&rays, &[0.6, 0.8, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0], 2e-6);
    }

    #[test]
    fn z_depth_matches_reference_ray_distances() {
        let rays = rays_from_pinhole(400.0, 400.0, 259.0, 259.0, 518, 518);
        for (pixel, depth, expected) in [(0, 0.3045974, 0.41300896), (300 * 518 + 300, 0.7860918, 0.79430777)] {
            let ray = &rays[3 * pixel..3 * pixel + 3];
            let output = depth_z_to_along_ray(&[depth], ray).expect("matching lengths");
            assert_close(&output, &[expected], 1e-6 * expected);
        }
        assert_close(&depth_z_to_along_ray(&[-2.0], &[3.0, 0.0, 4.0]).expect("nonunit ray"), &[2.5], 2e-6);
    }

    #[test]
    fn depth_conversions_reject_mismatched_maps() {
        for rays in [&[1.0, 2.0][..], &[1.0, 2.0, 3.0, 4.0][..], &[][..]] {
            assert!(depth_z_to_along_ray(&[1.0], rays).is_err());
            assert!(rays_depth_pose_to_points(rays, &[1.0], [0.0, 0.0, 0.0, 1.0], [0.0; 3]).is_err());
        }
    }

    #[test]
    fn rotation_quaternions_match_reference_sign_and_components() {
        let cases = [
            ([[-0.82533561, 0.27421463, 0.49358633], [0.0, -0.87415728, 0.48564293], [0.56464247, 0.40081841, 0.72147313]], [-0.2860719, -0.23963785, -0.9247927, 0.07412867]),
            ([[-0.98006658, 0.09648236, 0.17366824], [0.0, -0.87415728, 0.48564293], [0.19866933, 0.47596241, 0.85673233]], [-0.09664157, -0.24958815, -0.9631921, 0.02504235]),
            ([[-0.98006658, -0.09648236, -0.17366824], [0.0, -0.87415728, 0.48564293], [-0.19866933, 0.47596241, 0.85673233]], [-0.09664157, 0.24958815, 0.9631921, 0.02504235]),
        ];
        for (matrix, expected) in cases {
            assert_close(&rotation_to_quaternion(matrix), &expected, 1e-6);
        }
    }

    #[test]
    fn identity_and_half_turns_cover_all_quaternion_candidates() {
        for (diagonal, expected) in [
            ([1.0, 1.0, 1.0], [0.0, 0.0, 0.0, 1.0]),
            ([1.0, -1.0, -1.0], [1.0, 0.0, 0.0, 0.0]),
            ([-1.0, 1.0, -1.0], [0.0, 1.0, 0.0, 0.0]),
            ([-1.0, -1.0, 1.0], [0.0, 0.0, 1.0, 0.0]),
        ] {
            let matrix = [[diagonal[0], 0.0, 0.0], [0.0, diagonal[1], 0.0], [0.0, 0.0, diagonal[2]]];
            let q = rotation_to_quaternion(matrix);
            assert!(q[3] >= 0.0, "standardized scalar component");
            assert_close(&q, &expected, 1e-6);
            assert_rotation(quaternion_to_rotation(q), matrix, 1e-6);
        }
    }

    #[test]
    fn fifty_seeded_axis_angle_rotations_round_trip() {
        let mut state = 0x1234_5678_u32;
        let mut next = || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            f64::from(state) / f64::from(u32::MAX) * 2.0 - 1.0
        };
        for _ in 0..50 {
            let axis = [next(), next(), next()];
            let norm = axis.iter().map(|v| v * v).sum::<f64>().sqrt();
            let [x, y, z] = axis.map(|v| v / norm);
            let (s, c) = (next() * std::f64::consts::PI).sin_cos();
            let d = 1.0 - c;
            // Rodrigues' formula supplies a matrix independently of quaternion conversion.
            let matrix = [
                [c + x * x * d, x * y * d - z * s, x * z * d + y * s],
                [y * x * d + z * s, c + y * y * d, y * z * d - x * s],
                [z * x * d - y * s, z * y * d + x * s, c + z * z * d],
            ].map(|row| row.map(|v| v as f32));
            let q = rotation_to_quaternion(matrix);
            assert!(q[3] >= 0.0);
            assert_rotation(quaternion_to_rotation(q), matrix, 1e-5);
        }
    }

    #[test]
    fn quaternion_inverse_handles_nonunit_norm_and_multiply_preserves_order() {
        let q = [1.0, -2.0, 3.0, 4.0];
        assert_close(&quaternion_inverse(q), &[-1.0 / 30.0, 2.0 / 30.0, -3.0 / 30.0, 4.0 / 30.0], 2e-6);
        assert_close(&quaternion_multiply(q, quaternion_inverse(q)), &[0.0, 0.0, 0.0, 1.0], 2e-6);
        assert_close(&quaternion_multiply([1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0]), &[0.0, 0.0, 1.0, 0.0], 2e-6);
        assert_close(&quaternion_multiply([0.0, 1.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0]), &[0.0, 0.0, -1.0, 0.0], 2e-6);
        assert_rotation(quaternion_to_rotation([0.0, 0.0, 2.0, 2.0]), [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]], 1e-6);
    }

    #[test]
    fn pose_in_its_own_frame_is_identity() {
        let q = [0.2, -0.3, 0.4, 0.5];
        let t = [3.0, -2.0, 1.0];
        let (relative_q, relative_t) = pose_in_frame_of(q, t, q, t);
        assert_close(&relative_q, &[0.0, 0.0, 0.0, 1.0], 2e-6);
        assert_close(&relative_t, &[0.0; 3], 2e-6);
    }

    #[test]
    fn relative_pose_matches_direct_homogeneous_matrix_composition() {
        // Independent, exact quarter turns about x and z.
        let m1: [[f64; 4]; 4] = [[1.0, 0.0, 0.0, 2.0], [0.0, 0.0, -1.0, -3.0], [0.0, 1.0, 0.0, 4.0], [0.0, 0.0, 0.0, 1.0]];
        let m2: [[f64; 4]; 4] = [[0.0, -1.0, 0.0, -5.0], [1.0, 0.0, 0.0, 6.0], [0.0, 0.0, 1.0, 7.0], [0.0, 0.0, 0.0, 1.0]];
        let mut inv1 = [[0.0; 4]; 4];
        for i in 0..3 {
            for j in 0..3 {
                inv1[i][j] = m1[j][i];
            }
            inv1[i][3] = -(0..3).map(|j| inv1[i][j] * m1[j][3]).sum::<f64>();
        }
        inv1[3][3] = 1.0;
        let expected: [[f64; 4]; 4] = std::array::from_fn(|i| std::array::from_fn(|j| (0..4).map(|k| inv1[i][k] * m2[k][j]).sum()));
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let (q, t) = pose_in_frame_of([s, 0.0, 0.0, s], [2.0, -3.0, 4.0], [0.0, 0.0, s, s], [-5.0, 6.0, 7.0]);
        let rotation = quaternion_to_rotation(q);
        for i in 0..3 {
            assert_close(&rotation[i], &[expected[i][0] as f32, expected[i][1] as f32, expected[i][2] as f32], 2e-6);
            assert_close(&[t[i]], &[expected[i][3] as f32], 2e-6);
        }
    }

    #[test]
    fn identity_pose_points_are_ray_times_depth() {
        let points = rays_depth_pose_to_points(&[0.6, 0.0, 0.8, 0.0, -1.0, 0.0], &[2.0, 3.0], [0.0, 0.0, 0.0, 1.0], [0.0; 3]).expect("valid maps");
        assert_close(&points, &[1.2, 0.0, 1.6, 0.0, -3.0, 0.0], 2e-6);
    }

    #[test]
    fn points_apply_z_quarter_turn_then_translation() {
        let points = rays_depth_pose_to_points(&[0.6, 0.0, 0.8, 0.0, -1.0, 0.0], &[2.0, 3.0], [0.0, 0.0, 2.0, 2.0], [1.0, 2.0, 3.0]).expect("valid maps");
        assert_close(&points, &[1.0, 3.2, 4.6, 4.0, 2.0, 3.0], 2e-6);
    }

    #[test]
    fn pinhole_recovery_matches_asymmetric_intrinsics() {
        let rays = rays_from_pinhole(400.0, 410.0, 259.0, 251.0, 518, 518);
        let intrinsics = pinhole_from_rays(&rays, 518, 518).expect("recoverable pinhole");
        assert_close(&intrinsics, &[400.0, 410.0, 259.0, 251.0], 1e-2);
    }

    #[test]
    fn pinhole_recovery_uses_only_reference_sample_grid() {
        let mut rays = rays_from_pinhole(90.0, 80.0, 31.0, 27.0, 101, 103);
        for y in 0..103 {
            for x in 0..101 {
                if x % 2 != 0 || y % 2 != 0 {
                    rays[3 * (y * 101 + x)..3 * (y * 101 + x) + 3].fill(f32::NAN);
                }
            }
        }
        assert_close(&pinhole_from_rays(&rays, 101, 103).expect("unsampled pixels ignored"), &[90.0, 80.0, 31.0, 27.0], 1e-2);
    }

    #[test]
    fn pinhole_recovery_rejects_high_resolution_and_invalid_systems() {
        let error = pinhole_from_rays(&vec![0.0; 3 * 1001 * 1000], 1001, 1000).expect_err("unsupported high resolution");
        assert!(error.contains("1,000,000"));
        assert!(pinhole_from_rays(&[], 2, 2).is_err());
        assert!(pinhole_from_rays(&[], usize::MAX, 2).is_err());
        assert!(pinhole_from_rays(&[], 0, 0).is_err());
        assert!(pinhole_from_rays(&[0.0, 0.0, 1.0].repeat(4), 2, 2).is_err());
        assert!(pinhole_from_rays(&[f32::NAN; 12], 2, 2).is_err());
    }
}
