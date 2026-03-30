use {
    crate::{
        cx::Cx, event::DigitId, makepad_live_id::live_id_num, makepad_math::*,
        makepad_micro_serde::*, Area, CxWindowPool, DigitDevice, FingerDownEvent, Hit, HitOptions,
        Inset, KeyModifiers, LiveId, SmallVec,
    },
    std::{cell::Cell, rc::Rc},
};

pub const XR_TOUCH_DOWN_FRONT: f32 = 6.0;
pub const XR_TOUCH_DOWN_BACK: f32 = -12.0;
pub const XR_TOUCH_CAPTURE_FRONT: f32 = 11.0;
// About 2cm behind the panel at XrView's default logical-pixel scale.
pub const XR_TOUCH_CAPTURE_BACK: f32 = -16.0;
pub const XR_TOUCH_REARM_FRONT: f32 = 28.0;

#[derive(Clone, Debug, Default, SerBin, DeBin)]
pub struct XrController {
    pub grip_pose: Pose,
    pub aim_pose: Pose,
    pub trigger: f32,
    pub grip: f32,
    pub buttons: u16,
    pub stick: Vec2f,
}

impl XrController {
    pub const CLICK_X: u16 = 1 << 0;
    pub const CLICK_Y: u16 = 1 << 1;
    pub const CLICK_A: u16 = 1 << 2;
    pub const CLICK_B: u16 = 1 << 3;
    pub const CLICK_MENU: u16 = 1 << 4;

    pub const ACTIVE: u16 = 1 << 5;
    pub const CLICK_THUMBSTICK: u16 = 1 << 6;

    pub const TOUCH_X: u16 = 1 << 7;
    pub const TOUCH_Y: u16 = 1 << 8;
    pub const TOUCH_A: u16 = 1 << 9;
    pub const TOUCH_B: u16 = 1 << 10;
    pub const TOUCH_THUMBSTICK: u16 = 1 << 11;
    pub const TOUCH_TRIGGER: u16 = 1 << 12;
    pub const TOUCH_THUMBREST: u16 = 1 << 13;
    pub fn triggered(&self) -> bool {
        self.trigger > 0.8
    }
    pub fn active(&self) -> bool {
        self.buttons & Self::ACTIVE != 0
    }

    pub fn click_x(&self) -> bool {
        self.buttons & Self::CLICK_X != 0
    }
    pub fn click_y(&self) -> bool {
        self.buttons & Self::CLICK_Y != 0
    }
    pub fn click_a(&self) -> bool {
        self.buttons & Self::CLICK_A != 0
    }
    pub fn click_b(&self) -> bool {
        self.buttons & Self::CLICK_B != 0
    }
    pub fn click_thumbstick(&self) -> bool {
        self.buttons & Self::CLICK_THUMBSTICK != 0
    }
    pub fn click_menu(&self) -> bool {
        self.buttons & Self::CLICK_MENU != 0
    }

    pub fn touch_x(&self) -> bool {
        self.buttons & Self::TOUCH_X != 0
    }
    pub fn touch_y(&self) -> bool {
        self.buttons & Self::TOUCH_Y != 0
    }
    pub fn touch_a(&self) -> bool {
        self.buttons & Self::TOUCH_A != 0
    }
    pub fn touch_b(&self) -> bool {
        self.buttons & Self::TOUCH_B != 0
    }
    pub fn touch_thumbstick(&self) -> bool {
        self.buttons & Self::TOUCH_THUMBSTICK != 0
    }
    pub fn touch_trigger(&self) -> bool {
        self.buttons & Self::TOUCH_TRIGGER != 0
    }
    pub fn touch_thumbrest(&self) -> bool {
        self.buttons & Self::TOUCH_THUMBREST != 0
    }
}

#[derive(Clone, Debug, Default, SerBin, DeBin)]
pub struct XrHand {
    pub flags: u8,
    pub joints: [Pose; Self::JOINT_COUNT],
    pub tips: [f32; 5],
    pub tips_active: u8,
    pub aim_pose: Pose,
    pub pinch: [u8; 4],
}

impl XrHand {
    pub const GRAB_ACTIVE: u8 = 1 << 5;
    pub const GRAB_ACTIVE_THRESHOLD: f32 = 0.55;
    const MIN_PALM_SPAN_METERS: f32 = 0.01;
    const MAX_PALM_SPAN_METERS: f32 = 0.22;
    const MAX_JOINT_DISTANCE_FROM_PALM_METERS: f32 = 0.28;
    const MAX_FINGER_SEGMENT_LENGTH_METERS: f32 = 0.12;
    const MAX_TIP_LENGTH_METERS: f32 = 0.10;

    pub fn in_view(&self) -> bool {
        self.flags & Self::IN_VIEW != 0
    }
    pub fn aim_valid(&self) -> bool {
        self.flags & Self::AIM_VALID != 0
    }
    pub fn menu_pressed(&self) -> bool {
        self.flags & Self::MENU_PRESSED != 0
    }
    //pub fn system_gesture(&self)->bool{self.flags & Self::SYSTEM_GESTURE != 0}
    pub fn dominant_hand(&self) -> bool {
        self.flags & Self::DOMINANT_HAND != 0
    }

    pub fn pinch_index(&self) -> bool {
        self.flags & Self::PINCH_INDEX != 0
    }
    pub fn pinch_middle(&self) -> bool {
        self.flags & Self::PINCH_MIDDLE != 0
    }
    pub fn pinch_ring(&self) -> bool {
        self.flags & Self::PINCH_RING != 0
    }
    pub fn pinch_little(&self) -> bool {
        self.flags & Self::PINCH_LITTLE != 0
    }

