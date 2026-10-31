//! Controllers and hands (restored from the retired Android backend's
//! `platform/src/os/linux/openxr_input.rs`): the action set with the Touch (Plus) and
//! hand_interaction_ext bindings and META detached controllers, XR_EXT_hand_tracking with
//! XR_FB_hand_tracking_aim (26 joints, pinches, aim) and the grasp with hysteresis. They fill
//! the HAL's `XrController` / `XrHand` (the old Cx types, field for field) instead of an
//! `XrState`: the session sends them in each `XrFrame`.
use super::sys::*;
use crate::{xr_controller, xr_hand, XrController, XrHand, XrPose};

fn xr_pose(p: XrPosef) -> XrPose {
    XrPose { orientation: [p.orientation.x, p.orientation.y, p.orientation.z, p.orientation.w], position: [p.position.x, p.position.y, p.position.z] }
}

impl CxOpenXrInputs {
    /// Syncs the actions and polls both controllers and hands at `time` in `local_space` (the
    /// old `new_xr_update_event`, whose head pose, anchor and `XrState` are the session's and
    /// Cx's now). `None` while the session is not focused.
    pub fn poll(&mut self, xr: &LibOpenXr, session: XrSession, local_space: XrSpace, time: XrTime) -> Option<([XrController; 2], [XrHand; 2])> {
        let active_action_set = XrActiveActionSet {
            action_set: self.action_set,
            subaction_path: XrPath(0),
        };

        let sync_info = XrActionsSyncInfo {
            count_active_action_sets: 1,
            active_action_sets: &active_action_set as *const _,
            ..Default::default()
        };

        if unsafe { (xr.xrSyncActions)(session, &sync_info) } != XrResult::SUCCESS {
            return None;
        };

        let left_controller = self.left_controller.poll(xr, session, local_space, time, true, &self.actions);
        let right_controller = self.right_controller.poll(xr, session, local_space, time, false, &self.actions);
        let left_hand = self.left_hand.poll(xr, session, local_space, time, &self.actions);
        let right_hand = self.right_hand.poll(xr, session, local_space, time, &self.actions);
        Some(([left_controller, right_controller], [left_hand, right_hand]))
    }

    /// A buzz on a controller (`hand` 0 left, 1 right): `HalCmd::XrHaptic`.
    pub fn apply_haptic(&self, xr: &LibOpenXr, session: XrSession, hand: u32, amplitude: f32, duration_us: u32) {
        let path = if hand == 0 { self.left_controller.path } else { self.right_controller.path };
        let info = XrHapticActionInfo { ty: XrStructureType::HAPTIC_ACTION_INFO, next: std::ptr::null(), action: self.actions.haptic_action, subaction_path: path };
        let buzz = XrHapticVibration {
            ty: XrStructureType::HAPTIC_VIBRATION,
            next: std::ptr::null(),
            duration: XrDuration(duration_us as i64 * 1000),
            frequency: 0.0,
            amplitude: amplitude.clamp(0.0, 1.0),
        };
        unsafe { (xr.xrApplyHapticFeedback)(session, &info, &buzz) }.log_error("xrApplyHapticFeedback");
    }

    /// Each controller's grip and aim located at `time` with their velocities, as
    /// `locate_hand_grips` gives them (the spaces `poll` reads: the held or the detached
    /// controller's). The actions must have been synced (`poll`).
    pub fn locate_controllers(&self, xr: &LibOpenXr, session: XrSession, local_space: XrSpace, time: XrTime) -> [[(XrPose, [f32; 3], [f32; 3], u32); 2]; 2] {
        [&self.left_controller, &self.right_controller].map(|c| c.locate_moving(xr, session, local_space, time, &self.actions))
    }

