//! Exact host decoder for HY-Motion 1.0's 22-joint `o6dp` output.
//!
//! This mirrors the released pipeline's sigma-1 quaternion/Markley smoothing,
//! Savitzky-Golay translation filter, WoodenMesh forward kinematics and global
//! ground alignment. The final 66 network channels are intentionally ignored,
//! exactly as in the official 22-joint decoder.

use std::path::{Path, PathBuf};

use makepad_micro_serde::DeJson;

use crate::hy_motion::{
    hy_motion_rot6d_to_matrix, HY_MOTION_ACTIVE_OUTPUT_DIM, HY_MOTION_BODY_JOINTS,
    HY_MOTION_INPUT_DIM,
};
use crate::{DiffusionError, Result};

pub const HY_MOTION_WOODEN_JOINTS: usize = 52;
pub const HY_MOTION_WOODEN_INFLUENCES: usize = 4;
pub const HY_MOTION_ACTIVE_JOINT_NAMES: [&str; HY_MOTION_BODY_JOINTS] = [
    "Pelvis",
    "L_Hip",
    "R_Hip",
    "Spine1",
    "L_Knee",
    "R_Knee",
    "Spine2",
    "L_Ankle",
    "R_Ankle",
    "Spine3",
    "L_Foot",
    "R_Foot",
    "Neck",
    "L_Collar",
    "R_Collar",
    "Head",
    "L_Shoulder",
    "R_Shoulder",
    "L_Elbow",
    "R_Elbow",
    "L_Wrist",
    "R_Wrist",
];

/// Stable skeleton contract consumed by native retarget/export. The first 22
/// entries correspond one-for-one to the generated local rotations; the 30
/// finger joints retain identity local rotations in the official decoder.
#[derive(Clone, Debug)]
pub struct HyMotionSkeleton {
    pub joint_names: Vec<String>,
    pub rest_joints: Vec<[f32; 3]>,
    pub parents: Vec<i32>,
}

#[derive(Clone, Debug)]
pub struct HyMotionDecoded {
    pub frames: usize,
    /// Denormalized network rows (`frames x 201`).
    pub latent_denorm: Vec<f32>,
    /// Smoothed local rotations (`frames x 22 x 6`).
    pub rotations_6d: Vec<f32>,
    /// Smoothed, ground-aligned mesh translation (`frames x 3`). This is
    /// separate from the local Pelvis rotation and must be applied once.
    pub translations: Vec<f32>,
    /// Smoothed local matrices (`frames x 22 x 3 x 3`, row-major). Entry 0
    /// already is the root orientation; do not multiply it by itself again.
    pub local_rotation_matrices: Vec<f32>,
    /// Smoothed root matrices in row-major order (`frames x 3 x 3`).
    pub root_rotation_matrices: Vec<f32>,
    /// Official WoodenMesh FK joints (`frames x 52 x 3`), ground-aligned in
    /// Y. These mirror the released diagnostic keypoints and deliberately do
    /// not have `translations` added; consumers should use the local matrices
    /// plus the separate mesh translation for animation export.
    pub keypoints_3d: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct HyMotionWoodenModel {
    source: PathBuf,
    vertices: Vec<[f32; 3]>,
    skeleton: HyMotionSkeleton,
    skin_weights: Vec<[f32; HY_MOTION_WOODEN_INFLUENCES]>,
    skin_indices: Vec<[u16; HY_MOTION_WOODEN_INFLUENCES]>,
}

#[derive(Clone, Copy, Debug)]
struct Transform {
    rotation: [[f32; 3]; 3],
    translation: [f32; 3],
}

impl HyMotionWoodenModel {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let source = path.as_ref().to_path_buf();
        let vertex_values = read_f32_file(&source.join("v_template.bin"))?;
        let joint_values = read_f32_file(&source.join("j_template.bin"))?;
        let parents = read_i32_file(&source.join("kintree.bin"))?;
        let joint_names_path = source.join("joint_names.json");
        let joint_names_text = std::fs::read_to_string(&joint_names_path)
            .map_err(|error| DiffusionError::io(&joint_names_path, error.to_string()))?;
        let joint_names = Vec::<String>::deserialize_json(&joint_names_text)
            .map_err(|error| DiffusionError::json(&joint_names_path, format!("{error:?}")))?;
        let weight_values = read_f32_file(&source.join("skinWeights.bin"))?;
        let index_values = read_u16_file(&source.join("skinIndice.bin"))?;