    pub fn pinch_only_little(&self) -> bool {
        self.pinch[Self::PINCH_STRENGTH_INDEX] < 100
            && self.pinch[Self::PINCH_STRENGTH_MIDDLE] < 100
            && self.pinch[Self::PINCH_STRENGTH_RING] < 100
            && self.pinch[Self::PINCH_STRENGTH_LITTLE] > 160
    }
    pub fn pinch_only_index(&self) -> bool {
        self.pinch[Self::PINCH_STRENGTH_INDEX] > 160
            && self.pinch[Self::PINCH_STRENGTH_MIDDLE] < 100
            && self.pinch[Self::PINCH_STRENGTH_RING] < 100
            && self.pinch[Self::PINCH_STRENGTH_LITTLE] < 100
    }

    pub fn pinch_not_index(&self) -> bool {
        self.pinch[Self::PINCH_STRENGTH_INDEX] < 100
            && (self.pinch[Self::PINCH_STRENGTH_MIDDLE] > 160
                || self.pinch[Self::PINCH_STRENGTH_RING] > 160
                || self.pinch[Self::PINCH_STRENGTH_LITTLE] > 160)
    }

    pub fn pinch_strength_index(&self) -> f32 {
        self.pinch[Self::PINCH_STRENGTH_INDEX] as f32 / u8::MAX as f32
    }
    pub fn pinch_strength_middle(&self) -> f32 {
        self.pinch[Self::PINCH_STRENGTH_MIDDLE] as f32 / u8::MAX as f32
    }
    pub fn pinch_strength_ring(&self) -> f32 {
        self.pinch[Self::PINCH_STRENGTH_RING] as f32 / u8::MAX as f32
    }
    pub fn pinch_strength_pinky(&self) -> f32 {
        self.pinch[Self::PINCH_STRENGTH_LITTLE] as f32 / u8::MAX as f32
    }

    pub fn grabbing(&self) -> bool {
        self.tips_active & Self::GRAB_ACTIVE != 0
    }

    fn finite_joint_pose(&self, joint: usize) -> Option<Pose> {
        let pose = self.joints[joint];
        pose.is_finite().then_some(pose)
    }

    pub fn tracking_pose(&self) -> Option<Pose> {
        if !self.in_view() {
            return None;
        }
        let center = self.finite_joint_pose(Self::CENTER)?;
        let wrist = self.finite_joint_pose(Self::WRIST)?.position;
        let palm_span = (center.position - wrist).length();
        if !palm_span.is_finite()
            || !(Self::MIN_PALM_SPAN_METERS..=Self::MAX_PALM_SPAN_METERS).contains(&palm_span)
        {
            return None;
        }
        Some(Pose::new(
            center.orientation,
            center.position * 0.78 + wrist * 0.22,
        ))
    }

    pub fn joint_pose_checked(&self, joint: usize) -> Option<Pose> {
        let pose = self.finite_joint_pose(joint)?;
        if joint == Self::CENTER || joint == Self::WRIST {
            self.tracking_pose()?;
            return Some(pose);
        }
        let palm_center = self.tracking_pose()?.position;
        let distance = (pose.position - palm_center).length();
        (distance.is_finite() && distance <= Self::MAX_JOINT_DISTANCE_FROM_PALM_METERS)
            .then_some(pose)
    }

    fn joint_position_near_palm(&self, joint: usize, palm_center: Vec3f) -> Option<Vec3f> {
        let position = self.finite_joint_pose(joint)?.position;
        let distance = (position - palm_center).length();
        (distance.is_finite() && distance <= Self::MAX_JOINT_DISTANCE_FROM_PALM_METERS)
            .then_some(position)
    }

    fn tip_length_checked(&self, tip: usize) -> Option<f32> {
        let length = self.tips[tip];
        (length.is_finite() && (0.0..=Self::MAX_TIP_LENGTH_METERS).contains(&length))
            .then_some(length)
    }

    pub fn tip_pos_checked(&self, tip: usize) -> Option<Vec3f> {
        let palm_center = self.tracking_pose()?.position;
        let knuckle = Self::END_KNUCKLES.get(tip).copied()?;
        let knuckle_pose = self.finite_joint_pose(knuckle)?;
        let tip_length = self.tip_length_checked(tip)?;
        let tip_position = knuckle_pose
            .to_mat4()
            .transform_vec4(vec4(0.0, 0.0, -tip_length, 1.0))
            .to_vec3f();
        let distance = (tip_position - palm_center).length();
        (tip_position.is_finite()
            && distance.is_finite()
            && distance <= Self::MAX_JOINT_DISTANCE_FROM_PALM_METERS)
            .then_some(tip_position)
    }

    pub fn joint_chain_positions(&self, chain: &[usize]) -> Option<SmallVec<[Vec3f; 4]>> {
        if chain.is_empty() {
            return None;
        }
        let palm_center = self.tracking_pose()?.position;
        let mut points = SmallVec::<[Vec3f; 4]>::new();
        for &joint in chain {
            points.push(self.joint_position_near_palm(joint, palm_center)?);
        }
        for pair in points.windows(2) {
            let segment = pair[1] - pair[0];
            let length = segment.length();
            if !length.is_finite()
                || length <= 0.0001
                || length > Self::MAX_FINGER_SEGMENT_LENGTH_METERS
            {
                return None;
            }
        }
        Some(points)
    }

