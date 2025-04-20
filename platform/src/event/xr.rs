use {
    crate::makepad_math::*,
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

#[derive(Clone, Debug, Default)]
pub struct XrHand{
}

#[derive(Clone, Debug, Default)]
pub struct XrState{
    pub time: f64,
    pub head_pose: Pose,
    pub left: XrController,
    pub right: XrController,
    pub left_hand: XrHand,
    pub right_hand: XrHand,
}
 
#[derive(Clone, Debug)]
pub struct XrUpdateEvent {
    pub now: XrState,
    pub last: XrState,
}
