use {
    crate::makepad_math::*,
    std::rc::Rc,
};

#[derive(Clone, Debug, Default)]
pub struct XrButton {
    pub analog: f32,
    pub pressed: bool,
    pub last_change_time: f64,
    pub last_pressed: bool,
    pub touched: bool,
}

#[derive(Clone, Debug, Default)]
pub struct XrStick {
    pub x: f32,
    pub y: f32,
    pub pressed: bool,
    pub touched: bool,
    pub last: bool
}

#[derive(Clone, Debug, Default)]
pub struct XrController {
    pub active: bool,
    pub grip_pose: Pose,
    pub aim_pose: Pose,
    pub trigger: XrButton,
    pub grip: XrButton,
    pub a: XrButton,
    pub b: XrButton,
    pub x: XrButton,
    pub y: XrButton,
    pub menu: XrButton,
    pub stick: XrStick,
}

const XR_HAND_JOINT_COUNT: usize = 26;

#[derive(Clone, Debug, Default)]
pub struct XrHandJoint{
    pub tracked: bool,
    pub valid: bool,
    pub pose: Pose
}

#[derive(Clone, Debug, Default)]
pub struct XrHand{
    pub in_view: bool,
    pub joints: [XrHandJoint;XR_HAND_JOINT_COUNT]
}

#[derive(Clone, Debug, Default)]
pub struct XrState{
    pub time: f64,
    pub head_pose: Pose,
    pub left_controller: XrController,
    pub right_controller: XrController,
    pub left_hand: XrHand,
    pub right_hand: XrHand,
}
 
#[derive(Clone, Debug)]
pub struct XrUpdateEvent {
    pub state: Rc<XrState>,
    pub last: Rc<XrState>,
}