    pub fn finger_chain_positions(&self, tip: usize) -> Option<SmallVec<[Vec3f; 5]>> {
        let mut points = SmallVec::<[Vec3f; 5]>::new();
        points.extend(self.joint_chain_positions(Self::finger_chain(tip))?);
        let tip_position = self.tip_pos_checked(tip)?;
        let tail_length = (tip_position - *points.last()?).length();
        if !tail_length.is_finite()
            || tail_length <= 0.0001
            || tail_length > Self::MAX_FINGER_SEGMENT_LENGTH_METERS
        {
            return None;
        }
        points.push(tip_position);
        Some(points)
    }

    fn index_grab_release_metrics(&self) -> Option<(f32, f32, f32)> {
        let points = self.finger_chain_positions(Self::INDEX_TIP)?;
        let [base, knuckle1, knuckle2, knuckle3, tip] = [
            points[0], points[1], points[2], points[3], points[4],
        ];
        let seg0 = Self::normalized_segment_direction(base, knuckle1)?;
        let seg1 = Self::normalized_segment_direction(knuckle1, knuckle2)?;
        let seg2 = Self::normalized_segment_direction(knuckle2, knuckle3)?;
        let seg3 = Self::normalized_segment_direction(knuckle3, tip)?;

        let chain_length = (knuckle1 - base).length()
            + (knuckle2 - knuckle1).length()
            + (knuckle3 - knuckle2).length()
            + (tip - knuckle3).length();
        if chain_length <= 0.0001 {
            return None;
        }

        let direct_length = (tip - base).length();
        let segment_dots = [seg0.dot(seg1), seg1.dot(seg2), seg2.dot(seg3)];
        let max_bend_angle_degrees = segment_dots
            .into_iter()
            .map(|dot| dot.clamp(-1.0, 1.0).acos().to_degrees())
            .fold(0.0, f32::max);
        let straightness = segment_dots.into_iter().fold(1.0, f32::min);
        let extension_ratio = direct_length / chain_length;
        Some((max_bend_angle_degrees, straightness, extension_ratio))
    }

    pub fn grab_intent(&self) -> bool {
        if !self.grabbing() {
            return false;
        }
        let Some((max_bend_angle_degrees, straightness, extension_ratio)) =
            self.index_grab_release_metrics()
        else {
            return true;
        };
        !(max_bend_angle_degrees <= 36.0
            && straightness >= 0.78
            && extension_ratio >= 0.90)
    }

    pub fn pinch_anchor_pose(&self) -> Option<Pose> {
        let pinch_active = self.pinch_index()
            || self.pinch_strength_index() >= Self::GRAB_ACTIVE_THRESHOLD;
        if !pinch_active {
            return None;
        }
        let palm_pose = self.tracking_pose()?;
        let thumb_tip = self.tip_pos_checked(Self::THUMB_TIP)?;
        let index_tip = self.tip_pos_checked(Self::INDEX_TIP)?;
        Some(Pose::new(
            palm_pose.orientation,
            (thumb_tip + index_tip) * 0.5,
        ))
    }

    pub const IN_VIEW: u8 = 1 << 0;
    pub const AIM_VALID: u8 = 1 << 1;
    pub const PINCH_INDEX: u8 = 1 << 2;
    pub const PINCH_MIDDLE: u8 = 1 << 3;
    pub const PINCH_RING: u8 = 1 << 4;
    pub const PINCH_LITTLE: u8 = 1 << 5;
    pub const DOMINANT_HAND: u8 = 1 << 6;
    pub const MENU_PRESSED: u8 = 1 << 7;

    pub const PINCH_STRENGTH_INDEX: usize = 0;
    pub const PINCH_STRENGTH_MIDDLE: usize = 1;
    pub const PINCH_STRENGTH_RING: usize = 2;
    pub const PINCH_STRENGTH_LITTLE: usize = 3;

    pub const JOINT_COUNT: usize = 21;
    pub const CENTER: usize = 0;
    pub const WRIST: usize = 1;
    pub const THUMB_BASE: usize = 2;
    pub const THUMB_KNUCKLE1: usize = 3;
    pub const THUMB_KNUCKLE2: usize = 4;
    pub const INDEX_BASE: usize = 5;
    pub const INDEX_KNUCKLE1: usize = 6;
    pub const INDEX_KNUCKLE2: usize = 7;
    pub const INDEX_KNUCKLE3: usize = 8;
    pub const MIDDLE_BASE: usize = 9;
    pub const MIDDLE_KNUCKLE1: usize = 10;
    pub const MIDDLE_KNUCKLE2: usize = 11;
    pub const MIDDLE_KNUCKLE3: usize = 12;
    pub const RING_BASE: usize = 13;
    pub const RING_KNUCKLE1: usize = 14;
    pub const RING_KNUCKLE2: usize = 15;
    pub const RING_KNUCKLE3: usize = 16;
    pub const LITTLE_BASE: usize = 17;
    pub const LITTLE_KNUCKLE1: usize = 18;
    pub const LITTLE_KNUCKLE2: usize = 19;
    pub const LITTLE_KNUCKLE3: usize = 20;

    pub const END_KNUCKLES: [usize; 5] = [
        Self::THUMB_KNUCKLE2,
        Self::INDEX_KNUCKLE3,
        Self::MIDDLE_KNUCKLE3,
        Self::RING_KNUCKLE3,
        Self::LITTLE_KNUCKLE3,
    ];