    /// Each hand's grip pose (hand_interaction_ext's grip action space) located at `time`, with
    /// its velocity: (pose, linear velocity m/s, angular velocity rad/s, location flags | velocity
    /// flags << 8). The actions must have been synced (`poll`).
    pub fn locate_hand_grips(&self, xr: &LibOpenXr, local_space: XrSpace, time: XrTime) -> [(XrPose, [f32; 3], [f32; 3], u32); 2] {
        [&self.left_hand, &self.right_hand].map(|h| {
            let mut velocity = XrSpaceVelocity::default();
            let mut location = XrSpaceLocation { next: &mut velocity as *mut _ as *mut _, ..Default::default() };
            unsafe { (xr.xrLocateSpace)(h.grip_space, local_space, time, &mut location) }.log_error("xrLocateSpace hand grip");
            let v = |a: XrVector3f| [a.x, a.y, a.z];
            (xr_pose(location.pose), v(velocity.linear_velocity), v(velocity.angular_velocity), [XrSpaceLocationFlags::ORIENTATION_VALID, XrSpaceLocationFlags::POSITION_VALID, XrSpaceLocationFlags::ORIENTATION_TRACKED, XrSpaceLocationFlags::POSITION_TRACKED].iter().enumerate().map(|(i, f)| (location.location_flags.contains(*f) as u32) << i).sum::<u32>() | (velocity.velocity_flags as u32) << 8)
        })
    }

    /// Both hands located at `time` (the tracking log's tracked path: a time already past)
    /// without touching the polled state (the last hand kept for untracked joints, the grab's
    /// hysteresis) and without syncing the actions.
    pub fn locate_hands(&mut self, xr: &LibOpenXr, session: XrSession, local_space: XrSpace, time: XrTime) -> [XrHand; 2] {
        let actions = self.actions;
        [&mut self.left_hand, &mut self.right_hand].map(|h| {
            let (last, grab) = (h.last_hand, h.grab_active);
            let hand = h.poll(xr, session, local_space, time, &actions);
            h.last_hand = last;
            h.grab_active = grab;
            hand
        })
    }
}

#[derive(Copy, Clone)]
struct CxOpenXrInputActions {
    trigger_action: XrAction,
    grip_action: XrAction,
    hand_grab_action: XrAction,
    hand_grab_ready_action: XrAction,
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
    /// Both controllers' haptic output (`apply_haptic`).
    haptic_action: XrAction,
}

pub struct CxOpenXrInputs {
    actions: CxOpenXrInputActions,
    action_set: XrActionSet,
    left_controller: CxOpenXrController,
    right_controller: CxOpenXrController,
    left_hand: CxOpenXrHand,
    right_hand: CxOpenXrHand,
}

pub struct CxOpenXrHand {
    path: XrPath,
    tracker: XrHandTrackerEXT,
    joint_locations: [XrHandJointLocationEXT; HAND_JOINT_COUNT_EXT],
    grab_active: bool,
    last_hand: XrHand,
    /// The hand's grip pose action space (hand_interaction_ext).
    grip_space: XrSpace,
}

impl CxOpenXrHand {
    fn destroy(&self, xr: &LibOpenXr) {
        unsafe { (xr.xrDestroyHandTrackerEXT)(self.tracker) }.log_error("xrDestroyHandTrackerEXT");
        self.grip_space.destroy(xr);
    }

