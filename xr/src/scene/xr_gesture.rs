use crate::prelude::*;

const OPEN_HAND_SYNC_PALM_OFFSET_METERS: f32 = 0.020;
const FLOOR_SET_MIN_HAND_GAP_METERS: f32 = 0.08;
const FLOOR_SET_MAX_HAND_GAP_METERS: f32 = 0.55;
const FLOOR_SET_MAX_HAND_VERTICAL_SPLIT_METERS: f32 = 0.06;
const FLOOR_SET_MAX_HAND_DEPTH_SPLIT_METERS: f32 = 0.14;
const FLOOR_SET_MAX_HEAD_HORIZONTAL_DISTANCE_METERS: f32 = 0.24;
const FLOOR_SET_MIN_HEAD_CLEARANCE_METERS: f32 = 0.25;

#[derive(Clone, Copy, Debug)]
pub(crate) struct XrFloorSetGestureSample {
    pub anchor: XrAnchor,
    pub floor_y: f32,
    pub midpoint: Vec3f,
    pub hand_gap: f32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct XrArmPairMetrics {
    pub left_forward: f32,
    pub right_forward: f32,
    pub left_lateral: f32,
    pub right_lateral: f32,
    pub hand_gap: f32,
    pub average_forward_distance: f32,
    pub left_elevation_degrees: f32,
    pub right_elevation_degrees: f32,
}

pub(crate) fn flat_head_forward(orientation: Quat) -> Vec3f {
    let mut forward = orientation.rotate_vec3(&vec3f(0.0, 0.0, -1.0));
    forward.y = 0.0;
    if forward.length() <= 1.0e-6 {
        vec3f(0.0, 0.0, -1.0)
    } else {
        forward.normalize()
    }
}

pub(crate) fn flat_head_right(orientation: Quat) -> Vec3f {
    let mut right = orientation.rotate_vec3(&vec3f(1.0, 0.0, 0.0));
    right.y = 0.0;
    if right.length() <= 1.0e-6 {
        vec3f(1.0, 0.0, 0.0)
    } else {
        right.normalize()
    }
}

pub(crate) fn arm_pair_metrics(
    head_pose: Pose,
    left_point: Vec3f,
    right_point: Vec3f,
) -> Option<XrArmPairMetrics> {
    let forward = flat_head_forward(head_pose.orientation);
    let right = flat_head_right(head_pose.orientation);
    let left_local = left_point - head_pose.position;
    let right_local = right_point - head_pose.position;
    let left_forward = left_local.dot(forward);
    let right_forward = right_local.dot(forward);
    let left_lateral = left_local.dot(right);
    let right_lateral = right_local.dot(right);
    let hand_gap = (right_point - left_point).length();
    let left_horizontal = (left_forward * left_forward + left_lateral * left_lateral).sqrt();
    let right_horizontal = (right_forward * right_forward + right_lateral * right_lateral).sqrt();
    if !left_horizontal.is_finite()
        || !right_horizontal.is_finite()
        || left_horizontal <= 1.0e-4
        || right_horizontal <= 1.0e-4
    {
        return None;
    }
    Some(XrArmPairMetrics {
        left_forward,
        right_forward,
        left_lateral,
        right_lateral,
        hand_gap,
        average_forward_distance: (left_forward + right_forward) * 0.5,
        left_elevation_degrees: left_local.y.atan2(left_horizontal).abs().to_degrees(),
        right_elevation_degrees: right_local.y.atan2(right_horizontal).abs().to_degrees(),
    })
}

pub(crate) fn hand_closed_fist_contact_point(
    hand: &XrHand,
    forward: Vec3f,
    is_left: bool,
) -> Option<Vec3f> {
    if !(hand.is_open() && hand.is_upright_for_box_sync()) {
        return None;
    }
    let pose = hand.tracking_pose()?;
    let offset_direction = hand_palm_surface_direction(hand, is_left).unwrap_or_else(|| {
        if forward.length() > 1.0e-5 {
            forward.normalize()
        } else {
            vec3f(0.0, 0.0, -1.0)
        }
    });
    Some(pose.position + offset_direction.scale(OPEN_HAND_SYNC_PALM_OFFSET_METERS))
}

pub(crate) fn hand_closed_fist_contact_point_geometry_only(
    hand: &XrHand,
    forward: Vec3f,
    is_left: bool,
) -> Option<Vec3f> {
    hand_closed_fist_contact_point(hand, forward, is_left)
}

pub(crate) fn hand_open_palm_contact_point(hand: &XrHand, is_left: bool) -> Option<Vec3f> {
    if !(hand.is_open() && hand.is_palm_down(is_left)) {
        return None;
    }
    let pose = hand.tracking_pose()?;
    let offset_direction = hand_palm_surface_direction(hand, is_left)?;
    Some(pose.position + offset_direction.scale(OPEN_HAND_SYNC_PALM_OFFSET_METERS))
}

pub(crate) fn floor_set_gesture_sample(state: &XrState) -> Option<XrFloorSetGestureSample> {
    let left_point = hand_open_palm_contact_point(&state.left_hand, true)?;
    let right_point = hand_open_palm_contact_point(&state.right_hand, false)?;
    let metrics = arm_pair_metrics(state.head_pose, left_point, right_point)?;
    let midpoint = (left_point + right_point) * 0.5;
    let midpoint_delta = midpoint - state.head_pose.position;
    let head_forward = flat_head_forward(state.head_pose.orientation);
    let head_right = flat_head_right(state.head_pose.orientation);
    let midpoint_forward = midpoint_delta.dot(head_forward);
    let midpoint_lateral = midpoint_delta.dot(head_right);
    let midpoint_horizontal_distance =
        (midpoint_forward * midpoint_forward + midpoint_lateral * midpoint_lateral).sqrt();
    if metrics.left_lateral >= -0.01
        || metrics.right_lateral <= 0.01
        || metrics.hand_gap < FLOOR_SET_MIN_HAND_GAP_METERS
        || metrics.hand_gap > FLOOR_SET_MAX_HAND_GAP_METERS
        || (left_point.y - right_point.y).abs() > FLOOR_SET_MAX_HAND_VERTICAL_SPLIT_METERS
        || (metrics.left_forward - metrics.right_forward).abs()
            > FLOOR_SET_MAX_HAND_DEPTH_SPLIT_METERS
        || midpoint_horizontal_distance > FLOOR_SET_MAX_HEAD_HORIZONTAL_DISTANCE_METERS
        || state.head_pose.position.y - midpoint.y < FLOOR_SET_MIN_HEAD_CLEARANCE_METERS
    {
        return None;
    }
    Some(XrFloorSetGestureSample {
        anchor: XrAnchor {
            left: left_point,
            right: right_point,
        },
        floor_y: midpoint.y,
        midpoint,
        hand_gap: metrics.hand_gap,
    })
}

fn hand_palm_surface_direction(hand: &XrHand, is_left: bool) -> Option<Vec3f> {
    let center = hand.joint_pose_checked(XrHand::CENTER)?.position;
    let wrist = hand.joint_pose_checked(XrHand::WRIST)?.position;
    let along_hand = center - wrist;
    if along_hand.length() <= 1.0e-5 {
        return None;
    }
    let across_hand = if is_left {
        hand.joint_pose_checked(XrHand::INDEX_BASE)?.position
            - hand.joint_pose_checked(XrHand::LITTLE_BASE)?.position
    } else {
        hand.joint_pose_checked(XrHand::LITTLE_BASE)?.position
            - hand.joint_pose_checked(XrHand::INDEX_BASE)?.position
    };
    if across_hand.length() <= 1.0e-5 {
        return None;
    }
    let back_of_hand = Vec3f::cross(across_hand.normalize(), along_hand.normalize());
    if back_of_hand.length() <= 1.0e-5 {
        return None;
    }
    Some(back_of_hand.normalize().scale(-1.0))
}

#[cfg(test)]
include!("../tests/scene/xr_gesture.rs");
