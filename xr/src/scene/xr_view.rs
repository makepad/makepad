use crate::xr_node::{xr_widget_world_transform, XrNode};
use crate::*;
use makepad_widgets::{
    animator::{Animator, AnimatorImpl},
    event::XrFingerTip,
};
use std::{cell::Cell, rc::Rc};

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.draw.DrawXrFingerCursor = mod.std.set_type_default() do #(DrawXrFingerCursor::script_shader(vm)){
        ..mod.draw.DrawQuad
        fill_color: vec4(0.26, 0.78, 1.0, 0.22)
        stroke_color: vec4(0.92, 0.97, 1.0, 0.96)
        stroke_width: 2.0

        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size);
            let center = self.rect_size * 0.5;
            let radius = min(self.rect_size.x, self.rect_size.y) * 0.5 - self.stroke_width;
            sdf.circle(center.x, center.y, radius.max(1.0));
            sdf.fill_keep(self.fill_color);
            sdf.stroke(self.stroke_color, self.stroke_width);
            return sdf.result;
        }
    }

    let XrViewMode = set_type_default() do #(XrViewMode::script_api(vm))
    mod.widgets.XrViewMode = XrViewMode

    mod.widgets.XrViewBase = #(XrView::register_widget(vm))
    mod.widgets.XrView = set_type_default() do mod.widgets.XrViewBase{
        mode: XrViewMode.World
        wrist_left: true
        show_in_non_xr: false
        fit_size: false
        multitouch: false
        pixel_scale: 0.0004
        dpi_factor: 3.0
        logical_size: vec2(320, 400)
        depth_scale: 300.0
        animator: Animator{
            cursor_pulse: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_cursor: {pulse: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Forward {duration: 0.18}}
                    apply: {
                        draw_cursor: {pulse: [
                            {time: 0.0, value: 0.0},
                            {time: 0.35, value: 1.0},
                            {time: 1.0, value: 0.0}
                        ]}
                    }
                }
            }
        }
        draw_cursor: mod.draw.DrawXrFingerCursor{}
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawXrFingerCursor {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    fill_color: Vec4f,
    #[live]
    stroke_color: Vec4f,
    #[live]
    stroke_width: f32,
    #[live(0.0)]
    pulse: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct XrFingerCursor {
    index: usize,
    pos: Vec2d,
    size: f64,
    is_left: bool,
    active: bool,
    down: bool,
}

#[derive(Clone, Copy, Debug)]
struct XrPanelRayHit {
    projected: Vec3f,
    cursor_depth: f32,
    touch_z: f32,
}

#[derive(Clone, Copy)]
pub struct XrViewEventScopeData {
    pub content_transform: Mat4f,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Script, ScriptHook)]
pub enum XrViewMode {
    #[pick]
    World,
    StuckToWrist,
}

impl Default for XrViewMode {
    fn default() -> Self {
        Self::World
    }
}

#[derive(Script, Animator, WidgetRef, WidgetRegister)]
pub struct XrView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[apply_default]
    animator: Animator,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[rust]
    area: Area,

    // 3D placement
    #[deref]
    node: XrNode,
    #[live]
    mode: XrViewMode,
    #[live(true)]
    wrist_left: bool,
    #[live(false)]
    show_in_non_xr: bool,
    #[live(false)]
    fit_size: bool,
    #[live(false)]
    multitouch: bool,

    // Panel rendering
    #[live(vec2(320.0, 400.0))]
    logical_size: Vec2d,
    #[live(0.0004)]
    pixel_scale: f32,
    #[live(3.0)]
    dpi_factor: f64,
    #[live(300.0)]
    depth_scale: f32,
    #[live]
    draw_cursor: DrawXrFingerCursor,
    #[new]
    draw_list: DrawList2d,

    // 2D children
    #[rust]
    child_widgets: Vec<(LiveId, WidgetRef)>,
    #[rust]
    finger_cursors: Vec<XrFingerCursor>,
    #[rust]
    last_xr_state: Option<Rc<XrState>>,
    #[rust]
    world_pose_override: Option<Pose>,
    #[rust]
    fitted_logical_size: Vec2d,
    #[rust]
    tip_down_state: [bool; XrHand::TIP_COUNT * 2],
    #[rust]
    live_update_order: SmallVec<[LiveId; 1]>,
}