    fn poll(
        &mut self,
        xr: &LibOpenXr,
        session: XrSession,
        local_space: XrSpace,
        time: XrTime,
        actions: &CxOpenXrInputActions,
    ) -> XrHand {
        let mut scale = XrHandTrackingScaleFB {
            sensor_output: 1.0,
            current_output: 1.0,
            override_value_input: 1.0,
            override_hand_scale: XrBool32::from_bool(false),
            ..Default::default()
        };
        let mut aim_state = XrHandTrackingAimStateFB {
            next: &mut scale as *mut _ as *mut _,
            ..Default::default()
        };
        let mut locations = XrHandJointLocationsEXT {
            next: &mut aim_state as *mut _ as *mut _,
            joint_count: HAND_JOINT_COUNT_EXT as _,
            joint_locations: self.joint_locations.as_mut_ptr(),
            ..Default::default()
        };
        let locate_info = XrHandJointsLocateInfoEXT {
            base_space: local_space,
            time,
            ..Default::default()
        };
        unsafe { (xr.xrLocateHandJointsEXT)(self.tracker, &locate_info, &mut locations) }
            .log_error("xrLocateHandJointsEXT");
        // alrighty lets convert the joints to our XrHand
        let mut hand = self.last_hand.clone();
        hand.flags = 0;
        hand.tips_active = 0;
        hand.pinch = [0; 4];
        let grasp = XrActionStateFloat::get(xr, session, actions.hand_grab_action, self.path);
        let grasp_ready =
            XrActionStateBoolean::get(xr, session, actions.hand_grab_ready_action, self.path);
        let mut center_tracked = false;
        let mut wrist_tracked = false;
        let mut s = 0;
        for i in 0..self.joint_locations.len() {
            let tracked = self.joint_locations[i]
                .location_flags
                .contains(XrSpaceLocationFlags::ORIENTATION_TRACKED)
                && self.joint_locations[i]
                    .location_flags
                    .contains(XrSpaceLocationFlags::POSITION_TRACKED);
            let pose = self.joint_locations[i].pose;
            let pose_finite = xr_pose_is_finite(pose);
            // we're going to skip the tips and only store the distance
            if i == 5 || i == 10 || i == 15 || i == 20 || i == 25 {
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
                let slot = i / 5 - 1;
                let prev_pose = self.joint_locations[i - 1].pose;
                let prev_tracked = self.joint_locations[i - 1]
                    .location_flags
                    .contains(XrSpaceLocationFlags::ORIENTATION_TRACKED)
                    && self.joint_locations[i - 1]
                        .location_flags
                        .contains(XrSpaceLocationFlags::POSITION_TRACKED)
                    && xr_pose_is_finite(prev_pose);
                if tracked && pose_finite && prev_tracked {
                    let d = pose.position - prev_pose.position;
                    if d.x.is_finite() && d.y.is_finite() && d.z.is_finite() {
                        hand.tips[slot] = d.length();
                        hand.tips_active |= 1 << slot;
                    }
                }
                continue;
            }

            if tracked && pose_finite {
                hand.joints[s] = xr_pose(pose);
                if s == xr_hand::CENTER {
                    center_tracked = true;
                }
                if s == xr_hand::WRIST {
                    wrist_tracked = true;
                }
            }
            s += 1;
        }
        let aim_pose_valid = xr_pose_is_finite(aim_state.aim_pose);
        if aim_pose_valid {
            hand.aim_pose = xr_pose(aim_state.aim_pose);
        }
        hand.flags |= if center_tracked && wrist_tracked{xr_hand::IN_VIEW}else{0}|
        if aim_pose_valid && aim_state.status.contains(XrHandTrackingAimFlagsFB::VALID){xr_hand::AIM_VALID}else{0}|
        if aim_state.status.contains(XrHandTrackingAimFlagsFB::INDEX_PINCHING){xr_hand::PINCH_INDEX}else{0}|
        if aim_state.status.contains(XrHandTrackingAimFlagsFB::MIDDLE_PINCHING){xr_hand::PINCH_MIDDLE}else{0}|
        if aim_state.status.contains(XrHandTrackingAimFlagsFB::RING_PINCHING){xr_hand::PINCH_RING}else{0}|
        if aim_state.status.contains(XrHandTrackingAimFlagsFB::LITTLE_PINCHING){xr_hand::PINCH_LITTLE}else{0}|
        if aim_state.status.contains(XrHandTrackingAimFlagsFB::DOMINANT_HAND){xr_hand::DOMINANT_HAND}else{0}|
        if aim_state.status.contains(XrHandTrackingAimFlagsFB::MENU_PRESSED){xr_hand::MENU_PRESSED}else{0}|
        //if aim_state.status.contains(XrHandTrackingAimFlagsFB::SYSTEM_GESTURE){XrHand::SYSTEM_GESTURE}else{0}|
        0;
        let grab_ready = grasp_ready.is_active.as_bool() && grasp_ready.current_state.as_bool();
        let grab_strength = if grasp.is_active.as_bool() {
            grasp.current_state
        } else {
            0.0
        };
        self.grab_active = xr_hand_grab_active(self.grab_active, grab_ready, grab_strength);
        if self.grab_active {
            hand.tips_active |= xr_hand::GRAB_ACTIVE;
        }
        hand.pinch[xr_hand::PINCH_STRENGTH_INDEX] =
            (aim_state.pinch_strength_index * u8::MAX as f32) as u8;
        hand.pinch[xr_hand::PINCH_STRENGTH_MIDDLE] =
            (aim_state.pinch_strength_middle * u8::MAX as f32) as u8;
        hand.pinch[xr_hand::PINCH_STRENGTH_RING] =
            (aim_state.pinch_strength_ring * u8::MAX as f32) as u8;
        hand.pinch[xr_hand::PINCH_STRENGTH_LITTLE] =
            (aim_state.pinch_strength_little * u8::MAX as f32) as u8;
        self.last_hand = hand.clone();
        hand
    }
}

