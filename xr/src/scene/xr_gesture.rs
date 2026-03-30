use crate::prelude::*;

#[derive(Clone, Copy, Debug)]
pub(crate) struct ClosedFistGestureConfig {
    pub min_finger_bend_degrees: f32,
    pub min_thumb_bend_degrees: f32,
    pub max_finger_forward_extension_ratio: f32,
    pub max_thumb_center_distance_ratio: f32,
    pub min_back_of_hand_up_dot: f32,
    pub max_pinch_strength: f32,
}

pub(crate) const CLOSED_FIST_GESTURE: ClosedFistGestureConfig = ClosedFistGestureConfig {
    min_finger_bend_degrees: 55.0,
    min_thumb_bend_degrees: 32.0,
    max_finger_forward_extension_ratio: 0.60,
    max_thumb_center_distance_ratio: 0.95,
    min_back_of_hand_up_dot: 0.35,
    max_pinch_strength: 0.88,
};

pub(crate) fn hand_is_palm_down_closed_fist(
    hand: &XrHand,
    is_left: bool,
    config: ClosedFistGestureConfig,
) -> bool {
    if !hand.in_view() {
        return false;
    }
    if hand.pinch_strength_index() > config.max_pinch_strength
        || hand.pinch_strength_middle() > config.max_pinch_strength
    {
        return false;
    }

    let Some(along_hand) = hand_along_direction(hand) else {
        return false;
    };
    let Some(palm_width) = palm_width(hand) else {
        return false;
    };
    let Some(back_of_hand) = back_of_hand_normal(hand, is_left) else {
        return false;
    };
    if back_of_hand.y < config.min_back_of_hand_up_dot {
        return false;
    }

    [
        XrHand::INDEX_TIP,
        XrHand::MIDDLE_TIP,
        XrHand::RING_TIP,
        XrHand::LITTLE_TIP,
    ]
    .into_iter()
    .all(|tip| finger_is_tucked(hand, tip, along_hand, palm_width, config))
        && thumb_is_tucked(hand, palm_width, config)
}

fn hand_along_direction(hand: &XrHand) -> Option<Vec3f> {
    let along_hand = hand.joints[XrHand::CENTER].position - hand.joints[XrHand::WRIST].position;
    (along_hand.length() > 1.0e-5).then_some(along_hand.normalize())
}

fn palm_width(hand: &XrHand) -> Option<f32> {
    let width = (hand.joints[XrHand::INDEX_BASE].position
        - hand.joints[XrHand::LITTLE_BASE].position)
        .length();
    (width > 1.0e-5).then_some(width)
}

fn back_of_hand_normal(hand: &XrHand, is_left: bool) -> Option<Vec3f> {
    let along_hand = hand_along_direction(hand)?;
    let across_hand = if is_left {
        hand.joints[XrHand::INDEX_BASE].position - hand.joints[XrHand::LITTLE_BASE].position
    } else {
        hand.joints[XrHand::LITTLE_BASE].position - hand.joints[XrHand::INDEX_BASE].position
    };
    if across_hand.length() <= 1.0e-5 {
        return None;
    }
    let back_of_hand = Vec3f::cross(across_hand.normalize(), along_hand);
    (back_of_hand.length() > 1.0e-5).then_some(back_of_hand.normalize())
}

fn finger_base_joint(tip: usize) -> Option<usize> {
    match tip {
        XrHand::INDEX_TIP => Some(XrHand::INDEX_BASE),
        XrHand::MIDDLE_TIP => Some(XrHand::MIDDLE_BASE),
        XrHand::RING_TIP => Some(XrHand::RING_BASE),
        XrHand::LITTLE_TIP => Some(XrHand::LITTLE_BASE),
        _ => None,
    }
}

fn finger_is_tucked(
    hand: &XrHand,
    tip: usize,
    along_hand: Vec3f,
    palm_width: f32,
    config: ClosedFistGestureConfig,
) -> bool {
    let Some(base_joint) = finger_base_joint(tip) else {
        return false;
    };
    let Some(bend) = hand.finger_max_bend_angle_degrees(tip) else {
        return false;
    };
    if bend < config.min_finger_bend_degrees {
        return false;
    }

    let tip_position = hand.tip_pos_for_index(tip);
    let base_position = hand.joints[base_joint].position;
    let forward_extension = (tip_position - base_position).dot(along_hand);
    forward_extension <= palm_width * config.max_finger_forward_extension_ratio
}

fn thumb_is_tucked(hand: &XrHand, palm_width: f32, config: ClosedFistGestureConfig) -> bool {
    let Some(bend) = hand.finger_max_bend_angle_degrees(XrHand::THUMB_TIP) else {
        return false;
    };
    if bend < config.min_thumb_bend_degrees {
        return false;
    }

    let thumb_tip = hand.tip_pos_thumb();
    let palm_center = hand.joints[XrHand::CENTER].position;
    (thumb_tip - palm_center).length() <= palm_width * config.max_thumb_center_distance_ratio
}