    pub fn base_knuckles(&self) -> [&Pose; 5] {
        [
            &self.joints[XrHand::THUMB_BASE],
            &self.joints[XrHand::INDEX_BASE],
            &self.joints[XrHand::MIDDLE_BASE],
            &self.joints[XrHand::RING_BASE],
            &self.joints[XrHand::LITTLE_BASE],
        ]
    }

    pub fn end_knuckles(&self) -> [&Pose; 5] {
        [
            &self.joints[XrHand::THUMB_KNUCKLE2],
            &self.joints[XrHand::INDEX_KNUCKLE3],
            &self.joints[XrHand::MIDDLE_KNUCKLE3],
            &self.joints[XrHand::RING_KNUCKLE3],
            &self.joints[XrHand::LITTLE_KNUCKLE3],
        ]
    }

    pub const THUMB_TIP: usize = 0;
    pub const INDEX_TIP: usize = 1;
    pub const MIDDLE_TIP: usize = 2;
    pub const RING_TIP: usize = 3;
    pub const LITTLE_TIP: usize = 4;
    pub const TIP_COUNT: usize = 5;

    pub fn tip_active(&self, tip: usize) -> bool {
        self.tips_active & (1 << tip) != 0
    }

    fn normalized_segment_direction(a: Vec3f, b: Vec3f) -> Option<Vec3f> {
        let delta = b - a;
        (delta.length() > 0.0001).then_some(delta.normalize())
    }

    fn tip_pos(&self, tip: usize, knuckle: usize) -> Vec3f {
        let pos = vec4(0.0, 0.0, -self.tips[tip], 1.0);
        self.joints[knuckle]
            .to_mat4()
            .transform_vec4(pos)
            .to_vec3f()
    }

    pub fn tip_pos_thumb(&self) -> Vec3f {
        self.tip_pos(0, XrHand::THUMB_KNUCKLE2)
    }
    pub fn tip_pos_index(&self) -> Vec3f {
        self.tip_pos(Self::INDEX_TIP, XrHand::INDEX_KNUCKLE3)
    }
    pub fn tip_pos_middle(&self) -> Vec3f {
        self.tip_pos(Self::MIDDLE_TIP, XrHand::MIDDLE_KNUCKLE3)
    }
    pub fn tip_pos_ring(&self) -> Vec3f {
        self.tip_pos(Self::RING_TIP, XrHand::RING_KNUCKLE3)
    }
    pub fn tip_pos_little(&self) -> Vec3f {
        self.tip_pos(Self::LITTLE_TIP, XrHand::LITTLE_KNUCKLE3)
    }

    pub fn tip_pos_for_index(&self, tip: usize) -> Vec3f {
        match tip {
            Self::THUMB_TIP => self.tip_pos_thumb(),
            Self::INDEX_TIP => self.tip_pos_index(),
            Self::MIDDLE_TIP => self.tip_pos_middle(),
            Self::RING_TIP => self.tip_pos_ring(),
            Self::LITTLE_TIP => self.tip_pos_little(),
            _ => self.tip_pos_index(),
        }
    }

    fn finger_chain(tip: usize) -> &'static [usize] {
        match tip {
            Self::THUMB_TIP => &[Self::THUMB_BASE, Self::THUMB_KNUCKLE1, Self::THUMB_KNUCKLE2],
            Self::INDEX_TIP => &[
                Self::INDEX_BASE,
                Self::INDEX_KNUCKLE1,
                Self::INDEX_KNUCKLE2,
                Self::INDEX_KNUCKLE3,
            ],
            Self::MIDDLE_TIP => &[
                Self::MIDDLE_BASE,
                Self::MIDDLE_KNUCKLE1,
                Self::MIDDLE_KNUCKLE2,
                Self::MIDDLE_KNUCKLE3,
            ],
            Self::RING_TIP => &[
                Self::RING_BASE,
                Self::RING_KNUCKLE1,
                Self::RING_KNUCKLE2,
                Self::RING_KNUCKLE3,
            ],
            Self::LITTLE_TIP => &[
                Self::LITTLE_BASE,
                Self::LITTLE_KNUCKLE1,
                Self::LITTLE_KNUCKLE2,
                Self::LITTLE_KNUCKLE3,
            ],
            _ => &[
                Self::INDEX_BASE,
                Self::INDEX_KNUCKLE1,
                Self::INDEX_KNUCKLE2,
                Self::INDEX_KNUCKLE3,
            ],
        }
    }

    pub fn finger_max_bend_angle_degrees(&self, tip: usize) -> Option<f32> {
        if !self.tip_active(tip) {
            return None;
        }
        let points = self.finger_chain_positions(tip)?;
        if points.len() < 3 {
            return None;
        }

        let mut directions = SmallVec::<[Vec3f; 4]>::new();
        for pair in points.windows(2) {
            directions.push(Self::normalized_segment_direction(pair[0], pair[1])?);
        }

        directions.windows(2).fold(None, |max_angle, pair| {
            let dot = pair[0].dot(pair[1]).clamp(-1.0, 1.0);
            let angle = dot.acos().to_degrees();
            Some(max_angle.map_or(angle, |current: f32| current.max(angle)))
        })
    }

    pub fn finger_is_active_for_touch(&self, tip: usize, max_bend_angle_degrees: f32) -> bool {
        self.in_view()
            && self.tip_active(tip)
            && self
                .finger_max_bend_angle_degrees(tip)
                .is_some_and(|angle| angle <= max_bend_angle_degrees)
    }
}

#[cfg(test)]
mod local_event_tests {
    use super::*;

    fn point_pose(base: Vec3f, z: f32) -> Pose {
        Pose::new(Quat::default(), base + vec3f(0.0, 0.0, z))
    }

