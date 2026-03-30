use super::xr_env::XrEnv;
use super::xr_select::{XrSelect, XrSelectAction};
use super::{hand_is_palm_down_closed_fist, CLOSED_FIST_GESTURE};
use crate::prelude::*;
use crate::util::scene_draw::{ray_from_scene_viewport, SceneState3D};
use makepad_widgets::event::{XrFingerTip, XrSyncAnchor};
use makepad_widgets::makepad_script::ScriptFnRef;
use std::{cell::Cell, cmp::Ordering, rc::Rc, time::Instant};

const DESKTOP_TOUCH_DOWN_Z: f32 = 0.0;
const DESKTOP_TOUCH_UP_Z: f32 = 64.0;
const XR_CONTENT_FORWARD_OFFSET: f32 = 0.28;
const XR_CONTENT_VERTICAL_OFFSET: f32 = -0.58;
const FISTBUMP_FORWARD_PEAK_MIN_METERS: f32 = 0.18;
const FISTBUMP_RETREAT_MIN_METERS: f32 = 0.04;
const FISTBUMP_MAX_WINDOW_SECONDS: f64 = 2.2;
const FISTBUMP_MIN_ACTIVE_SECONDS: f64 = 0.14;
const FISTBUMP_BROADCAST_SECONDS: f64 = 1.1;
const FISTBUMP_COOLDOWN_SECONDS: f64 = 0.85;
const FISTBUMP_MAX_VERTICAL_DELTA_METERS: f32 = 0.22;
const FISTBUMP_MAX_DEPTH_DELTA_METERS: f32 = 0.22;
const FISTBUMP_MIN_HAND_GAP_METERS: f32 = 0.06;
const FISTBUMP_MAX_HAND_GAP_METERS: f32 = 0.78;
const FISTBUMP_MIN_CHEST_DISTANCE_METERS: f32 = 0.10;
const FISTBUMP_MAX_CHEST_DISTANCE_METERS: f32 = 1.05;

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

