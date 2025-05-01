use{
    crate::{
        os::{
            linux::{
                openxr_sys::*,
                openxr::*,
            }
        },
        event::xr::*,
    },
    std::rc::Rc,
};

impl CxOpenXrSession{
        
    pub fn new_xr_update_event(&mut self, xr: &LibOpenXr, frame:&CxOpenXrFrame)->Option<XrUpdateEvent>{
        let predicted_display_time = frame.frame_state.predicted_display_time;
        let local_space = self.local_space;
        let last_state = self.inputs.last_state.clone();
                                
        let active_action_set = XrActiveActionSet{
            action_set: self.inputs.action_set,
            subaction_path: XrPath(0)
        };
                        
        let sync_info = XrActionsSyncInfo{
            count_active_action_sets: 1,
            active_action_sets: &active_action_set as * const _,
            ..Default::default()
        };
                        
        if unsafe{(xr.xrSyncActions)(self.handle, &sync_info)} != XrResult::SUCCESS{
            return None
        };
                        
        let left_controller = self.inputs.left_controller.poll(xr, self.handle, local_space, predicted_display_time, true, &self.inputs.actions);
        let right_controller = self.inputs.right_controller.poll(xr, self.handle, local_space, predicted_display_time, false, &self.inputs.actions);
                        
        let left_hand = self.inputs.left_hand.poll(xr, self.handle, local_space, predicted_display_time);
        let right_hand = self.inputs.right_hand.poll(xr, self.handle, local_space, predicted_display_time);
        
        let anchor = self.anchor.locate_anchor(xr, local_space, predicted_display_time);
        self.order_counter = self.order_counter.wrapping_add(1);
        
        let new_state = Rc::new(XrState{
                        
            order_counter: self.order_counter,
            time: (frame.frame_state.predicted_display_time.as_nanos() as f64) / 1e9f64,
            head_pose: frame.local_from_head.pose,
            anchor,
            left_controller,
            right_controller,
            left_hand,
            right_hand,
        });
        self.inputs.last_state = new_state.clone();
                
                
        Some(XrUpdateEvent{
            state: new_state,
            last: last_state
        })
    }
}


#[derive(Copy, Clone)]
struct CxOpenXrInputActions{
    trigger_action: XrAction,
    grip_action: XrAction,
    thumbstick_action: XrAction,
    click_a_action: XrAction,
    click_b_action: XrAction,
    click_x_action: XrAction,
    click_y_action: XrAction,
    click_menu_action: XrAction,
    click_thumbstick_action: XrAction,
    touch_thumbstick_action: XrAction,
    touch_trigger_action: XrAction,
    touch_a_action: XrAction,
    touch_b_action: XrAction,
    touch_x_action: XrAction,
    touch_y_action: XrAction,
    touch_thumbrest_action: XrAction,
    aim_pose_action: XrAction,
    grip_pose_action: XrAction,
    
}

pub struct CxOpenXrInputs{
    actions: CxOpenXrInputActions,
    action_set: XrActionSet,
    left_controller: CxOpenXrController,
    right_controller: CxOpenXrController,
    left_hand: CxOpenXrHand,
    right_hand: CxOpenXrHand,
    pub last_state: Rc<XrState>,
}


pub struct CxOpenXrHand{
    tracker: XrHandTrackerEXT,
    joint_locations: [XrHandJointLocationEXT; HAND_JOINT_COUNT_EXT]
}

impl CxOpenXrHand{
    fn destroy(self, xr: &LibOpenXr){
        unsafe{(xr.xrDestroyHandTrackerEXT)(self.tracker)}
        .log_error("xrDestroyHandTrackerEXT");
    }
        