impl XrView {
    const XR_CURSOR_HOVER_FRONT: f32 = 84.0;
    const XR_CURSOR_SIZE_NEAR: f64 = 30.0;
    const XR_CURSOR_SIZE_FAR: f64 = 16.0;
    const XR_TOUCH_DOWN_FRONT: f32 = 6.0;
    const XR_TOUCH_DOWN_BACK: f32 = -12.0;
    const FACE_PANEL_DISTANCE: f32 = 0.46;
    const FACE_PANEL_VERTICAL_OFFSET: f32 = -0.10;
    const WRIST_PANEL_SURFACE_OFFSET: f32 = 0.048;
    const WRIST_PANEL_ALONG_HAND_OFFSET: f32 = -0.010;
    const WRIST_PANEL_FACE_CULL_DOT: f32 = 0.0;
    const ARM_PANEL_MENU_SIDE_OFFSET: f32 = 0.15;
    const ARM_PANEL_MENU_BACK_OFFSET: f32 = 0.10;

    pub(crate) fn node(&self) -> &XrNode {
        &self.node
    }

    fn scaled_pose_matrix(&self, pose: Pose) -> Mat4f {
        Mat4f::mul(
            &pose.to_mat4(),
            &Mat4f::nonuniform_scaled_translation(
                vec3(
                    self.node.scale().x,
                    self.node.scale().y,
                    self.node.scale().z,
                ),
                vec3(0.0, 0.0, 0.0),
            ),
        )
    }

    fn direct_world_transform(&self) -> Mat4f {
        self.world_pose_override
            .map(|pose| self.scaled_pose_matrix(pose))
            .unwrap_or_else(|| self.node.local_transform())
    }

    fn event_world_transform(&self, scope: &mut Scope) -> Mat4f {
        if let Some(runtime_body) = xr_runtime_body_from_scope(scope, self.uid) {
            Mat4f::mul(
                &runtime_body.pose.to_mat4(),
                &Mat4f::nonuniform_scaled_translation(
                    vec3(
                        runtime_body.scale.x,
                        runtime_body.scale.y,
                        runtime_body.scale.z,
                    ),
                    vec3(0.0, 0.0, 0.0),
                ),
            )
        } else if let Some(pose) = self.world_pose_override {
            self.scaled_pose_matrix(pose)
        } else if let Some(event_scope) = scope.data.get::<XrViewEventScopeData>() {
            Mat4f::mul(&event_scope.content_transform, &self.node.local_transform())
        } else {
            self.node.local_transform()
        }
    }

    fn event_space_transform(&self, event_space_world: &Mat4f) -> Mat4f {
        if let Some(pose) = self.world_pose_override {
            Mat4f::mul(&event_space_world.invert(), &self.scaled_pose_matrix(pose))
        } else {
            self.node.local_transform()
        }
    }

    fn resolved_world_transform(&self, cx: &mut Cx3d, scope: &mut Scope) -> Mat4f {
        if let Some(runtime_body) = xr_runtime_body_from_scope(scope, self.uid) {
            Mat4f::mul(
                &runtime_body.pose.to_mat4(),
                &Mat4f::nonuniform_scaled_translation(
                    vec3(
                        runtime_body.scale.x,
                        runtime_body.scale.y,
                        runtime_body.scale.z,
                    ),
                    vec3(0.0, 0.0, 0.0),
                ),
            )
        } else if let Some(pose) = self.world_pose_override {
            self.scaled_pose_matrix(pose)
        } else {
            xr_widget_world_transform(cx, scope, self.uid, &self.node)
        }
    }

    fn xr_flat_forward(orientation: Quat) -> Vec3f {
        let mut forward = orientation.rotate_vec3(&vec3f(0.0, 0.0, -1.0));
        forward.y = 0.0;
        if forward.length() <= 1.0e-6 {
            vec3f(0.0, 0.0, -1.0)
        } else {
            forward.normalize()
        }
    }

    fn front_of_face_pose(state: &XrState) -> Pose {
        let forward = Self::xr_flat_forward(state.head_pose.orientation);
        Pose {
            position: state.head_pose.position
                + forward.scale(Self::FACE_PANEL_DISTANCE)
                + vec3f(0.0, Self::FACE_PANEL_VERTICAL_OFFSET, 0.0),
            orientation: Quat::look_rotation(forward.scale(-1.0), vec3f(0.0, 1.0, 0.0)),
        }
    }

