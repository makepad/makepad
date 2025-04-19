use {
    crate::makepad_math::*,
};

#[derive(Clone, Debug, Default)]
pub struct XrButton {
    pub value: f32,
    pub pressed: bool
}

#[derive(Clone, Debug, Default)]
pub struct XrStick {
    pub x: f32,
    pub y: f32,
    pub pressed: bool
}

#[derive(Clone, Debug, Default)]
pub struct XrInput {
    pub active: bool,
    pub hand: u32,
    pub grip_pose: Pose,
    pub ray_pose: Pose,
    pub trigger: XrButton,
    pub grip: XrButton,
    pub a: XrButton,
    pub b: XrButton,
    pub x: XrButton,
    pub y: XrButton,
    pub menu: XrButton,
    pub stick: XrStick,
}

#[derive(Clone, Debug)]
pub struct XrState{
    pub time: f64,
    pub head_pose: Pose,
    pub left: XrInput,
    pub right: XrInput,
}
 
#[derive(Clone, Debug)]
pub struct XrUpdateEvent {
    pub now: XrState,
    pub last: XrState,
}
