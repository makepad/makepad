use super::xr_env::XrEnv;
use super::xr_gesture::{floor_set_gesture_sample, XrFloorSetGestureSample};
use super::xr_select::XrSelectAction;
use super::{arm_pair_metrics, flat_head_forward, hand_closed_fist_contact_point};
use crate::prelude::*;
use crate::util::scene_draw::{ray_from_scene_viewport, SceneState3D};
use makepad_widgets::event::{XrFingerTip, XrSyncAnchor, XrSyncAnchorExtrema};
use makepad_widgets::makepad_script::ScriptFnRef;
use std::{cell::Cell, collections::HashMap, fmt::Write as _, rc::Rc, time::Instant};

const DESKTOP_TOUCH_DOWN_Z: f32 = 0.0;
const DESKTOP_TOUCH_UP_Z: f32 = 64.0;
const XR_CONTENT_FORWARD_OFFSET: f32 = 0.28;
const XR_ACTIVITY_RESET_FORWARD_OFFSET: f32 = 1.0;
const XR_CONTENT_VERTICAL_OFFSET: f32 = -0.58;
const SYNC_BOX_BROADCAST_SECONDS: f64 = 1.1;
const SYNC_BOX_DIRECTION_DEADZONE_METERS: f32 = 0.006;
const SYNC_BOX_REVERSAL_MIN_TRAVEL_METERS: f32 = 0.05;
const SYNC_BOX_MAX_VERTICAL_DELTA_METERS: f32 = 0.22;
const SYNC_BOX_MAX_DEPTH_DELTA_METERS: f32 = 0.22;
const SYNC_BOX_MIN_HAND_GAP_METERS: f32 = 0.06;
const SYNC_BOX_MAX_HAND_GAP_METERS: f32 = 0.78;
const SYNC_BOX_MIN_CHEST_DISTANCE_METERS: f32 = 0.10;
const SYNC_BOX_MAX_CHEST_DISTANCE_METERS: f32 = 1.05;
const SYNC_BOX_MAX_ARM_ELEVATION_DEGREES: f32 = 60.0;
const XR_FIXED_DEPTH_VOXEL_SIZE_METERS: f32 = 0.02;
const XR_ROOT_TOP_CHILD_DRAW_METRIC_COUNT: usize = 4;
const FLOOR_SET_HOLD_SECONDS: f64 = 2.0;
const FLOOR_SET_SAMPLE_POSITION_TOLERANCE_METERS: f32 = 0.05;
const FLOOR_SET_SAMPLE_GAP_TOLERANCE_METERS: f32 = 0.08;
const FLOOR_SET_SAMPLE_FLOOR_TOLERANCE_METERS: f32 = 0.03;
const FLOOR_SET_PREVIEW_SECONDS: f64 = 2.0;

script_mod! {
    use mod.prelude.widgets.*
    use mod.math.*
    use mod.widgets.*

    mod.widgets.XrCamera = set_type_default() do #(XrCamera::script_component(vm))

    mod.widgets.XrRootBase = #(XrRoot::register_widget(vm))
    mod.widgets.XrRoot = set_type_default() do mod.widgets.XrRootBase{
        width: Fill
        height: Fill
        flow: Overlay

        window +: {
            inner_size: vec2(1400, 900)
        }
        pass +: {
            clear_color: #x0b1118
            keep_camera_matrix: true
        }
        env: mod.widgets.XrEnv{}
        camera: mod.widgets.XrCamera{}
    }
}

#[derive(Script, ScriptHook, Clone)]
pub struct XrCamera {
    #[live(28.0)]
    pub fov_y: f32,
    #[live(vec3(0.0, -0.10, -1.30))]
    pub desktop_target: Vec3f,
    #[live(3.4)]
    pub distance: f32,
    #[live(0.05)]
    pub near: f32,
    #[live(200.0)]
    pub far: f32,
    #[live(0.25)]
    pub distance_min: f32,
    #[live(30.0)]
    pub distance_max: f32,
    #[live(0.08)]
    pub wheel_zoom_step: f32,
    #[rust(0.0)]
    pub orbit_yaw: f32,
    #[rust(0.0)]
    pub orbit_pitch: f32,
    #[rust]
    pub orbit_last_abs: Option<DVec2>,
    #[rust]
    pub viewport_rect: Option<Rect>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum XrRootAction {
    PhysicsReset,
    ContentPoseReset(Pose),
    #[default]
    None,
}

impl Default for XrCamera {
    fn default() -> Self {
        Self {
            fov_y: 28.0,
            distance: 3.4,
            near: 0.05,
            far: 200.0,
            desktop_target: vec3f(0.0, -0.10, -1.30),
            distance_min: 0.25,
            distance_max: 30.0,
            wheel_zoom_step: 0.08,
            orbit_yaw: 0.0,
            orbit_pitch: 0.0,
            orbit_last_abs: None,
            viewport_rect: None,
        }
    }
}

impl XrCamera {
    pub fn desktop_scene_state(&self, viewport_rect: Rect, time: f64) -> Option<SceneState3D> {
        if viewport_rect.size.x <= 1.0 || viewport_rect.size.y <= 1.0 {
            return None;
        }

        let aspect = (viewport_rect.size.x / viewport_rect.size.y).max(0.001) as f32;
        let distance_min = self.distance_min.max(0.01);
        let distance_max = self.distance_max.max(distance_min + 0.01);
        let distance = self.distance.clamp(distance_min, distance_max);
        let yaw = self.orbit_yaw;
        let pitch = self.orbit_pitch.clamp(-1.45, 1.45);
        let forward = vec3f(
            yaw.sin() * pitch.cos(),
            pitch.sin(),
            -yaw.cos() * pitch.cos(),
        )
        .normalize();
        let target = self.desktop_target;
        let camera_pos = target - forward * distance;
        let view = Mat4f::look_at(camera_pos, target, vec3f(0.0, 1.0, 0.0));
        let projection = Mat4f::perspective(
            self.fov_y.clamp(1.0, 179.0),
            aspect,
            self.near.max(0.001),
            self.far.max(self.near + 0.001),
        );

        Some(SceneState3D {
            time,
            camera_pos,
            view,
            projection,
            viewport_rect,
        })
    }

    pub fn xr_scene_state(&self, state: &XrState) -> SceneState3D {
        SceneState3D {
            time: state.time,
            camera_pos: state.head_pose.position,
            view: Mat4f::identity(),
            projection: Mat4f::identity(),
            viewport_rect: Rect::default(),
        }
    }

    pub fn set_desktop_viewport_rect(&mut self, viewport_rect: Rect) {
        self.viewport_rect = Some(viewport_rect);
    }

    fn contains_abs(&self, abs: DVec2) -> bool {
        self.viewport_rect.is_some_and(|rect| rect.contains(abs))
    }

