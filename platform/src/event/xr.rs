use {
    crate::{
        cx::Cx,
        Margin,
        Area,
        HitOptions,
        Hit,
        Event,
        SmallVec,
        FingerDownEvent,
        CxWindowPool,
        DigitDevice,
        KeyModifiers,
        makepad_micro_serde::*,
        makepad_math::*,
        LiveId,
        makepad_live_id::{live_id_num},
    },
    std::rc::Rc,
};

#[derive(Clone, Debug, Default, SerBin, DeBin)]
pub struct XrFloatButton {
    pub value: f32,
}

#[derive(Clone, Debug, Default, SerBin, DeBin)]
pub struct XrStick {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Debug, Default, SerBin, DeBin)]
pub struct XrController {
    pub grip_pose: Pose,
    pub aim_pose: Pose,
    pub trigger: XrFloatButton,
    pub grip: XrFloatButton,
    pub buttons: u16,
    pub stick: XrStick,
}

impl XrController{
    pub const ACTIVE: u16 = 1<<0;
    pub const X: u16 = 1<<1;
    pub const Y: u16 = 1<<2;
    pub const A: u16 = 1<<3;
    pub const B: u16 = 1<<4;
    pub const MENU: u16 = 1<<5;
    pub const ON_X: u16 = 1<<6;
    pub const ON_Y: u16 = 1<<7;
    pub const ON_A: u16 = 1<<8;
    pub const ON_B: u16 = 1<<9;
    pub const ON_STICK: u16 = 1<<10;
    pub fn x(&self)->bool{self.buttons & Self::X != 0}
    pub fn y(&self)->bool{self.buttons & Self::Y != 0}
    pub fn a(&self)->bool{self.buttons & Self::A != 0}
    pub fn b(&self)->bool{self.buttons & Self::B != 0}
    pub fn on_x(&self)->bool{self.buttons & Self::ON_X != 0}
    pub fn on_y(&self)->bool{self.buttons & Self::ON_Y != 0}
    pub fn on_a(&self)->bool{self.buttons & Self::ON_A != 0}
    pub fn on_b(&self)->bool{self.buttons & Self::ON_B != 0}
    pub fn on_stick(&self)->bool{self.buttons & Self::ON_STICK != 0}
    pub fn menu(&self)->bool{self.buttons & Self::MENU != 0}
}

#[derive(Clone, Debug, Default, SerBin, DeBin)]
pub struct XrHandJoint{
    pub pose: Pose
}

#[derive(Clone, Debug, Default, SerBin, DeBin)]
pub struct XrHand{
    pub in_view: bool,
    pub joints: [XrHandJoint;Self::JOINT_COUNT],
    pub tips: [f32;5],
}

impl Event{
    
}

impl XrHand{

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
    pub const PINKY_BASE: usize = 17;
    pub const PINKY_KNUCKLE1: usize = 18;
    pub const PINKY_KNUCKLE2: usize = 19;
    pub const PINKY_KNUCKLE3: usize = 20;
    /*
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
    pub const PINKY_TIP: usize = 25;*/
}

#[derive(Clone, Debug)]
pub struct XrFingerTip{
    pub index: usize,
    pub is_left: bool,
    pub pos: Vec3,
}

#[derive(Clone, Debug)]
pub struct XrLocalEvent{
    pub finger_tips: SmallVec<[XrFingerTip;10]>,
    pub update: XrUpdateEvent,
    pub modifiers: KeyModifiers,
    pub time: f64,
}

#[derive(Clone, Debug, Default, SerBin, DeBin)]
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

impl XrLocalEvent{
    pub fn from_update_event(e:&XrUpdateEvent, mat:&Mat4)->XrLocalEvent{
        // alright we have a matrix, take the inverse
        // then mul all the fingertips and store them in fingertips
        // then use that
        let _inv = mat.invert();
        // lets collect all fingertips
        let finger_tips = SmallVec::new();
        for (_hindex, hand) in [&e.state.left_hand, &e.state.right_hand].iter().enumerate(){
            if hand.in_view{
                for (_index,_joint) in hand.joints.iter().enumerate(){
                    /*if XrHand::is_tip(index){
                        let pos = inv.transform_vec4(joint.pose.position.to_vec4()).to_vec3();
                        // todo, ignore all non-push orientations (probably a halfdome)
                        finger_tips.push(XrFingerTip{
                            index: index,
                            is_left: hindex == 0,
                            pos
                        })
                    }*/
                }
            }
        }
        
        XrLocalEvent{
            finger_tips,
            modifiers: Default::default(),
            time: e.state.time,
            update: e.clone(),        
        }
    }
    
    pub fn hits_with_options_and_test<F>(&self, cx: &mut Cx, area: Area, options: HitOptions, hit_test:F) -> Hit 
    where F: Fn(DVec2, &Rect, &Option<Margin>)->bool
    {
        let rect = area.clipped_rect(cx);
        for tip in &self.finger_tips{
            let abs = tip.pos.to_vec2().into();
            // alright so. how do we clean this up
            
            if hit_test(abs, &rect, &options.margin) {
                if tip.pos.z <20.0 && tip.pos.z > -30.0{
                    let device = DigitDevice::XrHand { is_left: tip.is_left, index: tip.index };
                    let digit_id = live_id_num!(xrfinger, tip.index as u64 + if tip.is_left{10} else{0}).into();
                    cx.fingers.capture_digit(digit_id, area, options.sweep_area, self.time, abs);
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
        }
        /*
        if let Some(capture) = cx.fingers.find_area_capture(area) {
            let digit_id = live_id_num!(xrfinger, 0).into();
            let device = DigitDevice::XrHand { is_left: false, index: 0 };
            return Hit::FingerUp(FingerUpEvent {
                abs_start: capture.abs_start,
                rect,
                window_id: CxWindowPool::id_zero(),
                abs: dvec2(0.0,0.0),
                digit_id,
                device,
                has_long_press_occurred: false,
                capture_time: capture.time,
                modifiers: self.modifiers,
                time: self.time,
                tap_count: 0,
                is_over: false,
                is_sweep: false,
            })
        }
        */
        // alright lets implement hits for XrUpdateEvent and our area.
        // our area is a rect in space and our finger tip is a point
        // we should multiply our orientation with the inverse of the window matrix
        // and then we should be in the UI space
        // then its just xy and z
        
        return Hit::Nothing
    }
}