    fn make_pointing_hand() -> XrHand {
        let base = vec3f(0.20, 1.22, -0.22);
        let mut hand = XrHand::default();
        hand.flags = XrHand::IN_VIEW | XrHand::AIM_VALID;
        hand.tips_active = XrHand::GRAB_ACTIVE | (1 << XrHand::INDEX_TIP);
        hand.tips[XrHand::INDEX_TIP] = 0.038;
        hand.joints[XrHand::CENTER] = Pose::new(Quat::default(), base + vec3f(0.0, -0.03, 0.03));
        hand.joints[XrHand::WRIST] = Pose::new(Quat::default(), base + vec3f(0.0, -0.05, 0.08));
        hand.joints[XrHand::INDEX_BASE] = point_pose(base, 0.0);
        hand.joints[XrHand::INDEX_KNUCKLE1] = point_pose(base, -0.041);
        hand.joints[XrHand::INDEX_KNUCKLE2] = point_pose(base + vec3f(0.001, 0.002, 0.0), -0.082);
        hand.joints[XrHand::INDEX_KNUCKLE3] = point_pose(base + vec3f(0.002, 0.004, 0.0), -0.122);
        hand
    }

    fn make_index_pinch_hand() -> XrHand {
        let base = vec3f(0.20, 1.22, -0.22);
        let mut hand = XrHand::default();
        hand.flags = XrHand::IN_VIEW | XrHand::AIM_VALID | XrHand::PINCH_INDEX;
        hand.tips_active = (1 << XrHand::THUMB_TIP) | (1 << XrHand::INDEX_TIP);
        hand.tips[XrHand::THUMB_TIP] = 0.028;
        hand.tips[XrHand::INDEX_TIP] = 0.034;
        hand.pinch[XrHand::PINCH_STRENGTH_INDEX] = 220;
        hand.joints[XrHand::CENTER] = Pose::new(Quat::default(), base + vec3f(0.0, -0.03, 0.03));
        hand.joints[XrHand::WRIST] = Pose::new(Quat::default(), base + vec3f(0.0, -0.05, 0.08));
        hand.joints[XrHand::THUMB_BASE] = Pose::new(Quat::default(), base + vec3f(-0.020, -0.008, -0.010));
        hand.joints[XrHand::THUMB_KNUCKLE1] =
            Pose::new(Quat::default(), base + vec3f(-0.010, 0.000, -0.030));
        hand.joints[XrHand::THUMB_KNUCKLE2] =
            Pose::new(Quat::default(), base + vec3f(0.000, 0.008, -0.040));
        hand.joints[XrHand::INDEX_BASE] = point_pose(base + vec3f(0.008, 0.0, 0.0), -0.005);
        hand.joints[XrHand::INDEX_KNUCKLE1] = point_pose(base + vec3f(0.010, 0.004, 0.0), -0.032);
        hand.joints[XrHand::INDEX_KNUCKLE2] = point_pose(base + vec3f(0.012, 0.010, 0.0), -0.054);
        hand.joints[XrHand::INDEX_KNUCKLE3] = point_pose(base + vec3f(0.013, 0.018, 0.0), -0.068);
        hand
    }

    fn make_curled_grab_hand() -> XrHand {
        let base = vec3f(0.20, 1.22, -0.22);
        let mut hand = XrHand::default();
        hand.flags = XrHand::IN_VIEW | XrHand::AIM_VALID;
        hand.tips_active = XrHand::GRAB_ACTIVE | (1 << XrHand::INDEX_TIP);
        hand.tips[XrHand::INDEX_TIP] = 0.030;
        hand.joints[XrHand::CENTER] = Pose::new(Quat::default(), base + vec3f(0.0, -0.03, 0.03));
        hand.joints[XrHand::WRIST] = Pose::new(Quat::default(), base + vec3f(0.0, -0.05, 0.08));
        hand.joints[XrHand::INDEX_BASE] = point_pose(base, 0.0);
        hand.joints[XrHand::INDEX_KNUCKLE1] = point_pose(base, -0.030);
        hand.joints[XrHand::INDEX_KNUCKLE2] =
            Pose::new(Quat::default(), base + vec3f(0.018, -0.012, -0.040));
        hand.joints[XrHand::INDEX_KNUCKLE3] =
            Pose::new(Quat::default(), base + vec3f(0.034, -0.030, -0.032));
        hand
    }

    fn make_sparse_tracking_hand() -> XrHand {
        let base = vec3f(0.20, 1.22, -0.22);
        let mut hand = XrHand::default();
        hand.flags = XrHand::IN_VIEW | XrHand::AIM_VALID;
        hand.tips_active = XrHand::GRAB_ACTIVE;
        hand.joints[XrHand::CENTER] = Pose::new(Quat::default(), base + vec3f(0.0, -0.03, 0.03));
        hand.joints[XrHand::WRIST] = Pose::new(Quat::default(), base + vec3f(0.0, -0.05, 0.08));
        hand.aim_pose = Pose::new(Quat::default(), base + vec3f(0.0, 0.0, -0.12));
        hand
    }

    #[test]
    fn grab_intent_releases_for_clearly_pointing_index_even_when_raw_grab_bit_is_set() {
        let hand = make_pointing_hand();
        assert!(hand.grabbing());
        assert!(!hand.grab_intent());
    }

    #[test]
    fn grab_intent_keeps_curled_hand_as_active_grab() {
        let hand = make_curled_grab_hand();
        assert!(hand.grabbing());
        assert!(hand.grab_intent());
    }