    pub fn handle_desktop_interaction(&mut self, cx: &mut Cx, event: &Event) {
        match event {
            Event::MouseDown(fe) if self.contains_abs(fe.abs) && fe.button.is_primary() => {
                self.orbit_last_abs = Some(fe.abs);
                cx.set_cursor(MouseCursor::Grabbing);
            }
            Event::MouseMove(fe) => {
                if let Some(last_abs) = self.orbit_last_abs {
                    let delta = fe.abs - last_abs;
                    self.orbit_yaw -= (delta.x as f32) * 0.01;
                    self.orbit_pitch =
                        (self.orbit_pitch + (delta.y as f32) * 0.01).clamp(-1.45, 1.45);
                    self.orbit_last_abs = Some(fe.abs);
                    cx.set_cursor(MouseCursor::Grabbing);
                    cx.redraw_all();
                } else if self.contains_abs(fe.abs) {
                    cx.set_cursor(MouseCursor::Grab);
                } else {
                    cx.set_cursor(MouseCursor::Default);
                }
            }
            Event::Scroll(fs) if self.contains_abs(fs.abs) => {
                let scroll_axis = if fs.scroll.y.abs() > f64::EPSILON {
                    fs.scroll.y
                } else {
                    fs.scroll.x
                };
                if scroll_axis.abs() > f64::EPSILON {
                    let step = self.wheel_zoom_step.max(0.001);
                    let factor = if scroll_axis > 0.0 {
                        1.0 / (1.0 - step)
                    } else {
                        1.0 - step
                    };
                    self.distance = (self.distance * factor).clamp(
                        self.distance_min.max(0.01),
                        self.distance_max.max(self.distance_min.max(0.01) + 0.01),
                    );
                    cx.redraw_all();
                }
            }
            Event::MouseUp(fe) if fe.button.is_primary() => {
                let was_dragging = self.orbit_last_abs.take().is_some();
                if was_dragging || self.contains_abs(fe.abs) {
                    cx.set_cursor(if self.contains_abs(fe.abs) {
                        MouseCursor::Grab
                    } else {
                        MouseCursor::Default
                    });
                }
            }
            Event::MouseLeave(_) if self.orbit_last_abs.is_none() => {
                cx.set_cursor(MouseCursor::Default);
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct BoxSyncPoseSample {
    anchor: XrAnchor,
    captured_at: f64,
    vertical_split: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BoxSyncMotionDirection {
    Rising,
    Falling,
}

#[derive(Clone, Debug, Default)]
struct BoxSyncGestureDetector {
    previous_sample: Option<BoxSyncPoseSample>,
    direction: Option<BoxSyncMotionDirection>,
    extreme_sample: Option<BoxSyncPoseSample>,
}

#[derive(Default)]
struct XrRootRuntime {
    initialized: bool,
    started: bool,
    last_xr_state: Option<Rc<XrState>>,
    last_dispatched_xr_state: Option<Rc<XrState>>,
    next_frame: NextFrame,
    desktop_ui_pointer_active: bool,
}

#[derive(Default)]
struct XrRootFrameMetrics {
    last_frame_update_cpu_ms: f64,
    last_frame_draw_cpu_ms: f64,
    last_frame_cpu_ms: f64,
    last_draw_setup_cpu_ms: f64,
    last_draw_env_prepare_cpu_ms: f64,
    last_draw_sort_cpu_ms: f64,
    last_draw_children_cpu_ms: f64,
    last_draw_child_count: usize,
    last_draw_transparent_child_count: usize,
    last_draw_runtime_body_count: usize,
    last_draw_geometry_pool_slots: usize,
    last_draw_geometry_pool_live: usize,
    last_draw_draw_list_pool_slots: usize,
    last_draw_draw_list_pool_live: usize,
    last_draw_texture_pool_slots: usize,
    last_draw_texture_pool_live: usize,
    last_draw_depth_mesh_chunk_count: usize,
    last_draw_recycled_depth_mesh_geometry_count: usize,
    last_draw_depth_mesh_pending_upsert_count: usize,
    last_draw_depth_query_retained_hit_count: usize,
    last_draw_top_children: [(LiveId, f64); XR_ROOT_TOP_CHILD_DRAW_METRIC_COUNT],
    last_draw_top_child_count: usize,
}

impl XrRootFrameMetrics {
    fn update_total(&mut self) {
        self.last_frame_cpu_ms = self.last_frame_update_cpu_ms + self.last_frame_draw_cpu_ms;
    }

    fn finish_draw(&mut self, started: Instant) {
        self.last_frame_draw_cpu_ms = started.elapsed().as_secs_f64() * 1000.0;
        self.update_total();
    }

    fn finish_update(&mut self, started: Instant) {
        self.last_frame_update_cpu_ms = started.elapsed().as_secs_f64() * 1000.0;
        self.update_total();
    }
}

#[derive(Clone, Copy, Default)]
struct XrContentRig {
    pose: Option<Pose>,
}

impl XrContentRig {
    fn resolved_pose(&self, state: Option<&XrState>) -> Option<Pose> {
        self.pose.or_else(|| state.map(Self::pose_from_state))
    }

    fn flat_forward(orientation: Quat) -> Vec3f {
        let mut forward = orientation.rotate_vec3(&vec3f(0.0, 0.0, -1.0));
        forward.y = 0.0;
        if forward.length() <= 1.0e-6 {
            vec3f(0.0, 0.0, -1.0)
        } else {
            forward.normalize()
        }
    }

    fn pose_from_state_with_forward_offset(state: &XrState, forward_offset: f32) -> Pose {
        let forward = Self::flat_forward(state.head_pose.orientation);
        Pose {
            position: state.head_pose.position
                + forward.scale(forward_offset)
                + vec3f(0.0, XR_CONTENT_VERTICAL_OFFSET, 0.0),
            orientation: Quat::look_rotation(forward.scale(-1.0), vec3f(0.0, 1.0, 0.0)),
        }
    }

    fn pose_from_state(state: &XrState) -> Pose {
        Self::pose_from_state_with_forward_offset(state, XR_CONTENT_FORWARD_OFFSET)
    }

    fn reset_pose_from_state(state: &XrState) -> Pose {
        Self::pose_from_state_with_forward_offset(state, XR_ACTIVITY_RESET_FORWARD_OFFSET)
    }

    fn ensure_pose(&mut self, cx: &mut Cx, env: &mut XrEnv, state: &XrState) {
        if self.pose.is_some() {
            return;
        }
        let pose = Self::pose_from_state(state);
        self.pose = Some(pose);
        env.set_root_pose(cx, Some(pose));
    }

    fn clear_pose(&mut self, cx: &mut Cx, env: &mut XrEnv) {
        if self.pose.is_none() {
            return;
        }
        self.pose = None;
        env.set_root_pose(cx, None);
    }

    fn transform(&self, state: Option<&XrState>) -> Mat4f {
        self.resolved_pose(state)
            .map(|pose| pose.to_mat4())
            .unwrap_or_else(Mat4f::identity)
    }
}

#[derive(Clone, Debug, Default)]
struct XrSyncAnchorRuntime {
    detector: BoxSyncGestureDetector,
    pending_sync_anchor: Option<XrSyncAnchor>,
    pending_sync_anchor_emitted_at: Option<f64>,
    next_sync_anchor_id: u32,
}

#[derive(Clone, Copy, Debug)]
struct XrFloorSetGestureCommit {
    floor_y: f32,
    preview_center: Vec3f,
    anchor: Option<XrAnchor>,
    visible_until_time: f64,
}

#[derive(Clone, Debug, Default)]
struct XrFloorSetRuntime {
    active_sample: Option<XrFloorSetGestureSample>,
    active_since_time: Option<f64>,
    committed_while_active: bool,
}

impl XrFloorSetRuntime {
    fn reset(&mut self) {
        self.active_sample = None;
        self.active_since_time = None;
        self.committed_while_active = false;
    }

    fn sample_matches(a: XrFloorSetGestureSample, b: XrFloorSetGestureSample) -> bool {
        (a.midpoint - b.midpoint).length() <= FLOOR_SET_SAMPLE_POSITION_TOLERANCE_METERS
            && (a.floor_y - b.floor_y).abs() <= FLOOR_SET_SAMPLE_FLOOR_TOLERANCE_METERS
            && (a.hand_gap - b.hand_gap).abs() <= FLOOR_SET_SAMPLE_GAP_TOLERANCE_METERS
    }

    fn update(&mut self, state: &XrState) -> Option<XrFloorSetGestureCommit> {
        let sample = floor_set_gesture_sample(state);
        let Some(sample) = sample else {
            self.reset();
            return None;
        };
        if self
            .active_sample
            .is_none_or(|previous| !Self::sample_matches(previous, sample))
        {
            self.active_sample = Some(sample);
            self.active_since_time = Some(state.time);
            self.committed_while_active = false;
            return None;
        }
        self.active_sample = Some(sample);
        if self.committed_while_active {
            return None;
        }
        let active_since_time = self.active_since_time.unwrap_or(state.time);
        if state.time - active_since_time < FLOOR_SET_HOLD_SECONDS {
            return None;
        }
        self.committed_while_active = true;
        Some(XrFloorSetGestureCommit {
            floor_y: sample.floor_y,
            preview_center: sample.midpoint,
            anchor: (!state.anchor_persisted).then_some(sample.anchor),
            visible_until_time: state.time + FLOOR_SET_PREVIEW_SECONDS,
        })
    }
}

impl XrSyncAnchorRuntime {
    fn box_sync_pose_sample(state: &XrState) -> Option<BoxSyncPoseSample> {
        let forward = flat_head_forward(state.head_pose.orientation);
        let left_point = hand_closed_fist_contact_point(&state.left_hand, forward, true)?;
        let right_point = hand_closed_fist_contact_point(&state.right_hand, forward, false)?;
        let metrics = arm_pair_metrics(state.head_pose, left_point, right_point)?;
        if metrics.left_lateral >= metrics.right_lateral
            || metrics.hand_gap < SYNC_BOX_MIN_HAND_GAP_METERS
            || metrics.hand_gap > SYNC_BOX_MAX_HAND_GAP_METERS
            || metrics.left_forward < SYNC_BOX_MIN_CHEST_DISTANCE_METERS
            || metrics.left_forward > SYNC_BOX_MAX_CHEST_DISTANCE_METERS
            || metrics.right_forward < SYNC_BOX_MIN_CHEST_DISTANCE_METERS
            || metrics.right_forward > SYNC_BOX_MAX_CHEST_DISTANCE_METERS
            || metrics.left_elevation_degrees > SYNC_BOX_MAX_ARM_ELEVATION_DEGREES
            || metrics.right_elevation_degrees > SYNC_BOX_MAX_ARM_ELEVATION_DEGREES
            || (left_point.y - right_point.y).abs() > SYNC_BOX_MAX_VERTICAL_DELTA_METERS * 2.0
            || (metrics.left_forward - metrics.right_forward).abs()
                > SYNC_BOX_MAX_DEPTH_DELTA_METERS * 2.0
        {
            return None;
        }
        if !(SYNC_BOX_MIN_CHEST_DISTANCE_METERS..=SYNC_BOX_MAX_CHEST_DISTANCE_METERS)
            .contains(&metrics.average_forward_distance)
        {
            return None;
        }
        Some(BoxSyncPoseSample {
            anchor: XrAnchor {
                left: left_point,
                right: right_point,
            },
            captured_at: state.time,
            // Opposing-hand wiggles keep the midpoint almost fixed, so track the
            // vertical split between hands instead of the average hand height.
            vertical_split: left_point.y - right_point.y,
        })
    }

    fn update_detector_sample(
        &mut self,
        sample: Option<BoxSyncPoseSample>,
    ) -> Option<XrSyncAnchor> {
        let Some(sample) = sample else {
            self.detector.previous_sample = None;
            self.detector.direction = None;
            self.detector.extreme_sample = None;
            return None;
        };
        let Some(previous_sample) = self.detector.previous_sample else {
            self.detector.previous_sample = Some(sample);
            self.detector.extreme_sample = Some(sample);
            return None;
        };

        match self.detector.direction {
            None => {
                let phase_delta = sample.vertical_split - previous_sample.vertical_split;
                if phase_delta.abs() < SYNC_BOX_DIRECTION_DEADZONE_METERS {
                    return None;
                }
                let next_direction = if phase_delta > 0.0 {
                    BoxSyncMotionDirection::Rising
                } else {
                    BoxSyncMotionDirection::Falling
                };
                self.detector.direction = Some(next_direction);
                self.detector.extreme_sample = Some(Self::more_extreme_sample(
                    previous_sample,
                    sample,
                    next_direction,
                ));
                self.detector.previous_sample = Some(sample);
                None
            }
            Some(current_direction) => {
                let extreme_sample = self.detector.extreme_sample.unwrap_or(previous_sample);
                let continues_current_direction = match current_direction {
                    BoxSyncMotionDirection::Rising => {
                        sample.vertical_split >= extreme_sample.vertical_split
                    }
                    BoxSyncMotionDirection::Falling => {
                        sample.vertical_split <= extreme_sample.vertical_split
                    }
                };

                if continues_current_direction {
                    self.detector.extreme_sample = Some(Self::more_extreme_sample(
                        extreme_sample,
                        sample,
                        current_direction,
                    ));
                    self.detector.previous_sample = Some(sample);
                    return None;
                }

                let retreat = (sample.vertical_split - extreme_sample.vertical_split).abs();
                if retreat < SYNC_BOX_REVERSAL_MIN_TRAVEL_METERS {
                    self.detector.previous_sample = Some(sample);
                    return None;
                }

                let emitted = Some(XrSyncAnchor {
                    id: self.next_sync_anchor_id,
                    captured_at: extreme_sample.captured_at,
                    extrema: match current_direction {
                        BoxSyncMotionDirection::Rising => XrSyncAnchorExtrema::High,
                        BoxSyncMotionDirection::Falling => XrSyncAnchorExtrema::Low,
                    },
                    anchor: extreme_sample.anchor,
                });
                self.next_sync_anchor_id = self.next_sync_anchor_id.wrapping_add(1);
                self.detector.direction = Some(match current_direction {
                    BoxSyncMotionDirection::Rising => BoxSyncMotionDirection::Falling,
                    BoxSyncMotionDirection::Falling => BoxSyncMotionDirection::Rising,
                });
                self.detector.extreme_sample = Some(sample);
                self.detector.previous_sample = Some(sample);
                emitted
            }
        }
    }

    fn more_extreme_sample(
        a: BoxSyncPoseSample,
        b: BoxSyncPoseSample,
        direction: BoxSyncMotionDirection,
    ) -> BoxSyncPoseSample {
        match direction {
            BoxSyncMotionDirection::Rising => {
                if b.vertical_split >= a.vertical_split {
                    b
                } else {
                    a
                }
            }
            BoxSyncMotionDirection::Falling => {
                if b.vertical_split <= a.vertical_split {
                    b
                } else {
                    a
                }
            }
        }
    }

    fn update_detector(&mut self, state: &XrState) -> Option<XrSyncAnchor> {
        self.update_detector_sample(Self::box_sync_pose_sample(state))
    }

    fn current_sync_anchor(&mut self, state_time: f64) -> Option<XrSyncAnchor> {
        let (Some(sync_anchor), Some(emitted_at)) = (
            self.pending_sync_anchor,
            self.pending_sync_anchor_emitted_at,
        ) else {
            return None;
        };
        if state_time - emitted_at <= SYNC_BOX_BROADCAST_SECONDS {
            Some(sync_anchor)
        } else {
            self.pending_sync_anchor = None;
            self.pending_sync_anchor_emitted_at = None;
            None
        }
    }

    fn augment_state(&mut self, state: &XrState) -> Rc<XrState> {
        if let Some(sync_anchor) = self.update_detector(state) {
            self.pending_sync_anchor = Some(sync_anchor);
            self.pending_sync_anchor_emitted_at = Some(state.time);
        }
        let mut augmented = state.clone();
        augmented.sync_anchor = self.current_sync_anchor(state.time);
        Rc::new(augmented)
    }
}

#[derive(Script, WidgetRef, WidgetSet, WidgetRegister)]
pub struct XrRoot {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,

    // Window + Pass
    #[live]
    window: ScriptWindowHandle,
    #[live]
    pass: ScriptDrawPass,
    #[new]
    depth_texture: Texture,
    #[new]
    draw_list: DrawList,
    #[new]
    permissions_draw_list: DrawList2d,

    // Environment (physics + env draws)
    #[live]
    env: XrEnv,

    // Camera
    #[live]
    camera: XrCamera,

    // Startup callback
    #[live]
    on_startup: ScriptFnRef,

    // Children (from := declarations)
    #[rust]
    children: Vec<(LiveId, WidgetRef)>,
    #[rust]
    permissions_widget: WidgetRef,

    // State
    #[rust]
    runtime: XrRootRuntime,
    #[rust]
    frame_metrics: XrRootFrameMetrics,
    #[rust]
    content_rig: XrContentRig,
    #[rust]
    sync_runtime: XrSyncAnchorRuntime,
    #[rust]
    floor_runtime: XrFloorSetRuntime,
}

impl XrRoot {
    fn write_debug_live_id_label(out: &mut String, id: LiveId) {
        if id == LiveId(0) {
            out.push('-');
            return;
        }
        id.as_string(|name| {
            if let Some(name) = name {
                out.push_str(name);
            } else {
                let _ = write!(out, "{:x}", id.0);
            }
        });
    }

    fn reset_scene_physics_and_emit_action(&mut self, cx: &mut Cx) {
        self.env.reset_physics(cx);
        cx.widget_action(self.widget_uid(), XrRootAction::PhysicsReset);
    }

    fn apply_content_pose(&mut self, cx: &mut Cx, pose: Pose) {
        self.content_rig.pose = Some(pose);
        self.env.set_root_pose(cx, Some(pose));
        self.env.reset_physics(cx);
        cx.redraw_all();
    }

    fn reset_scene_pose_to_headset_and_emit_action(&mut self, cx: &mut Cx) {
        let Some(state) = self.runtime.last_xr_state.as_deref() else {
            return;
        };
        let pose = XrContentRig::reset_pose_from_state(state);
        self.apply_content_pose(cx, pose);
        cx.widget_action(self.widget_uid(), XrRootAction::ContentPoseReset(pose));
    }

    pub fn spawn_body(&mut self, cx: &mut Cx, spawn: XrBodySpawn) {
        self.env.spawn_body(cx, spawn);
    }

    pub fn despawn_body(&mut self, cx: &mut Cx, widget_uid: WidgetUid) {
        self.env.despawn_body(cx, widget_uid);
    }

    pub fn apply_body_impulse(&mut self, cx: &mut Cx, impulse: XrBodyImpulse) {
        self.env.apply_body_impulse(cx, impulse);
    }

    pub fn apply_body_wrench(&mut self, cx: &mut Cx, wrench: XrBodyWrench) {
        self.env.apply_body_wrench(cx, wrench);
    }

    pub fn apply_body_drive(&mut self, cx: &mut Cx, drive: XrBodyDrive) {
        self.env.apply_body_drive(cx, drive);
    }

    pub fn apply_car_control(&mut self, cx: &mut Cx, control: XrCarControl) {
        self.env.apply_car_control(cx, control);
    }

    pub fn set_content_pose(&mut self, cx: &mut Cx, pose: Pose) {
        self.apply_content_pose(cx, pose);
    }

    pub fn content_pose(&self) -> Option<Pose> {
        self.content_rig
            .resolved_pose(self.runtime.last_xr_state.as_deref())
    }

    pub fn force_scene_rebuild(&mut self, cx: &mut Cx) {
        self.env.mark_scene_dirty();
        self.env.ensure_physics(cx, &self.children);
        cx.redraw_all();
    }

    fn set_depth_mesh_visible(&mut self, cx: &mut Cx, visible: bool) -> bool {
        self.env.set_depth_mesh_visible(visible);
        self.env.mark_scene_dirty();
        self.env.ensure_physics(cx, &self.children);
        cx.redraw_all();
        visible
    }

    fn set_depth_query_hits_visible(&mut self, cx: &mut Cx, visible: bool) -> bool {
        self.env.set_depth_query_hits_visible(visible);
        cx.redraw_all();
        visible
    }

    fn set_depth_mesh_focus_cube_enabled(&mut self, cx: &mut Cx, enabled: bool) -> bool {
        self.env.set_depth_mesh_focus_cube_enabled(enabled);
        cx.redraw_all();
        enabled
    }

    fn set_depth_voxel_size(&mut self, cx: &mut Cx, voxel_size_meters: f32) -> f32 {
        let _ = voxel_size_meters;
        let voxel_size_meters = cx
            .xr_tsdf()
            .set_voxel_size_meters(XR_FIXED_DEPTH_VOXEL_SIZE_METERS);
        self.env.reset_physics(cx);
        voxel_size_meters
    }

    fn permissions_ui_visible(&self) -> bool {
        self.permissions_widget
            .borrow::<XrPermissionsFlow>()
            .is_some_and(|widget| widget.desktop_preflight_visible())
    }

    fn ensure_initialized(&mut self, cx: &mut Cx) {
        if self.runtime.initialized {
            return;
        }
        self.runtime.initialized = true;
        self.window.handle.set_pass(cx, &self.pass.handle);
        self.pass.handle.set_pass_name(cx, "xr_root_window");
        self.depth_texture = Texture::new_with_format(
            cx,
            TextureFormat::DepthD32 {
                size: TextureSize::Auto,
                initial: true,
            },
        );
        self.pass.handle.set_depth_texture(
            cx,
            &self.depth_texture,
            DrawPassClearDepth::ClearWith(1.0),
        );
    }

    fn set_pass_camera(&self, cx: &mut Cx, scene: &SceneState3D) {
        let camera_inv = scene.view.invert();
        let pass_uniforms = &mut cx.passes[self.pass.handle.draw_pass_id()].pass_uniforms;
        pass_uniforms.camera_projection = scene.projection;
        pass_uniforms.camera_projection_r = scene.projection;
        pass_uniforms.camera_view = scene.view;
        pass_uniforms.camera_view_r = scene.view;
        pass_uniforms.depth_projection = scene.projection;
        pass_uniforms.depth_projection_r = scene.projection;
        pass_uniforms.depth_view = scene.view;
        pass_uniforms.depth_view_r = scene.view;
        pass_uniforms.camera_inv = camera_inv;
        pass_uniforms.camera_inv_r = camera_inv;
    }

    fn desktop_scene_state(&self, time: f64) -> Option<SceneState3D> {
        let viewport_rect = self.camera.viewport_rect?;
        self.camera.desktop_scene_state(viewport_rect, time)
    }

    fn desktop_pick_ray(&self, abs: DVec2, time: f64) -> Option<(Vec3f, Vec3f)> {
        let scene = self.desktop_scene_state(time)?;
        ray_from_scene_viewport(&scene, abs)
    }

    fn desktop_xr_update_event(time: f64) -> XrUpdateEvent {
        let state = Rc::new(XrState {
            time,
            ..Default::default()
        });
        XrUpdateEvent {
            state: state.clone(),
            last: state,
        }
    }

    fn desktop_mouse_tip(ray_origin: Vec3f, ray_dir: Vec3f, touch_z: f32) -> XrFingerTip {
        XrFingerTip {
            index: XrHand::INDEX_TIP,
            is_left: false,
            active: true,
            interactive: true,
            pos: ray_origin,
            ray_dir,
            touch_z,
            handled: Cell::new(Area::Empty),
        }
    }

    fn dispatch_desktop_xr_local(
        &mut self,
        cx: &mut Cx,
        scope: &mut Scope,
        time: f64,
        modifiers: KeyModifiers,
        tip: Option<XrFingerTip>,
    ) {
        let mut finger_tips = SmallVec::new();
        if let Some(tip) = tip {
            finger_tips.push(tip);
        }
        let xr_event = Event::XrLocal(XrLocalEvent {
            finger_tips,
            space_transform: Mat4f::identity(),
            digit_namespace: 0,
            update: Self::desktop_xr_update_event(time),
            modifiers,
            time,
        });
        for i in 0..self.children.len() {
            let child = self.children[i].1.clone();
            if child.borrow::<XrView>().is_some() {
                child.handle_event(cx, &xr_event, scope);
            }
        }
    }

    fn desktop_ui_hit(&self, ray_origin: Vec3f, ray_dir: Vec3f) -> bool {
        for i in 0..self.children.len() {
            let child = self.children[i].1.clone();
            let child_hit = if let Some(view) = child.borrow::<XrView>() {
                view.hits_parent_ray(ray_origin, ray_dir)
            } else {
                false
            };
            if child_hit {
                return true;
            }
        }
        false
    }

    fn ensure_xr_content_pose(&mut self, cx: &mut Cx, state: &XrState) {
        self.content_rig.ensure_pose(cx, &mut self.env, state);
    }

    fn clear_xr_content_pose(&mut self, cx: &mut Cx) {
        self.content_rig.clear_pose(cx, &mut self.env);
    }

    fn xr_content_transform(&self, state: Option<&XrState>) -> Mat4f {
        self.content_rig.transform(state)
    }

    fn transform_point(transform: &Mat4f, point: Vec3f) -> Vec3f {
        let point = transform.transform_vec4(vec4f(point.x, point.y, point.z, 1.0));
        if point.w.abs() > 1.0e-6 {
            vec3f(point.x / point.w, point.y / point.w, point.z / point.w)
        } else {
            point.to_vec3f()
        }
    }

    fn augment_xr_state(&mut self, state: &XrState) -> Rc<XrState> {
        self.sync_runtime.augment_state(state)
    }

    fn handle_desktop_xr_pointer(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) -> bool {
        match event {
            Event::MouseDown(fe) if fe.button.is_primary() => {
                let Some((ray_origin, ray_dir)) = self.desktop_pick_ray(fe.abs, fe.time) else {
                    return false;
                };
                if !self.desktop_ui_hit(ray_origin, ray_dir) {
                    return false;
                }
                self.runtime.desktop_ui_pointer_active = true;
                self.dispatch_desktop_xr_local(
                    cx,
                    scope,
                    fe.time,
                    fe.modifiers,
                    Some(Self::desktop_mouse_tip(
                        ray_origin,
                        ray_dir,
                        DESKTOP_TOUCH_DOWN_Z,
                    )),
                );
                true
            }
            Event::MouseMove(fe) if self.runtime.desktop_ui_pointer_active => {
                let tip = self
                    .desktop_pick_ray(fe.abs, fe.time)
                    .map(|(ray_origin, ray_dir)| {
                        Self::desktop_mouse_tip(ray_origin, ray_dir, DESKTOP_TOUCH_DOWN_Z)
                    });
                self.dispatch_desktop_xr_local(cx, scope, fe.time, fe.modifiers, tip);
                true
            }
            Event::MouseUp(fe)
                if fe.button.is_primary() && self.runtime.desktop_ui_pointer_active =>
            {
                let tip = self
                    .desktop_pick_ray(fe.abs, fe.time)
                    .map(|(ray_origin, ray_dir)| {
                        Self::desktop_mouse_tip(ray_origin, ray_dir, DESKTOP_TOUCH_UP_Z)
                    });
                self.runtime.desktop_ui_pointer_active = false;
                self.dispatch_desktop_xr_local(cx, scope, fe.time, fe.modifiers, tip);
                true
            }
            Event::MouseLeave(fe) if self.runtime.desktop_ui_pointer_active => {
                self.runtime.desktop_ui_pointer_active = false;
                self.dispatch_desktop_xr_local(cx, scope, fe.time, fe.modifiers, None);
                true
            }
            _ => false,
        }
    }

    fn draw_3d_content(&mut self, cx: &mut Cx3d, _scope: &mut Scope, scene_state: SceneState3D) {
        let setup_started = Instant::now();
        self.draw_list.begin_always(cx);
        let root_transform = if cx.cx.in_xr_mode() {
            self.xr_content_transform(self.runtime.last_xr_state.as_deref())
        } else {
            Mat4f::identity()
        };

        cx.begin_scene_3d(scene_state);
        let previous_world = cx.set_scene_world_transform_3d(root_transform);
        self.frame_metrics.last_draw_setup_cpu_ms = setup_started.elapsed().as_secs_f64() * 1000.0;

        let env_prepare_started = Instant::now();
        let mut draw_scope = {
            let cx2d = &mut Cx2d::new(cx.cx);
            self.env.prepare_and_draw(cx2d)
        };
        self.frame_metrics.last_draw_env_prepare_cpu_ms =
            env_prepare_started.elapsed().as_secs_f64() * 1000.0;
        draw_scope.tracking_from_content = root_transform;
        draw_scope.content_from_tracking = root_transform.invert();
        self.frame_metrics.last_draw_runtime_body_count = draw_scope.runtime_bodies.len();
        self.frame_metrics.last_draw_geometry_pool_slots = cx.cx.geometry_pool_slot_count();
        self.frame_metrics.last_draw_geometry_pool_live = cx.cx.geometry_pool_live_count();
        self.frame_metrics.last_draw_draw_list_pool_slots = cx.cx.draw_list_pool_slot_count();
        self.frame_metrics.last_draw_draw_list_pool_live = cx.cx.draw_list_pool_live_count();
        self.frame_metrics.last_draw_texture_pool_slots = cx.cx.texture_pool_slot_count();
        self.frame_metrics.last_draw_texture_pool_live = cx.cx.texture_pool_live_count();
        self.frame_metrics.last_draw_depth_mesh_chunk_count = self.env.depth_mesh_chunk_count();
        self.frame_metrics
            .last_draw_recycled_depth_mesh_geometry_count =
            self.env.recycled_depth_mesh_geometry_count();
        self.frame_metrics.last_draw_depth_mesh_pending_upsert_count =
            self.env.depth_mesh_pending_upsert_count();
        self.frame_metrics.last_draw_depth_query_retained_hit_count =
            self.env.depth_query_retained_hit_count();

        let mut scene_scope = Scope::with_data(&mut draw_scope);
        let sort_started = Instant::now();
        let mut draw_order_entries = Vec::new();
        for i in 0..self.children.len() {
            let child = self.children[i].1.clone();
            let child_center = xr_widget_local_sort_center(&child)
                .map(|center| Self::transform_point(&root_transform, center));
            if let Some(child_center) = child_center {
                draw_order_entries.push((
                    i,
                    xr_draw_list_depth(&scene_state, child_center),
                    xr_widget_is_transparent(&child),
                ));
            } else {
                draw_order_entries.push((i, 0.0, false));
            }
        }

        xr_sort_child_draw_order(&mut draw_order_entries);
        self.frame_metrics.last_draw_sort_cpu_ms = sort_started.elapsed().as_secs_f64() * 1000.0;
        self.frame_metrics.last_draw_child_count = draw_order_entries.len();
        self.frame_metrics.last_draw_transparent_child_count = draw_order_entries
            .iter()
            .filter(|(_, _, transparent)| *transparent)
            .count();
        let children_draw_started = Instant::now();
        let mut child_timings = Vec::with_capacity(draw_order_entries.len());
        for (index, _, _) in draw_order_entries {
            let child_id = self.children[index].0;
            let child = self.children[index].1.clone();
            let child_started = Instant::now();
            child.draw_3d_all(cx, &mut scene_scope);
            child_timings.push((child_id, child_started.elapsed().as_secs_f64() * 1000.0));
        }
        self.frame_metrics.last_draw_children_cpu_ms =
            children_draw_started.elapsed().as_secs_f64() * 1000.0;
        child_timings.sort_by(|left, right| {
            right
                .1
                .partial_cmp(&left.1)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        self.frame_metrics.last_draw_top_child_count = 0;
        for (slot_index, (id, cpu_ms)) in child_timings
            .into_iter()
            .take(XR_ROOT_TOP_CHILD_DRAW_METRIC_COUNT)
            .enumerate()
        {
            self.frame_metrics.last_draw_top_children[slot_index] = (id, cpu_ms);
            self.frame_metrics.last_draw_top_child_count = slot_index + 1;
        }
        for slot_index in
            self.frame_metrics.last_draw_top_child_count..XR_ROOT_TOP_CHILD_DRAW_METRIC_COUNT
        {
            self.frame_metrics.last_draw_top_children[slot_index] = (LiveId(0), 0.0);
        }
        if let Some(previous_world) = previous_world {
            let _ = cx.set_scene_world_transform_3d(previous_world);
        }
        cx.end_scene_3d();

        self.draw_list.end(cx);
    }

    fn handle_draw_event(&mut self, cx: &mut Cx, e: &DrawEvent, scope: &mut Scope) {
        let started = Instant::now();
        self.ensure_initialized(cx);
        cx.passes[self.pass.handle.draw_pass_id()].keep_camera_matrix =
            if cx.in_xr_mode() || !self.permissions_ui_visible() {
                self.pass.keep_camera_matrix
            } else {
                false
            };
        self.pass.handle.set_window_clear_color(
            cx,
            if cx.in_xr_mode() {
                vec4(0.0, 0.0, 0.0, 0.0)
            } else {
                self.pass.clear_color
            },
        );
        if cx.in_xr_mode() {
            if self.runtime.last_xr_state.is_none() {
                if let Some(xr_state) = e.xr_state.as_ref() {
                    self.runtime.last_xr_state = Some(xr_state.clone());
                }
            }
            let Some(xr_state) = self
                .runtime
                .last_xr_state
                .as_ref()
                .or_else(|| e.xr_state.as_ref())
            else {
                return;
            };
            let mut cx_draw = CxDraw::new(cx, e);
            let cx3d = &mut Cx3d::new(&mut cx_draw);
            self.pass.handle.set_as_xr_pass(cx3d);
            cx3d.begin_pass(&self.pass.handle, Some(4.0));
            self.draw_3d_content(cx3d, scope, self.camera.xr_scene_state(xr_state));
            cx3d.end_pass(&self.pass.handle);
        } else {
            let mut cx_draw = CxDraw::new(cx, e);
            let cx2d = &mut Cx2d::new(&mut cx_draw);
            self.draw_all(cx2d, scope);
        }
        self.frame_metrics.finish_draw(started);
    }

    pub fn depth_mesh_visible(&self) -> bool {
        self.env.depth_mesh_visible()
    }

    pub fn depth_query_hits_visible(&self) -> bool {
        self.env.depth_query_hits_visible()
    }

    pub fn depth_mesh_focus_cube_enabled(&self) -> bool {
        self.env.depth_mesh_focus_cube_enabled()
    }

    pub fn toggle_depth_mesh_visible(&mut self, cx: &mut Cx) -> bool {
        let visible = self.env.toggle_depth_mesh_visible();
        cx.redraw_all();
        visible
    }

    pub fn toggle_depth_mesh_focus_cube(&mut self, cx: &mut Cx) -> bool {
        let enabled = self.env.toggle_depth_mesh_focus_cube();
        cx.redraw_all();
        enabled
    }

    pub fn toggle_depth_query_hits_visible(&mut self, cx: &mut Cx) -> bool {
        let visible = self.env.toggle_depth_query_hits_visible();
        cx.redraw_all();
        visible
    }

    pub fn set_depth_mesh_focus_point(&mut self, point: Option<Vec3f>) {
        self.env.set_depth_mesh_focus_point(point);
    }

    pub fn physics_compute_ms(&self) -> f64 {
        self.env.physics_compute_ms()
    }

    pub fn physics_tsdf_query_ms(&self) -> f64 {
        self.env.physics_tsdf_query_ms()
    }

    pub fn physics_rapier_step_ms(&self) -> f64 {
        self.env.physics_rapier_step_ms()
    }

    pub fn physics_time_scale(&self) -> f32 {
        self.env.physics_time_scale()
    }

    pub fn physics_depth_query_surface_count(&self) -> usize {
        self.env.physics_depth_query_surface_count()
    }

    pub fn physics_scene_body_count(&self) -> usize {
        self.env.physics_scene_body_count()
    }

    pub fn physics_body_spawn_apply_count(&self) -> usize {
        self.env.physics_body_spawn_apply_count()
    }

    pub fn physics_body_spawn_miss_count(&self) -> usize {
        self.env.physics_body_spawn_miss_count()
    }

    pub fn physics_revision(&self) -> u64 {
        self.env.physics_revision()
    }

    pub fn runtime_bodies(&self) -> Rc<HashMap<WidgetUid, XrRuntimeBodyState>> {
        self.env.runtime_bodies()
    }

    pub fn runtime_contacts(&self) -> Rc<Vec<(WidgetUid, WidgetUid)>> {
        self.env.runtime_contacts()
    }

    pub fn frame_cpu_ms(&self) -> f64 {
        self.frame_metrics.last_frame_cpu_ms
    }

    pub fn frame_update_cpu_ms(&self) -> f64 {
        self.frame_metrics.last_frame_update_cpu_ms
    }

    pub fn frame_draw_cpu_ms(&self) -> f64 {
        self.frame_metrics.last_frame_draw_cpu_ms
    }

    pub fn draw_setup_cpu_ms(&self) -> f64 {
        self.frame_metrics.last_draw_setup_cpu_ms
    }

    pub fn draw_env_prepare_cpu_ms(&self) -> f64 {
        self.frame_metrics.last_draw_env_prepare_cpu_ms
    }

    pub fn draw_sort_cpu_ms(&self) -> f64 {
        self.frame_metrics.last_draw_sort_cpu_ms
    }

    pub fn draw_children_cpu_ms(&self) -> f64 {
        self.frame_metrics.last_draw_children_cpu_ms
    }

    pub fn draw_child_count(&self) -> usize {
        self.frame_metrics.last_draw_child_count
    }

    pub fn draw_transparent_child_count(&self) -> usize {
        self.frame_metrics.last_draw_transparent_child_count
    }

    pub fn draw_runtime_body_count(&self) -> usize {
        self.frame_metrics.last_draw_runtime_body_count
    }

    pub fn draw_geometry_pool_slots(&self) -> usize {
        self.frame_metrics.last_draw_geometry_pool_slots
    }

    pub fn draw_geometry_pool_live(&self) -> usize {
        self.frame_metrics.last_draw_geometry_pool_live
    }

    pub fn draw_draw_list_pool_slots(&self) -> usize {
        self.frame_metrics.last_draw_draw_list_pool_slots
    }

    pub fn draw_draw_list_pool_live(&self) -> usize {
        self.frame_metrics.last_draw_draw_list_pool_live
    }

    pub fn draw_texture_pool_slots(&self) -> usize {
        self.frame_metrics.last_draw_texture_pool_slots
    }

    pub fn draw_texture_pool_live(&self) -> usize {
        self.frame_metrics.last_draw_texture_pool_live
    }

    pub fn draw_depth_mesh_chunk_count(&self) -> usize {
        self.frame_metrics.last_draw_depth_mesh_chunk_count
    }

    pub fn draw_recycled_depth_mesh_geometry_count(&self) -> usize {
        self.frame_metrics
            .last_draw_recycled_depth_mesh_geometry_count
    }

    pub fn draw_depth_mesh_pending_upsert_count(&self) -> usize {
        self.frame_metrics.last_draw_depth_mesh_pending_upsert_count
    }

    pub fn draw_depth_query_retained_hit_count(&self) -> usize {
        self.frame_metrics.last_draw_depth_query_retained_hit_count
    }

    pub fn write_draw_top_children_text(&self, out: &mut String) {
        out.clear();
        for (index, (id, cpu_ms)) in self
            .frame_metrics
            .last_draw_top_children
            .iter()
            .take(self.frame_metrics.last_draw_top_child_count)
            .enumerate()
        {
            if index != 0 {
                out.push_str(" > ");
            }
            Self::write_debug_live_id_label(out, *id);
            let _ = write!(out, " {:.2}", cpu_ms);
        }
    }
}

impl ScriptHook for XrRoot {
    fn on_before_apply(
        &mut self,
        _vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if apply.is_reload() {
            self.children.clear();
            self.permissions_widget = WidgetRef::empty();
        }
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        if !apply.is_eval() {
            if let Some(obj) = value.as_object() {
                self.children.clear();
                self.permissions_widget = WidgetRef::empty();
                vm.vec_with(obj, |vm, vec| {
                    for kv in vec {
                        let Some(id) = kv.key.as_id() else { continue };
                        if !WidgetRef::value_is_newable_widget(vm, kv.value) {
                            continue;
                        }
                        let child = WidgetRef::script_from_value_scoped(vm, scope, kv.value);
                        if id == live_id!(xr_permissions)
                            || child.borrow::<XrPermissionsFlow>().is_some()
                        {
                            self.permissions_widget = child.clone();
                        }
                        self.children.push((id, child.clone()));
                        vm.cx_mut()
                            .widget_tree_insert_child_deep(self.uid, id, child);
                    }
                });
            }
        }
        vm.cx_mut().widget_tree_mark_dirty(self.uid);
    }
}

impl WidgetNode for XrRoot {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }
    fn area(&self) -> Area {
        Area::Empty
    }

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        for (id, child) in &self.children {
            visit(*id, child.clone());
        }
    }

    fn redraw(&mut self, cx: &mut Cx) {
        cx.redraw_all();
    }
}

impl Widget for XrRoot {
    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(set_depth) || method == live_id!(set_depth_mesh) {
            let mut visible = self.depth_mesh_visible();
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                visible = vm
                    .bx
                    .heap
                    .cast_to_bool(vm.bx.heap.vec_value(args_obj, 0, trap));
            }
            vm.with_cx_mut(|cx| {
                visible = self.set_depth_mesh_visible(cx, visible);
            });
            return ScriptAsyncResult::Return(ScriptValue::from_bool(visible));
        }
        if method == live_id!(depth_toggle) || method == live_id!(toggle_depth_mesh) {
            let mut visible = self.depth_mesh_visible();
            vm.with_cx_mut(|cx| {
                visible = self.toggle_depth_mesh_visible(cx);
            });
            return ScriptAsyncResult::Return(ScriptValue::from_bool(visible));
        }
        if method == live_id!(depth_mesh_visible) {
            return ScriptAsyncResult::Return(ScriptValue::from_bool(self.depth_mesh_visible()));
        }
        if method == live_id!(set_depth_mesh_focus_cube) {
            let mut enabled = self.depth_mesh_focus_cube_enabled();
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                enabled = vm
                    .bx
                    .heap
                    .cast_to_bool(vm.bx.heap.vec_value(args_obj, 0, trap));
            }
            vm.with_cx_mut(|cx| {
                enabled = self.set_depth_mesh_focus_cube_enabled(cx, enabled);
            });
            return ScriptAsyncResult::Return(ScriptValue::from_bool(enabled));
        }
        if method == live_id!(toggle_depth_mesh_focus_cube) {
            let mut enabled = self.depth_mesh_focus_cube_enabled();
            vm.with_cx_mut(|cx| {
                enabled = self.toggle_depth_mesh_focus_cube(cx);
            });
            return ScriptAsyncResult::Return(ScriptValue::from_bool(enabled));
        }
        if method == live_id!(depth_mesh_focus_cube_enabled) {
            return ScriptAsyncResult::Return(ScriptValue::from_bool(
                self.depth_mesh_focus_cube_enabled(),
            ));
        }
        if method == live_id!(set_depth_query_hits) {
            let mut visible = self.depth_query_hits_visible();
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                visible = vm
                    .bx
                    .heap
                    .cast_to_bool(vm.bx.heap.vec_value(args_obj, 0, trap));
            }
            vm.with_cx_mut(|cx| {
                visible = self.set_depth_query_hits_visible(cx, visible);
            });
            return ScriptAsyncResult::Return(ScriptValue::from_bool(visible));
        }
        if method == live_id!(toggle_depth_query_hits) {
            let mut visible = self.depth_query_hits_visible();
            vm.with_cx_mut(|cx| {
                visible = self.toggle_depth_query_hits_visible(cx);
            });
            return ScriptAsyncResult::Return(ScriptValue::from_bool(visible));
        }
        if method == live_id!(depth_query_hits_visible) {
            return ScriptAsyncResult::Return(ScriptValue::from_bool(
                self.depth_query_hits_visible(),
            ));
        }
        if method == live_id!(set_depth_voxel_size) || method == live_id!(set_depth_resolution) {
            let mut voxel_size_meters = vm.cx().xr_tsdf().voxel_size_meters();
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                if let Some(value) = vm.bx.heap.vec_value(args_obj, 0, trap).as_f64() {
                    voxel_size_meters = value as f32;
                }
            }
            vm.with_cx_mut(|cx| {
                voxel_size_meters = self.set_depth_voxel_size(cx, voxel_size_meters);
            });
            return ScriptAsyncResult::Return(ScriptValue::from_f64(voxel_size_meters as f64));
        }
        if method == live_id!(depth_voxel_size) || method == live_id!(depth_resolution) {
            return ScriptAsyncResult::Return(ScriptValue::from_f64(
                vm.cx().xr_tsdf().voxel_size_meters() as f64,
            ));
        }
        if method == live_id!(set_render_scale) || method == live_id!(set_xr_render_scale) {
            let mut scale = vm.cx().xr_render_scale().unwrap_or(1.4) as f32;
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                if let Some(value) = vm.bx.heap.vec_value(args_obj, 0, trap).as_f64() {
                    scale = value as f32;
                }
            }
            vm.with_cx_mut(|cx| {
                cx.xr_set_render_scale(scale);
            });
            return ScriptAsyncResult::Return(ScriptValue::from_f64(scale as f64));
        }
        if method == live_id!(render_scale) || method == live_id!(xr_render_scale) {
            return ScriptAsyncResult::Return(
                vm.cx()
                    .xr_render_scale()
                    .map(ScriptValue::from_f64)
                    .unwrap_or(NIL),
            );
        }
        if method == live_id!(reset_physics) || method == live_id!(reset_scene_physics) {
            vm.with_cx_mut(|cx| {
                self.reset_scene_physics_and_emit_action(cx);
            });
            return ScriptAsyncResult::Return(NIL);
        }
        if method == live_id!(reset_activity_pose) || method == live_id!(pose_activity) {
            vm.with_cx_mut(|cx| {
                self.reset_scene_pose_to_headset_and_emit_action(cx);
            });
            return ScriptAsyncResult::Return(NIL);
        }
        if method == live_id!(set_physics_time_scale) || method == live_id!(set_sim_time_scale) {
            let mut scale = self.physics_time_scale();
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                if let Some(value) = vm.bx.heap.vec_value(args_obj, 0, trap).as_f64() {
                    scale = value as f32;
                }
            }
            vm.with_cx_mut(|cx| {
                scale = self.env.set_physics_time_scale(cx, scale);
            });
            return ScriptAsyncResult::Return(ScriptValue::from_f64(scale as f64));
        }
        if method == live_id!(physics_time_scale) || method == live_id!(sim_time_scale) {
            return ScriptAsyncResult::Return(ScriptValue::from_f64(
                self.physics_time_scale() as f64
            ));
        }
        if method == live_id!(render_scene) {
            self.env.mark_scene_dirty();
            for i in 0..self.children.len() {
                let child = self.children[i].1.clone();
                let _ = child.script_call(vm, live_id!(render), NIL);
            }
            vm.with_cx_mut(|cx| {
                self.env.ensure_physics(cx, &self.children);
            });
            return ScriptAsyncResult::Return(NIL);
        }
        let _ = args;
        ScriptAsyncResult::MethodNotFound
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::Draw(e) = event {
            if !cx.in_xr_mode() {
                for i in 0..self.children.len() {
                    let child = self.children[i].1.clone();
                    if child.borrow::<super::xr_peer_sync::XrPeerSync>().is_some() {
                        child.handle_event(cx, event, scope);
                    }
                }
            }
            self.handle_draw_event(cx, e, scope);
            return;
        }

