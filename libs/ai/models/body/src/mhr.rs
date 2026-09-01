//! The Momentum Human Rig forward pass: kinematics, skinning and keypoint
//! regression on the CPU (small), the two dense products — identity
//! blendshapes and the pose-corrective output layer — on the GPU when one is
//! prepared, since they are the whole cost of a rig evaluation.

use crate::backend::{gpu_download, gpu_linear_f32_resident, gpu_upload, GpuTensor};
use crate::weights::BodyWeights;
use crate::{
    DiffusionError, Result, MHR_JOINTS, MHR_JOINT_PARAMS, MHR_KEYPOINTS_ALL,
    MHR_MODEL_PARAMS, MHR_VERTS, NUM_EXPR, NUM_SCALE, NUM_SHAPE,
};

const XYZ: usize = 3;
const STATE_WIDTH: usize = 8;
const JOINT_PARAM_WIDTH: usize = 7;
// Corrective features cover joints 2.. (the root and the first child carry no
// pose-corrective term): 125 joints x 6.
const POSE_CORR_FIRST_JOINT: usize = 2;
const POSE_FEATURES: usize = (MHR_JOINTS - POSE_CORR_FIRST_JOINT) * 6;
const POSE_HIDDEN: usize = 3000;
const LBS_ENTRIES: usize = 51337;
const POSE_SPARSE_ENTRIES: usize = 53136;

pub struct MhrRig {
    pub base_shape: Vec<f32>,
    pub identity_basis: Vec<f32>,
    pub expr_basis: Vec<f32>,
    pub param_transform: Vec<f32>,
    pub skel_joint_parents: Vec<i64>,
    pub skel_joint_prerotations: Vec<f32>,
    pub skel_joint_translation_offsets: Vec<f32>,
    pub skel_pmi: Vec<i64>,
    pub skel_pmi_buffer_sizes: Vec<i64>,
    pub lbs_inverse_bind_pose: Vec<f32>,
    pub lbs_skin_indices: Vec<u32>,
    pub lbs_skin_weights: Vec<f32>,
    pub lbs_vert_indices: Vec<u32>,
    pub pose_corr_sparse_indices: Vec<u32>,
    pub pose_corr_sparse_weight: Vec<f32>,
    pub pose_corr_sparse_shape: Vec<i64>,
    pub pose_corr_weight: Vec<f32>,
    pub keypoint_mapping: Vec<f32>,

    pub(crate) scale_mean: Vec<f32>,
    pub(crate) scale_comps: Vec<f32>,
    pub(crate) hand_pose_mean: Vec<f32>,
    pub(crate) hand_pose_comps: Vec<f32>,
    pub(crate) hand_joint_idxs_left: Vec<u32>,
    pub(crate) hand_joint_idxs_right: Vec<u32>,

    keypoint_row_offsets: Vec<usize>,
    keypoint_columns: Vec<u32>,
    keypoint_values: Vec<f32>,
    gpu: Option<MhrGpu>,
}

/// The rig's two dense matrices, resident on the GPU as linear layers:
/// identity blendshapes as `[55317, 45]` (basis transposed) and the pose
/// corrective output layer as `[55317, 3000]`.
struct MhrGpu {
    identity_w: GpuTensor,
    corrective_w: GpuTensor,
}

#[derive(Clone, Debug)]
pub struct MhrOutput {
    pub verts: Vec<f32>,
    pub skel_state: Vec<f32>,
    pub keypoints308: Vec<f32>,
}