        let vertices = to_vec3(&vertex_values, "v_template")?;
        let joints = to_vec3(&joint_values, "j_template")?;
        if joints.len() != HY_MOTION_WOODEN_JOINTS
            || parents.len() != HY_MOTION_WOODEN_JOINTS
            || joint_names.len() != HY_MOTION_WOODEN_JOINTS
            || parents.first().copied() != Some(-1)
        {
            return Err(DiffusionError::model(format!(
                "HY-Motion wooden skeleton contract mismatch: joints={} names={} parents={} root={:?}",
                joints.len(),
                joint_names.len(),
                parents.len(),
                parents.first()
            )));
        }
        if joint_names
            .iter()
            .take(HY_MOTION_BODY_JOINTS)
            .map(String::as_str)
            .ne(HY_MOTION_ACTIVE_JOINT_NAMES)
        {
            return Err(DiffusionError::model(
                "HY-Motion wooden active joint ordering does not match the 22-joint decoder",
            ));
        }
        for (joint, &parent) in parents.iter().enumerate().skip(1) {
            if parent < 0 || parent as usize >= joint {
                return Err(DiffusionError::model(format!(
                    "HY-Motion wooden parent {parent} is invalid for joint {joint}"
                )));
            }
        }
        let skin_weights = to_vec4_f32(&weight_values, "skinWeights")?;
        let skin_indices = to_vec4_u16(&index_values, "skinIndice")?;
        if skin_weights.len() != vertices.len() || skin_indices.len() != vertices.len() {
            return Err(DiffusionError::model(format!(
                "HY-Motion wooden skin contract mismatch: vertices={} weights={} indices={}",
                vertices.len(),
                skin_weights.len(),
                skin_indices.len()
            )));
        }
        if skin_indices
            .iter()
            .flatten()
            .any(|&index| index as usize >= HY_MOTION_WOODEN_JOINTS)
        {
            return Err(DiffusionError::model(
                "HY-Motion wooden skin index exceeds joint count",
            ));
        }
        Ok(Self {
            source,
            vertices,
            skeleton: HyMotionSkeleton {
                joint_names,
                rest_joints: joints,
                parents,
            },
            skin_weights,
            skin_indices,
        })
    }

    pub fn source(&self) -> &Path {
        &self.source
    }

    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    pub fn joint_count(&self) -> usize {
        self.skeleton.rest_joints.len()
    }

    pub fn skeleton(&self) -> &HyMotionSkeleton {
        &self.skeleton
    }