        let measure_frame_cpu = matches!(event, Event::XrUpdate(_))
            || self.runtime.next_frame.is_event(event).is_some();
        let started = measure_frame_cpu.then(Instant::now);

        if !cx.in_xr_mode() {
            self.clear_xr_content_pose(cx);
        }

        self.env.handle_event(cx, event);

        match event {
            Event::Startup => {
                if !self.runtime.started {
                    self.runtime.started = true;
                    cx.widget_to_script_call(
                        self.uid,
                        NIL,
                        self.source.clone(),
                        self.on_startup.clone(),
                        &[],
                    );
                    cx.with_vm(|vm| {
                        for i in 0..self.children.len() {
                            let child = self.children[i].1.clone();
                            let _ = child.script_call(vm, live_id!(render), NIL);
                        }
                    });
                    self.env.ensure_physics(cx, &self.children);
                    self.runtime.next_frame = cx.new_next_frame();
                    cx.redraw_all();
                }
            }
            Event::XrUpdate(update) => {
                let mut augmented_state = self.augment_xr_state(update.state.as_ref());
                if let Some(commit) = self.floor_runtime.update(augmented_state.as_ref()) {
                    cx.xr_set_local_floor(commit.floor_y);
                    if let Some(anchor) = commit.anchor {
                        cx.xr_set_local_anchor(anchor);
                    }
                    self.env.show_debug_floor_preview(
                        cx,
                        commit.floor_y,
                        commit.preview_center,
                        commit.visible_until_time,
                    );
                    let state = Rc::make_mut(&mut augmented_state);
                    state.floor_y = Some(commit.floor_y);
                    if let Some(anchor) = commit.anchor {
                        state.anchor = Some(anchor);
                        state.anchor_persisted = true;
                    }
                }
                let last = self
                    .runtime
                    .last_dispatched_xr_state
                    .clone()
                    .unwrap_or_else(|| augmented_state.clone());
                let augmented_update = XrUpdateEvent {
                    state: augmented_state.clone(),
                    last,
                };
                self.runtime.last_dispatched_xr_state = Some(augmented_state.clone());
                if cx.in_xr_mode() {
                    self.ensure_xr_content_pose(cx, &augmented_state);
                }
                self.runtime.last_xr_state = Some(augmented_state.clone());
                self.env.set_runtime_xr_state(augmented_state.clone());
                if augmented_update.clicked_menu() {
                    self.reset_scene_physics_and_emit_action(cx);
                }
                self.env.ensure_physics(cx, &self.children);
                self.env.step_physics(cx);
                let mut event_scope_data = super::xr_view::XrViewEventScopeData {
                    content_transform: self.xr_content_transform(Some(&augmented_update.state)),
                    runtime_bodies: self.env.runtime_bodies(),
                };
                let mut event_scope = Scope::with_data(&mut event_scope_data);
                let augmented_event = Event::XrUpdate(augmented_update);
                for i in 0..self.children.len() {
                    let child = self.children[i].1.clone();
                    child.handle_event(cx, &augmented_event, &mut event_scope);
                }
            }
            Event::NextFrame(_) if self.runtime.next_frame.is_event(event).is_some() => {
                if !cx.in_xr_mode() {
                    self.env.ensure_physics(cx, &self.children);
                    self.env.step_physics(cx);
                }
                self.runtime.next_frame = cx.new_next_frame();
            }
            _ => {}
        }

