//! Full-mode hand crops, the right-hand-canonical hand decoder, and wrist
//! fusion back into the body estimate.

use crate::backend::GpuTensor;
use crate::condition::{dense_pe, ray_features, RayCond};
use crate::decoder::{Decoder, DecoderNames, PointPrompt, StepFeedback, StepInput};
use crate::model::{close_the_loop, BodyModel, BodyOutput};
use crate::pose::{
    camera_translation_scaled, euler_xzy_to_matrix, euler_xyz_to_matrix, matrix_mul,
    matrix_to_euler_xzy, matrix_to_euler_xyz, matrix_transpose, model_params, project,
    unpack_pose, PoseHeadParams,
};
use crate::preprocess::{
    condition_info, crop_geometry_at, crop_normalized_mirrored, full_to_crop, patch_rays,
    CropGeometry,
};
use crate::weights::BodyWeights;
use crate::{
    DiffusionError, Result, DINO_DIM, IMAGE_SIZE, MHR_JOINTS, MHR_VERTS, NUM_KEYPOINTS,
};

const LEFT: usize = 0;
const RIGHT: usize = 1;
const LOWARM: [usize; 2] = [76, 40];
const WRIST_TWIST: [usize; 2] = [77, 41];
const WRIST: [usize; 2] = [78, 42];
const WRIST_POSE: [[usize; 3]; 2] = [[41, 43, 42], [31, 33, 32]];

pub type HandOutput = BodyOutput;

#[derive(Clone, Copy, Debug)]
pub struct BodyImage<'a> {
    pub rgb: &'a [u8],
    pub width: usize,
    pub height: usize,
}

/// The hand camera head's `default_scale_factor` (the reference config's
/// `DEFAULT_SCALE_FACTOR_HAND`): it multiplies the box size inside the
/// CLIFF translation. Pinned by the oracle: the origin keypoint's crop
/// position at step 0 solves to exactly 10.
const HAND_CAMERA_SCALE_FACTOR: f32 = 10.0;

#[derive(Clone, Debug)]
pub struct HandCrop {
    /// The oracle's `batch_img_N`: planar RGB after warp, scaled to `[0, 1]`.
    pub rgb01: Vec<f32>,
    /// ImageNet-normalised planar RGB consumed by DINO.
    pub normalized: Vec<f32>,
    /// Box in the original, unmirrored full image.
    pub box_xyxy: [f32; 4],
    /// Box and crop geometry in the image actually sampled by the decoder.
    pub sample_box_xyxy: [f32; 4],
    pub geometry: CropGeometry,
    pub mirror: bool,
    pub image_width: usize,
    pub image_height: usize,
}

#[derive(Clone, Debug, Default)]
pub struct FusionReport {
    pub angle_difference: [f32; 2],
    pub wrist_distance: [f32; 2],
    pub valid_angle: [bool; 2],
    pub hand_valid: [bool; 2],
    pub fused: [bool; 2],
    pub prompt_count: usize,
}

pub struct HandBranch {
    ray_cond: RayCond,
    decoder: Decoder,
    dense_pe: Vec<f32>,
    local_to_world_wrist: [[f32; 3]; 3],
    right_wrist_coords: [f32; 3],
    root_coords: [f32; 3],
    nonhand_param_idxs: Vec<usize>,
    joint_rotation: Vec<f32>,
    scale_mean: Vec<f32>,
    scale_comps: Vec<f32>,
}