impl MhrRig {
    pub fn load(weights: &BodyWeights) -> Result<Self> {
        let base_shape = weights.f32_shaped("mhr.base_shape", &[MHR_VERTS, XYZ])?;
        let identity_basis =
            weights.f32_shaped("mhr.identity_basis", &[NUM_SHAPE, MHR_VERTS, XYZ])?;
        let expr_basis =
            weights.f32_shaped("mhr.expr_basis", &[NUM_EXPR, MHR_VERTS, XYZ])?;
        let param_transform = weights.f32_shaped(
            "mhr.param_transform",
            &[MHR_JOINT_PARAMS, MHR_MODEL_PARAMS],
        )?;
        let skel_joint_parents =
            weights.i64_shaped("mhr.skel_joint_parents", &[MHR_JOINTS])?;
        let skel_joint_prerotations =
            weights.f32_shaped("mhr.skel_joint_prerotations", &[MHR_JOINTS, 4])?;
        let skel_joint_translation_offsets = weights.f32_shaped(
            "mhr.skel_joint_translation_offsets",
            &[MHR_JOINTS, XYZ],
        )?;
        let skel_pmi = weights.i64_shaped("mhr.skel_pmi", &[2, 266])?;
        let skel_pmi_buffer_sizes =
            weights.i64_shaped("mhr.skel_pmi_buffer_sizes", &[4])?;
        let lbs_inverse_bind_pose =
            weights.f32_shaped("mhr.lbs_inverse_bind_pose", &[MHR_JOINTS, STATE_WIDTH])?;
        let lbs_skin_indices = u32_tensor(
            weights,
            "mhr.lbs_skin_indices",
            &[LBS_ENTRIES],
            MHR_JOINTS,
        )?;
        let lbs_skin_weights =
            weights.f32_shaped("mhr.lbs_skin_weights", &[LBS_ENTRIES])?;
        let lbs_vert_indices = u32_tensor(
            weights,
            "mhr.lbs_vert_indices",
            &[LBS_ENTRIES],
            MHR_VERTS,
        )?;
        let pose_corr_sparse_indices = u32_tensor(
            weights,
            "mhr.pose_corr_sparse_indices",
            &[2, POSE_SPARSE_ENTRIES],
            usize::MAX,
        )?;
        let pose_corr_sparse_weight = weights.f32_shaped(
            "mhr.pose_corr_sparse_weight",
            &[POSE_SPARSE_ENTRIES],
        )?;
        let pose_corr_sparse_shape =
            weights.i64_shaped("mhr.pose_corr_sparse_shape", &[2])?;
        if pose_corr_sparse_shape != [POSE_HIDDEN as i64, POSE_FEATURES as i64] {
            return Err(DiffusionError::model(format!(
                "mhr.pose_corr_sparse_shape is {pose_corr_sparse_shape:?}, expected [{POSE_HIDDEN}, {POSE_FEATURES}]"
            )));
        }
        for entry in 0..POSE_SPARSE_ENTRIES {
            let row = pose_corr_sparse_indices[entry] as usize;
            let column = pose_corr_sparse_indices[POSE_SPARSE_ENTRIES + entry] as usize;
            if row >= POSE_HIDDEN || column >= POSE_FEATURES {
                return Err(DiffusionError::model(format!(
                    "mhr.pose_corr_sparse_indices entry {entry} is ({row}, {column})"
                )));
            }
        }
        let pose_corr_weight = weights.f32_shaped(
            "mhr.pose_corr_weight",
            &[MHR_VERTS * XYZ, POSE_HIDDEN],
        )?;
        let keypoint_mapping = weights.f32_shaped(
            "head_pose.keypoint_mapping",
            &[MHR_KEYPOINTS_ALL, MHR_VERTS + MHR_JOINTS],
        )?;

        let scale_mean = weights.f32_shaped("head_pose.scale_mean", &[68])?;
        let scale_comps = weights.f32_shaped("head_pose.scale_comps", &[NUM_SCALE, 68])?;
        let hand_pose_mean = weights.f32_shaped("head_pose.hand_pose_mean", &[54])?;
        let hand_pose_comps = weights.f32_shaped("head_pose.hand_pose_comps", &[54, 54])?;
        let hand_joint_idxs_left =
            u32_tensor(weights, "head_pose.hand_joint_idxs_left", &[27], 136)?;
        let hand_joint_idxs_right =
            u32_tensor(weights, "head_pose.hand_joint_idxs_right", &[27], 136)?;

        validate_parents(&skel_joint_parents)?;
        let (keypoint_row_offsets, keypoint_columns, keypoint_values) =
            compress_mapping(&keypoint_mapping);

        Ok(Self {
            base_shape,
            identity_basis,
            expr_basis,
            param_transform,
            skel_joint_parents,
            skel_joint_prerotations,
            skel_joint_translation_offsets,
            skel_pmi,
            skel_pmi_buffer_sizes,
            lbs_inverse_bind_pose,
            lbs_skin_indices,
            lbs_skin_weights,
            lbs_vert_indices,
            pose_corr_sparse_indices,
            pose_corr_sparse_weight,
            pose_corr_sparse_shape,
            pose_corr_weight,
            keypoint_mapping,
            scale_mean,
            scale_comps,
            hand_pose_mean,
            hand_pose_comps,
            hand_joint_idxs_left,
            hand_joint_idxs_right,
            keypoint_row_offsets,
            keypoint_columns,
            keypoint_values,
            gpu: None,
        })
    }