fn xr_pose_is_finite(pose: XrPosef) -> bool {
    pose.position.x.is_finite()
        && pose.position.y.is_finite()
        && pose.position.z.is_finite()
        && pose.orientation.x.is_finite()
        && pose.orientation.y.is_finite()
        && pose.orientation.z.is_finite()
        && pose.orientation.w.is_finite()
}

const XR_HAND_GRAB_ACTIVATE_THRESHOLD: f32 = 0.72;
const XR_HAND_GRAB_RELEASE_THRESHOLD: f32 = 0.42;

fn xr_hand_grab_active(previous: bool, grab_ready: bool, grab_strength: f32) -> bool {
    if !grab_ready || !grab_strength.is_finite() {
        return false;
    }
    if previous {
        grab_strength >= XR_HAND_GRAB_RELEASE_THRESHOLD
    } else {
        grab_strength >= XR_HAND_GRAB_ACTIVATE_THRESHOLD
    }
}

pub struct CxOpenXrController {
    path: XrPath,
    aim_space: XrSpace,
    grip_space: XrSpace,
    detached_aim_space: XrSpace,
    detached_grip_space: XrSpace,
}

impl CxOpenXrController {
    /// The grip and the aim at `time` with their velocities (see `CxOpenXrInputs::locate_controllers`).
    fn locate_moving(&self, xr: &LibOpenXr, session: XrSession, local_space: XrSpace, time: XrTime, actions: &CxOpenXrInputActions) -> [(XrPose, [f32; 3], [f32; 3], u32); 2] {
        let grip_active = XrActionStatePose::get(xr, session, actions.grip_pose_action, self.path).is_active.as_bool();
        let aim_active = XrActionStatePose::get(xr, session, actions.aim_pose_action, self.path).is_active.as_bool();
        let spaces = [if grip_active { self.grip_space } else { self.detached_grip_space }, if aim_active { self.aim_space } else { self.detached_aim_space }];
        spaces.map(|space| {
            let mut velocity = XrSpaceVelocity::default();
            let mut location = XrSpaceLocation { next: &mut velocity as *mut _ as *mut _, ..Default::default() };
            unsafe { (xr.xrLocateSpace)(space, local_space, time, &mut location) }.log_error("xrLocateSpace controller");
            let v = |a: XrVector3f| [a.x, a.y, a.z];
            (xr_pose(location.pose), v(velocity.linear_velocity), v(velocity.angular_velocity), [XrSpaceLocationFlags::ORIENTATION_VALID, XrSpaceLocationFlags::POSITION_VALID, XrSpaceLocationFlags::ORIENTATION_TRACKED, XrSpaceLocationFlags::POSITION_TRACKED].iter().enumerate().map(|(i, f)| (location.location_flags.contains(*f) as u32) << i).sum::<u32>() | (velocity.velocity_flags as u32) << 8)
        })
    }

    fn destroy(&self, xr: &LibOpenXr) {
        self.aim_space.destroy(xr);
        self.grip_space.destroy(xr);
        self.detached_aim_space.destroy(xr);
        self.detached_grip_space.destroy(xr);
    }