    pub fn decode_denormalized(
        &self,
        latent_denorm: &[f32],
        frames: usize,
        smooth: bool,
    ) -> Result<HyMotionDecoded> {
        if frames == 0 || latent_denorm.len() != frames * HY_MOTION_INPUT_DIM {
            return Err(DiffusionError::workflow(format!(
                "HY-Motion decoded latent shape mismatch: {} values for {frames} frames",
                latent_denorm.len()
            )));
        }

        let mut translations = vec![0.0f32; frames * 3];
        let mut rotations_6d = vec![0.0f32; frames * HY_MOTION_BODY_JOINTS * 6];
        for frame in 0..frames {
            let source = &latent_denorm
                [frame * HY_MOTION_INPUT_DIM..(frame + 1) * HY_MOTION_INPUT_DIM];
            translations[frame * 3..frame * 3 + 3].copy_from_slice(&source[..3]);
            rotations_6d[frame * HY_MOTION_BODY_JOINTS * 6
                ..(frame + 1) * HY_MOTION_BODY_JOINTS * 6]
                .copy_from_slice(&source[3..HY_MOTION_ACTIVE_OUTPUT_DIM]);
        }

        if smooth {
            rotations_6d = smooth_rotations_markley(&rotations_6d, frames)?;
            translations = savgol_smooth_translation(&translations, frames)?;
        }

        let mut root_rotation_matrices = vec![0.0f32; frames * 9];
        let mut local_rotation_matrices =
            vec![0.0f32; frames * HY_MOTION_BODY_JOINTS * 9];
        let mut keypoints_3d = vec![0.0f32; frames * HY_MOTION_WOODEN_JOINTS * 3];
        let mut global_min_y = f32::INFINITY;
        let identity = [[1.0f32, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

        for frame in 0..frames {
            let mut local_rotations = vec![identity; HY_MOTION_WOODEN_JOINTS];
            for (joint, rotation) in local_rotations
                .iter_mut()
                .take(HY_MOTION_BODY_JOINTS)
                .enumerate()
            {
                let offset = (frame * HY_MOTION_BODY_JOINTS + joint) * 6;
                let value = [
                    rotations_6d[offset],
                    rotations_6d[offset + 1],
                    rotations_6d[offset + 2],
                    rotations_6d[offset + 3],
                    rotations_6d[offset + 4],
                    rotations_6d[offset + 5],
                ];
                *rotation = hy_motion_rot6d_to_matrix(value);
            }
            for row in 0..3 {
                for col in 0..3 {
                    root_rotation_matrices[frame * 9 + row * 3 + col] =
                        local_rotations[0][row][col];
                }
            }
            for (joint, rotation) in local_rotations
                .iter()
                .take(HY_MOTION_BODY_JOINTS)
                .enumerate()
            {
                for row in 0..3 {
                    for col in 0..3 {
                        local_rotation_matrices
                            [((frame * HY_MOTION_BODY_JOINTS + joint) * 3 + row) * 3 + col] =
                            rotation[row][col];
                    }
                }
            }

            let transforms = self.forward_transforms(&local_rotations)?;
            for (joint, transform) in transforms.iter().enumerate() {
                let offset = (frame * HY_MOTION_WOODEN_JOINTS + joint) * 3;
                keypoints_3d[offset..offset + 3].copy_from_slice(&transform.translation);
            }

            let frame_translation = [
                translations[frame * 3],
                translations[frame * 3 + 1],
                translations[frame * 3 + 2],
            ];
            for (vertex, (&position, (weights, indices))) in self
                .vertices
                .iter()
                .zip(self.skin_weights.iter().zip(&self.skin_indices))
                .enumerate()
            {
                let mut skinned_y = 0.0f32;
                for influence in 0..HY_MOTION_WOODEN_INFLUENCES {
                    let joint = indices[influence] as usize;
                    let transform = relative_skin_transform(
                        transforms[joint],
                        self.skeleton.rest_joints[joint],
                    );
                    let transformed = transform_point(transform, position);
                    skinned_y += weights[influence] * transformed[1];
                }
                let world_y = skinned_y + frame_translation[1];
                if !world_y.is_finite() {
                    return Err(DiffusionError::model(format!(
                        "HY-Motion wooden vertex {vertex} became non-finite"
                    )));
                }
                global_min_y = global_min_y.min(world_y);
            }
        }

        if !global_min_y.is_finite() {
            return Err(DiffusionError::model(
                "HY-Motion wooden ground alignment found no finite vertex",
            ));
        }
        for translation in translations.chunks_exact_mut(3) {
            translation[1] -= global_min_y;
        }
        for keypoint in keypoints_3d.chunks_exact_mut(3) {
            keypoint[1] -= global_min_y;
        }

        Ok(HyMotionDecoded {
            frames,
            latent_denorm: latent_denorm.to_vec(),
            rotations_6d,
            translations,
            local_rotation_matrices,
            root_rotation_matrices,
            keypoints_3d,
        })
    }

    fn forward_transforms(&self, rotations: &[[[f32; 3]; 3]]) -> Result<Vec<Transform>> {
        if rotations.len() != HY_MOTION_WOODEN_JOINTS {
            return Err(DiffusionError::model(
                "HY-Motion wooden rotation count mismatch",
            ));
        }
        let mut transforms = Vec::with_capacity(HY_MOTION_WOODEN_JOINTS);
        for joint in 0..HY_MOTION_WOODEN_JOINTS {
            let relative = if joint == 0 {
                self.skeleton.rest_joints[0]
            } else {
                sub3(
                    self.skeleton.rest_joints[joint],
                    self.skeleton.rest_joints[self.skeleton.parents[joint] as usize],
                )
            };
            let local = Transform {
                rotation: rotations[joint],
                translation: relative,
            };
            transforms.push(if joint == 0 {
                local
            } else {
                compose_transform(transforms[self.skeleton.parents[joint] as usize], local)
            });
        }
        Ok(transforms)
    }
}

/// The exact released latent denormalization. Standard deviations below
/// `1e-3` are replaced with zero before the affine transform.
pub fn hy_motion_denormalize(
    latent: &[f32],
    frames: usize,
    mean: &[f32],
    std: &[f32],
) -> Result<Vec<f32>> {
    if frames == 0
        || latent.len() != frames * HY_MOTION_INPUT_DIM
        || mean.len() != HY_MOTION_INPUT_DIM
        || std.len() != HY_MOTION_INPUT_DIM
    {
        return Err(DiffusionError::workflow(format!(
            "HY-Motion denormalize shape mismatch: latent={} frames={frames} mean={} std={}",
            latent.len(),
            mean.len(),
            std.len()
        )));
    }
    Ok(latent
        .iter()
        .enumerate()
        .map(|(index, &value)| {
            let channel = index % HY_MOTION_INPUT_DIM;
            let scale = if std[channel] < 1.0e-3 {
                0.0
            } else {
                std[channel]
            };
            value * scale + mean[channel]
        })
        .collect())
}

fn smooth_rotations_markley(input: &[f32], frames: usize) -> Result<Vec<f32>> {
    let expected = frames * HY_MOTION_BODY_JOINTS * 6;
    if input.len() != expected {
        return Err(DiffusionError::model(
            "HY-Motion rotation smoothing shape mismatch",
        ));
    }
    let mut quaternions = vec![[0.0f32; 4]; frames * HY_MOTION_BODY_JOINTS];
    for frame in 0..frames {
        for joint in 0..HY_MOTION_BODY_JOINTS {
            let offset = (frame * HY_MOTION_BODY_JOINTS + joint) * 6;
            let matrix = hy_motion_rot6d_to_matrix([
                input[offset],
                input[offset + 1],
                input[offset + 2],
                input[offset + 3],
                input[offset + 4],
                input[offset + 5],
            ]);
            quaternions[frame * HY_MOTION_BODY_JOINTS + joint] = matrix_to_quaternion(matrix);
        }
    }

    // Released `quaternion_fix_continuity`: flips are accumulated from dot
    // products between the original consecutive representations.
    let raw_quaternions = quaternions.clone();
    for joint in 0..HY_MOTION_BODY_JOINTS {
        let mut flip = false;
        for frame in 1..frames {
            let previous = raw_quaternions[(frame - 1) * HY_MOTION_BODY_JOINTS + joint];
            let current = raw_quaternions[frame * HY_MOTION_BODY_JOINTS + joint];
            if dot4(previous, current) < 0.0 {
                flip = !flip;
            }
            if flip {
                quaternions[frame * HY_MOTION_BODY_JOINTS + joint] = negate4(current);
            }
        }
    }

    let weights = gaussian_kernel_sigma_one();
    let mut output = vec![0.0f32; expected];
    for frame in 0..frames {
        for joint in 0..HY_MOTION_BODY_JOINTS {
            let reference = quaternions[frame * HY_MOTION_BODY_JOINTS + joint];
            let mut window = [[0.0f32; 4]; 9];
            for (slot, value) in window.iter_mut().enumerate() {
                let source_frame = (frame as isize + slot as isize - 4)
                    .clamp(0, frames.saturating_sub(1) as isize)
                    as usize;
                let mut q = quaternions[source_frame * HY_MOTION_BODY_JOINTS + joint];
                if dot4(q, reference) < 0.0 {
                    q = negate4(q);
                }
                *value = q;
            }
            let average = markley_average(&window, &weights);
            let matrix = quaternion_to_matrix(average);
            let offset = (frame * HY_MOTION_BODY_JOINTS + joint) * 6;
            output[offset] = matrix[0][0];
            output[offset + 1] = matrix[0][1];
            output[offset + 2] = matrix[1][0];
            output[offset + 3] = matrix[1][1];
            output[offset + 4] = matrix[2][0];
            output[offset + 5] = matrix[2][1];
        }
    }
    Ok(output)
}

fn savgol_smooth_translation(input: &[f32], frames: usize) -> Result<Vec<f32>> {
    const WINDOW: usize = 11;
    if input.len() != frames * 3 || frames < WINDOW {
        return Err(DiffusionError::workflow(format!(
            "HY-Motion Savitzky-Golay requires at least {WINDOW} frames"
        )));
    }
    let weights: Vec<[f64; WINDOW]> = (0..WINDOW).map(savgol_weights).collect();
    let mut output = vec![0.0f32; input.len()];
    for frame in 0..frames {
        let (start, target) = if frame < WINDOW / 2 {
            (0, frame)
        } else if frame + WINDOW / 2 >= frames {
            (frames - WINDOW, frame - (frames - WINDOW))
        } else {
            (frame - WINDOW / 2, WINDOW / 2)
        };
        for axis in 0..3 {
            let mut value = 0.0f64;
            for sample in 0..WINDOW {
                value += weights[target][sample] * input[(start + sample) * 3 + axis] as f64;
            }
            output[frame * 3 + axis] = value as f32;
        }
    }
    Ok(output)
}

fn savgol_weights(target: usize) -> [f64; 11] {
    const DEGREE: usize = 5;
    let mut normal = [[0.0f64; DEGREE + 1]; DEGREE + 1];
    for sample in 0..11 {
        let x = sample as f64 - target as f64;
        let mut powers = [1.0f64; 2 * DEGREE + 1];
        for power in 1..powers.len() {
            powers[power] = powers[power - 1] * x;
        }
        for row in 0..=DEGREE {
            for col in 0..=DEGREE {
                normal[row][col] += powers[row + col];
            }
        }
    }
    let mut rhs = [0.0f64; DEGREE + 1];
    rhs[0] = 1.0;
    let coefficients = solve_6x6(normal, rhs);
    let mut weights = [0.0f64; 11];
    for sample in 0..11 {
        let x = sample as f64 - target as f64;
        let mut power = 1.0f64;
        for coefficient in coefficients {
            weights[sample] += coefficient * power;
            power *= x;
        }
    }
    weights
}

fn solve_6x6(mut matrix: [[f64; 6]; 6], mut rhs: [f64; 6]) -> [f64; 6] {
    for pivot in 0..6 {
        let best = (pivot..6)
            .max_by(|&left, &right| {
                matrix[left][pivot]
                    .abs()
                    .total_cmp(&matrix[right][pivot].abs())
            })
            .unwrap();
        if best != pivot {
            matrix.swap(best, pivot);
            rhs.swap(best, pivot);
        }
        let divisor = matrix[pivot][pivot];
        for col in pivot..6 {
            matrix[pivot][col] /= divisor;
        }
        rhs[pivot] /= divisor;
        for row in 0..6 {
            if row == pivot {
                continue;
            }
            let factor = matrix[row][pivot];
            for col in pivot..6 {
                matrix[row][col] -= factor * matrix[pivot][col];
            }
            rhs[row] -= factor * rhs[pivot];
        }
    }
    rhs
}

fn gaussian_kernel_sigma_one() -> [f64; 9] {
    let mut weights = [0.0f64; 9];
    let mut sum = 0.0f64;
    for (index, weight) in weights.iter_mut().enumerate() {
        let x = index as f64 - 4.0;
        *weight = (-0.5 * x * x).exp();
        sum += *weight;
    }
    for weight in &mut weights {
        *weight /= sum;
    }
    weights
}

fn markley_average(quaternions: &[[f32; 4]; 9], weights: &[f64; 9]) -> [f32; 4] {
    let mut matrix = [[0.0f64; 4]; 4];
    let mut weight_sum = 0.0f64;
    for (quaternion, &weight) in quaternions.iter().zip(weights) {
        let q = if quaternion[0] < 0.0 {
            negate4(*quaternion)
        } else {
            *quaternion
        };
        for row in 0..4 {
            for col in 0..4 {
                matrix[row][col] += weight * q[row] as f64 * q[col] as f64;
            }
        }
        weight_sum += weight;
    }
    for row in &mut matrix {
        for value in row {
            *value /= weight_sum;
        }
    }
    let eigenvector = largest_eigenvector_symmetric4(matrix);
    [
        eigenvector[0] as f32,
        eigenvector[1] as f32,
        eigenvector[2] as f32,
        eigenvector[3] as f32,
    ]
}

fn largest_eigenvector_symmetric4(mut matrix: [[f64; 4]; 4]) -> [f64; 4] {
    let mut vectors = [[0.0f64; 4]; 4];
    for (index, row) in vectors.iter_mut().enumerate() {
        row[index] = 1.0;
    }
    for _ in 0..64 {
        let mut p = 0usize;
        let mut q = 1usize;
        let mut maximum = matrix[p][q].abs();
        for row in 0..4 {
            for col in row + 1..4 {
                if matrix[row][col].abs() > maximum {
                    maximum = matrix[row][col].abs();
                    p = row;
                    q = col;
                }
            }
        }
        if maximum < 1.0e-15 {
            break;
        }
        let angle = 0.5 * (2.0 * matrix[p][q]).atan2(matrix[q][q] - matrix[p][p]);
        let cosine = angle.cos();
        let sine = angle.sin();
        let app = matrix[p][p];
        let aqq = matrix[q][q];
        let apq = matrix[p][q];
        matrix[p][p] = cosine * cosine * app - 2.0 * sine * cosine * apq
            + sine * sine * aqq;
        matrix[q][q] = sine * sine * app + 2.0 * sine * cosine * apq
            + cosine * cosine * aqq;
        matrix[p][q] = 0.0;
        matrix[q][p] = 0.0;
        for index in 0..4 {
            if index == p || index == q {
                continue;
            }
            let aip = matrix[index][p];
            let aiq = matrix[index][q];
            matrix[index][p] = cosine * aip - sine * aiq;
            matrix[p][index] = matrix[index][p];
            matrix[index][q] = sine * aip + cosine * aiq;
            matrix[q][index] = matrix[index][q];
        }
        for row in &mut vectors {
            let vip = row[p];
            let viq = row[q];
            row[p] = cosine * vip - sine * viq;
            row[q] = sine * vip + cosine * viq;
        }
    }
    let index = (0..4)
        .max_by(|&left, &right| matrix[left][left].total_cmp(&matrix[right][right]))
        .unwrap();
    let mut result = [
        vectors[0][index],
        vectors[1][index],
        vectors[2][index],
        vectors[3][index],
    ];
    let inverse_norm = 1.0 / result.iter().map(|value| value * value).sum::<f64>().sqrt();
    for value in &mut result {
        *value *= inverse_norm;
    }
    result
}

fn matrix_to_quaternion(matrix: [[f32; 3]; 3]) -> [f32; 4] {
    let m00 = matrix[0][0];
    let m01 = matrix[0][1];
    let m02 = matrix[0][2];
    let m10 = matrix[1][0];
    let m11 = matrix[1][1];
    let m12 = matrix[1][2];
    let m20 = matrix[2][0];
    let m21 = matrix[2][1];
    let m22 = matrix[2][2];
    let q_abs = [
        (1.0 + m00 + m11 + m22).max(0.0).sqrt(),
        (1.0 + m00 - m11 - m22).max(0.0).sqrt(),
        (1.0 - m00 + m11 - m22).max(0.0).sqrt(),
        (1.0 - m00 - m11 + m22).max(0.0).sqrt(),
    ];
    let candidates = [
        [q_abs[0] * q_abs[0], m21 - m12, m02 - m20, m10 - m01],
        [m21 - m12, q_abs[1] * q_abs[1], m10 + m01, m02 + m20],
        [m02 - m20, m10 + m01, q_abs[2] * q_abs[2], m12 + m21],
        [m10 - m01, m20 + m02, m21 + m12, q_abs[3] * q_abs[3]],
    ];
    let best = (0..4)
        .max_by(|&left, &right| q_abs[left].total_cmp(&q_abs[right]))
        .unwrap();
    let divisor = 2.0 * q_abs[best].max(0.1);
    let mut quaternion = [
        candidates[best][0] / divisor,
        candidates[best][1] / divisor,
        candidates[best][2] / divisor,
        candidates[best][3] / divisor,
    ];
    if quaternion[0] < 0.0 {
        quaternion = negate4(quaternion);
    }
    quaternion
}

fn quaternion_to_matrix(q: [f32; 4]) -> [[f32; 3]; 3] {
    let [r, i, j, k] = q;
    let two_s = 2.0 / dot4(q, q);
    [
        [
            1.0 - two_s * (j * j + k * k),
            two_s * (i * j - k * r),
            two_s * (i * k + j * r),
        ],
        [
            two_s * (i * j + k * r),
            1.0 - two_s * (i * i + k * k),
            two_s * (j * k - i * r),
        ],
        [
            two_s * (i * k - j * r),
            two_s * (j * k + i * r),
            1.0 - two_s * (i * i + j * j),
        ],
    ]
}

fn compose_transform(parent: Transform, local: Transform) -> Transform {
    Transform {
        rotation: multiply_matrix(parent.rotation, local.rotation),
        translation: add3(
            multiply_matrix_vector(parent.rotation, local.translation),
            parent.translation,
        ),
    }
}

fn relative_skin_transform(transform: Transform, rest_joint: [f32; 3]) -> Transform {
    Transform {
        rotation: transform.rotation,
        translation: sub3(
            transform.translation,
            multiply_matrix_vector(transform.rotation, rest_joint),
        ),
    }
}

fn transform_point(transform: Transform, point: [f32; 3]) -> [f32; 3] {
    add3(
        multiply_matrix_vector(transform.rotation, point),
        transform.translation,
    )
}

fn multiply_matrix(left: [[f32; 3]; 3], right: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut output = [[0.0f32; 3]; 3];
    for row in 0..3 {
        for col in 0..3 {
            for inner in 0..3 {
                output[row][col] += left[row][inner] * right[inner][col];
            }
        }
    }
    output
}

fn multiply_matrix_vector(matrix: [[f32; 3]; 3], vector: [f32; 3]) -> [f32; 3] {
    [
        matrix[0][0] * vector[0]
            + matrix[0][1] * vector[1]
            + matrix[0][2] * vector[2],
        matrix[1][0] * vector[0]
            + matrix[1][1] * vector[1]
            + matrix[1][2] * vector[2],
        matrix[2][0] * vector[0]
            + matrix[2][1] * vector[1]
            + matrix[2][2] * vector[2],
    ]
}

fn add3(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [left[0] + right[0], left[1] + right[1], left[2] + right[2]]
}

fn sub3(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}

fn dot4(left: [f32; 4], right: [f32; 4]) -> f32 {
    left[0] * right[0] + left[1] * right[1] + left[2] * right[2] + left[3] * right[3]
}

fn negate4(value: [f32; 4]) -> [f32; 4] {
    [-value[0], -value[1], -value[2], -value[3]]
}

fn read_f32_file(path: &Path) -> Result<Vec<f32>> {
    let bytes = read_aligned_file(path, 4)?;
    Ok(bytes
        .chunks_exact(4)
        .map(|value| f32::from_le_bytes([value[0], value[1], value[2], value[3]]))
        .collect())
}

fn read_i32_file(path: &Path) -> Result<Vec<i32>> {
    let bytes = read_aligned_file(path, 4)?;
    Ok(bytes
        .chunks_exact(4)
        .map(|value| i32::from_le_bytes([value[0], value[1], value[2], value[3]]))
        .collect())
}

fn read_u16_file(path: &Path) -> Result<Vec<u16>> {
    let bytes = read_aligned_file(path, 2)?;
    Ok(bytes
        .chunks_exact(2)
        .map(|value| u16::from_le_bytes([value[0], value[1]]))
        .collect())
}

fn read_aligned_file(path: &Path, alignment: usize) -> Result<Vec<u8>> {
    let bytes = std::fs::read(path).map_err(|error| DiffusionError::io(path, error.to_string()))?;
    if bytes.len() % alignment != 0 {
        return Err(DiffusionError::model(format!(
            "HY-Motion wooden file {} is not {alignment}-byte aligned",
            path.display()
        )));
    }
    Ok(bytes)
}

fn to_vec3(values: &[f32], name: &str) -> Result<Vec<[f32; 3]>> {
    if values.len() % 3 != 0 {
        return Err(DiffusionError::model(format!(
            "HY-Motion wooden {name} is not vec3-aligned"
        )));
    }
    Ok(values
        .chunks_exact(3)
        .map(|value| [value[0], value[1], value[2]])
        .collect())
}

fn to_vec4_f32(values: &[f32], name: &str) -> Result<Vec<[f32; 4]>> {
    if values.len() % 4 != 0 {
        return Err(DiffusionError::model(format!(
            "HY-Motion wooden {name} is not vec4-aligned"
        )));
    }
    Ok(values
        .chunks_exact(4)
        .map(|value| [value[0], value[1], value[2], value[3]])
        .collect())
}

fn to_vec4_u16(values: &[u16], name: &str) -> Result<Vec<[u16; 4]>> {
    if values.len() % 4 != 0 {
        return Err(DiffusionError::model(format!(
            "HY-Motion wooden {name} is not vec4-aligned"
        )));
    }
    Ok(values
        .chunks_exact(4)
        .map(|value| [value[0], value[1], value[2], value[3]])
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gaussian_kernel_matches_sigma_one_contract() {
        let weights = gaussian_kernel_sigma_one();
        assert!((weights.iter().sum::<f64>() - 1.0).abs() < 1.0e-14);
        for index in 0..4 {
            assert_eq!(weights[index], weights[8 - index]);
        }
    }

    #[test]
    fn savgol_reproduces_degree_five_polynomials_including_edges() {
        for target in 0..11 {
            let weights = savgol_weights(target);
            for degree in 0..=5 {
                let actual = (0..11)
                    .map(|sample| {
                        weights[sample]
                            * (sample as f64 - target as f64).powi(degree as i32)
                    })
                    .sum::<f64>();
                let expected = if degree == 0 { 1.0 } else { 0.0 };
                assert!((actual - expected).abs() < 1.0e-8);
            }
        }
    }

    #[test]
    fn markley_average_of_identical_quaternions_is_same_rotation() {
        let quaternion = [0.9238795, 0.0, 0.38268343, 0.0];
        let quaternions = [quaternion; 9];
        let average = markley_average(&quaternions, &gaussian_kernel_sigma_one());
        let expected = quaternion_to_matrix(quaternion);
        let actual = quaternion_to_matrix(average);
        for row in 0..3 {
            for col in 0..3 {
                assert!((actual[row][col] - expected[row][col]).abs() < 1.0e-6);
            }
        }
    }

    #[test]
    fn denormalization_zeroes_tiny_standard_deviation() {
        let mut latent = vec![0.0f32; HY_MOTION_INPUT_DIM];
        latent[0] = 2.0;
        latent[1] = 3.0;
        let mut mean = vec![0.0f32; HY_MOTION_INPUT_DIM];
        mean[0] = 7.0;
        let mut std = vec![1.0f32; HY_MOTION_INPUT_DIM];
        std[0] = 0.0009;
        std[1] = 2.0;
        let output = hy_motion_denormalize(&latent, 1, &mean, &std).unwrap();
        assert_eq!(output[0], 7.0);
        assert_eq!(output[1], 6.0);
    }
}