    /// Put the two dense products on the GPU. Without this the rig runs
    /// entirely on the host (tests, tools); the results are the same.
    pub fn prepare_gpu(&mut self) -> Result<()> {
        let vertex_values = MHR_VERTS * XYZ;
        let mut identity_t = vec![0.0f32; vertex_values * NUM_SHAPE];
        for basis in 0..NUM_SHAPE {
            let source = &self.identity_basis[basis * vertex_values..(basis + 1) * vertex_values];
            for (row, &value) in source.iter().enumerate() {
                identity_t[row * NUM_SHAPE + basis] = value;
            }
        }
        let identity_w = gpu_upload(&identity_t, vertex_values, NUM_SHAPE).map_err(DiffusionError::model)?;
        let corrective_w = gpu_upload(&self.pose_corr_weight, vertex_values, POSE_HIDDEN)
            .map_err(DiffusionError::model)?;
        self.gpu = Some(MhrGpu {
            identity_w,
            corrective_w,
        });
        Ok(())
    }

    pub fn gpu_prepared(&self) -> bool {
        self.gpu.is_some()
    }

    /// Build unposed vertices in rig-space centimetres.
    pub fn rest_vertices(&self, identity: &[f32; 45], expr: &[f32; 72]) -> Vec<f32> {
        let vertex_values = MHR_VERTS * XYZ;
        let mut output = self.base_shape.clone();
        let mut identity_done = false;
        if let Some(gpu) = &self.gpu {
            if let Ok(delta) = gpu_upload(identity, 1, NUM_SHAPE)
                .and_then(|coeffs| gpu_linear_f32_resident(&coeffs, &gpu.identity_w, None))
                .and_then(|delta| gpu_download(&delta))
            {
                if delta.len() == vertex_values {
                    for (target, value) in output.iter_mut().zip(delta) {
                        *target += value;
                    }
                    identity_done = true;
                }
            }
        }
        if !identity_done {
            for (basis, &coefficient) in identity.iter().enumerate() {
                if coefficient == 0.0 {
                    continue;
                }
                let source = &self.identity_basis[basis * vertex_values..(basis + 1) * vertex_values];
                for (target, &value) in output.iter_mut().zip(source) {
                    *target += coefficient * value;
                }
            }
        }
        for (basis, &coefficient) in expr.iter().enumerate() {
            if coefficient == 0.0 {
                continue;
            }
            let source = &self.expr_basis[basis * vertex_values..(basis + 1) * vertex_values];
            for (target, &value) in output.iter_mut().zip(source) {
                *target += coefficient * value;
            }
        }
        output
    }

    pub fn joint_params(&self, model_params: &[f32]) -> Vec<f32> {
        assert_eq!(
            model_params.len(),
            MHR_MODEL_PARAMS,
            "MHR model parameters must contain 249 values"
        );
        let mut output = vec![0.0; MHR_JOINT_PARAMS];
        for (row, target) in output.iter_mut().enumerate() {
            let weights = &self.param_transform
                [row * MHR_MODEL_PARAMS..(row + 1) * MHR_MODEL_PARAMS];
            *target = dot(weights, model_params);
        }
        output
    }

