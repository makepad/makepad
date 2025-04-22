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



#[derive(Clone, Debug, Default)]
pub struct XrHandJoint{
    pub tracked: bool,
    pub valid: bool,
    pub pose: Pose
}

#[derive(Clone, Debug, Default)]
pub struct XrHand{
    pub in_view: bool,
    pub joints: [XrHandJoint;Self::JOINT_COUNT]
}

impl XrHand{
    pub fn is_tip(id:usize)->bool{
        match id{
            Self::THUMB_TIP | 
            Self::INDEX_TIP |
            Self::MIDDLE_TIP|
            Self::RING_TIP |
            Self::PINKY_TIP=>true,
            _=>false
        }
    }
    
    pub const JOINT_COUNT: usize = 26;
    pub const CENTER: usize = 0;
    pub const WRIST: usize = 1;
    pub const THUMB_BASE: usize = 2;
    pub const THUMB_KNUCKLE1: usize = 3;
    pub const THUMB_KNUCKLE2: usize = 4;
    pub const THUMB_TIP: usize = 5;
    pub const INDEX_BASE: usize = 6;
    pub const INDEX_KNUCKLE1: usize = 7;
    pub const INDEX_KNUCKLE2: usize = 8;
    pub const INDEX_KNUCKLE3: usize = 9;
    pub const INDEX_TIP: usize = 10;
    pub const MIDDLE_BASE: usize = 11;
    pub const MIDDLE_KNUCKLE1: usize = 12;
    pub const MIDDLE_KNUCKLE2: usize = 13;
    pub const MIDDLE_KNUCKLE3: usize = 14;
    pub const MIDDLE_TIP: usize = 15;
    pub const RING_BASE: usize = 16;
    pub const RING_KNUCKLE1: usize = 17;
    pub const RING_KNUCKLE2: usize = 18;
    pub const RING_KNUCKLE3: usize = 19;
    pub const RING_TIP: usize = 20;
    pub const PINKY_BASE: usize = 21;
    pub const PINKY_KNUCKLE1: usize = 22;
    pub const PINKY_KNUCKLE2: usize = 23;
    pub const PINKY_KNUCKLE3: usize = 24;
    pub const PINKY_TIP: usize = 25;
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