    fn poll(&mut self, xr: &LibOpenXr, _session:XrSession, local_space:XrSpace, time:XrTime)->XrHand{
        let mut scale = XrHandTrackingScaleFB{
            sensor_output: 1.0,
            current_output: 1.0,
            override_value_input: 1.0,
            override_hand_scale: XrBool32::from_bool(false),
            ..Default::default()
        };
        let mut aim_state = XrHandTrackingAimStateFB{
            next: &mut scale as *mut _  as *mut _,
            ..Default::default()
        };
        let mut locations = XrHandJointLocationsEXT{
            next:  &mut aim_state as *mut _  as *mut _,
            joint_count: HAND_JOINT_COUNT_EXT as _,
            joint_locations: self.joint_locations.as_mut_ptr(),
            ..Default::default()
        };
        let locate_info = XrHandJointsLocateInfoEXT{
            base_space: local_space,
            time,
            ..Default::default()
        }; 
        unsafe{(xr.xrLocateHandJointsEXT)(self.tracker, &locate_info, &mut locations)}
        .log_error("xrLocateHandJointsEXT");
        // alrighty lets convert the joints to our XrHand
        let mut hand = XrHand::default();
        let mut hand_in_view = false;
        let mut s = 0;
        for i in 0..self.joint_locations.len(){
            let tracked = self.joint_locations[i].location_flags.contains(XrSpaceLocationFlags::ORIENTATION_TRACKED) && self.joint_locations[i].location_flags.contains(XrSpaceLocationFlags::POSITION_TRACKED);
            // we're going to skip the tips and only store the distance
            if i == 5 || i == 10 || i == 15 || i ==20 || i == 25{
                // indices
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
                // only store the distance to the tip so we can fit the entire
                // tracked quest state in a UDP packet :)
                let d = self.joint_locations[i].pose.position - self.joint_locations[i-1].pose.position;
                let slot = i/5-1;
                hand.tips[slot] = d.length();
                hand.tips_active |= if tracked{1<<slot}else{0};
                continue;
            }
            
            hand.joints[s] = self.joint_locations[i].pose;
            s += 1;
            if tracked{
                hand_in_view = true
            }
        }
        hand.flags |= 
        if hand_in_view{XrHand::IN_VIEW}else{0}|
        if aim_state.status.contains(XrHandTrackingAimFlagsFB::VALID){XrHand::AIM_VALID}else{0}|
        if aim_state.status.contains(XrHandTrackingAimFlagsFB::INDEX_PINCHING){XrHand::PINCH_INDEX}else{0}|
        if aim_state.status.contains(XrHandTrackingAimFlagsFB::MIDDLE_PINCHING){XrHand::PINCH_MIDDLE}else{0}|
        if aim_state.status.contains(XrHandTrackingAimFlagsFB::RING_PINCHING){XrHand::PINCH_RING}else{0}|
        if aim_state.status.contains(XrHandTrackingAimFlagsFB::LITTLE_PINCHING){XrHand::PINCH_LITTLE}else{0}|
        if aim_state.status.contains(XrHandTrackingAimFlagsFB::DOMINANT_HAND){XrHand::DOMINANT_HAND}else{0}|
        if aim_state.status.contains(XrHandTrackingAimFlagsFB::MENU_PRESSED){XrHand::MENU_PRESSED}else{0}|
        //if aim_state.status.contains(XrHandTrackingAimFlagsFB::SYSTEM_GESTURE){XrHand::SYSTEM_GESTURE}else{0}|
        0;
        hand.aim_pose = aim_state.aim_pose;
        hand.pinch[XrHand::PINCH_STRENGTH_INDEX] = (aim_state.pinch_strength_index * u8::MAX as f32) as u8;
        hand.pinch[XrHand::PINCH_STRENGTH_MIDDLE] = (aim_state.pinch_strength_middle * u8::MAX as f32) as u8;
        hand.pinch[XrHand::PINCH_STRENGTH_RING] = (aim_state.pinch_strength_ring * u8::MAX as f32) as u8;
        hand.pinch[XrHand::PINCH_STRENGTH_LITTLE] = (aim_state.pinch_strength_little * u8::MAX as f32) as u8;
        hand
    }
}

pub struct CxOpenXrController{
    path: XrPath,
    aim_space: XrSpace,
    grip_space: XrSpace,
    detached_aim_space: XrSpace,
    detached_grip_space: XrSpace,
}