impl HandBranch {
    pub fn load(weights: &BodyWeights) -> Result<Self> {
        let matrix = weights.f32_shaped(
            "hand_pe_layer.positional_encoding_gaussian_matrix",
            &[2, DINO_DIM / 2],
        )?;
        let hand_name = |suffix: &str| {
            let hand = format!("head_pose_hand.{suffix}");
            if weights.has(&hand) {
                hand
            } else {
                format!("head_pose.{suffix}")
            }
        };
        let local = weights.f32_shaped(&hand_name("local_to_world_wrist"), &[3, 3])?;
        let right_wrist = weights.f32_shaped(&hand_name("right_wrist_coords"), &[3])?;
        let root = weights.f32_shaped(&hand_name("root_coords"), &[3])?;
        let nonhand = weights.i64_shaped(&hand_name("nonhand_param_idxs"), &[145])?;
        let nonhand_param_idxs = nonhand
            .into_iter()
            .map(|value| {
                usize::try_from(value).map_err(|_| {
                    DiffusionError::model(format!(
                        "head_pose_hand.nonhand_param_idxs contains {value}"
                    ))
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            ray_cond: RayCond::prepare_named(weights, "ray_cond_emb_hand")?,
            decoder: Decoder::load_named(weights, DecoderNames::HAND)?,
            dense_pe: dense_pe(&matrix),
            local_to_world_wrist: [
                local[0..3].try_into().unwrap(),
                local[3..6].try_into().unwrap(),
                local[6..9].try_into().unwrap(),
            ],
            right_wrist_coords: right_wrist.try_into().unwrap(),
            root_coords: root.try_into().unwrap(),
            nonhand_param_idxs,
            joint_rotation: weights.f32_shaped(
                "head_pose.joint_rotation",
                &[MHR_JOINTS, 3, 3],
            )?,
            scale_mean: weights.f32_shaped("head_pose.scale_mean", &[68])?,
            scale_comps: weights.f32_shaped("head_pose.scale_comps", &[28, 68])?,
        })
    }

    pub fn infer_hand(&self, model: &BodyModel, crop: &HandCrop) -> Result<HandOutput> {
        let embeddings = model.dino.forward_normalized(&crop.normalized)?;
        let features = ray_features(&patch_rays(&crop.geometry));
        let context = self
            .ray_cond
            .apply(&embeddings, &model.no_mask_embed, &features)?;
        let tokens = self.decoder.build_tokens(condition_info(&crop.geometry));
        let mut last = None;
        let output = self.decoder.run(tokens, &context, &self.dense_pe, |step| {
            let correctives = model.correctives_every_step || step.layer + 1 == crate::DEC_DEPTH;
            let result = self.close_hand_loop(model, crop, &step, correctives);
            let feedback = StepFeedback {
                kp2d_cropped: result.pred_keypoints_2d_cropped.clone(),
                depth: result.pred_keypoints_2d_depth.clone(),
                kp3d: result.pred_keypoints_3d.clone(),
            };
            last = Some(result);
            feedback
        })?;
        let mut last = last.ok_or_else(|| DiffusionError::model("hand decoder ran no steps"))?;
        last.hand_box = output.hand_boxes;
        last.hand_logits = output.hand_logits;
        last.bbox = crop.sample_box_xyxy;
        if crop.mirror {
            self.unmirror_left(&mut last, crop.image_width);
        }
        Ok(last)
    }

    fn close_hand_loop(
        &self,
        model: &BodyModel,
        crop: &HandCrop,
        step: &StepInput,
        correctives: bool,
    ) -> HandOutput {
        let pose = unpack_pose(&step.pose_pred_519);
        let params = self.hand_model_params(model, &pose);
        let rigged = model.rig.forward(&pose.shape, &params, &pose.expr, correctives);

        let mut kp_rig = rigged.keypoints308[..NUM_KEYPOINTS * 3].to_vec();
        kp_rig[..21 * 3].fill(0.0);
        kp_rig[42 * 3..].fill(0.0);
        let kp3d = camera_points(&kp_rig, NUM_KEYPOINTS);
        let vertices = camera_points(&rigged.verts, MHR_VERTS);
        let mut joint_rig = Vec::with_capacity(MHR_JOINTS * 3);
        for joint in 0..MHR_JOINTS {
            joint_rig.extend_from_slice(&rigged.skel_state[joint * 8..joint * 8 + 3]);
        }
        let joints = camera_points(&joint_rig, MHR_JOINTS);
        let pred_cam: [f32; 3] = step.cam_pred_3.as_slice().try_into().unwrap();
        let cam_t = camera_translation_scaled(
            pred_cam,
            crop.geometry.center,
            crop.geometry.side,
            crop.geometry.focal,
            crop.geometry.principal,
            HAND_CAMERA_SCALE_FACTOR,
        );
        let (kp2d, depth) = project(
            &kp3d,
            cam_t,
            crop.geometry.focal,
            crop.geometry.principal,
        );
        BodyOutput {
            pred_pose_raw: step.pose_pred_519[..266].to_vec(),
            global_rot: pose.global_rot,
            body_pose: pose.body,
            shape: pose.shape,
            scale: pose.scale,
            hand: pose.hands,
            face: pose.expr,
            pred_keypoints_3d: kp3d,
            pred_vertices: vertices,
            pred_joint_coords: joints,
            joint_global_rots: rigged.joint_global_rots,
            mhr_model_params: params,
            pred_cam,
            pred_keypoints_2d: kp2d.clone(),
            pred_cam_t: cam_t,
            focal_length: crop.geometry.focal,
            pred_keypoints_2d_depth: depth,
            pred_keypoints_2d_cropped: full_to_crop(&kp2d, &crop.geometry),
            hand_box: [[0.0; 4]; 2],
            hand_logits: [[0.0; 2]; 2],
            bbox: crop.sample_box_xyxy,
        }
    }

    fn hand_model_params(&self, model: &BodyModel, pose: &PoseHeadParams) -> [f32; 204] {
        let original = euler_xyz_to_matrix(pose.global_rot);
        let global_matrix = matrix_mul(original, self.local_to_world_wrist);
        let global_rot = matrix_to_euler_xyz(global_matrix);
        let wrist_delta = [
            self.right_wrist_coords[0] - self.root_coords[0],
            self.right_wrist_coords[1] - self.root_coords[1],
            self.right_wrist_coords[2] - self.root_coords[2],
        ];
        let rotated = matrix_vec(global_matrix, wrist_delta);
        let global_trans = [
            -(rotated[0] + self.root_coords[0]),
            -(rotated[1] + self.root_coords[1]),
            -(rotated[2] + self.root_coords[2]),
        ];
        let mut params = model_params(&model.rig, pose);
        for axis in 0..3 {
            params[axis] = global_trans[axis] * 10.0;
            params[3 + axis] = global_rot[axis];
        }
        for &index in &self.nonhand_param_idxs {
            params[index] = 0.0;
        }
        params
    }

    fn unmirror_left(&self, output: &mut HandOutput, image_width: usize) {
        let scale8 = output.scale[8];
        output.scale[9] = ((self.scale_mean[8]
            + self.scale_comps[8 * 68 + 8] * scale8)
            - self.scale_mean[9])
            / self.scale_comps[9 * 68 + 9];
        let source = output.joint_global_rots[42 * 9..43 * 9].to_vec();
        output.joint_global_rots[78 * 9..79 * 9].copy_from_slice(&source);
        for value in &mut output.joint_global_rots[78 * 9 + 3..79 * 9] {
            *value = -*value;
        }
        output.hand.copy_within(54..108, 0);
        let [x1, y1, x2, y2] = output.bbox;
        output.bbox = [
            image_width as f32 - x2 - 1.0,
            y1,
            image_width as f32 - x1 - 1.0,
            y2,
        ];
    }

    pub fn fuse(
        &self,
        model: &BodyModel,
        body: &mut BodyOutput,
        left: &HandOutput,
        right: &HandOutput,
        crops: &[HandCrop; 2],
        body_geo: &CropGeometry,
        body_context: &GpuTensor,
        body_pe: &[f32],
    ) -> Result<FusionReport> {
        let hands = [left, right];
        let mut report = FusionReport::default();
        let mut original_local = [[[0.0; 3]; 3]; 2];
        for hand in 0..2 {
            let indices = WRIST_POSE[hand];
            original_local[hand] = euler_xzy_to_matrix([
                body.body_pose[indices[0]],
                body.body_pose[indices[1]],
                body.body_pose[indices[2]],
            ]);
        }

        let mut hand2 = [0.0f32; 108];
        hand2[..54].copy_from_slice(&left.hand[..54]);
        hand2[54..].copy_from_slice(&right.hand[54..]);
        let mut scale2 = body.scale;
        scale2[9] = left.scale[9];
        scale2[8] = right.scale[8];
        for index in 18..28 {
            scale2[index] = 0.5 * (left.scale[index] + right.scale[index]);
        }
        let mut shape2 = body.shape;
        for index in 40..45 {
            shape2[index] = 0.5 * (left.shape[index] + right.shape[index]);
        }
        // Stage A (before the re-prompt): the angle gate uses the first
        // body pass's own joint rotations, no rig call.
        let fused_from = |rotations: &[f32], hand: usize| {
            let lowarm = matrix_at(rotations, LOWARM[hand]);
            let twist = matrix_at(&self.joint_rotation, WRIST_TWIST[hand]);
            let zero = matrix_mul(lowarm, twist);
            let predicted = matrix_at(&hands[hand].joint_global_rots, WRIST[hand]);
            matrix_mul(matrix_transpose(zero), predicted)
        };
        let angle_between = |a: [[f32; 3]; 3], b: [[f32; 3]; 3]| {
            let relative = matrix_mul(a, matrix_transpose(b));
            let trace = relative[0][0] + relative[1][1] + relative[2][2];
            ((trace - 1.0) * 0.5).clamp(-1.0, 1.0).acos()
        };
        for hand in 0..2 {
            let fused = fused_from(&body.joint_global_rots, hand);
            report.valid_angle[hand] = angle_between(original_local[hand], fused) < 1.4;
        }
        let left_wrist = unmirrored_wrist(left, crops[LEFT].image_width, true);
        let right_wrist = unmirrored_wrist(right, crops[RIGHT].image_width, false);
        let body_left = point2(&body.pred_keypoints_2d, 62);
        let body_right = point2(&body.pred_keypoints_2d, 41);
        report.wrist_distance[LEFT] = distance(left_wrist, body_left) / crops[RIGHT].geometry.side;
        report.wrist_distance[RIGHT] = distance(right_wrist, body_right) / crops[LEFT].geometry.side;
        for hand in 0..2 {
            let crop_ok = crops[hand].geometry.side > 64.0;
            let points_ok = hands[hand]
                .pred_keypoints_2d_cropped
                .iter()
                .map(|value| value.abs())
                .fold(0.0f32, f32::max)
                < 0.5;
            report.hand_valid[hand] = report.valid_angle[hand]
                && crop_ok
                && points_ok
                && report.wrist_distance[hand] < 0.25;
        }

        let candidates = [
            (right_wrist, 41usize, RIGHT),
            (left_wrist, 62usize, LEFT),
            (point2(&body.pred_keypoints_2d, 8), 8usize, RIGHT),
            (point2(&body.pred_keypoints_2d, 7), 7usize, LEFT),
        ];
        let mut prompts = Vec::new();
        for (point, label, side) in candidates {
            let crop_point = full_to_crop(&point, body_geo);
            if report.hand_valid[side]
                && crop_point[0] > -0.5
                && crop_point[0] < 0.5
                && crop_point[1] > -0.5
                && crop_point[1] < 0.5
            {
                prompts.push(PointPrompt {
                    point: [
                        (crop_point[0] + 0.5).clamp(0.0, 1.0),
                        (crop_point[1] + 0.5).clamp(0.0, 1.0),
                    ],
                    label,
                });
            }
        }
        report.prompt_count = prompts.len();
        if !prompts.is_empty() {
            let mut previous = Vec::with_capacity(522);
            previous.extend_from_slice(&body.pred_pose_raw);
            previous.extend_from_slice(&body.shape);
            previous.extend_from_slice(&body.scale);
            previous.extend_from_slice(&body.hand);
            previous.extend_from_slice(&body.face);
            previous.extend_from_slice(&body.pred_cam);
            let mut reprompted = None;
            let decoded = model.decoder.run_with_prompts(
                condition_info(body_geo),
                &prompts,
                &previous,
                body_context,
                body_pe,
                |step| {
                    let result = close_the_loop(&model.rig, body_geo, &step, true);
                    let feedback = StepFeedback {
                        kp2d_cropped: result.pred_keypoints_2d_cropped.clone(),
                        depth: result.pred_keypoints_2d_depth.clone(),
                        kp3d: result.pred_keypoints_3d.clone(),
                    };
                    reprompted = Some(result);
                    feedback
                },
            )?;
            let mut next = reprompted
                .ok_or_else(|| DiffusionError::model("body re-prompt decoder ran no steps"))?;
            next.hand_box = decoded.hand_boxes;
            next.hand_logits = decoded.hand_logits;
            next.bbox = body.bbox;
            *body = next;
        }

        // Stage B (after the re-prompt): the wrist angles come from a rig
        // forward of the current body pose with the hands' parameters, and
        // the angle gate is re-evaluated against the first pass's wrists.
        let fusion_pose = PoseHeadParams {
            global_rot: body.global_rot,
            body: body.body_pose,
            shape: shape2,
            scale: scale2,
            hands: hand2,
            expr: body.face,
        };
        let fusion_params = model_params(&model.rig, &fusion_pose);
        let rotations = model
            .rig
            .forward(&shape2, &fusion_params, &body.face, false)
            .joint_global_rots;
        let mut wrist_angles = [[0.0f32; 3]; 2];
        for hand in 0..2 {
            let fused = fused_from(&rotations, hand);
            wrist_angles[hand] = fix_wrist_euler(matrix_to_euler_xzy(fused));
            report.angle_difference[hand] = angle_between(original_local[hand], fused);
            report.fused[hand] = report.angle_difference[hand] < 1.4 && report.hand_valid[hand];
        }
        for hand in 0..2 {
            if !report.fused[hand] {
                continue;
            }
            let indices = WRIST_POSE[hand];
            for axis in 0..3 {
                body.body_pose[indices[axis]] = wrist_angles[hand][axis];
            }
            body.hand[hand * 54..(hand + 1) * 54]
                .copy_from_slice(&hand2[hand * 54..(hand + 1) * 54]);
        }
        if report.fused[LEFT] {
            body.scale[9] = left.scale[9];
        }
        if report.fused[RIGHT] {
            body.scale[8] = right.scale[8];
        }
        let valid_count = report.fused.into_iter().filter(|value| *value).count();
        if valid_count != 0 {
            for index in 18..28 {
                body.scale[index] = (0..2)
                    .filter(|&hand| report.fused[hand])
                    .map(|hand| hands[hand].scale[index])
                    .sum::<f32>()
                    / valid_count as f32;
            }
            for index in 40..45 {
                body.shape[index] = (0..2)
                    .filter(|&hand| report.fused[hand])
                    .map(|hand| hands[hand].shape[index])
                    .sum::<f32>()
                    / valid_count as f32;
            }
        }
        rebuild_body(model, body, body_geo);
        Ok(report)
    }
}

pub fn hand_crops(
    body: &BodyOutput,
    body_geo: &CropGeometry,
    image: BodyImage<'_>,
) -> [HandCrop; 2] {
    hand_crops_from_boxes(body.hand_box, body_geo, image)
}

fn hand_crops_from_boxes(
    boxes: [[f32; 4]; 2],
    body_geo: &CropGeometry,
    image: BodyImage<'_>,
) -> [HandCrop; 2] {
    std::array::from_fn(|hand| {
        let value = boxes[hand];
        let centre_crop = [value[0] * IMAGE_SIZE as f32, value[1] * IMAGE_SIZE as f32];
        let scale_crop = value[2].max(value[3]) * IMAGE_SIZE as f32;
        let k = body_geo.affine[0];
        let centre = [
            (centre_crop[0] - body_geo.affine[2]) / k,
            (centre_crop[1] - body_geo.affine[5]) / k,
        ];
        let scale = scale_crop / k;
        let box_xyxy = [
            centre[0] - 0.5 * scale,
            centre[1] - 0.5 * scale,
            centre[0] + 0.5 * scale,
            centre[1] + 0.5 * scale,
        ];
        let mirror = hand == LEFT;
        let sample_box_xyxy = if mirror {
            [
                image.width as f32 - box_xyxy[2] - 1.0,
                box_xyxy[1],
                image.width as f32 - box_xyxy[0] - 1.0,
                box_xyxy[3],
            ]
        } else {
            box_xyxy
        };
        let geometry = crop_geometry_at(
            sample_box_xyxy,
            image.width,
            image.height,
            Some([body_geo.focal, body_geo.principal[0], body_geo.principal[1]]),
            IMAGE_SIZE,
            0.9,
        );
        let normalized = crop_normalized_mirrored(
            image.rgb,
            image.width,
            image.height,
            &geometry,
            mirror,
        );
        let plane = IMAGE_SIZE * IMAGE_SIZE;
        let mut rgb01 = normalized.clone();
        for channel in 0..3 {
            let mean = [0.485, 0.456, 0.406][channel];
            let std = [0.229, 0.224, 0.225][channel];
            for value in &mut rgb01[channel * plane..(channel + 1) * plane] {
                *value = *value * std + mean;
            }
        }
        HandCrop {
            rgb01,
            normalized,
            box_xyxy,
            sample_box_xyxy,
            geometry,
            mirror,
            image_width: image.width,
            image_height: image.height,
        }
    })
}

fn rebuild_body(model: &BodyModel, body: &mut BodyOutput, body_geo: &CropGeometry) {
    let pose = PoseHeadParams {
        global_rot: body.global_rot,
        body: body.body_pose,
        shape: body.shape,
        scale: body.scale,
        hands: body.hand,
        expr: body.face,
    };
    let params = model_params(&model.rig, &pose);
    let rigged = model.rig.forward(&body.shape, &params, &body.face, true);
    body.mhr_model_params = params;
    body.pred_keypoints_3d = camera_points(&rigged.keypoints308, NUM_KEYPOINTS);
    body.pred_vertices = camera_points(&rigged.verts, MHR_VERTS);
    let mut joints = Vec::with_capacity(MHR_JOINTS * 3);
    for joint in 0..MHR_JOINTS {
        joints.extend_from_slice(&rigged.skel_state[joint * 8..joint * 8 + 3]);
    }
    body.pred_joint_coords = camera_points(&joints, MHR_JOINTS);
    body.joint_global_rots = rigged.joint_global_rots;
    let (kp2d, depth) = project(
        &body.pred_keypoints_3d,
        body.pred_cam_t,
        body.focal_length,
        body_geo.principal,
    );
    body.pred_keypoints_2d_cropped = full_to_crop(&kp2d, body_geo);
    body.pred_keypoints_2d = kp2d;
    body.pred_keypoints_2d_depth = depth;
}

fn camera_points(values: &[f32], count: usize) -> Vec<f32> {
    let mut output = Vec::with_capacity(count * 3);
    for point in values[..count * 3].chunks_exact(3) {
        output.extend_from_slice(&[point[0] / 100.0, point[1] / -100.0, point[2] / -100.0]);
    }
    output
}

fn matrix_at(values: &[f32], joint: usize) -> [[f32; 3]; 3] {
    let row = &values[joint * 9..(joint + 1) * 9];
    [
        row[0..3].try_into().unwrap(),
        row[3..6].try_into().unwrap(),
        row[6..9].try_into().unwrap(),
    ]
}

fn matrix_vec(matrix: [[f32; 3]; 3], value: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|row| (0..3).map(|column| matrix[row][column] * value[column]).sum())
}

fn point2(values: &[f32], index: usize) -> [f32; 2] {
    [values[index * 2], values[index * 2 + 1]]
}

fn unmirrored_wrist(output: &HandOutput, width: usize, mirror: bool) -> [f32; 2] {
    let mut point = point2(&output.pred_keypoints_2d, 41);
    if mirror {
        point[0] = width as f32 - point[0] - 1.0;
    }
    point
}

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

fn wrap(value: f32) -> f32 {
    value.sin().atan2(value.cos())
}

fn wrist_violation(value: [f32; 3]) -> f32 {
    let limits = [(-2.2, 1.0), (-2.2, 1.5), (-1.2, 1.5)];
    value
        .into_iter()
        .zip(limits)
        .map(|(value, (low, high))| (low - value).max(0.0).powi(2) + (value - high).max(0.0).powi(2))
        .sum()
}

fn fix_wrist_euler(original: [f32; 3]) -> [f32; 3] {
    let alternate = [
        wrap(original[0] + std::f32::consts::PI),
        wrap(-(original[1] + std::f32::consts::PI)),
        wrap(original[2] + std::f32::consts::PI),
    ];
    if wrist_violation(alternate) < wrist_violation(original) {
        alternate
    } else {
        original
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{gpu_device_available, gpu_download, gpu_upload};
    use crate::fixture::{self, OracleRoot};

    fn max_abs(actual: &[f32], expected: &[f32]) -> f32 {
        assert_eq!(actual.len(), expected.len());
        actual
            .iter()
            .zip(expected)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max)
    }

    fn mean_relative_error(actual: &[f32], expected: &[f32]) -> f32 {
        assert_eq!(actual.len(), expected.len());
        let diff: f64 = actual
            .iter()
            .zip(expected)
            .map(|(a, e)| (a - e).abs() as f64)
            .sum();
        let norm: f64 = expected.iter().map(|e| e.abs() as f64).sum();
        (diff / norm.max(1e-12)) as f32
    }

    fn max_abs_at(actual: &[f32], expected: &[f32]) -> (f32, usize) {
        actual
            .iter()
            .zip(expected)
            .enumerate()
            .map(|(index, (a, b))| ((a - b).abs(), index))
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .unwrap()
    }

    fn oracle(name: &str) -> Vec<f32> {
        fixture::load_from(OracleRoot::Full, name)
            .unwrap_or_else(|| panic!("missing oracle_full field {name}"))
            .1
    }

    fn planar_to_tokens(values: &[f32]) -> Vec<f32> {
        let tokens = values.len() / DINO_DIM;
        let mut output = vec![0.0; values.len()];
        for channel in 0..DINO_DIM {
            for token in 0..tokens {
                output[token * DINO_DIM + channel] = values[channel * tokens + token];
            }
        }
        output
    }

    fn assert_oracle(name: &str, actual: &[f32], tolerance: f32) {
        let expected = oracle(name);
        let (error, at) = max_abs_at(actual, &expected);
        eprintln!("{name} max abs {error:.6}");
        assert!(
            error <= tolerance,
            "{name} max abs {error} at {at}: actual {} expected {}",
            actual[at],
            expected[at],
        );
    }

    #[test]
    fn oracle_hand_boxes_and_crops() {
        let Some((shape, image)) = fixture::load_from(OracleRoot::Full, "input_rgb_u8") else {
            eprintln!("SKIP oracle_hand_boxes_and_crops: oracle_full absent");
            return;
        };
        let Some((_, boxes)) = fixture::load_from(OracleRoot::Full, "hand_box_sigmoid") else {
            eprintln!("SKIP oracle_hand_boxes_and_crops: hand boxes absent");
            return;
        };
        let (height, width) = (shape[0], shape[1]);
        let rgb: Vec<u8> = image.into_iter().map(|value| value as u8).collect();
        let body_geo = crop_geometry_at(
            [0.0, 0.0, width as f32, height as f32],
            width,
            height,
            None,
            IMAGE_SIZE,
            1.25,
        );
        let crops = hand_crops_from_boxes(
            [boxes[0..4].try_into().unwrap(), boxes[4..8].try_into().unwrap()],
            &body_geo,
            BodyImage {
                rgb: &rgb,
                width,
                height,
            },
        );
        for (hand, expected_name) in ["hand_box_left_xyxy", "hand_box_right_xyxy"]
            .into_iter()
            .enumerate()
        {
            let expected = fixture::load_from(OracleRoot::Full, expected_name).unwrap().1;
            let error = max_abs(&crops[hand].sample_box_xyxy, &expected);
            eprintln!("{expected_name} max abs {error:.6} px");
            assert!(error < 5.0e-3, "{expected_name} max abs {error}");
            let crop_name = format!("batch_img_{}", hand + 1);
            let expected_crop = fixture::load_from(OracleRoot::Full, &crop_name).unwrap().1;
            let (crop_error, at) = max_abs_at(&crops[hand].rgb01, &expected_crop);
            eprintln!(
                "{crop_name} max abs {crop_error:.6} at {at}: actual {} expected {}",
                crops[hand].rgb01[at], expected_crop[at]
            );
            assert!(crop_error < 2.0e-2, "{crop_name} max abs {crop_error}");
        }
    }

    #[test]
    fn intrinsic_euler_pairs_round_trip() {
        for value in [[0.3, -0.4, 0.8], [-1.1, 0.6, -0.2]] {
            let matrix = euler_xyz_to_matrix(value);
            let rebuilt = euler_xyz_to_matrix(matrix_to_euler_xyz(matrix));
            assert!(max_abs(&matrix.concat(), &rebuilt.concat()) < 1.0e-6);
            let matrix = euler_xzy_to_matrix(value);
            let rebuilt = euler_xzy_to_matrix(matrix_to_euler_xzy(matrix));
            assert!(max_abs(&matrix.concat(), &rebuilt.concat()) < 1.0e-6);
        }
    }

    #[test]
    fn oracle_hand_backbone_context_decoder_heads_and_rig() {
        if fixture::oracle_dir_for(OracleRoot::Full).is_none() {
            eprintln!("SKIP oracle_hand_stages: oracle_full absent");
            return;
        }
        if !gpu_device_available() || !fixture::gpu_required_ops_available() {
            eprintln!("SKIP oracle_hand_stages: GPU unavailable");
            return;
        }
        let Some(weights_path) = fixture::weights_path() else {
            eprintln!("SKIP oracle_hand_stages: weights absent");
            return;
        };
        let mut model = BodyModel::load(&weights_path).expect("load body model");
        model.correctives_every_step = true;
        let branch = HandBranch::load(&model.weights).expect("load hand branch");
        let (shape, image) = fixture::load_from(OracleRoot::Full, "input_rgb_u8").unwrap();
        let (height, width) = (shape[0], shape[1]);
        let rgb: Vec<u8> = image.into_iter().map(|value| value as u8).collect();
        let body_geo = crop_geometry_at(
            [0.0, 0.0, width as f32, height as f32],
            width,
            height,
            None,
            IMAGE_SIZE,
            1.25,
        );
        let boxes = oracle("hand_box_sigmoid");
        let crops = hand_crops_from_boxes(
            [boxes[..4].try_into().unwrap(), boxes[4..].try_into().unwrap()],
            &body_geo,
            BodyImage {
                rgb: &rgb,
                width,
                height,
            },
        );

        for hand in 0..2 {
            let embeddings = model
                .dino
                .forward_normalized(&crops[hand].normalized)
                .expect("hand backbone");
            let backbone = gpu_download(&embeddings).expect("download hand backbone");
            let expected_backbone = planar_to_tokens(&oracle(&format!("backbone_out_{}", hand + 1)));
            let (error, _) = max_abs_at(&backbone, &expected_backbone);
            let mean_relative = mean_relative_error(&backbone, &expected_backbone);
            eprintln!(
                "backbone_out_{} max abs {error:.6}; relative mean error {mean_relative:.6}",
                hand + 1
            );
            assert!(
                mean_relative < 3.0e-2,
                "hand backbone relative mean error {mean_relative}"
            );
            // The stages below start from the reference's own backbone output
            // so their tolerances are not eaten by bf16 accumulation noise.
            let embeddings = gpu_upload(
                &expected_backbone,
                expected_backbone.len() / crate::DINO_DIM,
                crate::DINO_DIM,
            )
            .expect("upload oracle hand backbone");

            let context = branch
                .ray_cond
                .apply(
                    &embeddings,
                    &model.no_mask_embed,
                    &ray_features(&patch_rays(&crops[hand].geometry)),
                )
                .expect("hand ray conditioning");
            let context_host = gpu_download(&context).expect("download hand context");
            let expected_context = planar_to_tokens(&oracle(&format!("raycondhand_out_{hand}")));
            let (error, _) = max_abs_at(&context_host, &expected_context);
            eprintln!("raycondhand_out_{hand} max abs {error:.6}");
            assert!(error < 2.0e-2);

            let tokens = branch
                .decoder
                .build_tokens(condition_info(&crops[hand].geometry));
            assert_oracle(&format!("dechand_tokens_in_{hand}"), &tokens.tokens, 1.0e-4);
            assert_oracle(
                &format!("dechand_token_augment_in_{hand}"),
                &tokens.token_augment,
                1.0e-4,
            );
            let mut last = None;
            let mut trace = |layer: usize, hidden: &GpuTensor, normed: &[f32]| -> Result<()> {
                let hidden = gpu_download(hidden).map_err(DiffusionError::model)?;
                assert_oracle(
                    &format!("dechand{layer}_tokens_out_{hand}"),
                    &hidden,
                    5.0e-2,
                );
                assert_oracle(
                    &format!("normhand_final_out_{}", hand * 6 + layer),
                    normed,
                    2.0e-2,
                );
                Ok(())
            };
            branch
                .decoder
                .run_traced(
                    tokens,
                    &context,
                    &branch.dense_pe,
                    |step| {
                        let call = hand * 6 + step.layer;
                        let raw: Vec<f32> = step
                            .pose_pred_519
                            .iter()
                            .zip(branch.decoder.init_pose())
                            .map(|(value, init)| value - init)
                            .collect();
                        assert_oracle(
                            &format!("headhand_pose_proj_out_{call}"),
                            &raw,
                            2.0e-2,
                        );
                        let result = branch.close_hand_loop(&model, &crops[hand], &step, true);
                        if step.layer < 6 {
                            let posemb_in = oracle(&format!("kphand_posemb_in_{}", hand * 5 + step.layer.min(4)));
                            let (kp_err, kp_at) =
                                max_abs_at(&result.pred_keypoints_2d_cropped, &posemb_in);
                            let valid = result
                                .pred_keypoints_2d_cropped
                                .chunks_exact(2)
                                .zip(&result.pred_keypoints_2d_depth)
                                .filter(|(p, &d)| {
                                    (0.0..=1.0).contains(&(p[0] + 0.5))
                                        && (0.0..=1.0).contains(&(p[1] + 0.5))
                                        && d >= 1e-5
                                })
                                .count();
                            eprintln!(
                                "hand {hand} step {}: kp2d_cropped vs kphand_posemb_in max abs {kp_err:.5} at kp {} ({} valid of 70)",
                                step.layer,
                                kp_at / 2,
                                valid
                            );
                        }
                        assert_oracle(
                            &format!("mhrjithand_in_params_{call}"),
                            &result.mhr_model_params,
                            2.0e-3,
                        );
                        let rig_axes = |values: &[f32]| {
                            let mut output = values.to_vec();
                            for point in output.chunks_exact_mut(3) {
                                point[1] = -point[1];
                                point[2] = -point[2];
                            }
                            output
                        };
                        assert_oracle(
                            &format!("mhrhand_out_0_{call}"),
                            &rig_axes(&result.pred_vertices),
                            2.0e-3,
                        );
                        let expected_keypoints = oracle(&format!("mhrhand_out_1_{call}"));
                        let keypoints = rig_axes(&result.pred_keypoints_3d);
                        let (error, _) = max_abs_at(
                            &keypoints,
                            &expected_keypoints[..NUM_KEYPOINTS * 3],
                        );
                        assert!(error < 2.0e-3, "mhrhand_out_1_{call} max abs {error}");
                        assert_oracle(
                            &format!("mhrhand_out_2_{call}"),
                            &rig_axes(&result.pred_joint_coords),
                            2.0e-3,
                        );
                        assert_oracle(
                            &format!("mhrhand_out_3_{call}"),
                            &result.mhr_model_params,
                            2.0e-3,
                        );
                        assert_oracle(
                            &format!("mhrhand_out_4_{call}"),
                            &result.joint_global_rots,
                            2.0e-3,
                        );
                        let feedback = StepFeedback {
                            kp2d_cropped: result.pred_keypoints_2d_cropped.clone(),
                            depth: result.pred_keypoints_2d_depth.clone(),
                            kp3d: result.pred_keypoints_3d.clone(),
                        };
                        last = Some(result);
                        feedback
                    },
                    &mut trace,
                )
                .expect("run traced hand decoder");
            let raw = last.expect("hand result");
            assert_oracle(
                &format!("step_hand_pred_keypoints_3d_{hand}"),
                &raw.pred_keypoints_3d,
                2.0e-3,
            );
            assert_oracle(
                &format!("step_hand_pred_keypoints_2d_{hand}"),
                &raw.pred_keypoints_2d,
                0.5,
            );
            assert_oracle(
                &format!("step_hand_joint_global_rots_{hand}"),
                &raw.joint_global_rots,
                2.0e-3,
            );
        }
    }
}
