use {
    crate::{
        cx::Cx,
        Margin,
        Area,
        HitOptions,
        Hit,
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
pub struct XrController {
    pub grip_pose: Pose,
    pub aim_pose: Pose,
    pub trigger: f32,
    pub grip: f32,
    pub buttons: u16,
    pub stick: Vec2,
}

impl XrController{
    pub const CLICK_X: u16 = 1<<0;
    pub const CLICK_Y: u16 = 1<<1;
    pub const CLICK_A: u16 = 1<<2;
    pub const CLICK_B: u16 = 1<<3;
    pub const CLICK_MENU: u16 = 1<<4;
    
    pub const ACTIVE: u16 = 1<<5;
    pub const CLICK_THUMBSTICK: u16 = 1<<6;
        
    pub const TOUCH_X: u16 = 1<<7;
    pub const TOUCH_Y: u16 = 1<<8;
    pub const TOUCH_A: u16 = 1<<9;
    pub const TOUCH_B: u16 = 1<<10;
    pub const TOUCH_THUMBSTICK: u16 = 1<<11;
    pub const TOUCH_TRIGGER: u16 = 1<<12;
    pub const TOUCH_THUMBREST: u16 = 1<<13;
    pub fn triggered(&self)->bool{ self.trigger>0.8}
    pub fn active(&self)->bool{self.buttons & Self::ACTIVE != 0}

    pub fn click_x(&self)->bool{self.buttons & Self::CLICK_X != 0}
    pub fn click_y(&self)->bool{self.buttons & Self::CLICK_Y != 0}
    pub fn click_a(&self)->bool{self.buttons & Self::CLICK_A != 0}
    pub fn click_b(&self)->bool{self.buttons & Self::CLICK_B != 0}
    pub fn click_thumbstick(&self)->bool{self.buttons & Self::CLICK_THUMBSTICK != 0}
    pub fn click_menu(&self)->bool{self.buttons & Self::CLICK_MENU != 0}
        
    pub fn touch_x(&self)->bool{self.buttons & Self::TOUCH_X != 0}
    pub fn touch_y(&self)->bool{self.buttons & Self::TOUCH_Y != 0}
    pub fn touch_a(&self)->bool{self.buttons & Self::TOUCH_A != 0}
    pub fn touch_b(&self)->bool{self.buttons & Self::TOUCH_B != 0}
    pub fn touch_thumbstick(&self)->bool{self.buttons & Self::TOUCH_THUMBSTICK != 0}
    pub fn touch_trigger(&self)->bool{self.buttons & Self::TOUCH_TRIGGER != 0}
    pub fn touch_thumbrest(&self)->bool{self.buttons & Self::TOUCH_THUMBREST != 0}
}

#[derive(Clone, Debug, Default, SerBin, DeBin)]
pub struct XrHand{
    pub flags: u8,
    pub joints: [Pose;Self::JOINT_COUNT],
    pub tips: [f32;5],
    pub tips_active: u8,
    pub aim_pose: Pose,
    pub pinch: [u8;4]
}

impl XrHand{
    
    pub fn in_view(&self)->bool{self.flags & Self::IN_VIEW != 0}
    pub fn aim_valid(&self)->bool{self.flags & Self::AIM_VALID != 0}
    pub fn menu_pressed(&self)->bool{self.flags & Self::MENU_PRESSED != 0}
    //pub fn system_gesture(&self)->bool{self.flags & Self::SYSTEM_GESTURE != 0}
    pub fn dominant_hand(&self)->bool{self.flags & Self::DOMINANT_HAND != 0}
    
    pub fn pinch_index(&self)->bool{self.flags & Self::PINCH_INDEX != 0}
    pub fn pinch_middle(&self)->bool{self.flags & Self::PINCH_MIDDLE != 0}
    pub fn pinch_ring(&self)->bool{self.flags & Self::PINCH_RING != 0}
    pub fn pinch_little(&self)->bool{self.flags & Self::PINCH_LITTLE != 0}
    
    pub fn pinch_only_little(&self)->bool{
        self.pinch[Self::PINCH_STRENGTH_INDEX]<100 && 
        self.pinch[Self::PINCH_STRENGTH_MIDDLE]<100 && 
        self.pinch[Self::PINCH_STRENGTH_RING]<100 && 
        self.pinch[Self::PINCH_STRENGTH_LITTLE]>160
    }
    pub fn pinch_only_index(&self)->bool{
        self.pinch[Self::PINCH_STRENGTH_INDEX]>160 && 
        self.pinch[Self::PINCH_STRENGTH_MIDDLE]<100 && 
        self.pinch[Self::PINCH_STRENGTH_RING]<100 && 
        self.pinch[Self::PINCH_STRENGTH_LITTLE]<100
    }
    
    pub fn pinch_not_index(&self)->bool{
        self.pinch[Self::PINCH_STRENGTH_INDEX]<100 && (
        self.pinch[Self::PINCH_STRENGTH_MIDDLE]>160 || 
        self.pinch[Self::PINCH_STRENGTH_RING]>160 || 
        self.pinch[Self::PINCH_STRENGTH_LITTLE]>160) 
    }
            
    pub fn pinch_strength_index(&self)->f32{self.pinch[Self::PINCH_STRENGTH_INDEX] as f32 / u8::MAX as f32}
    pub fn pinch_strength_middle(&self)->f32{self.pinch[Self::PINCH_STRENGTH_MIDDLE] as f32 / u8::MAX as f32}
    pub fn pinch_strength_ring(&self)->f32{self.pinch[Self::PINCH_STRENGTH_RING] as f32 / u8::MAX as f32}
    pub fn pinch_strength_pinky(&self)->f32{self.pinch[Self::PINCH_STRENGTH_LITTLE] as f32 / u8::MAX as f32}
        
    pub const IN_VIEW: u8 = 1<<0;
    pub const AIM_VALID: u8 = 1<<1;
    pub const PINCH_INDEX: u8 = 1<<2;
    pub const PINCH_MIDDLE: u8 = 1<<3;
    pub const PINCH_RING: u8 = 1<<4;
    pub const PINCH_LITTLE: u8 = 1<<5;
    pub const DOMINANT_HAND: u8 = 1<<6;
    pub const MENU_PRESSED : u8 = 1<<7;
    
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
    
    pub const END_KNUCKLES: [usize;5] = [
        Self::THUMB_KNUCKLE2, 
        Self::INDEX_KNUCKLE3, 
        Self::MIDDLE_KNUCKLE3, 
        Self::RING_KNUCKLE3, 
        Self::LITTLE_KNUCKLE3
    ];

    pub fn end_knuckles(&self)->[&Pose;5]{
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
            
    pub fn tip_active(&self,tip:usize)->bool{
        self.tips_active & (1<<tip) != 0
    }
    
    fn tip_pos(&self,tip:usize, knuckle:usize)->Vec3{
        let pos = vec4(0.0,0.0,-self.tips[tip],1.0);
        self.joints[knuckle].to_mat4().transform_vec4(pos).to_vec3()
    }
    
    pub fn tip_pos_thumb(&self)->Vec3{self.tip_pos(0, XrHand::THUMB_KNUCKLE2)}
    pub fn tip_pos_index(&self)->Vec3{self.tip_pos(0, XrHand::INDEX_KNUCKLE3)}
    pub fn tip_pos_middle(&self)->Vec3{self.tip_pos(0, XrHand::MIDDLE_KNUCKLE3)}
    pub fn tip_pos_ring(&self)->Vec3{self.tip_pos(0, XrHand::RING_KNUCKLE3)}
    pub fn tip_pos_little(&self)->Vec3{self.tip_pos(0, XrHand::LITTLE_KNUCKLE3)}
        
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

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, PartialEq)]
pub struct XrAnchor{
    pub left: Vec3,
    pub right: Vec3,
}

impl XrAnchor{
    pub fn to_quat(&self)->Quat{
        let mut forward = self.right - self.left;
        forward.y = 0.0;
        Quat::look_rotation(forward, vec3(0.0,1.0,0.0))
    }
    
    pub fn to_quat_rev(&self)->Quat{
        let mut forward = self.left - self.right;
        forward.y = 0.0;
        Quat::look_rotation(forward, vec3(0.0,1.0,0.0))
    }
    
    pub fn to_mat4(&self)->Mat4{
        self.to_pose().to_mat4()
    }
        
    pub fn to_pose(&self)->Pose{
        Pose{position: (self.left+self.right)/2.0, orientation:self.to_quat()}
    }
    
    pub fn mapping_to(&self, other:&XrAnchor)->Mat4{
        Mat4::mul(&self.to_pose().to_mat4().invert(), &other.to_pose().to_mat4())
    }
}

#[derive(Clone, Debug, Default, SerBin, DeBin)]
pub struct XrState{
    pub time: f64,
    pub head_pose: Pose,
    pub order_counter: u8,
    pub anchor: Option<XrAnchor>,
    pub left_controller: XrController,
    pub right_controller: XrController,
    pub left_hand: XrHand,
    pub right_hand: XrHand,
}
impl XrState{
    pub fn vec_in_head_space(&self,pos:Vec3)->Vec3{
        self.head_pose.to_mat4().transform_vec4(pos.to_vec4()).to_vec3()
    }
    
    pub fn scene_anchor_pose(&self)->Option<Pose>{
        if let Some(_anchor) = &self.anchor{
            // lets construct a pose from 2 positions
            None
        }
        else{
            None
        }
    }
    
    pub fn hands(&self)->[&XrHand;2]{
        [&self.left_hand, &self.right_hand]
    }
    pub fn controllers(&self)->[&XrController;2]{
        [&self.left_controller, &self.right_controller]
    }
}

 
#[derive(Clone, Debug)]
pub struct XrUpdateEvent {
    pub state: Rc<XrState>,
    pub last: Rc<XrState>,
}

impl XrUpdateEvent{
            
    pub fn clicked_x(&self)->bool{self.state.left_controller.click_x() && !self.last.left_controller.click_x()}
    pub fn clicked_y(&self)->bool{self.state.left_controller.click_y() && !self.last.left_controller.click_y()}
    pub fn clicked_a(&self)->bool{self.state.right_controller.click_a() && !self.last.right_controller.click_a()}
    pub fn clicked_b(&self)->bool{self.state.right_controller.click_b() && !self.last.right_controller.click_b()}
    pub fn clicked_left_thumbstick(&self)->bool{self.state.left_controller.click_thumbstick() && !self.last.left_controller.click_thumbstick()}
    pub fn clicked_right_thumbstick(&self)->bool{self.state.right_controller.click_thumbstick() && !self.last.right_controller.click_thumbstick()}
    pub fn clicked_menu(&self)->bool{self.state.left_controller.click_menu() && !self.last.left_controller.click_menu()}
    pub fn menu_pressed(&self)->bool{self.state.left_hand.menu_pressed() && !self.last.left_hand.menu_pressed()}
            
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
            if hand.in_view(){
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