    fn look_rotation_with_up(forward: Vec3f, preferred_up: Vec3f) -> Quat {
        let forward = if forward.length() > 1.0e-6 {
            forward.normalize()
        } else {
            vec3f(0.0, 0.0, -1.0)
        };
        let mut up = preferred_up - forward.scale(preferred_up.dot(forward));
        if up.length() <= 1.0e-6 {
            up = vec3f(0.0, 1.0, 0.0) - forward.scale(forward.y);
        }
        if up.length() <= 1.0e-6 {
            up = vec3f(1.0, 0.0, 0.0);
        } else {
            up = up.normalize();
        }
        Quat::look_rotation(forward, up)
    }

    fn wrist_frame(&self, state: &XrState) -> Option<(Vec3f, Vec3f, Vec3f, Vec3f)> {
        let hand = if self.wrist_left {
            &state.left_hand
        } else {
            &state.right_hand
        };
        if !hand.in_view() {
            return None;
        }

        let wrist_pose = hand.joints[XrHand::WRIST];
        let wrist = wrist_pose.position;
        let along_hand = hand.joints[XrHand::CENTER].position - wrist;
        if along_hand.length() <= 1.0e-5 {
            return None;
        }

        let across_hand = if self.wrist_left {
            hand.joints[XrHand::INDEX_BASE].position - hand.joints[XrHand::LITTLE_BASE].position
        } else {
            hand.joints[XrHand::LITTLE_BASE].position - hand.joints[XrHand::INDEX_BASE].position
        };
        if across_hand.length() <= 1.0e-5 {
            return None;
        }

        let along_hand = along_hand.normalize();
        let palm_side = Vec3f::cross(across_hand.normalize(), along_hand);
        if palm_side.length() <= 1.0e-5 {
            return None;
        }
        let palm_side = palm_side.normalize();

        let mut wrist_surface_dir = wrist_pose.orientation.rotate_vec3(&vec3f(0.0, 1.0, 0.0));
        if wrist_surface_dir.dot(palm_side) < 0.0 {
            wrist_surface_dir = wrist_surface_dir.scale(-1.0);
        }
        wrist_surface_dir = wrist_surface_dir.normalize();

        let mut wrist_up = wrist_pose.orientation.rotate_vec3(&vec3f(0.0, 0.0, -1.0));
        if wrist_up.dot(along_hand) < 0.0 {
            wrist_up = wrist_up.scale(-1.0);
        }
        wrist_up = wrist_up.normalize();

        let wrist_surface_dir = wrist_surface_dir.scale(-1.0);
        let mut wrist_side = Vec3f::cross(wrist_up, wrist_surface_dir);
        if wrist_side.length() <= 1.0e-5 {
            wrist_side = across_hand.normalize();
        }
        if wrist_side.dot(across_hand) < 0.0 {
            wrist_side = wrist_side.scale(-1.0);
        }
        wrist_side = wrist_side.normalize();
        Some((wrist, wrist_surface_dir, wrist_up, wrist_side))
    }

    fn wrist_view_pose(&self, state: &XrState) -> Option<(Pose, bool)> {
        let (wrist, wrist_surface_dir, wrist_up, _) = self.wrist_frame(state)?;

        let position = wrist
            + wrist_surface_dir.scale(Self::WRIST_PANEL_SURFACE_OFFSET)
            + wrist_up.scale(Self::WRIST_PANEL_ALONG_HAND_OFFSET);
        let pose = Pose::new(
            Self::look_rotation_with_up(wrist_surface_dir, wrist_up),
            position,
        );
        let to_head = state.head_pose.position - position;
        let visible = to_head.length() > 1.0e-5
            && wrist_surface_dir.dot(to_head.normalize()) >= Self::WRIST_PANEL_FACE_CULL_DOT;
        Some((pose, visible))
    }