    fn poll(
        &self,
        xr: &LibOpenXr,
        session: XrSession,
        local_space: XrSpace,
        time: XrTime,
        is_left: bool,
        actions: &CxOpenXrInputActions,
    ) -> XrController {
        // lets query the trigger bool
        let stick = XrActionStateVector2f::get(xr, session, actions.thumbstick_action, self.path);
        let trigger = XrActionStateFloat::get(xr, session, actions.trigger_action, self.path);
        let grip = XrActionStateFloat::get(xr, session, actions.grip_action, self.path);
        let grip_state = XrActionStatePose::get(xr, session, actions.grip_pose_action, self.path);
        let aim_state = XrActionStatePose::get(xr, session, actions.aim_pose_action, self.path);

        //crate::log!("{:?}", XrActionStateBoolean::get(xr, session, actions.click_x_action, self.path).current_state.as_bool());

        fn bf(
            xr: &LibOpenXr,
            session: XrSession,
            path: XrPath,
            action: XrAction,
            flag: u32,
            on: bool,
        ) -> u32 {
            if !on {
                return 0;
            }
            if XrActionStateBoolean::get(xr, session, action, path)
                .current_state
                .as_bool()
            {
                flag
            } else {
                0
            }
        }

        XrController {
            grip_pose: if grip_state.is_active.as_bool() {
                xr_pose(XrSpaceLocation::locate(xr, local_space, time, self.grip_space).pose)
            } else {
                xr_pose(XrSpaceLocation::locate(xr, local_space, time, self.detached_grip_space).pose)
            },
            aim_pose: if aim_state.is_active.as_bool() {
                xr_pose(XrSpaceLocation::locate(xr, local_space, time, self.aim_space).pose)
            } else {
                xr_pose(XrSpaceLocation::locate(xr, local_space, time, self.detached_aim_space).pose)
            },
            // the runtime's sign (up positive); Cx flips it as the old backend did
            stick: [stick.current_state.x, stick.current_state.y],
            trigger: trigger.current_state,
            grip: grip.current_state,
            //last_buttons,
            buttons: if aim_state.is_active.as_bool() {
                xr_controller::ACTIVE
            } else {
                0
            } | bf(
                xr,
                session,
                self.path,
                actions.click_a_action,
                xr_controller::CLICK_A,
                !is_left,
            ) | bf(
                xr,
                session,
                self.path,
                actions.click_b_action,
                xr_controller::CLICK_B,
                !is_left,
            ) | bf(
                xr,
                session,
                self.path,
                actions.click_x_action,
                xr_controller::CLICK_X,
                is_left,
            ) | bf(
                xr,
                session,
                self.path,
                actions.click_y_action,
                xr_controller::CLICK_Y,
                is_left,
            ) | bf(
                xr,
                session,
                self.path,
                actions.click_menu_action,
                xr_controller::CLICK_MENU,
                is_left,
            ) | bf(
                xr,
                session,
                self.path,
                actions.click_thumbstick_action,
                xr_controller::CLICK_THUMBSTICK,
                true,
            ) | bf(
                xr,
                session,
                self.path,
                actions.touch_thumbstick_action,
                xr_controller::TOUCH_THUMBSTICK,
                true,
            ) | bf(
                xr,
                session,
                self.path,
                actions.touch_trigger_action,
                xr_controller::TOUCH_TRIGGER,
                true,
            ) | bf(
                xr,
                session,
                self.path,
                actions.touch_a_action,
                xr_controller::TOUCH_A,
                !is_left,
            ) | bf(
                xr,
                session,
                self.path,
                actions.touch_b_action,
                xr_controller::TOUCH_B,
                !is_left,
            ) | bf(
                xr,
                session,
                self.path,
                actions.touch_x_action,
                xr_controller::TOUCH_X,
                is_left,
            ) | bf(
                xr,
                session,
                self.path,
                actions.touch_y_action,
                xr_controller::TOUCH_Y,
                is_left,
            ) | bf(
                xr,
                session,
                self.path,
                actions.touch_thumbrest_action,
                xr_controller::TOUCH_THUMBREST,
                true,
            ),
        }
    }
}