        if let Event::Actions(actions) = event {
            for action in actions {
                let Some(widget_action) = action.as_widget_action() else {
                    continue;
                };
                if let Some(impulse) = widget_action.action.downcast_ref::<XrBodyImpulse>() {
                    self.env.apply_body_impulse(cx, *impulse);
                }
            }
            if actions.iter().any(|action| {
                action.as_widget_action().is_some_and(|action| {
                    matches!(action.cast::<XrNodeAction>(), XrNodeAction::SceneChanged)
                        || matches!(
                            action.cast::<XrSelectAction>(),
                            XrSelectAction::ActiveChildChanged(_)
                        )
                })
            }) {
                self.env.mark_scene_dirty();
                self.env.ensure_physics(cx, &self.children);
            }
            self.env.flush_pending_physics_commands(cx);
        }

        let desktop_scene_interaction = !cx.in_xr_mode() && !self.permissions_ui_visible();
        let handled_desktop_xr_pointer = if desktop_scene_interaction {
            self.handle_desktop_xr_pointer(cx, event, scope)
        } else {
            self.runtime.desktop_ui_pointer_active = false;
            false
        };
        let swallow_desktop_pointer_event = desktop_scene_interaction
            && handled_desktop_xr_pointer
            && matches!(
                event,
                Event::MouseDown(_)
                    | Event::MouseMove(_)
                    | Event::MouseUp(_)
                    | Event::MouseLeave(_)
            );