    /// Evaluate parent-first similarity transforms as `(t3, q_xyzw4, s)`.
    pub fn skeleton_state(&self, joint_params: &[f32]) -> Vec<f32> {
        assert_eq!(
            joint_params.len(),
            MHR_JOINT_PARAMS,
            "MHR joint parameters must contain 889 values"
        );
        let mut output = vec![0.0; MHR_JOINTS * STATE_WIDTH];
        for joint in 0..MHR_JOINTS {
            let param = &joint_params
                [joint * JOINT_PARAM_WIDTH..(joint + 1) * JOINT_PARAM_WIDTH];
            let offset = &self.skel_joint_translation_offsets[joint * XYZ..joint * XYZ + XYZ];
            let local_t = [offset[0] + param[0], offset[1] + param[1], offset[2] + param[2]];
            let prerotation = self.skel_joint_prerotations[joint * 4..joint * 4 + 4]
                .try_into()
                .unwrap();
            let local_q = quat_mul(prerotation, euler_zyx_quat([param[3], param[4], param[5]]));
            let local_s = (param[6] * std::f32::consts::LN_2).exp();
            let local = Transform {
                t: local_t,
                q: local_q,
                s: local_s,
            };
            let global = if self.skel_joint_parents[joint] < 0 {
                local
            } else {
                let parent = self.skel_joint_parents[joint] as usize;
                compose(read_transform(&output, parent), local)
            };
            write_transform(&mut output, joint, global);
        }
        output
    }

    /// Compute the pose-dependent displacement in rig-space centimetres.
    pub fn pose_correctives(&self, joint_params: &[f32]) -> Vec<f32> {
        assert_eq!(
            joint_params.len(),
            MHR_JOINT_PARAMS,
            "MHR joint parameters must contain 889 values"
        );
        let mut feature = [0.0; POSE_FEATURES];
        for joint in POSE_CORR_FIRST_JOINT..MHR_JOINTS {
            let offset = joint * JOINT_PARAM_WIDTH + 3;
            let matrix = euler_zyx_matrix([
                joint_params[offset],
                joint_params[offset + 1],
                joint_params[offset + 2],
            ]);
            let slot = joint - POSE_CORR_FIRST_JOINT;
            let target = &mut feature[slot * 6..slot * 6 + 6];
            target.copy_from_slice(&[
                matrix[0][0] - 1.0,
                matrix[1][0],
                matrix[2][0],
                matrix[0][1],
                matrix[1][1] - 1.0,
                matrix[2][1],
            ]);
        }

        let mut hidden = vec![0.0f32; POSE_HIDDEN];
        for entry in 0..POSE_SPARSE_ENTRIES {
            let row = self.pose_corr_sparse_indices[entry] as usize;
            let column = self.pose_corr_sparse_indices[POSE_SPARSE_ENTRIES + entry] as usize;
            hidden[row] += self.pose_corr_sparse_weight[entry] * feature[column];
        }
        for value in &mut hidden {
            *value = value.max(0.0);
        }

        if let Some(gpu) = &self.gpu {
            if let Ok(output) = gpu_upload(&hidden, 1, POSE_HIDDEN)
                .and_then(|h| gpu_linear_f32_resident(&h, &gpu.corrective_w, None))
                .and_then(|out| gpu_download(&out))
            {
                if output.len() == MHR_VERTS * XYZ {
                    return output;
                }
            }
        }
        let mut output = vec![0.0; MHR_VERTS * XYZ];
        for (row, target) in output.iter_mut().enumerate() {
            let weights = &self.pose_corr_weight[row * POSE_HIDDEN..(row + 1) * POSE_HIDDEN];
            *target = dot_unrolled(weights, &hidden);
        }
        output
    }