    fn next_to_wrist_pose(&self, state: &XrState) -> Option<Pose> {
        let (menu_pose, _) = self.wrist_view_pose(state)?;
        let gravity_up = vec3f(0.0, 1.0, 0.0);
        let to_head = state.head_pose.position - menu_pose.position;
        if to_head.length() <= 1.0e-5 {
            return None;
        }
        let menu_to_head = to_head.normalize();
        let mut menu_right = Vec3f::cross(gravity_up, menu_to_head);
        if menu_right.length() <= 1.0e-5 {
            menu_right = vec3f(1.0, 0.0, 0.0);
        } else {
            menu_right = menu_right.normalize();
        }
        let position = menu_pose.position + menu_right.scale(Self::ARM_PANEL_MENU_SIDE_OFFSET)
            - menu_to_head.scale(Self::ARM_PANEL_MENU_BACK_OFFSET);
        let forward = state.head_pose.position - position;
        Some(Pose::new(
            Self::look_rotation_with_up(forward, gravity_up),
            position,
        ))
    }

    fn sync_mode_pose_from_state(&mut self, cx: &mut Cx, state: &XrState) {
        if self.mode != XrViewMode::StuckToWrist {
            return;
        }
        let (world_pose_override, visible) =
            if let Some((pose, visible)) = self.wrist_view_pose(state) {
                (Some(pose), visible)
            } else {
                (self.world_pose_override, false)
            };
        self.world_pose_override = world_pose_override;
        if !visible {
            self.finger_cursors.clear();
        }
        self.node.set_visible(cx, visible);
    }

    fn sync_non_xr_visibility(&mut self, cx: &mut Cx) {
        if !self.show_in_non_xr {
            return;
        }
        if self.mode == XrViewMode::StuckToWrist {
            self.world_pose_override = None;
        }
        self.node.set_visible(cx, true);
    }

    fn move_in_front_of_face_now(&mut self, cx: &mut Cx) -> bool {
        let Some(state) = self.last_xr_state.as_deref() else {
            return false;
        };
        self.world_pose_override = Some(Self::front_of_face_pose(state));
        self.redraw(cx);
        true
    }

    fn move_next_to_wrist_now(&mut self, cx: &mut Cx) -> bool {
        let Some(state) = self.last_xr_state.as_deref() else {
            return false;
        };
        let Some(pose) = self.next_to_wrist_pose(state) else {
            return false;
        };
        self.world_pose_override = Some(pose);
        self.redraw(cx);
        true
    }

    fn panel_matrix(&self, world_transform: Mat4f) -> Mat4f {
        let scale = self.pixel_scale.max(0.00001) * self.dpi_factor.max(1.0) as f32;
        let logical_size = self.resolved_logical_size();
        let local_depth = Mat4f::nonuniform_scaled_translation(
            vec3(1.0, 1.0, self.depth_scale.max(0.00001)),
            vec3(0.0, 0.0, 0.0),
        );
        let local_panel = Mat4f::nonuniform_scaled_translation(
            vec3(scale, -scale, scale),
            vec3(
                -(logical_size.x as f32) * scale * 0.5,
                (logical_size.y as f32) * scale * 0.5,
                0.0,
            ),
        );
        let object_to_world = Mat4f::mul(&local_panel, &local_depth);
        Mat4f::mul(&world_transform, &object_to_world)
    }

    fn hit_matrix(&self, world_transform: Mat4f) -> Mat4f {
        let scale = self.pixel_scale.max(0.00001) * self.dpi_factor.max(1.0) as f32;
        let logical_size = self.resolved_logical_size();
        let local_panel = Mat4f::nonuniform_scaled_translation(
            vec3(scale, -scale, scale),
            vec3(
                -(logical_size.x as f32) * scale * 0.5,
                (logical_size.y as f32) * scale * 0.5,
                0.0,
            ),
        );
        Mat4f::mul(&world_transform, &local_panel)
    }

    fn panel_ray_hit(
        hit_matrix: &Mat4f,
        ray_origin: Vec3f,
        ray_dir: Vec3f,
        touch_z: f32,
    ) -> Option<XrPanelRayHit> {
        let inv = hit_matrix.invert();
        let origin = inv
            .transform_vec4(vec4(ray_origin.x, ray_origin.y, ray_origin.z, 1.0))
            .to_vec3f();
        let dir = inv
            .transform_vec4(vec4(ray_dir.x, ray_dir.y, ray_dir.z, 0.0))
            .to_vec3f();
        if dir.z.abs() <= 1.0e-6 {
            return None;
        }
        let t = -origin.z / dir.z;
        if t < 0.0 {
            return None;
        }
        Some(XrPanelRayHit {
            projected: origin + dir * t,
            cursor_depth: origin.z,
            touch_z,
        })
    }