impl CxOpenXrInputs {
    pub fn new_inputs(
        xr: &LibOpenXr,
        session: XrSession,
        instance: XrInstance,
    ) -> Result<CxOpenXrInputs, String> {
        // create handtrackers
        let left_hand_info = XrHandTrackerCreateInfoEXT {
            hand: XrHandEXT::LEFT,
            ..Default::default()
        };
        let mut left_hand_track = XrHandTrackerEXT(0);
        unsafe { (xr.xrCreateHandTrackerEXT)(session, &left_hand_info, &mut left_hand_track) }
            .log_error("xrCreateHandTrackerEXT");

        let right_hand_info = XrHandTrackerCreateInfoEXT {
            hand: XrHandEXT::RIGHT,
            ..Default::default()
        };
        let mut right_hand_track = XrHandTrackerEXT(0);
        unsafe { (xr.xrCreateHandTrackerEXT)(session, &right_hand_info, &mut right_hand_track) }
            .log_error("xrCreateHandTrackerEXT");

        let action_set =
            XrActionSet::new(xr, instance, 1, "makepad_action_set", "Main action set")?;

        let left_hand_path = XrPath::new(xr, instance, "/user/hand/left")?;
        let right_hand_path = XrPath::new(xr, instance, "/user/hand/right")?;
        let left_detached_path = XrPath::new(xr, instance, "/user/detached_controller_meta/left")?;
        let right_detached_path =
            XrPath::new(xr, instance, "/user/detached_controller_meta/right")?;
        let hand_paths = [left_hand_path, right_hand_path];
        let detached_paths = [left_detached_path, right_detached_path];

        let trigger_action = XrAction::new(
            xr,
            action_set,
            XrActionType::FLOAT_INPUT,
            "trigger",
            "",
            &hand_paths,
        )?;
        let hand_grab_action = XrAction::new(
            xr,
            action_set,
            XrActionType::FLOAT_INPUT,
            "hand_grab",
            "",
            &hand_paths,
        )?;
        let hand_grab_ready_action = XrAction::new(
            xr,
            action_set,
            XrActionType::BOOLEAN_INPUT,
            "hand_grab_ready",
            "",
            &hand_paths,
        )?;
        let grip_action = XrAction::new(
            xr,
            action_set,
            XrActionType::FLOAT_INPUT,
            "grip",
            "",
            &hand_paths,
        )?;
        let thumbstick_action = XrAction::new(
            xr,
            action_set,
            XrActionType::VECTOR2F_INPUT,
            "thumbstick_xy",
            "",
            &hand_paths,
        )?;
        let click_a_action = XrAction::new(
            xr,
            action_set,
            XrActionType::BOOLEAN_INPUT,
            "click_a",
            "",
            &hand_paths,
        )?;
        let click_b_action = XrAction::new(
            xr,
            action_set,
            XrActionType::BOOLEAN_INPUT,
            "click_b",
            "",
            &hand_paths,
        )?;
        let click_x_action = XrAction::new(
            xr,
            action_set,
            XrActionType::BOOLEAN_INPUT,
            "click_x",
            "",
            &hand_paths,
        )?;
        let click_y_action = XrAction::new(
            xr,
            action_set,
            XrActionType::BOOLEAN_INPUT,
            "click_y",
            "",
            &hand_paths,
        )?;
        let click_menu_action = XrAction::new(
            xr,
            action_set,
            XrActionType::BOOLEAN_INPUT,
            "click_menu",
            "",
            &hand_paths,
        )?;
        let click_thumbstick_action = XrAction::new(
            xr,
            action_set,
            XrActionType::BOOLEAN_INPUT,
            "click_thumbstick",
            "",
            &hand_paths,
        )?;
        let touch_thumbstick_action = XrAction::new(
            xr,
            action_set,
            XrActionType::BOOLEAN_INPUT,
            "touch_thumbstick",
            "",
            &hand_paths,
        )?;
        let touch_trigger_action = XrAction::new(
            xr,
            action_set,
            XrActionType::BOOLEAN_INPUT,
            "touch_trigger",
            "",
            &hand_paths,
        )?;
        let touch_a_action = XrAction::new(
            xr,
            action_set,
            XrActionType::BOOLEAN_INPUT,
            "touch_a",
            "",
            &hand_paths,
        )?;
        let touch_b_action = XrAction::new(
            xr,
            action_set,
            XrActionType::BOOLEAN_INPUT,
            "touch_b",
            "",
            &hand_paths,
        )?;
        let touch_x_action = XrAction::new(
            xr,
            action_set,
            XrActionType::BOOLEAN_INPUT,
            "touch_x",
            "",
            &hand_paths,
        )?;
        let touch_y_action = XrAction::new(
            xr,
            action_set,
            XrActionType::BOOLEAN_INPUT,
            "touchy_y",
            "",
            &hand_paths,
        )?;
        let touch_thumbrest_action = XrAction::new(
            xr,
            action_set,
            XrActionType::BOOLEAN_INPUT,
            "touch_thumbrest",
            "",
            &hand_paths,
        )?;

        let aim_pose_action = XrAction::new(
            xr,
            action_set,
            XrActionType::POSE_INPUT,
            "aim_pose",
            "",
            &hand_paths,
        )?;
        let haptic_action = XrAction::new(
            xr,
            action_set,
            XrActionType::VIBRATION_OUTPUT,
            "haptic",
            "",
            &hand_paths,
        )?;
        let grip_pose_action = XrAction::new(
            xr,
            action_set,
            XrActionType::POSE_INPUT,
            "grip_pose",
            "",
            &hand_paths,
        )?;

        // the hands' grip pose (hand_interaction_ext): the runtime's own grip of a tracked hand,
        // located like a controller (with velocity), apart from the joints
        let hand_grip_pose_action = XrAction::new(
            xr,
            action_set,
            XrActionType::POSE_INPUT,
            "hand_grip_pose",
            "",
            &hand_paths,
        )?;

        let detached_aim_pose_action = XrAction::new(
            xr,
            action_set,
            XrActionType::POSE_INPUT,
            "detached_aim_pose",
            "",
            &detached_paths,
        )?;
        let detached_grip_pose_action = XrAction::new(
            xr,
            action_set,
            XrActionType::POSE_INPUT,
            "detached_grip_pose",
            "",
            &detached_paths,
        )?;

        let interaction_profile = XrPath::new(
            xr,
            instance,
            "/interaction_profiles/meta/touch_controller_plus",
        )?;
        let hand_interaction_profile = XrPath::new(
            xr,
            instance,
            "/interaction_profiles/ext/hand_interaction_ext",
        )?;

        let bindings = [
            XrActionSuggestedBinding::new(
                xr,
                instance,
                trigger_action,
                "/user/hand/left/input/trigger",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                trigger_action,
                "/user/hand/right/input/trigger",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                grip_action,
                "/user/hand/left/input/squeeze/value",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                grip_action,
                "/user/hand/right/input/squeeze/value",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                thumbstick_action,
                "/user/hand/left/input/thumbstick",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                thumbstick_action,
                "/user/hand/right/input/thumbstick",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                click_a_action,
                "/user/hand/right/input/a/click",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                click_b_action,
                "/user/hand/right/input/b/click",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                click_x_action,
                "/user/hand/left/input/x/click",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                click_y_action,
                "/user/hand/left/input/y/click",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                click_menu_action,
                "/user/hand/left/input/menu/click",
            )?,
            XrActionSuggestedBinding::new(xr, instance, haptic_action, "/user/hand/left/output/haptic")?,
            XrActionSuggestedBinding::new(xr, instance, haptic_action, "/user/hand/right/output/haptic")?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                click_thumbstick_action,
                "/user/hand/left/input/thumbstick/click",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                click_thumbstick_action,
                "/user/hand/right/input/thumbstick/click",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                touch_thumbstick_action,
                "/user/hand/left/input/thumbstick/touch",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                touch_thumbstick_action,
                "/user/hand/right/input/thumbstick/touch",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                touch_trigger_action,
                "/user/hand/left/input/trigger/touch",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                touch_trigger_action,
                "/user/hand/right/input/trigger/touch",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                touch_a_action,
                "/user/hand/right/input/a/touch",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                touch_b_action,
                "/user/hand/right/input/b/touch",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                touch_x_action,
                "/user/hand/left/input/x/touch",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                touch_y_action,
                "/user/hand/left/input/y/touch",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                touch_thumbrest_action,
                "/user/hand/left/input/thumbrest/touch",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                touch_thumbrest_action,
                "/user/hand/right/input/thumbrest/touch",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                aim_pose_action,
                "/user/hand/left/input/aim/pose",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                aim_pose_action,
                "/user/hand/right/input/aim/pose",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                grip_pose_action,
                "/user/hand/left/input/grip/pose",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                grip_pose_action,
                "/user/hand/right/input/grip/pose",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                detached_aim_pose_action,
                "/user/detached_controller_meta/left/input/aim/pose",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                detached_aim_pose_action,
                "/user/detached_controller_meta/right/input/aim/pose",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                detached_grip_pose_action,
                "/user/detached_controller_meta/left/input/grip/pose",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                detached_grip_pose_action,
                "/user/detached_controller_meta/right/input/grip/pose",
            )?,
        ];