    /// Linear blend skin corrected rest vertices with global skeleton states.
    pub fn skin(&self, skel_state: &[f32], rest: &[f32]) -> Vec<f32> {
        assert_eq!(skel_state.len(), MHR_JOINTS * STATE_WIDTH);
        assert_eq!(rest.len(), MHR_VERTS * XYZ);
        // One skinning transform per joint, not per influence.
        let transforms: Vec<Transform> = (0..MHR_JOINTS)
            .map(|joint| {
                compose(
                    read_transform(skel_state, joint),
                    read_transform(&self.lbs_inverse_bind_pose, joint),
                )
            })
            .collect();
        let mut output = vec![0.0; MHR_VERTS * XYZ];
        let mut touched = vec![false; MHR_VERTS];
        for entry in 0..LBS_ENTRIES {
            let vertex = self.lbs_vert_indices[entry] as usize;
            let joint = self.lbs_skin_indices[entry] as usize;
            let transform = transforms[joint];
            let point = [rest[vertex * 3], rest[vertex * 3 + 1], rest[vertex * 3 + 2]];
            let posed = apply(transform, point);
            let weight = self.lbs_skin_weights[entry];
            output[vertex * 3] += weight * posed[0];
            output[vertex * 3 + 1] += weight * posed[1];
            output[vertex * 3 + 2] += weight * posed[2];
            touched[vertex] = true;
        }
        for (vertex, touched) in touched.into_iter().enumerate() {
            if !touched {
                output[vertex * 3..vertex * 3 + 3]
                    .copy_from_slice(&rest[vertex * 3..vertex * 3 + 3]);
            }
        }
        output
    }

    /// Regress all 308 keypoints from vertices followed by joint positions.
    pub fn keypoints(&self, verts: &[f32], skel_state: &[f32]) -> Vec<f32> {
        assert_eq!(verts.len(), MHR_VERTS * XYZ);
        assert_eq!(skel_state.len(), MHR_JOINTS * STATE_WIDTH);
        let mut output = vec![0.0; MHR_KEYPOINTS_ALL * XYZ];
        for row in 0..MHR_KEYPOINTS_ALL {
            for entry in self.keypoint_row_offsets[row]..self.keypoint_row_offsets[row + 1] {
                let column = self.keypoint_columns[entry] as usize;
                let weight = self.keypoint_values[entry];
                if column < MHR_VERTS {
                    output[row * 3] += weight * verts[column * 3];
                    output[row * 3 + 1] += weight * verts[column * 3 + 1];
                    output[row * 3 + 2] += weight * verts[column * 3 + 2];
                } else {
                    let joint = column - MHR_VERTS;
                    output[row * 3] += weight * skel_state[joint * STATE_WIDTH];
                    output[row * 3 + 1] += weight * skel_state[joint * STATE_WIDTH + 1];
                    output[row * 3 + 2] += weight * skel_state[joint * STATE_WIDTH + 2];
                }
            }
        }
        output
    }

    /// `model_params` is the 204-wide `[pose (136) | scales (68)]` the pose
    /// head assembles; the 45 identity-coefficient slots of the 249-wide rig
    /// parameter vector are padded with zeros here, as the rig itself does.
    /// A 249-wide vector is accepted as-is.
    pub fn forward(
        &self,
        identity: &[f32],
        model_params: &[f32],
        expr: &[f32],
        correctives: bool,
    ) -> MhrOutput {
        let identity: &[f32; 45] = identity
            .try_into()
            .expect("MHR identity coefficients must contain 45 values");
        let expr: &[f32; 72] = expr
            .try_into()
            .expect("MHR expression coefficients must contain 72 values");
        assert!(
            model_params.len() == 204 || model_params.len() == MHR_MODEL_PARAMS,
            "MHR model parameters must contain 204 or 249 values"
        );
        let mut padded = [0.0f32; MHR_MODEL_PARAMS];
        padded[..model_params.len()].copy_from_slice(model_params);
        let mut rest = self.rest_vertices(identity, expr);
        let joint_params = self.joint_params(&padded);
        let skel_state = self.skeleton_state(&joint_params);
        if correctives {
            let displacement = self.pose_correctives(&joint_params);
            for (value, correction) in rest.iter_mut().zip(displacement) {
                *value += correction;
            }
        }
        let verts = self.skin(&skel_state, &rest);
        let keypoints308 = self.keypoints(&verts, &skel_state);
        MhrOutput {
            verts,
            skel_state,
            keypoints308,
        }
    }
}

#[derive(Clone, Copy)]
struct Transform {
    t: [f32; 3],
    q: [f32; 4],
    s: f32,
}

