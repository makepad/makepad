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
        makepad_math::*,
        LiveId,
        makepad_live_id::{live_id_num},
    },
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

impl Event{
    
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

impl XrLocalEvent{
    pub fn from_update_event(e:&XrUpdateEvent, mat:&Mat4)->XrLocalEvent{
        // alright we have a matrix, take the inverse
        // then mul all the fingertips and store them in fingertips
        // then use that
        let inv = mat.invert();
        // lets collect all fingertips
        let mut finger_tips = SmallVec::new();
        for (hindex, hand) in [&e.state.left_hand, &e.state.right_hand].iter().enumerate(){
            if hand.in_view{
                for (index,joint) in hand.joints.iter().enumerate(){
                    if joint.valid && joint.tracked && XrHand::is_tip(index){
                        let pos = inv.transform_vec4(joint.pose.position.to_vec4()).to_vec3();
                        // todo, ignore all non-push orientations (probably a halfdome)
                        finger_tips.push(XrFingerTip{
                            index: index,
                            is_left: hindex == 0,
                            pos
                        })
                    }
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