impl CxOpenXrController{
    fn destroy(self, xr: &LibOpenXr){
        self.aim_space.destroy(xr);
        self.grip_space.destroy(xr);
        self.detached_aim_space.destroy(xr);
        self.detached_grip_space.destroy(xr);
    }
    
    fn poll(&self, xr: &LibOpenXr, session:XrSession, local_space:XrSpace, time:XrTime, is_left: bool, actions: &CxOpenXrInputActions)->XrController{
        // lets query the trigger bool
        let stick = XrActionStateVector2f::get(xr, session, actions.thumbstick_action, self.path);
        let trigger = XrActionStateFloat::get(xr, session, actions.trigger_action, self.path);
        let grip = XrActionStateFloat::get(xr, session, actions.grip_action, self.path);
        let grip_state = XrActionStatePose::get(xr, session, actions.grip_pose_action, self.path);
        let aim_state = XrActionStatePose::get(xr, session, actions.aim_pose_action, self.path);
        
        //crate::log!("{:?}", XrActionStateBoolean::get(xr, session, actions.click_x_action, self.path).current_state.as_bool());
        
        fn bf(xr: &LibOpenXr, session:XrSession, path:XrPath, action:XrAction, flag:u16, on:bool)->u16{
            if !on{
                return 0
            }
            if XrActionStateBoolean::get(xr, session, action, path).current_state.as_bool(){
                flag
            }
            else{
                0
            }
        }
        
        XrController{
            grip_pose:  if grip_state.is_active.as_bool(){
                XrSpaceLocation::locate(xr, local_space, time, self.grip_space).pose
            }
            else{
                XrSpaceLocation::locate(xr, local_space, time, self.detached_grip_space).pose
            },
            aim_pose: if aim_state.is_active.as_bool(){
                XrSpaceLocation::locate(xr, local_space, time, self.aim_space).pose
            }
            else{
                XrSpaceLocation::locate(xr, local_space, time, self.detached_aim_space).pose
            },
            stick: stick.current_state,
            trigger: trigger.current_state,
            grip: grip.current_state,
            //last_buttons,
            buttons: 
                if aim_state.is_active.as_bool(){XrController::ACTIVE}else{0}|
                bf(xr, session, self.path, actions.click_a_action, XrController::CLICK_A, !is_left) | 
                bf(xr, session, self.path, actions.click_b_action, XrController::CLICK_B, !is_left) | 
                bf(xr, session, self.path, actions.click_x_action, XrController::CLICK_X, is_left) | 
                bf(xr, session, self.path, actions.click_y_action, XrController::CLICK_Y, is_left) | 
                bf(xr, session, self.path, actions.click_menu_action, XrController::CLICK_MENU, is_left) | 
                bf(xr, session, self.path, actions.click_thumbstick_action, XrController::CLICK_THUMBSTICK, true) | 
                bf(xr, session, self.path, actions.touch_thumbstick_action, XrController::TOUCH_THUMBSTICK, true) | 
                bf(xr, session, self.path, actions.touch_trigger_action, XrController::TOUCH_TRIGGER, true) | 
                bf(xr, session, self.path, actions.touch_a_action, XrController::TOUCH_A, !is_left) | 
                bf(xr, session, self.path, actions.touch_b_action, XrController::TOUCH_B, !is_left) | 
                bf(xr, session, self.path, actions.touch_x_action, XrController::TOUCH_X, is_left) | 
                bf(xr, session, self.path, actions.touch_y_action, XrController::TOUCH_Y, is_left) | 
                bf(xr, session, self.path, actions.touch_thumbrest_action, XrController::TOUCH_THUMBREST, true) 
        }
    }
}

impl CxOpenXrInputs{
        
