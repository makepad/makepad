use {
    crate::{
        cx::Cx,
        event::DigitId,
        makepad_live_id::live_id_num,
        makepad_math::*,
        makepad_micro_serde::*,
        Area,
        CxWindowPool,
        DigitDevice,
        FingerDownEvent,
        Hit,
        HitOptions,
        Inset,
        KeyModifiers,
        LiveId,
        SmallVec,
    },
    std::{cell::Cell, rc::Rc},
};

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

    pub fn tip_active(&self, tip: usize) -> bool {
        self.tips_active & (1 << tip) != 0
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
}

#[derive(Clone, Debug)]
pub struct XrFingerTip {
    pub index: usize,
    pub is_left: bool,
    pub pos: Vec3f,
    pub handled: Cell<Area>,
}

#[derive(Clone, Debug)]
pub struct XrLocalEvent {
    pub finger_tips: SmallVec<[XrFingerTip; 10]>,
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

#[derive(Clone, Debug, Default, SerBin, DeBin)]
pub struct XrState {
    pub time: f64,
    pub head_pose: Pose,
    pub order_counter: u8,
    pub anchor: Option<XrAnchor>,
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
    const XR_TOUCH_DOWN_FRONT: f32 = 16.0;
    const XR_TOUCH_DOWN_BACK: f32 = -20.0;
    const XR_TOUCH_RELEASE_FRONT: f32 = 28.0;
    const XR_TOUCH_RELEASE_BACK: f32 = -36.0;

    fn fingertip_digit_id(is_left: bool) -> DigitId {
        live_id_num!(xrfinger, if is_left { 1 } else { 0 }).into()
    }

    fn fingertip_device(is_left: bool) -> DigitDevice {
        DigitDevice::XrHand {
            is_left,
            index: XrHand::INDEX_TIP,
        }
    }

    fn tip_for_digit(&self, digit_id: DigitId) -> Option<&XrFingerTip> {
        self.finger_tips
            .iter()
            .find(|tip| Self::fingertip_digit_id(tip.is_left) == digit_id)
    }

    fn collect_index_tip(finger_tips: &mut SmallVec<[XrFingerTip; 10]>, hand: &XrHand, is_left: bool, inv: &Mat4f) {
        if !hand.in_view() || !hand.tip_active(XrHand::INDEX_TIP) {
            return;
        }
        let pos = inv
            .transform_vec4(hand.tip_pos_index().to_vec4())
            .to_vec3f();
        finger_tips.push(XrFingerTip {
            index: XrHand::INDEX_TIP,
            is_left,
            pos,
            handled: Cell::new(Area::Empty),
        });
    }

    fn tip_is_touching_for_down(tip: &XrFingerTip) -> bool {
        tip.pos.z <= Self::XR_TOUCH_DOWN_FRONT && tip.pos.z >= Self::XR_TOUCH_DOWN_BACK
    }

    fn tip_is_touching_for_capture(tip: &XrFingerTip) -> bool {
        tip.pos.z <= Self::XR_TOUCH_RELEASE_FRONT && tip.pos.z >= Self::XR_TOUCH_RELEASE_BACK
    }

    pub fn from_update_event(e: &XrUpdateEvent, mat: &Mat4f) -> XrLocalEvent {
        let inv = mat.invert();
        let mut finger_tips = SmallVec::new();
        Self::collect_index_tip(&mut finger_tips, &e.state.left_hand, true, &inv);
        Self::collect_index_tip(&mut finger_tips, &e.state.right_hand, false, &inv);

        XrLocalEvent {
            finger_tips,
            modifiers: Default::default(),
            time: e.state.time,
            update: e.clone(),
        }
    }

    pub fn process_end(&self, cx: &mut Cx) {
        for is_left in [true, false] {
            let digit_id = Self::fingertip_digit_id(is_left);
            if let Some(tip) = self
                .finger_tips
                .iter()
                .find(|tip| tip.is_left == is_left)
            {
                cx.fingers.cycle_hover_area(digit_id);
                if !Self::tip_is_touching_for_capture(tip) {
                    cx.fingers.release_digit(digit_id);
                }
            } else {
                cx.fingers.release_digit(digit_id);
                cx.fingers.remove_hover(digit_id);
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
                if let Some(capture) = cx.fingers.find_digit_capture(digit_id) {
                    let abs = tip.pos.to_vec2().into();
                    let abs_start = capture.abs_start;
                    let capture_time = capture.time;
                    let has_long_press_occurred = capture.has_long_press_occurred;
                    let tap_count = cx.fingers.tap_count();
                    let device = Self::fingertip_device(tip.is_left);
                    let is_over = hit_test(abs, &rect, &options.margin);
                    if is_over {
                        cx.fingers.new_hover_area(digit_id, area);
                    }
                    if Self::tip_is_touching_for_capture(tip) {
                        return Hit::FingerMove(crate::FingerMoveEvent {
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
                            is_over,
                        });
                    }
                    return Hit::FingerUp(crate::FingerUpEvent {
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
                        is_over,
                        is_sweep: false,
                    });
                }
            } else if let Some(capture) = cx.fingers.find_area_capture(area) {
                let abs_start = capture.abs_start;
                let capture_time = capture.time;
                let has_long_press_occurred = capture.has_long_press_occurred;
                let is_left = digit_id == Self::fingertip_digit_id(true);
                return Hit::FingerUp(crate::FingerUpEvent {
                    window_id: CxWindowPool::id_zero(),
                    abs: abs_start,
                    abs_start,
                    capture_time,
                    time: self.time,
                    digit_id,
                    device: Self::fingertip_device(is_left),
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
            if !Self::tip_is_touching_for_down(tip) {
                continue;
            }

            let digit_id = Self::fingertip_digit_id(tip.is_left);
            if cx.fingers.find_digit_capture(digit_id).is_some() {
                continue;
            }
            if !options.capture_overload && !tip.handled.get().is_empty() {
                continue;
            }

            let abs = tip.pos.to_vec2().into();
            if hit_test(abs, &rect, &options.margin) {
                let device = Self::fingertip_device(tip.is_left);
                cx.fingers.process_tap_count(abs, self.time);
                cx.fingers
                    .capture_digit(digit_id, area, options.sweep_area, self.time, abs);
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