    #[test]
    fn finger_chain_positions_reject_sparse_default_joint_sample() {
        let hand = make_sparse_tracking_hand();
        assert!(hand.tracking_pose().is_some());
        assert!(hand.finger_chain_positions(XrHand::INDEX_TIP).is_none());
    }

    #[test]
    fn pinch_anchor_pose_uses_thumb_index_midpoint_instead_of_palm_center() {
        let hand = make_index_pinch_hand();
        let pinch_pose = hand.pinch_anchor_pose().expect("pinch anchor should exist");
        let palm_pose = hand.tracking_pose().expect("tracking pose should exist");
        let thumb_tip = hand
            .tip_pos_checked(XrHand::THUMB_TIP)
            .expect("thumb tip should be valid");
        let index_tip = hand
            .tip_pos_checked(XrHand::INDEX_TIP)
            .expect("index tip should be valid");
        let expected_midpoint = (thumb_tip + index_tip) * 0.5;
        assert!((pinch_pose.position - expected_midpoint).length() < 0.001);
        assert!((pinch_pose.position - palm_pose.position).length() > 0.01);
    }
}

#[derive(Clone, Debug)]
pub struct XrFingerTip {
    pub index: usize,
    pub is_left: bool,
    pub active: bool,
    pub interactive: bool,
    pub pos: Vec3f,
    pub ray_dir: Vec3f,
    pub touch_z: f32,
    pub handled: Cell<Area>,
}

#[derive(Clone, Debug)]
pub struct XrLocalEvent {
    pub finger_tips: SmallVec<[XrFingerTip; 10]>,
    pub space_transform: Mat4f,
    pub digit_namespace: u64,
    pub update: XrUpdateEvent,
    pub modifiers: KeyModifiers,
    pub time: f64,
}

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, PartialEq)]
pub struct XrAnchor {
    pub left: Vec3f,
    pub right: Vec3f,
}

impl XrAnchor {
    pub fn to_quat(&self) -> Quat {
        let mut forward = self.right - self.left;
        forward.y = 0.0;
        Quat::look_rotation(forward, vec3(0.0, 1.0, 0.0))
    }

    pub fn to_quat_rev(&self) -> Quat {
        let mut forward = self.left - self.right;
        forward.y = 0.0;
        Quat::look_rotation(forward, vec3(0.0, 1.0, 0.0))
    }

    pub fn to_mat4(&self) -> Mat4f {
        self.to_pose().to_mat4()
    }

    pub fn to_pose(&self) -> Pose {
        Pose {
            position: (self.left + self.right) / 2.0,
            orientation: self.to_quat(),
        }
    }

    pub fn mapping_to(&self, other: &XrAnchor) -> Mat4f {
        Mat4f::mul(
            &self.to_pose().to_mat4().invert(),
            &other.to_pose().to_mat4(),
        )
    }
}

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, PartialEq)]
pub struct XrSyncAnchor {
    pub id: u32,
    pub captured_at: f64,
    pub anchor: XrAnchor,
}

#[derive(Clone, Debug, Default, SerBin, DeBin)]
pub struct XrState {
    pub time: f64,
    pub head_pose: Pose,
    pub order_counter: u8,
    pub anchor: Option<XrAnchor>,
    pub sync_anchor: Option<XrSyncAnchor>,
    pub left_controller: XrController,
    pub right_controller: XrController,
    pub left_hand: XrHand,
    pub right_hand: XrHand,
}
impl XrState {
    pub fn from_lerp(a: &XrState, b: &XrState, f: f32) -> Self {
        Self {
            order_counter: b.order_counter,
            time: (b.time - a.time) * f as f64 + a.time,
            head_pose: Pose::from_lerp(a.head_pose, b.head_pose, f),
            anchor: b.anchor,
            sync_anchor: b.sync_anchor,
            left_controller: b.left_controller.clone(),
            right_controller: b.right_controller.clone(),
            left_hand: b.left_hand.clone(),
            right_hand: b.right_hand.clone(),
        }
    }

    pub fn vec_in_head_space(&self, pos: Vec3f) -> Vec3f {
        self.head_pose
            .to_mat4()
            .transform_vec4(pos.to_vec4())
            .to_vec3f()
    }

    pub fn scene_anchor_pose(&self) -> Option<Pose> {
        if let Some(_anchor) = &self.anchor {
            // lets construct a pose from 2 positions
            None
        } else {
            None
        }
    }

    pub fn hands(&self) -> [&XrHand; 2] {
        [&self.left_hand, &self.right_hand]
    }
    pub fn controllers(&self) -> [&XrController; 2] {
        [&self.left_controller, &self.right_controller]
    }
}

#[derive(Clone, Debug)]
pub struct XrUpdateEvent {
    pub state: Rc<XrState>,
    pub last: Rc<XrState>,
}

impl XrUpdateEvent {
    pub fn clicked_x(&self) -> bool {
        self.state.left_controller.click_x() && !self.last.left_controller.click_x()
    }
    pub fn clicked_y(&self) -> bool {
        self.state.left_controller.click_y() && !self.last.left_controller.click_y()
    }
    pub fn clicked_a(&self) -> bool {
        self.state.right_controller.click_a() && !self.last.right_controller.click_a()
    }
    pub fn clicked_b(&self) -> bool {
        self.state.right_controller.click_b() && !self.last.right_controller.click_b()
    }
    pub fn clicked_left_thumbstick(&self) -> bool {
        self.state.left_controller.click_thumbstick()
            && !self.last.left_controller.click_thumbstick()
    }
    pub fn clicked_right_thumbstick(&self) -> bool {
        self.state.right_controller.click_thumbstick()
            && !self.last.right_controller.click_thumbstick()
    }
    pub fn clicked_menu(&self) -> bool {
        self.state.left_controller.click_menu() && !self.last.left_controller.click_menu()
    }
    pub fn menu_pressed(&self) -> bool {
        self.state.left_hand.menu_pressed() && !self.last.left_hand.menu_pressed()
    }
}