    pub fn new_inputs(xr: &LibOpenXr, session:XrSession, instance: XrInstance)->Result<CxOpenXrInputs,String>{
        // create handtrackers
        let left_hand_info = XrHandTrackerCreateInfoEXT{hand: XrHandEXT::LEFT,..Default::default()};
        let mut left_hand_track = XrHandTrackerEXT(0);
        unsafe{(xr.xrCreateHandTrackerEXT)(session, &left_hand_info, &mut left_hand_track)}
        .log_error("xrCreateHandTrackerEXT");
        
        let right_hand_info = XrHandTrackerCreateInfoEXT{hand: XrHandEXT::RIGHT,..Default::default()};
        let mut right_hand_track = XrHandTrackerEXT(0);
        unsafe{(xr.xrCreateHandTrackerEXT)(session, &right_hand_info, &mut right_hand_track)}
        .log_error("xrCreateHandTrackerEXT");
        
        let action_set = XrActionSet::new(
            xr, instance, 1, "makepad_action_set", "Main action set"
        )?;
                
        let left_hand_path = XrPath::new(xr, instance, "/user/hand/left")?;
        let right_hand_path = XrPath::new(xr, instance, "/user/hand/right")?;
        let left_detached_path = XrPath::new(xr, instance, "/user/detached_controller_meta/left")?;
        let right_detached_path = XrPath::new(xr, instance, "/user/detached_controller_meta/right")?;
        let hand_paths = [left_hand_path, right_hand_path];
        let detached_paths = [left_detached_path, right_detached_path];
        
        let trigger_action = XrAction::new(xr, action_set, XrActionType::FLOAT_INPUT, "trigger", "", &hand_paths)?;
        let grip_action = XrAction::new(xr, action_set, XrActionType::FLOAT_INPUT, "grip", "", &hand_paths)?;
        let thumbstick_action = XrAction::new(xr, action_set, XrActionType::VECTOR2F_INPUT, "thumbstick_xy", "", &hand_paths)?;
        let click_a_action = XrAction::new(xr, action_set, XrActionType::BOOLEAN_INPUT, "click_a", "", &hand_paths)?;
        let click_b_action = XrAction::new(xr, action_set, XrActionType::BOOLEAN_INPUT, "click_b", "", &hand_paths)?;
        let click_x_action = XrAction::new(xr, action_set, XrActionType::BOOLEAN_INPUT, "click_x", "", &hand_paths)?;
        let click_y_action = XrAction::new(xr, action_set, XrActionType::BOOLEAN_INPUT, "click_y", "", &hand_paths)?;
        let click_menu_action = XrAction::new(xr, action_set, XrActionType::BOOLEAN_INPUT, "click_menu", "", &hand_paths)?;
        let click_thumbstick_action = XrAction::new(xr, action_set, XrActionType::BOOLEAN_INPUT, "click_thumbstick", "", &hand_paths)?;
        let touch_thumbstick_action = XrAction::new(xr, action_set, XrActionType::BOOLEAN_INPUT, "touch_thumbstick", "", &hand_paths)?;
        let touch_trigger_action = XrAction::new(xr, action_set, XrActionType::BOOLEAN_INPUT, "touch_trigger", "", &hand_paths)?;
        let touch_a_action = XrAction::new(xr, action_set, XrActionType::BOOLEAN_INPUT, "touch_a", "", &hand_paths)?;
        let touch_b_action = XrAction::new(xr, action_set, XrActionType::BOOLEAN_INPUT, "touch_b", "", &hand_paths)?;
        let touch_x_action = XrAction::new(xr, action_set, XrActionType::BOOLEAN_INPUT, "touch_x", "", &hand_paths)?;
        let touch_y_action = XrAction::new(xr, action_set, XrActionType::BOOLEAN_INPUT, "touchy_y", "", &hand_paths)?;
        let touch_thumbrest_action = XrAction::new(xr, action_set, XrActionType::BOOLEAN_INPUT, "touch_thumbrest", "", &hand_paths)?;
                                                
        let aim_pose_action = XrAction::new(xr, action_set, XrActionType::POSE_INPUT, "aim_pose", "", &hand_paths)?;
        let grip_pose_action = XrAction::new(xr, action_set, XrActionType::POSE_INPUT, "grip_pose", "", &hand_paths)?;
        
        let detached_aim_pose_action = XrAction::new(xr, action_set, XrActionType::POSE_INPUT, "detached_aim_pose", "", &detached_paths)?;
        let detached_grip_pose_action = XrAction::new(xr, action_set, XrActionType::POSE_INPUT, "detached_grip_pose", "", &detached_paths)?;
                
        let interaction_profile = XrPath::new(xr, instance,  "/interaction_profiles/meta/touch_controller_plus")?;
                
        let bindings = [
            XrActionSuggestedBinding::new(xr, instance, trigger_action, "/user/hand/left/input/trigger")?,
            XrActionSuggestedBinding::new(xr, instance, trigger_action, "/user/hand/right/input/trigger")?,
            
            XrActionSuggestedBinding::new(xr, instance, grip_action, "/user/hand/left/input/squeeze/value")?,
            XrActionSuggestedBinding::new(xr, instance, grip_action, "/user/hand/right/input/squeeze/value")?,
            
            XrActionSuggestedBinding::new(xr, instance, thumbstick_action, "/user/hand/left/input/thumbstick")?,
            XrActionSuggestedBinding::new(xr, instance, thumbstick_action, "/user/hand/right/input/thumbstick")?,
            
            XrActionSuggestedBinding::new(xr, instance, click_a_action, "/user/hand/right/input/a/click")?,
            XrActionSuggestedBinding::new(xr, instance, click_b_action, "/user/hand/right/input/b/click")?,
            XrActionSuggestedBinding::new(xr, instance, click_x_action, "/user/hand/left/input/x/click")?,
            XrActionSuggestedBinding::new(xr, instance, click_y_action, "/user/hand/left/input/y/click")?,
            XrActionSuggestedBinding::new(xr, instance, click_menu_action, "/user/hand/left/input/menu/click")?,
            XrActionSuggestedBinding::new(xr, instance, click_thumbstick_action, "/user/hand/left/input/thumbstick/click")?,
            XrActionSuggestedBinding::new(xr, instance, click_thumbstick_action, "/user/hand/right/input/thumbstick/click")?,
            XrActionSuggestedBinding::new(xr, instance, touch_thumbstick_action, "/user/hand/left/input/thumbstick/touch")?,
            XrActionSuggestedBinding::new(xr, instance, touch_thumbstick_action, "/user/hand/right/input/thumbstick/touch")?,
            
            XrActionSuggestedBinding::new(xr, instance, touch_trigger_action, "/user/hand/left/input/trigger/touch")?,
            XrActionSuggestedBinding::new(xr, instance, touch_trigger_action, "/user/hand/right/input/trigger/touch")?,
            
            XrActionSuggestedBinding::new(xr, instance, touch_a_action, "/user/hand/right/input/a/touch")?,
            XrActionSuggestedBinding::new(xr, instance, touch_b_action, "/user/hand/right/input/b/touch")?,
            XrActionSuggestedBinding::new(xr, instance, touch_x_action, "/user/hand/left/input/x/touch")?,
            XrActionSuggestedBinding::new(xr, instance, touch_y_action, "/user/hand/left/input/y/touch")?,
            
            XrActionSuggestedBinding::new(xr, instance, touch_thumbrest_action, "/user/hand/left/input/thumbrest/touch")?,
            XrActionSuggestedBinding::new(xr, instance, touch_thumbrest_action, "/user/hand/right/input/thumbrest/touch")?,
            
            XrActionSuggestedBinding::new(xr, instance, aim_pose_action, "/user/hand/left/input/aim/pose")?,
            XrActionSuggestedBinding::new(xr, instance, aim_pose_action, "/user/hand/right/input/aim/pose")?,
            XrActionSuggestedBinding::new(xr, instance, grip_pose_action, "/user/hand/left/input/grip/pose")?,
            XrActionSuggestedBinding::new(xr, instance, grip_pose_action, "/user/hand/right/input/grip/pose")?,
            
            XrActionSuggestedBinding::new(xr, instance, detached_aim_pose_action, "/user/detached_controller_meta/left/input/aim/pose")?,
            XrActionSuggestedBinding::new(xr, instance, detached_aim_pose_action, "/user/detached_controller_meta/right/input/aim/pose")?,
            XrActionSuggestedBinding::new(xr, instance, detached_grip_pose_action, "/user/detached_controller_meta/left/input/grip/pose")?,
            XrActionSuggestedBinding::new(xr, instance, detached_grip_pose_action, "/user/detached_controller_meta/right/input/grip/pose")?,
        ];
                
        let suggested_bindings = XrInteractionProfileSuggestedBinding{
            interaction_profile,
            count_suggested_bindings: bindings.len() as _,
            suggested_bindings: bindings.as_ptr(),
            ..Default::default()
        };
                
        unsafe{(xr.xrSuggestInteractionProfileBindings)(
            instance,
            &suggested_bindings
        )}.to_result("xrSuggestInteractionProfileBindings")?;
                
        let attach_info = XrSessionActionSetsAttachInfo{
            count_action_sets: 1,
            action_sets: &action_set as * const _,
            ..Default::default()
        };
                    
        unsafe{(xr.xrAttachSessionActionSets)(
            session,
            &attach_info
        )}.to_result("xrAttachSessionActionSets")?;
        let mut pose = XrPosef::default();
        pose.orientation.w = 1.0;
        
        let resume_info = XrSimultaneousHandsAndControllersTrackingResumeInfoMETA::default();
        unsafe{(xr.xrResumeSimultaneousHandsAndControllersTrackingMETA)(session, &resume_info)}
        .log_error("xrResumeSimultaneousHandsAndControllersTrackingMETA");
        
        let actions = CxOpenXrInputActions{
            trigger_action,
            grip_action,
            thumbstick_action,
            click_a_action,
            click_b_action,
            click_x_action,
            click_y_action,
            click_menu_action,
            click_thumbstick_action,
            touch_thumbstick_action,
            touch_trigger_action,
            touch_a_action,
            touch_b_action,
            touch_x_action,
            touch_y_action,
            touch_thumbrest_action,
            aim_pose_action,
            grip_pose_action,
        };
        
        Ok(CxOpenXrInputs{
            action_set,
            actions,
            left_hand: CxOpenXrHand{
                tracker:left_hand_track,
                joint_locations: Default::default()
            },
            right_hand: CxOpenXrHand{
                tracker:right_hand_track,
                joint_locations: Default::default()
            },
            left_controller: CxOpenXrController{
                path: left_hand_path,
                aim_space: XrSpace::new_action_space(xr, session, aim_pose_action, left_hand_path, pose)?,
                grip_space: XrSpace::new_action_space(xr, session, grip_pose_action, left_hand_path, pose)?,
                detached_aim_space: XrSpace::new_action_space(xr, session, detached_aim_pose_action, left_detached_path, pose)?,
                detached_grip_space: XrSpace::new_action_space(xr, session, detached_grip_pose_action, left_detached_path, pose)?
            },
            right_controller: CxOpenXrController{
                path: right_hand_path,
                aim_space: XrSpace::new_action_space(xr, session, aim_pose_action, right_hand_path, pose)?,
                grip_space: XrSpace::new_action_space(xr, session, grip_pose_action, right_hand_path, pose)?,
                detached_aim_space: XrSpace::new_action_space(xr, session, detached_aim_pose_action, right_detached_path, pose)?,
                detached_grip_space: XrSpace::new_action_space(xr, session, detached_grip_pose_action, right_detached_path, pose)?
            },
            last_state: Default::default()
        })
    }
    
    pub fn destroy_input(self, xr: &LibOpenXr){
        self.left_controller.destroy(xr);
        self.right_controller.destroy(xr);
        self.left_hand.destroy(xr);
        self.right_hand.destroy(xr);
    }
    
}