    fn panel_normal_hit(hit_matrix: &Mat4f, tip_pos: Vec3f) -> XrPanelRayHit {
        let inv = hit_matrix.invert();
        let local = inv
            .transform_vec4(vec4(tip_pos.x, tip_pos.y, tip_pos.z, 1.0))
            .to_vec3f();
        XrPanelRayHit {
            projected: vec3f(local.x, local.y, 0.0),
            cursor_depth: local.z,
            touch_z: local.z,
        }
    }

    fn contains_local(&self, local: Vec3f) -> bool {
        let logical_size = self.resolved_logical_size();
        local.x >= 0.0
            && local.y >= 0.0
            && local.x <= logical_size.x as f32
            && local.y <= logical_size.y as f32
    }

    pub(crate) fn hits_parent_ray(&self, ray_origin: Vec3f, ray_dir: Vec3f) -> bool {
        if !self.node.visible() {
            return false;
        }
        let hit_mat = self.hit_matrix(self.direct_world_transform());
        Self::panel_ray_hit(&hit_mat, ray_origin, ray_dir, 0.0)
            .is_some_and(|hit| self.contains_local(hit.projected))
    }

    fn cursor_from_hit(&self, hit: XrPanelRayHit, is_left: bool) -> Option<XrFingerCursor> {
        if !self.contains_local(hit.projected) {
            return None;
        }
        let distance = hit.cursor_depth.abs();
        if distance > Self::XR_CURSOR_HOVER_FRONT {
            return None;
        }
        let proximity =
            1.0 - (distance.min(Self::XR_CURSOR_HOVER_FRONT) / Self::XR_CURSOR_HOVER_FRONT);
        let size = Self::XR_CURSOR_SIZE_FAR
            + (Self::XR_CURSOR_SIZE_NEAR - Self::XR_CURSOR_SIZE_FAR) * proximity as f64;
        Some(XrFingerCursor {
            index: XrHand::INDEX_TIP,
            pos: dvec2(hit.projected.x as f64, hit.projected.y as f64),
            size,
            is_left,
            active: true,
            down: false,
        })
    }

    fn tip_is_touching_for_down(touch_z: f32) -> bool {
        touch_z <= Self::XR_TOUCH_DOWN_FRONT && touch_z >= Self::XR_TOUCH_DOWN_BACK
    }

    fn fingertip_slot(is_left: bool, index: usize) -> usize {
        index + if is_left { XrHand::TIP_COUNT } else { 0 }
    }

    fn resolved_logical_size(&self) -> Vec2d {
        if self.fit_size && self.fitted_logical_size.x > 1.0 && self.fitted_logical_size.y > 1.0 {
            self.fitted_logical_size
        } else {
            self.logical_size
        }
    }

    fn update_fit_size_from_used(&mut self, cx2d: &mut Cx2d, used: Vec2d) {
        if !self.fit_size {
            self.fitted_logical_size = self.logical_size;
            return;
        }
        let next = dvec2(used.x.max(1.0), used.y.max(1.0));
        if (next.x - self.fitted_logical_size.x).abs() > 0.5
            || (next.y - self.fitted_logical_size.y).abs() > 0.5
        {
            self.fitted_logical_size = next;
            cx2d.cx.redraw_all();
        }
    }

    fn cursor_base_color(cursor: XrFingerCursor) -> Vec4f {
        match cursor.index {
            XrHand::THUMB_TIP => {
                if cursor.is_left {
                    vec4f(0.98, 0.48, 0.43, 1.0)
                } else {
                    vec4f(1.0, 0.60, 0.32, 1.0)
                }
            }
            XrHand::INDEX_TIP => {
                if cursor.is_left {
                    vec4f(0.30, 0.78, 1.0, 1.0)
                } else {
                    vec4f(0.22, 0.86, 0.98, 1.0)
                }
            }
            XrHand::MIDDLE_TIP => {
                if cursor.is_left {
                    vec4f(0.50, 0.90, 0.42, 1.0)
                } else {
                    vec4f(0.66, 0.88, 0.38, 1.0)
                }
            }
            XrHand::RING_TIP => {
                if cursor.is_left {
                    vec4f(0.84, 0.56, 1.0, 1.0)
                } else {
                    vec4f(0.92, 0.50, 0.96, 1.0)
                }
            }
            XrHand::LITTLE_TIP => {
                if cursor.is_left {
                    vec4f(1.0, 0.82, 0.34, 1.0)
                } else {
                    vec4f(1.0, 0.90, 0.40, 1.0)
                }
            }
            _ => vec4f(0.86, 0.90, 0.96, 1.0),
        }
    }