fn compose(parent: Transform, local: Transform) -> Transform {
    let rotated = quat_rotate(parent.q, local.t);
    Transform {
        t: [
            parent.t[0] + parent.s * rotated[0],
            parent.t[1] + parent.s * rotated[1],
            parent.t[2] + parent.s * rotated[2],
        ],
        q: quat_mul(parent.q, local.q),
        s: parent.s * local.s,
    }
}

fn apply(transform: Transform, point: [f32; 3]) -> [f32; 3] {
    let rotated = quat_rotate(transform.q, point);
    [
        transform.s * rotated[0] + transform.t[0],
        transform.s * rotated[1] + transform.t[1],
        transform.s * rotated[2] + transform.t[2],
    ]
}

fn read_transform(values: &[f32], index: usize) -> Transform {
    let offset = index * STATE_WIDTH;
    Transform {
        t: [values[offset], values[offset + 1], values[offset + 2]],
        q: [values[offset + 3], values[offset + 4], values[offset + 5], values[offset + 6]],
        s: values[offset + 7],
    }
}

fn write_transform(values: &mut [f32], index: usize, transform: Transform) {
    let offset = index * STATE_WIDTH;
    values[offset..offset + 3].copy_from_slice(&transform.t);
    values[offset + 3..offset + 7].copy_from_slice(&transform.q);
    values[offset + 7] = transform.s;
}

fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

fn quat_rotate(q: [f32; 4], point: [f32; 3]) -> [f32; 3] {
    let qv = [q[0], q[1], q[2]];
    let uv = cross3(qv, point);
    let uuv = cross3(qv, uv);
    [
        point[0] + 2.0 * (q[3] * uv[0] + uuv[0]),
        point[1] + 2.0 * (q[3] * uv[1] + uuv[1]),
        point[2] + 2.0 * (q[3] * uv[2] + uuv[2]),
    ]
}

fn euler_zyx_quat([rx, ry, rz]: [f32; 3]) -> [f32; 4] {
    let (sx, cx) = (0.5 * rx).sin_cos();
    let (sy, cy) = (0.5 * ry).sin_cos();
    let (sz, cz) = (0.5 * rz).sin_cos();
    quat_mul(
        [0.0, 0.0, sz, cz],
        quat_mul([0.0, sy, 0.0, cy], [sx, 0.0, 0.0, cx]),
    )
}

fn euler_zyx_matrix([rx, ry, rz]: [f32; 3]) -> [[f32; 3]; 3] {
    let (sx, cx) = rx.sin_cos();
    let (sy, cy) = ry.sin_cos();
    let (sz, cz) = rz.sin_cos();
    [
        [cz * cy, cz * sy * sx - sz * cx, cz * sy * cx + sz * sx],
        [sz * cy, sz * sy * sx + cz * cx, sz * sy * cx - cz * sx],
        [-sy, cy * sx, cy * cx],
    ]
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(left: &[f32], right: &[f32]) -> f32 {
    left.iter().zip(right).map(|(a, b)| a * b).sum()
}

fn dot_unrolled(left: &[f32], right: &[f32]) -> f32 {
    debug_assert_eq!(left.len(), right.len());
    let mut sums = [0.0f32; 8];
    let chunks = left.len() / 8;
    for chunk in 0..chunks {
        let offset = chunk * 8;
        sums[0] += left[offset] * right[offset];
        sums[1] += left[offset + 1] * right[offset + 1];
        sums[2] += left[offset + 2] * right[offset + 2];
        sums[3] += left[offset + 3] * right[offset + 3];
        sums[4] += left[offset + 4] * right[offset + 4];
        sums[5] += left[offset + 5] * right[offset + 5];
        sums[6] += left[offset + 6] * right[offset + 6];
        sums[7] += left[offset + 7] * right[offset + 7];
    }
    let mut output: f32 = sums.into_iter().sum();
    for index in chunks * 8..left.len() {
        output += left[index] * right[index];
    }
    output
}

fn u32_tensor(
    weights: &BodyWeights,
    name: &str,
    shape: &[usize],
    upper_bound: usize,
) -> Result<Vec<u32>> {
    let values = weights.i64_shaped(name, shape)?;
    values
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            let value = u32::try_from(value).map_err(|_| {
                DiffusionError::model(format!("body tensor {name}[{index}] is negative or exceeds u32"))
            })?;
            if (value as usize) >= upper_bound {
                return Err(DiffusionError::model(format!(
                    "body tensor {name}[{index}]={value} exceeds bound {upper_bound}"
                )));
            }
            Ok(value)
        })
        .collect()
}