impl XrLocalEvent {
    const XR_TOUCH_ACTIVE_FINGER_MAX_BEND_DEGREES: f32 = 70.0;

    fn fingertip_slot(is_left: bool, index: usize) -> usize {
        index + if is_left { XrHand::TIP_COUNT } else { 0 }
    }

    fn fingertip_digit_id(namespace: u64, is_left: bool, index: usize) -> DigitId {
        let slot = Self::fingertip_slot(is_left, index) as u64;
        live_id_num!(xrfinger, namespace.wrapping_mul(16).wrapping_add(slot)).into()
    }

    fn fingertip_device(is_left: bool, index: usize) -> DigitDevice {
        DigitDevice::XrHand { is_left, index }
    }

    fn fingertip_slot_for_digit(&self, digit_id: DigitId) -> Option<(bool, usize)> {
        for is_left in [false, true] {
            for index in 0..XrHand::TIP_COUNT {
                if Self::fingertip_digit_id(self.digit_namespace, is_left, index) == digit_id {
                    return Some((is_left, index));
                }
            }
        }
        None
    }

    fn tip_for_digit(&self, digit_id: DigitId) -> Option<&XrFingerTip> {
        self.finger_tips.iter().find(|tip| {
            Self::fingertip_digit_id(self.digit_namespace, tip.is_left, tip.index) == digit_id
        })
    }

    fn collect_hand_tips(
        finger_tips: &mut SmallVec<[XrFingerTip; 10]>,
        hand: &XrHand,
        is_left: bool,
        inv: &Mat4f,
    ) {
        if !hand.in_view() {
            return;
        }
        for index in 0..XrHand::TIP_COUNT {
            if !hand.tip_active(index) {
                continue;
            }
            let Some(tip_position) = hand.tip_pos_checked(index) else {
                continue;
            };
            let pos = inv
                .transform_vec4(tip_position.to_vec4())
                .to_vec3f();
            finger_tips.push(XrFingerTip {
                index,
                is_left,
                active: hand.finger_is_active_for_touch(
                    index,
                    Self::XR_TOUCH_ACTIVE_FINGER_MAX_BEND_DEGREES,
                ),
                interactive: true,
                pos,
                ray_dir: vec3f(0.0, 0.0, -1.0),
                touch_z: pos.z,
                handled: Cell::new(Area::Empty),
            });
        }
    }

    fn tip_is_touching_for_down(tip: &XrFingerTip) -> bool {
        tip.active && tip.touch_z <= XR_TOUCH_DOWN_FRONT && tip.touch_z >= XR_TOUCH_DOWN_BACK
    }

    fn tip_is_touching_for_capture(tip: &XrFingerTip) -> bool {
        tip.touch_z <= XR_TOUCH_CAPTURE_FRONT && tip.touch_z >= XR_TOUCH_CAPTURE_BACK
    }

    pub fn from_update_event(e: &XrUpdateEvent, mat: &Mat4f) -> XrLocalEvent {
        let inv = mat.invert();
        let mut finger_tips = SmallVec::new();
        Self::collect_hand_tips(&mut finger_tips, &e.state.left_hand, true, &inv);
        Self::collect_hand_tips(&mut finger_tips, &e.state.right_hand, false, &inv);

        XrLocalEvent {
            finger_tips,
            space_transform: *mat,
            digit_namespace: 0,
            modifiers: Default::default(),
            time: e.state.time,
            update: e.clone(),
        }
    }

    pub fn process_end(&self, cx: &mut Cx) {
        for is_left in [true, false] {
            for index in 0..XrHand::TIP_COUNT {
                let digit_id = Self::fingertip_digit_id(self.digit_namespace, is_left, index);
                if let Some(tip) = self
                    .finger_tips
                    .iter()
                    .find(|tip| tip.is_left == is_left && tip.index == index)
                {
                    if tip.touch_z > XR_TOUCH_REARM_FRONT {
                        cx.fingers.xr_poke_unlock(digit_id);
                    }
                    if tip.active || Self::tip_is_touching_for_capture(tip) {
                        cx.fingers.cycle_hover_area(digit_id);
                    } else {
                        cx.fingers.remove_hover(digit_id);
                    }
                    if !Self::tip_is_touching_for_capture(tip) {
                        cx.fingers.release_digit(digit_id);
                    }
                } else {
                    cx.fingers.release_digit(digit_id);
                    cx.fingers.remove_hover(digit_id);
                    cx.fingers.xr_poke_unlock(digit_id);
                }
            }
        }
        cx.fingers.switch_captures();
    }