    fn draw_cursor_for_tip(&mut self, cx2d: &mut Cx2d, cursor: XrFingerCursor) {
        let base = Self::cursor_base_color(cursor);
        let fill_alpha = if cursor.down {
            0.84
        } else if cursor.active {
            0.18
        } else {
            0.05
        };
        let stroke_alpha = if cursor.down {
            1.0
        } else if cursor.active {
            0.96
        } else {
            0.34
        };
        let pulse = if cursor.down {
            self.draw_cursor.pulse.max(0.0) as f64
        } else {
            0.0
        };
        let draw_size = cursor.size
            * if cursor.down {
                1.08 + pulse * 0.10
            } else if cursor.active {
                1.0
            } else {
                0.86
            };

        self.draw_cursor.fill_color = vec4f(base.x, base.y, base.z, fill_alpha);
        self.draw_cursor.stroke_color = vec4f(base.x, base.y, base.z, stroke_alpha);
        self.draw_cursor.stroke_width = if cursor.down {
            1.25
        } else if cursor.active {
            2.2
        } else {
            1.4
        };
        self.draw_cursor.draw_abs(
            cx2d,
            Rect {
                pos: dvec2(
                    cursor.pos.x - draw_size * 0.5,
                    cursor.pos.y - draw_size * 0.5,
                ),
                size: dvec2(draw_size, draw_size),
            },
        );
    }

    fn dispatch_local_xr_event(
        &mut self,
        cx: &mut Cx,
        xr_event: &XrLocalEvent,
        scope: &mut Scope,
        world_transform: Mat4f,
    ) {
        let hit_mat = self.hit_matrix(world_transform);
        let mut local_tips = SmallVec::new();
        let mut finger_cursors = Vec::new();
        let mut next_tip_down_state = [false; XrHand::TIP_COUNT * 2];
        for tip in &xr_event.finger_tips {
            let use_normal_projection = if tip.is_left {
                xr_event.update.state.left_hand.in_view()
                    && xr_event.update.state.left_hand.tip_active(tip.index)
            } else {
                xr_event.update.state.right_hand.in_view()
                    && xr_event.update.state.right_hand.tip_active(tip.index)
            };

            let hit = if use_normal_projection {
                Some(Self::panel_normal_hit(&hit_mat, tip.pos))
            } else {
                Self::panel_ray_hit(&hit_mat, tip.pos, tip.ray_dir, tip.touch_z)
            };

            if let Some(hit) = hit {
                let tip_is_interactive = self.multitouch || tip.index == XrHand::INDEX_TIP;
                let tip_is_down = tip.active
                    && tip_is_interactive
                    && self.contains_local(hit.projected)
                    && Self::tip_is_touching_for_down(hit.touch_z);
                let tip_slot = Self::fingertip_slot(tip.is_left, tip.index);
                let was_tip_down = self.tip_down_state[tip_slot];

                if let Some(mut cursor) = self.cursor_from_hit(hit, tip.is_left) {
                    cursor.index = tip.index;
                    cursor.active = tip.active && tip_is_interactive;
                    cursor.down = tip_is_down;
                    finger_cursors.push(cursor);
                }
                if tip_is_down && !was_tip_down {
                    self.animator_play(cx, ids!(cursor_pulse.off));
                    self.animator_play(cx, ids!(cursor_pulse.on));
                }
                next_tip_down_state[tip_slot] = tip_is_down;
                local_tips.push(XrFingerTip {
                    index: tip.index,
                    is_left: tip.is_left,
                    active: tip.active,
                    interactive: tip_is_interactive,
                    pos: vec3f(hit.projected.x, hit.projected.y, hit.touch_z),
                    ray_dir: vec3f(0.0, 0.0, -1.0),
                    touch_z: hit.touch_z,
                    handled: Cell::new(Area::Empty),
                });
            }
        }
        self.tip_down_state = next_tip_down_state;
        self.finger_cursors = finger_cursors;
        let local_event = XrLocalEvent {
            finger_tips: local_tips,
            space_transform: Mat4f::identity(),
            digit_namespace: self.uid.0,
            update: xr_event.update.clone(),
            modifiers: xr_event.modifiers,
            time: xr_event.time,
        };
        let event = Event::XrLocal(local_event.clone());
        for i in 0..self.child_widgets.len() {
            let child = self.child_widgets[i].1.clone();
            child.handle_event(cx, &event, scope);
        }
        local_event.process_end(cx);
    }
}