        let hand_bindings = [
            XrActionSuggestedBinding::new(
                xr,
                instance,
                hand_grip_pose_action,
                "/user/hand/left/input/grip/pose",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                hand_grip_pose_action,
                "/user/hand/right/input/grip/pose",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                hand_grab_action,
                "/user/hand/left/input/grasp_ext/value",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                hand_grab_action,
                "/user/hand/right/input/grasp_ext/value",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                hand_grab_ready_action,
                "/user/hand/left/input/grasp_ext/ready_ext",
            )?,
            XrActionSuggestedBinding::new(
                xr,
                instance,
                hand_grab_ready_action,
                "/user/hand/right/input/grasp_ext/ready_ext",
            )?,
        ];

        let suggested_bindings = XrInteractionProfileSuggestedBinding {
            interaction_profile,
            count_suggested_bindings: bindings.len() as _,
            suggested_bindings: bindings.as_ptr(),
            ..Default::default()
        };

        unsafe { (xr.xrSuggestInteractionProfileBindings)(instance, &suggested_bindings) }
            .to_result("xrSuggestInteractionProfileBindings")?;

        let hand_suggested_bindings = XrInteractionProfileSuggestedBinding {
            interaction_profile: hand_interaction_profile,
            count_suggested_bindings: hand_bindings.len() as _,
            suggested_bindings: hand_bindings.as_ptr(),
            ..Default::default()
        };