        if swallow_desktop_pointer_event {
        } else if matches!(event, Event::XrUpdate(_)) {
        } else {
            for i in 0..self.children.len() {
                let child = self.children[i].1.clone();
                child.handle_event(cx, event, scope);
            }
        }

        if desktop_scene_interaction && !handled_desktop_xr_pointer {
            self.camera.handle_desktop_interaction(cx, event);
        }

        if let Some(started) = started {
            self.frame_metrics.finish_update(started);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
        if cx.cx.in_xr_mode() {
            return DrawStep::done();
        }

        self.ensure_initialized(cx.cx);
        cx.begin_pass(&self.pass.handle, None);
        let size = cx.current_pass_size();

        if self.permissions_ui_visible() {
            self.permissions_draw_list.begin_always(cx);
            cx.begin_root_turtle(size, Layout::flow_down());
            self.permissions_widget
                .draw_walk_all(cx, scope, Walk::fill());
            cx.end_pass_sized_turtle();
            self.permissions_draw_list.end(cx);
            cx.end_pass(&self.pass.handle);
            return DrawStep::done();
        }

        let pass_rect = Rect {
            pos: dvec2(0.0, 0.0),
            size,
        };
        self.camera.set_desktop_viewport_rect(pass_rect);

        if let Some(scene_state) = self.camera.desktop_scene_state(pass_rect, cx.time()) {
            self.set_pass_camera(cx.cx, &scene_state);
            let cx3d = &mut Cx3d::new(cx.cx);
            self.draw_3d_content(cx3d, scope, scene_state);
        }

        cx.end_pass(&self.pass.handle);
        DrawStep::done()
    }
}

#[cfg(test)]
include!("../tests/scene/xr_root.rs");