impl ScriptHook for XrView {
    fn on_before_apply(
        &mut self,
        _vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if apply.is_reload() {
            self.live_update_order.clear();
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
                let mut anon_index = 0usize;
                vm.vec_with(obj, |vm, vec| {
                    for kv in vec {
                        let id = if let Some(id) = kv.key.as_id() {
                            Some(id)
                        } else if kv.key.is_nil() {
                            let id = LiveId(anon_index as u64);
                            anon_index += 1;
                            Some(id)
                        } else {
                            None
                        };
                        let Some(id) = id else { continue };
                        if !WidgetRef::value_is_newable_widget(vm, kv.value) {
                            continue;
                        }
                        if apply.is_reload() {
                            self.live_update_order.push(id);
                        }
                        if let Some((_, child)) =
                            self.child_widgets.iter_mut().find(|(id2, _)| *id2 == id)
                        {
                            child.script_apply(vm, apply, scope, kv.value);
                        } else {
                            let child = WidgetRef::script_from_value_scoped(vm, scope, kv.value);
                            self.child_widgets.push((id, child.clone()));
                            vm.cx_mut()
                                .widget_tree_insert_child_deep(self.uid, id, child);
                        }
                    }
                });
            }
        }
        if apply.is_reload() {
            if !self.live_update_order.is_empty() || self.child_widgets.is_empty() {
                for (idx, id) in self.live_update_order.iter().enumerate() {
                    if let Some(pos) = self.child_widgets.iter().position(|(i, _)| *i == *id) {
                        self.child_widgets.swap(idx, pos);
                    }
                }
                self.child_widgets.truncate(self.live_update_order.len());
            }
        }
        vm.with_cx_mut(|cx| {
            cx.widget_tree_mark_dirty(self.uid);
        });
    }
}

impl WidgetNode for XrView {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }
    fn area(&self) -> Area {
        self.area
    }

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        for (id, child) in &self.child_widgets {
            visit(*id, child.clone());
        }
    }

    fn redraw(&mut self, cx: &mut Cx) {
        cx.redraw_all();
    }

    fn visible(&self) -> bool {
        self.node.visible()
    }
    fn set_visible(&mut self, cx: &mut Cx, visible: bool) {
        self.node.set_visible(cx, visible);
    }
}