        unsafe { (xr.xrSuggestInteractionProfileBindings)(instance, &hand_suggested_bindings) }
            .to_result("xrSuggestInteractionProfileBindings hand_interaction_ext")?;

        let attach_info = XrSessionActionSetsAttachInfo {
            count_action_sets: 1,
            action_sets: &action_set as *const _,
            ..Default::default()
        };

        unsafe { (xr.xrAttachSessionActionSets)(session, &attach_info) }
            .to_result("xrAttachSessionActionSets")?;
        let mut pose = XrPosef::default();
        pose.orientation.w = 1.0;

        let resume_info = XrSimultaneousHandsAndControllersTrackingResumeInfoMETA::default();
        unsafe { (xr.xrResumeSimultaneousHandsAndControllersTrackingMETA)(session, &resume_info) }
            .log_error("xrResumeSimultaneousHandsAndControllersTrackingMETA");

        let actions = CxOpenXrInputActions {
            trigger_action,
            grip_action,
            hand_grab_action,
            hand_grab_ready_action,
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
            haptic_action,
        };

        Ok(CxOpenXrInputs {
            action_set,
            actions,
            left_hand: CxOpenXrHand {
                path: left_hand_path,
                tracker: left_hand_track,
                joint_locations: Default::default(),
                grab_active: false,
                last_hand: XrHand::default(),
                grip_space: XrSpace::new_action_space(xr, session, hand_grip_pose_action, left_hand_path, pose)?,
            },
            right_hand: CxOpenXrHand {
                path: right_hand_path,
                tracker: right_hand_track,
                joint_locations: Default::default(),
                grab_active: false,
                last_hand: XrHand::default(),
                grip_space: XrSpace::new_action_space(xr, session, hand_grip_pose_action, right_hand_path, pose)?,
            },
            left_controller: CxOpenXrController {
                path: left_hand_path,
                aim_space: XrSpace::new_action_space(
                    xr,
                    session,
                    aim_pose_action,
                    left_hand_path,
                    pose,
                )?,
                grip_space: XrSpace::new_action_space(
                    xr,
                    session,
                    grip_pose_action,
                    left_hand_path,
                    pose,
                )?,
                detached_aim_space: XrSpace::new_action_space(
                    xr,
                    session,
                    detached_aim_pose_action,
                    left_detached_path,
                    pose,
                )?,
                detached_grip_space: XrSpace::new_action_space(
                    xr,
                    session,
                    detached_grip_pose_action,
                    left_detached_path,
                    pose,
                )?,
            },
            right_controller: CxOpenXrController {
                path: right_hand_path,
                aim_space: XrSpace::new_action_space(
                    xr,
                    session,
                    aim_pose_action,
                    right_hand_path,
                    pose,
                )?,
                grip_space: XrSpace::new_action_space(
                    xr,
                    session,
                    grip_pose_action,
                    right_hand_path,
                    pose,
                )?,
                detached_aim_space: XrSpace::new_action_space(
                    xr,
                    session,
                    detached_aim_pose_action,
                    right_detached_path,
                    pose,
                )?,
                detached_grip_space: XrSpace::new_action_space(
                    xr,
                    session,
                    detached_grip_pose_action,
                    right_detached_path,
                    pose,
                )?,
            },
        })
    }

    pub fn destroy_input(&self, xr: &LibOpenXr) {
        self.left_controller.destroy(xr);
        self.right_controller.destroy(xr);
        self.left_hand.destroy(xr);
        self.right_hand.destroy(xr);
        unsafe { (xr.xrDestroyActionSet)(self.action_set) }.log_error("xrDestroyActionSet");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xr_hand_grab_hysteresis_requires_stronger_activation_than_release() {
        assert!(!xr_hand_grab_active(false, true, 0.60));
        assert!(xr_hand_grab_active(false, true, 0.80));
        assert!(xr_hand_grab_active(true, true, 0.50));
        assert!(!xr_hand_grab_active(true, true, 0.30));
    }

    #[test]
    fn xr_hand_grab_hysteresis_clears_when_grab_ready_drops() {
        assert!(!xr_hand_grab_active(true, false, 1.0));
        assert!(!xr_hand_grab_active(true, true, f32::NAN));
    }
}