impl Default for XrCamera {
    fn default() -> Self {
        Self {
            fov_y: 28.0,
            distance: 3.4,
            near: 0.05,
            far: 200.0,
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
        let target = vec3f(0.0, -0.10, -1.30);
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
struct FistbumpPoseSample {
    anchor: XrAnchor,
    forward_distance: f32,
}

#[derive(Clone, Debug, Default)]
struct FistbumpGestureDetector {
    started_at: Option<f64>,
    peak_sample: Option<FistbumpPoseSample>,
    cooldown_until: f64,
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
    fn flat_forward(orientation: Quat) -> Vec3f {
        let mut forward = orientation.rotate_vec3(&vec3f(0.0, 0.0, -1.0));
        forward.y = 0.0;
        if forward.length() <= 1.0e-6 {
            vec3f(0.0, 0.0, -1.0)
        } else {
            forward.normalize()
        }
    }

    fn pose_from_state(state: &XrState) -> Pose {
        let forward = Self::flat_forward(state.head_pose.orientation);
        Pose {
            position: state.head_pose.position
                + forward.scale(XR_CONTENT_FORWARD_OFFSET)
                + vec3f(0.0, XR_CONTENT_VERTICAL_OFFSET, 0.0),
            orientation: Quat::look_rotation(forward.scale(-1.0), vec3f(0.0, 1.0, 0.0)),
        }
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
        self.pose
            .or_else(|| state.map(Self::pose_from_state))
            .map(|pose| pose.to_mat4())
            .unwrap_or_else(Mat4f::identity)
    }
}

#[derive(Clone, Debug, Default)]
struct XrSyncAnchorRuntime {
    detector: FistbumpGestureDetector,
    pending_sync_anchor: Option<XrSyncAnchor>,
    next_sync_anchor_id: u32,
}

impl XrSyncAnchorRuntime {
    fn hand_is_fist(hand: &XrHand, is_left: bool) -> bool {
        hand_is_palm_down_closed_fist(hand, is_left, CLOSED_FIST_GESTURE)
    }

    fn hand_fist_anchor_point(hand: &XrHand, forward: Vec3f, is_left: bool) -> Option<Vec3f> {
        if !Self::hand_is_fist(hand, is_left) {
            return None;
        }
        let mut best_point = None;
        let mut best_projection = f32::NEG_INFINITY;
        for joint_index in [
            XrHand::INDEX_KNUCKLE3,
            XrHand::MIDDLE_KNUCKLE3,
            XrHand::RING_KNUCKLE3,
            XrHand::LITTLE_KNUCKLE3,
        ] {
            let point = hand.joints[joint_index].position;
            let projection = point.dot(forward);
            if projection > best_projection {
                best_projection = projection;
                best_point = Some(point);
            }
        }
        best_point
    }

    fn fistbump_pose_sample(state: &XrState) -> Option<FistbumpPoseSample> {
        let forward = XrContentRig::flat_forward(state.head_pose.orientation);
        let mut right = state
            .head_pose
            .orientation
            .rotate_vec3(&vec3f(1.0, 0.0, 0.0));
        right.y = 0.0;
        right = if right.length() <= 1.0e-6 {
            vec3f(1.0, 0.0, 0.0)
        } else {
            right.normalize()
        };

        let left_point = Self::hand_fist_anchor_point(&state.left_hand, forward, true)?;
        let right_point = Self::hand_fist_anchor_point(&state.right_hand, forward, false)?;
        let left_local = left_point - state.head_pose.position;
        let right_local = right_point - state.head_pose.position;
        let left_forward = left_local.dot(forward);
        let right_forward = right_local.dot(forward);
        let left_lateral = left_local.dot(right);
        let right_lateral = right_local.dot(right);
        let hand_gap = (right_point - left_point).length();
        if left_lateral >= right_lateral
            || (left_point.y - right_point.y).abs() > FISTBUMP_MAX_VERTICAL_DELTA_METERS
            || (left_forward - right_forward).abs() > FISTBUMP_MAX_DEPTH_DELTA_METERS
            || hand_gap < FISTBUMP_MIN_HAND_GAP_METERS
            || hand_gap > FISTBUMP_MAX_HAND_GAP_METERS
        {
            return None;
        }
        let forward_distance = (left_forward + right_forward) * 0.5;
        if !(FISTBUMP_MIN_CHEST_DISTANCE_METERS..=FISTBUMP_MAX_CHEST_DISTANCE_METERS)
            .contains(&forward_distance)
        {
            return None;
        }
        Some(FistbumpPoseSample {
            anchor: XrAnchor {
                left: left_point,
                right: right_point,
            },
            forward_distance,
        })
    }

    fn update_detector(&mut self, state: &XrState) -> Option<XrSyncAnchor> {
        let time = state.time;
        if time < self.detector.cooldown_until {
            return None;
        }

        let Some(sample) = Self::fistbump_pose_sample(state) else {
            self.detector.started_at = None;
            self.detector.peak_sample = None;
            return None;
        };

        let started_at = self.detector.started_at.unwrap_or(time);
        if time - started_at > FISTBUMP_MAX_WINDOW_SECONDS {
            self.detector.started_at = Some(time);
            self.detector.peak_sample = Some(sample);
            return None;
        }

        self.detector.started_at = Some(started_at);
        if self
            .detector
            .peak_sample
            .is_none_or(|peak| sample.forward_distance >= peak.forward_distance)
        {
            self.detector.peak_sample = Some(sample);
        }

        let Some(peak_sample) = self.detector.peak_sample else {
            return None;
        };
        let retreat = peak_sample.forward_distance - sample.forward_distance;
        if time - started_at < FISTBUMP_MIN_ACTIVE_SECONDS
            || peak_sample.forward_distance < FISTBUMP_FORWARD_PEAK_MIN_METERS
            || retreat < FISTBUMP_RETREAT_MIN_METERS
        {
            return None;
        }

        let sync_anchor = XrSyncAnchor {
            id: self.next_sync_anchor_id,
            captured_at: time,
            anchor: peak_sample.anchor,
        };
        self.next_sync_anchor_id = self.next_sync_anchor_id.wrapping_add(1);
        self.detector.started_at = None;
        self.detector.peak_sample = None;
        self.detector.cooldown_until = time + FISTBUMP_COOLDOWN_SECONDS;
        Some(sync_anchor)
    }

    fn current_sync_anchor(&mut self, state_time: f64) -> Option<XrSyncAnchor> {
        let Some(sync_anchor) = self.pending_sync_anchor else {
            return None;
        };
        if state_time - sync_anchor.captured_at <= FISTBUMP_BROADCAST_SECONDS {
            Some(sync_anchor)
        } else {
            self.pending_sync_anchor = None;
            None
        }
    }

    fn augment_state(&mut self, state: &XrState) -> Rc<XrState> {
        if let Some(sync_anchor) = self.update_detector(state) {
            self.pending_sync_anchor = Some(sync_anchor);
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
}

impl XrRoot {
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

    fn set_depth_voxel_size(&mut self, cx: &mut Cx, voxel_size_meters: f32) -> f32 {
        let voxel_size_meters = cx.xr_tsdf().set_voxel_size_meters(voxel_size_meters);
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

    fn child_world_sort_center(child: &WidgetRef) -> Option<Vec3f> {
        if let Some(select) = child.borrow::<XrSelect>() {
            return Some(select.node().pos());
        }
        if let Some(view) = child.borrow::<XrView>() {
            return Some(view.node().pos());
        }
        if let Some(cube) = child.borrow::<Cube>() {
            return Some(cube.node().pos());
        }
        if let Some(ico) = child.borrow::<IcoSphere>() {
            return Some(ico.node().pos());
        }
        if let Some(refractive_cube) = child.borrow::<RefractiveCube>() {
            return Some(refractive_cube.node().pos());
        }
        if let Some(gltf) = child.borrow::<Gltf>() {
            return Some(gltf.node().pos());
        }
        if let Some(tree) = child.borrow::<Tree>() {
            return Some(tree.node().pos());
        }
        if let Some(node) = child.borrow::<XrNode>() {
            return Some(node.pos());
        }
        None
    }

    fn child_is_transparent(child: &WidgetRef) -> bool {
        child.borrow::<RefractiveCube>().is_some() || child.borrow::<XrView>().is_some()
    }

    fn draw_list_depth(scene_state: &SceneState3D, world_pos: Vec3f) -> f32 {
        let view_pos =
            scene_state
                .view
                .transform_vec4(vec4f(world_pos.x, world_pos.y, world_pos.z, 1.0));
        if view_pos.w.abs() > 1.0e-6 {
            view_pos.z / view_pos.w
        } else {
            view_pos.z
        }
    }

    fn sort_child_draw_order(draw_order_entries: &mut [(usize, f32, bool)]) {
        if draw_order_entries.len() <= 1 {
            return;
        }

        draw_order_entries.sort_by(|a, b| match (a.2, b.2) {
            (false, true) => Ordering::Less,
            (true, false) => Ordering::Greater,
            (false, false) => {
                b.1.partial_cmp(&a.1)
                    .unwrap_or(Ordering::Equal)
                    .then_with(|| a.0.cmp(&b.0))
            }
            (true, true) => {
                a.1.partial_cmp(&b.1)
                    .unwrap_or(Ordering::Equal)
                    .then_with(|| a.0.cmp(&b.0))
            }
        });
    }

    fn draw_3d_content(&mut self, cx: &mut Cx3d, _scope: &mut Scope, scene_state: SceneState3D) {
        self.draw_list.begin_always(cx);
        let root_transform = if cx.cx.in_xr_mode() {
            self.xr_content_transform(self.runtime.last_xr_state.as_deref())
        } else {
            Mat4f::identity()
        };

        cx.begin_scene_3d(scene_state);
        let previous_world = cx.set_scene_world_transform_3d(root_transform);

        let mut draw_scope = {
            let cx2d = &mut Cx2d::new(cx.cx);
            self.env.prepare_and_draw(cx2d)
        };
        draw_scope.tracking_from_content = root_transform;
        draw_scope.content_from_tracking = root_transform.invert();

        let mut scene_scope = Scope::with_data(&mut draw_scope);
        let mut draw_order_entries = Vec::new();
        for i in 0..self.children.len() {
            let child = self.children[i].1.clone();
            let child_center = Self::child_world_sort_center(&child)
                .map(|center| Self::transform_point(&root_transform, center));
            if let Some(child_center) = child_center {
                draw_order_entries.push((
                    i,
                    Self::draw_list_depth(&scene_state, child_center),
                    Self::child_is_transparent(&child),
                ));
            } else {
                draw_order_entries.push((i, 0.0, false));
            }
        }

        Self::sort_child_draw_order(&mut draw_order_entries);
        for (index, _, _) in draw_order_entries {
            let child = self.children[index].1.clone();
            child.draw_3d_all(cx, &mut scene_scope);
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

    pub fn toggle_depth_mesh_visible(&mut self, cx: &mut Cx) -> bool {
        let visible = self.env.toggle_depth_mesh_visible();
        cx.redraw_all();
        visible
    }

    pub fn toggle_depth_query_hits_visible(&mut self, cx: &mut Cx) -> bool {
        let visible = self.env.toggle_depth_query_hits_visible();
        cx.redraw_all();
        visible
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

    pub fn frame_cpu_ms(&self) -> f64 {
        self.frame_metrics.last_frame_cpu_ms
    }

    pub fn frame_update_cpu_ms(&self) -> f64 {
        self.frame_metrics.last_frame_update_cpu_ms
    }

    pub fn frame_draw_cpu_ms(&self) -> f64 {
        self.frame_metrics.last_frame_draw_cpu_ms
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
                self.env.reset_physics(cx);
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
                let augmented_state = self.augment_xr_state(update.state.as_ref());
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
                self.ensure_xr_content_pose(cx, &augmented_state);
                self.runtime.last_xr_state = Some(augmented_state.clone());
                if augmented_update.clicked_menu() {
                    self.env.reset_physics(cx);
                }
                self.env.ensure_physics(cx, &self.children);
                self.env.step_physics(cx);
                let mut event_scope_data = super::xr_view::XrViewEventScopeData {
                    content_transform: self.xr_content_transform(Some(&augmented_update.state)),
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
                let Some(body_spawn) = widget_action.action.downcast_ref::<XrBodySpawn>() else {
                    continue;
                };
                self.env.spawn_body(cx, *body_spawn);
            }
            if actions.iter().any(|action| {
                action.as_widget_action().is_some_and(|action| {
                    matches!(
                        action.cast::<XrSelectAction>(),
                        XrSelectAction::ActiveChildChanged(_)
                    )
                })
            }) {
                self.env.mark_scene_dirty();
                self.env.ensure_physics(cx, &self.children);
            }
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