impl Widget for XrView {
    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(set_visible) {
            let mut visible = self.node.visible();
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                visible = vm
                    .bx
                    .heap
                    .cast_to_bool(vm.bx.heap.vec_value(args_obj, 0, trap));
            }
            vm.with_cx_mut(|cx| {
                self.node.set_visible(cx, visible);
            });
            return ScriptAsyncResult::Return(ScriptValue::from_bool(visible));
        }
        if method == live_id!(toggle_visible) {
            let visible = !self.node.visible();
            vm.with_cx_mut(|cx| {
                self.node.set_visible(cx, visible);
            });
            return ScriptAsyncResult::Return(ScriptValue::from_bool(visible));
        }
        if method == live_id!(visible) {
            return ScriptAsyncResult::Return(ScriptValue::from_bool(self.node.visible()));
        }
        if method == live_id!(move_in_front_of_face) {
            let moved = vm.with_cx_mut(|cx| self.move_in_front_of_face_now(cx));
            return ScriptAsyncResult::Return(ScriptValue::from_bool(moved));
        }
        if method == live_id!(show_in_front_of_face) {
            let moved = vm.with_cx_mut(|cx| {
                let moved = self.move_in_front_of_face_now(cx);
                self.node.set_visible(cx, true);
                moved
            });
            return ScriptAsyncResult::Return(ScriptValue::from_bool(moved));
        }
        if method == live_id!(move_next_to_wrist) {
            let moved = vm.with_cx_mut(|cx| self.move_next_to_wrist_now(cx));
            return ScriptAsyncResult::Return(ScriptValue::from_bool(moved));
        }
        if method == live_id!(show_next_to_wrist) {
            let moved = vm.with_cx_mut(|cx| {
                let moved = self.move_next_to_wrist_now(cx);
                self.node.set_visible(cx, true);
                moved
            });
            return ScriptAsyncResult::Return(ScriptValue::from_bool(moved));
        }
        if method == live_id!(toggle_visible_in_front_of_face) {
            let visible = vm.with_cx_mut(|cx| {
                let visible = !self.node.visible();
                if visible {
                    let _ = self.move_in_front_of_face_now(cx);
                }
                self.node.set_visible(cx, visible);
                visible
            });
            return ScriptAsyncResult::Return(ScriptValue::from_bool(visible));
        }
        if method == live_id!(toggle_visible_next_to_wrist) {
            let visible = vm.with_cx_mut(|cx| {
                let visible = !self.node.visible();
                if visible {
                    let _ = self.move_next_to_wrist_now(cx);
                }
                self.node.set_visible(cx, visible);
                visible
            });
            return ScriptAsyncResult::Return(ScriptValue::from_bool(visible));
        }
        let _ = args;
        ScriptAsyncResult::MethodNotFound
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.animator_handle_event(cx, event).must_redraw() {
            self.redraw(cx);
        }

        if let Event::XrUpdate(update) = event {
            self.last_xr_state = Some(update.state.clone());
            self.sync_mode_pose_from_state(cx, &update.state);
        } else if !cx.in_xr_mode() {
            self.sync_non_xr_visibility(cx);
        }
        if !self.node.visible() && event.requires_visibility() {
            return;
        }

        if let Event::XrUpdate(update) = event {
            let xr_event = XrLocalEvent::from_update_event(update, &Mat4f::identity());
            let world_transform = self.event_world_transform(scope);
            self.dispatch_local_xr_event(cx, &xr_event, scope, world_transform);
            return;
        }

        // Forward XrLocal events — transform finger tips to panel-local 2D coords
        if let Event::XrLocal(xr_event) = event {
            let world_transform = self.event_space_transform(&xr_event.space_transform);
            self.dispatch_local_xr_event(cx, xr_event, scope, world_transform);
            return;
        }

        // Forward other events to children
        for i in 0..self.child_widgets.len() {
            let child = self.child_widgets[i].1.clone();
            child.handle_event(cx, event, scope);
        }

        if matches!(event, Event::MouseLeave(_) | Event::MouseUp(_)) {
            self.finger_cursors.clear();
        }
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }

    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        if !self.node.visible() {
            return DrawStep::done();
        }

        let world_transform = self.resolved_world_transform(cx, scope);
        let matrix = self.panel_matrix(world_transform);
        let logical_size = self.resolved_logical_size();

        // Draw 2D children into a DrawList2d with the panel transform
        let cx2d = &mut Cx2d::new(cx.cx);
        let previous_dpi = cx2d.current_dpi_factor();
        cx2d.set_current_pass_dpi_factor(self.dpi_factor.max(1.0));

        self.draw_list.set_reset_zbias(cx2d.cx, true);
        self.draw_list.begin_always(cx2d);
        self.draw_list.set_view_transform(cx2d, &matrix);
        let size = dvec2(logical_size.x.max(1.0), logical_size.y.max(1.0));
        cx2d.begin_root_turtle(size, Layout::flow_down());

        for i in 0..self.child_widgets.len() {
            let child = self.child_widgets[i].1.clone();
            child.draw_all(cx2d, scope);
        }

        self.update_fit_size_from_used(cx2d, cx2d.turtle().used());

        let finger_cursors = self.finger_cursors.clone();
        for pass in 0..3 {
            for cursor in finger_cursors.iter().copied() {
                let draw_in_pass = match pass {
                    0 => !cursor.active,
                    1 => cursor.active && !cursor.down,
                    _ => cursor.down,
                };
                if draw_in_pass {
                    self.draw_cursor_for_tip(cx2d, cursor);
                }
            }
        }

        cx2d.end_pass_sized_turtle();
        self.draw_list.end(cx2d);
        cx2d.set_current_pass_dpi_factor(previous_dpi);

        DrawStep::done()
    }
}