fn validate_parents(parents: &[i64]) -> Result<()> {
    if parents.first() != Some(&-1) {
        return Err(DiffusionError::model("MHR skeleton root parent must be -1"));
    }
    for (joint, &parent) in parents.iter().enumerate().skip(1) {
        if parent < 0 || parent as usize >= joint {
            return Err(DiffusionError::model(format!(
                "MHR joint {joint} parent {parent} is not parents-first"
            )));
        }
    }
    Ok(())
}

fn compress_mapping(mapping: &[f32]) -> (Vec<usize>, Vec<u32>, Vec<f32>) {
    let columns = MHR_VERTS + MHR_JOINTS;
    let mut offsets = Vec::with_capacity(MHR_KEYPOINTS_ALL + 1);
    let mut sparse_columns = Vec::new();
    let mut sparse_values = Vec::new();
    offsets.push(0);
    for row in 0..MHR_KEYPOINTS_ALL {
        for column in 0..columns {
            let value = mapping[row * columns + column];
            if value != 0.0 {
                sparse_columns.push(column as u32);
                sparse_values.push(value);
            }
        }
        offsets.push(sparse_columns.len());
    }
    (offsets, sparse_columns, sparse_values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture;
    use std::f32::consts::FRAC_PI_2;
    use std::time::Instant;

    fn assert_vec_near(actual: [f32; 3], expected: [f32; 3]) {
        for axis in 0..3 {
            assert!(
                (actual[axis] - expected[axis]).abs() < 2.0e-6,
                "axis {axis}: {} != {}",
                actual[axis],
                expected[axis]
            );
        }
    }

    #[test]
    fn quaternion_layout_and_zyx_order_are_xyzw() {
        let q = euler_zyx_quat([0.0, 0.0, FRAC_PI_2]);
        assert_vec_near(quat_rotate(q, [1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]);
        let euler = [0.27, -0.41, 0.62];
        let q = euler_zyx_quat(euler);
        let matrix = euler_zyx_matrix(euler);
        for axis in 0..3 {
            let mut basis = [0.0; 3];
            basis[axis] = 1.0;
            assert_vec_near(
                quat_rotate(q, basis),
                [matrix[0][axis], matrix[1][axis], matrix[2][axis]],
            );
        }
    }

    #[test]
    fn similarity_composition_applies_child_first() {
        let parent = Transform {
            t: [2.0, 3.0, 4.0],
            q: euler_zyx_quat([0.0, 0.0, FRAC_PI_2]),
            s: 2.0,
        };
        let child = Transform {
            t: [1.0, 0.0, 0.0],
            q: [0.0, 0.0, 0.0, 1.0],
            s: 0.5,
        };
        let combined = compose(parent, child);
        assert_vec_near(combined.t, [2.0, 5.0, 4.0]);
        assert!((combined.s - 1.0).abs() < 1.0e-6);
        assert_vec_near(apply(combined, [1.0, 0.0, 0.0]), [2.0, 6.0, 4.0]);
    }

    #[test]
    fn corrective_identity_feature_is_zero() {
        let matrix = euler_zyx_matrix([0.0, 0.0, 0.0]);
        let feature = [
            matrix[0][0] - 1.0,
            matrix[1][0],
            matrix[2][0],
            matrix[0][1],
            matrix[1][1] - 1.0,
            matrix[2][1],
        ];
        assert_eq!(feature, [0.0; 6]);
    }

    fn max_abs(left: &[f32], right: &[f32]) -> f32 {
        assert_eq!(left.len(), right.len());
        left.iter()
            .zip(right)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn oracle_mhr_forward_with_pose_correctives() {
        let Some(rig) = fixture::rig() else {
            eprintln!("SKIP oracle_mhr_forward_with_pose_correctives: fixtures or weights absent");
            return;
        };
        let Some((_, identity)) = fixture::load("mhrjit_in_shape_0") else {
            eprintln!("SKIP oracle_mhr_forward_with_pose_correctives: input fixture absent");
            return;
        };
        let Some((_, params)) = fixture::load("mhrjit_in_params_0") else {
            eprintln!("SKIP oracle_mhr_forward_with_pose_correctives: input fixture absent");
            return;
        };
        let Some((_, expr)) = fixture::load("mhrjit_in_expr_0") else {
            eprintln!("SKIP oracle_mhr_forward_with_pose_correctives: input fixture absent");
            return;
        };
        let Some((_, expected_verts)) = fixture::load("mhrjit_out_verts_0") else {
            eprintln!("SKIP oracle_mhr_forward_with_pose_correctives: output fixture absent");
            return;
        };
        let Some((_, expected_skel)) = fixture::load("mhrjit_out_skel_0") else {
            eprintln!("SKIP oracle_mhr_forward_with_pose_correctives: output fixture absent");
            return;
        };

        let without = rig.forward(&identity, &params, &expr, false);
        let started = Instant::now();
        let with = rig.forward(&identity, &params, &expr, true);
        let elapsed = started.elapsed();
        let vert_error = max_abs(&with.verts, &expected_verts);
        let skel_error = max_abs(&with.skel_state, &expected_skel);
        let corrective_effect = max_abs(&with.verts, &without.verts);
        eprintln!(
            "MHR corrective forward: {elapsed:?}, vertex max abs {vert_error:.7} cm, skeleton max abs {skel_error:.7}, corrective effect {corrective_effect:.7} cm"
        );
        assert!(vert_error <= 1.0e-3, "vertex max abs error {vert_error} cm");
        assert!(skel_error <= 1.0e-3, "skeleton max abs error {skel_error}");
        // The head's keypoint regression on the same step, in metres, before
        // the camera-axis flip.
        if let Some((_, expected_kp)) = fixture::load("mhr_out_1_0") {
            let metres: Vec<f32> = with.keypoints308.iter().map(|v| v / 100.0).collect();
            let kp_error = max_abs(&metres, &expected_kp);
            eprintln!("MHR 308-keypoint max abs error {kp_error:.7} m");
            assert!(kp_error <= 1.0e-4, "keypoint max abs error {kp_error} m");
        }
        assert!(
            corrective_effect > 1.0e-5,
            "fixture pose did not exercise pose correctives"
        );
    }

    #[test]
    fn oracle_keypoints_first_70_in_camera_axes() {
        let Some(rig) = fixture::rig() else {
            eprintln!("SKIP oracle_keypoints_first_70_in_camera_axes: fixtures or weights absent");
            return;
        };
        // The final refinement step (5) is what the reference reports.
        let Some((_, verts)) = fixture::load("mhrjit_out_verts_5") else {
            eprintln!("SKIP oracle_keypoints_first_70_in_camera_axes: MHR fixture absent");
            return;
        };
        let Some((_, skel)) = fixture::load("mhrjit_out_skel_5") else {
            eprintln!("SKIP oracle_keypoints_first_70_in_camera_axes: MHR fixture absent");
            return;
        };
        let Some((_, expected)) = fixture::load("final_pred_keypoints_3d") else {
            eprintln!("SKIP oracle_keypoints_first_70_in_camera_axes: final fixture absent");
            return;
        };
        let all = rig.keypoints(&verts, &skel);
        let mut actual = all[..70 * 3].to_vec();
        for point in actual.chunks_exact_mut(3) {
            point[0] /= 100.0;
            point[1] /= -100.0;
            point[2] /= -100.0;
        }
        let error = max_abs(&actual, &expected);
        eprintln!("MHR first-70 keypoint max abs error {error:.7} m");
        assert!(error <= 1.0e-4, "keypoint max abs error {error} m");
    }
}