    pub fn hits_with_options_and_test<F>(
        &self,
        cx: &mut Cx,
        area: Area,
        options: HitOptions,
        hit_test: F,
    ) -> Hit
    where
        F: Fn(Vec2d, &Rect, &Option<Inset>) -> bool,
    {
        if cx.fingers.test_sweep_lock(options.sweep_area) {
            return Hit::Nothing;
        }

        let rect = area.clipped_rect(cx);
        if let Some(digit_id) = cx.fingers.find_digit_for_captured_area(area) {
            if let Some(tip) = self.tip_for_digit(digit_id) {
                let tap_count = cx.fingers.tap_count();
                let abs = tip.pos.to_vec2().into();
                let device = Self::fingertip_device(tip.is_left, tip.index);
                let queue_hover;
                let hit = {
                    let Some(capture) = cx.fingers.find_digit_capture(digit_id) else {
                        return Hit::Nothing;
                    };
                    let abs_start = capture.abs_start;
                    let capture_time = capture.time;
                    let has_long_press_occurred = capture.has_long_press_occurred;
                    let rect_check = hit_test(abs, &rect, &options.margin);
                    let layout_shift_fallback = (self.time - capture_time
                        < crate::event::finger::TAP_COUNT_TIME)
                        && ((abs - abs_start).length() < crate::event::finger::TAP_COUNT_DISTANCE);
                    let release_is_over = rect_check || layout_shift_fallback;
                    queue_hover = rect_check;
                    if Self::tip_is_touching_for_capture(tip) {
                        if !rect_check {
                            if capture.switch_capture.is_none() {
                                capture.switch_capture = Some(Area::Empty);
                            }
                            Hit::FingerUp(crate::FingerUpEvent {
                                window_id: CxWindowPool::id_zero(),
                                abs,
                                abs_start,
                                capture_time,
                                time: self.time,
                                digit_id,
                                device,
                                has_long_press_occurred,
                                tap_count,
                                modifiers: self.modifiers,
                                rect,
                                is_over: false,
                                is_sweep: true,
                            })
                        } else {
                            Hit::FingerMove(crate::FingerMoveEvent {
                                window_id: CxWindowPool::id_zero(),
                                abs,
                                digit_id,
                                device,
                                has_long_press_occurred,
                                tap_count,
                                modifiers: self.modifiers,
                                time: self.time,
                                abs_start,
                                rect,
                                is_over: true,
                            })
                        }
                    } else {
                        Hit::FingerUp(crate::FingerUpEvent {
                            window_id: CxWindowPool::id_zero(),
                            abs,
                            abs_start,
                            capture_time,
                            time: self.time,
                            digit_id,
                            device,
                            has_long_press_occurred,
                            tap_count,
                            modifiers: self.modifiers,
                            rect,
                            is_over: release_is_over,
                            is_sweep: false,
                        })
                    }
                };
                if queue_hover {
                    cx.fingers.new_hover_area(digit_id, area);
                }
                return hit;
            } else if let Some(capture) = cx.fingers.find_area_capture(area) {
                let abs_start = capture.abs_start;
                let capture_time = capture.time;
                let has_long_press_occurred = capture.has_long_press_occurred;
                let (is_left, index) = self
                    .fingertip_slot_for_digit(digit_id)
                    .unwrap_or((false, XrHand::INDEX_TIP));
                return Hit::FingerUp(crate::FingerUpEvent {
                    window_id: CxWindowPool::id_zero(),
                    abs: abs_start,
                    abs_start,
                    capture_time,
                    time: self.time,
                    digit_id,
                    device: Self::fingertip_device(is_left, index),
                    has_long_press_occurred,
                    tap_count: cx.fingers.tap_count(),
                    modifiers: self.modifiers,
                    rect,
                    is_over: false,
                    is_sweep: false,
                });
            }
        }

        for tip in &self.finger_tips {
            if !tip.interactive || !Self::tip_is_touching_for_down(tip) {
                continue;
            }

            let digit_id = Self::fingertip_digit_id(self.digit_namespace, tip.is_left, tip.index);
            if cx.fingers.find_digit_capture(digit_id).is_some() {
                continue;
            }
            if cx.fingers.xr_poke_is_locked(digit_id) {
                continue;
            }
            if !options.capture_overload && !tip.handled.get().is_empty() {
                continue;
            }

            let abs = tip.pos.to_vec2().into();
            if hit_test(abs, &rect, &options.margin) {
                let device = Self::fingertip_device(tip.is_left, tip.index);
                cx.fingers.process_tap_count(abs, self.time);
                cx.fingers
                    .capture_digit(digit_id, area, options.sweep_area, self.time, abs);
                cx.fingers.xr_poke_lock(digit_id);
                cx.fingers.new_hover_area(digit_id, area);
                tip.handled.set(area);
                return Hit::FingerDown(FingerDownEvent {
                    window_id: CxWindowPool::id_zero(),
                    abs,
                    digit_id,
                    device,
                    tap_count: cx.fingers.tap_count(),
                    modifiers: self.modifiers,
                    time: self.time,
                    rect,
                });
            }
        }
        return Hit::Nothing;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tip(active: bool, touch_z: f32) -> XrFingerTip {
        XrFingerTip {
            index: XrHand::INDEX_TIP,
            is_left: false,
            active,
            interactive: true,
            pos: vec3f(0.0, 0.0, touch_z),
            ray_dir: vec3f(0.0, 0.0, -1.0),
            touch_z,
            handled: Cell::new(Area::Empty),
        }
    }

    #[test]
    fn xr_touch_down_still_requires_active_finger() {
        assert!(XrLocalEvent::tip_is_touching_for_down(&tip(true, 0.0)));
        assert!(!XrLocalEvent::tip_is_touching_for_down(&tip(false, 0.0)));
    }

    #[test]
    fn xr_touch_capture_allows_push_through_without_active_finger() {
        assert!(XrLocalEvent::tip_is_touching_for_capture(&tip(
            false, -15.0
        )));
        assert!(!XrLocalEvent::tip_is_touching_for_capture(&tip(
            false, -18.0
        )));
    }
}
