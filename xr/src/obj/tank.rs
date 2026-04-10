use crate::obj::car::{car_drive_command, CarDriveConfig};
use crate::obj::{Cube, IcoSphere};
use crate::prelude::*;
use makepad_widgets::{
    makepad_derive_widget::*,
    widget::*,
    widget_async::{ScriptAsyncId, ScriptAsyncResult},
};
use std::collections::HashSet;
use std::rc::Rc;

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.TankBase = #(Tank::register_widget(vm))
    mod.widgets.Tank = set_type_default() do mod.widgets.TankBase{
        body: mod.widgets.XrBodyKind.Disabled
        shared_object_policy: mod.widgets.XrSharedObjectPolicy.None
    }
}

pub const TANK_TURRET_YAW_SPEED_RADPS: f32 = 1.5;
pub const TANK_TURRET_PITCH_SPEED_RADPS: f32 = 4.2;
pub const TANK_TURRET_PITCH_MIN_RAD: f32 = -0.35;
pub const TANK_TURRET_PITCH_MAX_RAD: f32 = 0.55;
pub const TANK_PROJECTILE_RATE_HZ: f32 = 10.0;
pub const TANK_PROJECTILE_SPEED_MPS: f32 = 7.5;
pub const TANK_PROJECTILE_RADIUS_METERS: f32 = 0.024;
pub const TANK_PROJECTILE_MAX_EMITS_PER_UPDATE: usize = 2;
pub const TANK_HIT_FLASH_SECONDS: f64 = 0.35;
pub const TANK_SPAWN_RING_RADIUS_METERS: f32 = 0.06;
pub const TANK_WHEEL_COUNT: usize = 4;
pub const TANK_WHEEL_LATERAL_OFFSET_METERS: f32 = 0.113;
pub const TANK_WHEEL_VERTICAL_OFFSET_METERS: f32 = -0.045;
pub const TANK_WHEEL_FRONT_OFFSET_METERS: f32 = 0.189;
pub const TANK_WHEEL_BACK_OFFSET_METERS: f32 = -0.189;
pub const TANK_BODY_HALF_WIDTH_METERS: f32 = 0.145;
pub const TANK_BODY_HALF_HEIGHT_METERS: f32 = 0.045;
pub const TANK_BODY_HALF_DEPTH_METERS: f32 = 0.205;
pub const TANK_PLATE_TOP_LOCAL_Y_METERS: f32 = -0.02;
pub const TANK_FOUR_WHEEL_RADIUS_SCALE: f32 = 3.20;
pub const TANK_FOUR_WHEEL_REST_LENGTH_SCALE: f32 = 0.50;
pub const TANK_FOUR_WHEEL_RADIUS_MIN_METERS: f32 = 0.036;
pub const TANK_FOUR_WHEEL_RADIUS_MAX_METERS: f32 = 0.160;
pub const TANK_FOUR_WHEEL_REST_LENGTH_MIN_METERS: f32 = 0.024;
pub const TANK_FOUR_WHEEL_REST_LENGTH_MAX_METERS: f32 = 0.110;
pub const TANK_SPAWN_SUSPENSION_PRELOAD_WORLD_METERS: f32 = 0.004;
pub const TANK_SPAWN_EXTRA_CLEARANCE_WORLD_METERS: f32 = 0.030;
pub const TANK_BODY_VISUAL_SUSPENSION_RESPONSE: f32 = 0.0;
pub const TANK_BODY_VISUAL_AXLE_CLEARANCE_SCALE: f32 = 0.42;
pub const TANK_BODY_VISUAL_LIFT_MIN_METERS: f32 = 0.0;
pub const TANK_BODY_VISUAL_LIFT_MAX_METERS: f32 = 0.300;
pub const TANK_SCENE_STATUS_TEXT: &str =
    "Tank mode: left stick steers, right trigger accelerates, left trigger reverses, right stick aims the turret, A/X fire shells, B resets the tank, and controller grip picks the tank up.";

#[derive(Clone, Copy, Debug)]
pub struct TankDriveConfig {
    pub turn_gain: f32,
    pub max_speed_mps: f32,
    pub max_yaw_speed_radps: f32,
    pub max_linear_accel_mps2: f32,
    pub max_angular_accel_radps2: f32,
    pub stick_deadzone: f32,
    pub stick_response_exponent: f32,
}

impl Default for TankDriveConfig {
    fn default() -> Self {
        Self {
            turn_gain: 0.32,
            max_speed_mps: 0.72,
            max_yaw_speed_radps: 1.25,
            max_linear_accel_mps2: 1.8,
            max_angular_accel_radps2: 3.2,
            stick_deadzone: 0.24,
            stick_response_exponent: 1.75,
        }
    }
}

pub trait TankSceneHost {
    fn ui_root(&self) -> WidgetRef;
    fn local_peer_id(&self, cx: &mut Cx) -> Option<XrNetPeerId>;
    fn shared_object_authority_for_widget(
        &self,
        cx: &mut Cx,
        widget_uid: WidgetUid,
    ) -> Option<XrNetPeerId>;
    fn widget_is_local_shared_object(&self, cx: &mut Cx, widget_uid: WidgetUid) -> bool;
    fn emit_local_shared_body_spawn(&mut self, cx: &mut Cx, spawn: XrBodySpawn) -> WidgetUid;
    fn emit_local_shared_body_spawn_exact(
        &mut self,
        cx: &mut Cx,
        spawn: XrBodySpawn,
    ) -> WidgetUid;
    fn runtime_body_state(&self, widget_uid: WidgetUid) -> Option<XrRuntimeBodyState>;
    fn runtime_contacts(&self) -> Option<Rc<Vec<(WidgetUid, WidgetUid)>>>;
    fn apply_car_control(&mut self, cx: &mut Cx, control: XrCarControl);
    fn depth_mesh_focus_cube_enabled(&self) -> bool;
    fn toggle_depth_mesh_focus_cube(&mut self, cx: &mut Cx);
    fn set_depth_mesh_focus_point(&mut self, cx: &mut Cx, point: Option<Vec3f>);
}

impl TankSceneHost for WidgetRef {
    fn ui_root(&self) -> WidgetRef {
        self.clone()
    }

    fn local_peer_id(&self, cx: &mut Cx) -> Option<XrNetPeerId> {
        self.widget(cx, ids!(xr_peer_sync))
            .borrow::<XrPeerSync>()
            .and_then(|peer_sync| peer_sync.local_peer_id())
    }

    fn shared_object_authority_for_widget(
        &self,
        cx: &mut Cx,
        widget_uid: WidgetUid,
    ) -> Option<XrNetPeerId> {
        self.widget(cx, ids!(xr_peer_sync))
            .borrow::<XrPeerSync>()
            .and_then(|peer_sync| peer_sync.shared_object_authority_for_widget(widget_uid))
    }

    fn widget_is_local_shared_object(&self, cx: &mut Cx, widget_uid: WidgetUid) -> bool {
        self.widget(cx, ids!(xr_peer_sync))
            .borrow::<XrPeerSync>()
            .is_some_and(|peer_sync| peer_sync.widget_is_local_shared_object(widget_uid))
    }

    fn emit_local_shared_body_spawn(&mut self, cx: &mut Cx, spawn: XrBodySpawn) -> WidgetUid {
        let peer_sync_widget = self.widget(cx, ids!(xr_peer_sync));
        if let Some(mut peer_sync) = peer_sync_widget.borrow_mut::<XrPeerSync>() {
            if let Some(spawn) = peer_sync.send_local_body_spawn(spawn) {
                let widget_uid = spawn.widget_uid;
                if let Some(mut root) = self.borrow_mut::<XrRoot>() {
                    root.spawn_body(cx, spawn);
                }
                return widget_uid;
            }
        }
        let widget_uid = spawn.widget_uid;
        if let Some(mut root) = self.borrow_mut::<XrRoot>() {
            root.spawn_body(cx, spawn);
        }
        widget_uid
    }

    fn emit_local_shared_body_spawn_exact(
        &mut self,
        cx: &mut Cx,
        spawn: XrBodySpawn,
    ) -> WidgetUid {
        let peer_sync_widget = self.widget(cx, ids!(xr_peer_sync));
        if let Some(mut peer_sync) = peer_sync_widget.borrow_mut::<XrPeerSync>() {
            if let Some(spawn) = peer_sync.send_local_body_spawn_exact(spawn) {
                let widget_uid = spawn.widget_uid;
                if let Some(mut root) = self.borrow_mut::<XrRoot>() {
                    root.spawn_body(cx, spawn);
                }
                return widget_uid;
            }
        }
        let widget_uid = spawn.widget_uid;
        if let Some(mut root) = self.borrow_mut::<XrRoot>() {
            root.spawn_body(cx, spawn);
        }
        widget_uid
    }

    fn runtime_body_state(&self, widget_uid: WidgetUid) -> Option<XrRuntimeBodyState> {
        let runtime_bodies = self.borrow::<XrRoot>().map(|root| root.runtime_bodies())?;
        runtime_bodies.get(&widget_uid).cloned()
    }

    fn runtime_contacts(&self) -> Option<Rc<Vec<(WidgetUid, WidgetUid)>>> {
        self.borrow::<XrRoot>().map(|root| root.runtime_contacts())
    }

    fn apply_car_control(&mut self, cx: &mut Cx, control: XrCarControl) {
        if let Some(mut root) = self.borrow_mut::<XrRoot>() {
            root.apply_car_control(cx, control);
        }
    }

    fn depth_mesh_focus_cube_enabled(&self) -> bool {
        self.borrow::<XrRoot>()
            .is_some_and(|root| root.depth_mesh_focus_cube_enabled())
    }

    fn toggle_depth_mesh_focus_cube(&mut self, cx: &mut Cx) {
        if let Some(mut root) = self.borrow_mut::<XrRoot>() {
            root.toggle_depth_mesh_focus_cube(cx);
        }
    }

    fn set_depth_mesh_focus_point(&mut self, cx: &mut Cx, point: Option<Vec3f>) {
        if let Some(mut root) = self.borrow_mut::<XrRoot>() {
            root.set_depth_mesh_focus_point(point);
            root.redraw(cx);
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct DesktopTankInput {
    left_stick: Vec2f,
    right_stick: Vec2f,
    left_trigger: f32,
    right_trigger: f32,
    a: f32,
    b: f32,
    x: f32,
}

pub struct TankSceneRuntime {
    tank_drive: TankDriveConfig,
    car_drive: CarDriveConfig,
    tank_turret_yaw: f32,
    tank_turret_pitch: f32,
    tank_pool_uids: Vec<WidgetUid>,
    tank_spawn_requested: bool,
    primary_tank_widget_uid: Option<WidgetUid>,
    tank_projectile_pool_uids: Vec<WidgetUid>,
    tank_projectile_cursor: usize,
    tank_projectile_next_emit_at: Option<f64>,
    tank_active_hit_projectiles: HashSet<WidgetUid>,
    tank_hit_flash_until: f64,
    last_desktop_tank_drive_at: Option<f64>,
    last_desktop_tank_reset_pressed: bool,
    desktop_tank_drive_armed: bool,
    tank_depth_focus_cube_auto_enabled: bool,
}

#[derive(Script, ScriptHook, Widget)]
pub struct Tank {
    #[rust]
    runtime: TankSceneRuntime,
    #[cast]
    #[deref]
    node: XrNode,
}

impl Default for TankSceneRuntime {
    fn default() -> Self {
        Self {
            tank_drive: TankDriveConfig::default(),
            car_drive: CarDriveConfig::default(),
            tank_turret_yaw: 0.0,
            tank_turret_pitch: 0.0,
            tank_pool_uids: Vec::new(),
            tank_spawn_requested: false,
            primary_tank_widget_uid: None,
            tank_projectile_pool_uids: Vec::new(),
            tank_projectile_cursor: 0,
            tank_projectile_next_emit_at: None,
            tank_active_hit_projectiles: HashSet::new(),
            tank_hit_flash_until: 0.0,
            last_desktop_tank_drive_at: None,
            last_desktop_tank_reset_pressed: false,
            desktop_tank_drive_armed: false,
            tank_depth_focus_cube_auto_enabled: false,
        }
    }
}

impl Tank {
    fn root_widget_ref(&self, cx: &Cx) -> WidgetRef {
        cx.widget_tree().widget(cx.widget_tree().root_uid())
    }
}

impl TankSceneRuntime {
    pub fn handle_event<H: TankSceneHost>(&mut self, host: &mut H, cx: &mut Cx, event: &Event) {
        if matches!(event, Event::GameInputConnected(_)) {
            self.on_game_input_connected();
        }
        if let Event::XrUpdate(update) = event {
            self.handle_xr_update(host, cx, update);
        } else if let Event::NextFrame(next_frame) = event {
            self.handle_next_frame(host, cx, next_frame, cx.in_xr_mode());
        }
    }

    pub fn on_game_input_connected(&mut self) {
        self.desktop_tank_drive_armed = false;
    }

    pub fn handle_xr_update<H: TankSceneHost>(
        &mut self,
        host: &mut H,
        cx: &mut Cx,
        update: &XrUpdateEvent,
    ) {
        self.last_desktop_tank_drive_at = None;
        self.drive_tank_for_update(host, cx, update);
    }

    pub fn handle_next_frame<H: TankSceneHost>(
        &mut self,
        host: &mut H,
        cx: &mut Cx,
        event: &NextFrameEvent,
        in_xr_mode: bool,
    ) {
        if in_xr_mode {
            self.last_desktop_tank_drive_at = None;
        } else {
            self.drive_tank_for_desktop_frame(host, cx, event);
        }
    }

    pub fn maintain<H: TankSceneHost>(&mut self, host: &mut H, cx: &mut Cx) {
        self.ensure_local_tank_spawned(host, cx);
        self.sync_tank_depth_mesh_focus(host, cx);
    }

    fn tank_physics_wheel_radius_meters() -> f32 {
        let support_base = TANK_BODY_HALF_WIDTH_METERS
            .min(TANK_BODY_HALF_HEIGHT_METERS)
            .min(TANK_BODY_HALF_DEPTH_METERS)
            .max(0.0005);
        (support_base * TANK_FOUR_WHEEL_RADIUS_SCALE).clamp(
            TANK_FOUR_WHEEL_RADIUS_MIN_METERS,
            TANK_FOUR_WHEEL_RADIUS_MAX_METERS,
        )
    }

    fn tank_physics_body_collider_bottom_meters() -> f32 {
        let physics_half_height = TANK_BODY_HALF_HEIGHT_METERS * 0.70;
        let physics_center_offset = TANK_BODY_HALF_HEIGHT_METERS * 0.20;
        physics_center_offset - physics_half_height
    }

    fn tank_physics_wheel_local_pose(index: usize) -> Option<Pose> {
        let x = if index < 2 {
            -TANK_WHEEL_LATERAL_OFFSET_METERS
        } else {
            TANK_WHEEL_LATERAL_OFFSET_METERS
        };
        let z = match index % 2 {
            0 => TANK_WHEEL_FRONT_OFFSET_METERS,
            1 => TANK_WHEEL_BACK_OFFSET_METERS,
            _ => return None,
        };
        Some(Pose::new(
            Quat::default(),
            vec3f(x, TANK_WHEEL_VERTICAL_OFFSET_METERS, z),
        ))
    }

    fn tank_physics_body_mount_lift_meters() -> f32 {
        -Self::tank_physics_body_collider_bottom_meters() + Self::tank_physics_wheel_radius_meters()
    }

    fn tank_support_world_metrics<H: TankSceneHost>(&self, host: &H, cx: &mut Cx) -> (f32, f32) {
        let scene_scale = self
            .tank_scene_spawn_basis(host, cx)
            .map(|(_, _, scale)| scale)
            .unwrap_or(vec3f(1.0, 1.0, 1.0));
        let half_extents = vec3f(
            TANK_BODY_HALF_WIDTH_METERS * scene_scale.x,
            TANK_BODY_HALF_HEIGHT_METERS * scene_scale.y,
            TANK_BODY_HALF_DEPTH_METERS * scene_scale.z,
        );
        let support_base = half_extents
            .x
            .min(half_extents.y)
            .min(half_extents.z)
            .max(0.0005);
        let radius = (support_base * TANK_FOUR_WHEEL_RADIUS_SCALE).clamp(
            TANK_FOUR_WHEEL_RADIUS_MIN_METERS,
            TANK_FOUR_WHEEL_RADIUS_MAX_METERS,
        );
        let rest_length = (radius * TANK_FOUR_WHEEL_REST_LENGTH_SCALE).clamp(
            TANK_FOUR_WHEEL_REST_LENGTH_MIN_METERS,
            TANK_FOUR_WHEEL_REST_LENGTH_MAX_METERS,
        );
        (radius, rest_length)
    }

    fn tank_spawn_support_clearance_meters<H: TankSceneHost>(
        &self,
        host: &H,
        cx: &mut Cx,
    ) -> f32 {
        let scene_scale_y = self
            .tank_scene_spawn_basis(host, cx)
            .map(|(_, _, scale)| scale.y.abs())
            .filter(|scale| *scale > 1.0e-4)
            .unwrap_or(1.0);
        let (support_radius_world, support_rest_world) = self.tank_support_world_metrics(host, cx);
        let support_radius_local = support_radius_world / scene_scale_y;
        let support_rest_local = support_rest_world / scene_scale_y;
        let preload_local = TANK_SPAWN_SUSPENSION_PRELOAD_WORLD_METERS / scene_scale_y;
        let extra_clearance_local = TANK_SPAWN_EXTRA_CLEARANCE_WORLD_METERS / scene_scale_y;
        TANK_PLATE_TOP_LOCAL_Y_METERS
            + TANK_BODY_HALF_HEIGHT_METERS
            + support_rest_local
            + support_radius_local
            + extra_clearance_local
            - preload_local
    }

    fn desktop_tank_input_is_neutral(input: DesktopTankInput) -> bool {
        input.right_stick.length() <= 0.16
            && input.left_stick.length() <= 0.16
            && input.left_trigger <= 0.08
            && input.right_trigger <= 0.08
            && input.a <= 0.5
            && input.b <= 0.5
            && input.x <= 0.5
    }

    fn tank_wheel_pivot_ref(cx: &mut Cx, tank_widget: &WidgetRef, index: usize) -> WidgetRef {
        match index {
            0 => tank_widget.widget(cx, ids!(tank_wheel_0)),
            1 => tank_widget.widget(cx, ids!(tank_wheel_1)),
            2 => tank_widget.widget(cx, ids!(tank_wheel_2)),
            3 => tank_widget.widget(cx, ids!(tank_wheel_3)),
            _ => WidgetRef::default(),
        }
    }

    fn tank_wheel_mesh_ref(cx: &mut Cx, tank_widget: &WidgetRef, index: usize) -> WidgetRef {
        match index {
            0 => tank_widget.widget(cx, ids!(tank_wheel_0.wheel_mesh)),
            1 => tank_widget.widget(cx, ids!(tank_wheel_1.wheel_mesh)),
            2 => tank_widget.widget(cx, ids!(tank_wheel_2.wheel_mesh)),
            3 => tank_widget.widget(cx, ids!(tank_wheel_3.wheel_mesh)),
            _ => WidgetRef::default(),
        }
    }

    fn sync_tank_wheel_widgets(
        &self,
        cx: &mut Cx,
        tank_widget: &WidgetRef,
        tank_body: &XrRuntimeBodyState,
    ) {
        for index in 0..TANK_WHEEL_COUNT {
            let Some(local_pose) = tank_body.linked_support_local_poses[index]
                .or_else(|| Self::tank_physics_wheel_local_pose(index))
            else {
                continue;
            };
            let steering = tank_body.linked_support_steer_angles[index].unwrap_or(0.0);
            let spin = tank_body.linked_support_spin_angles[index].unwrap_or(0.0);
            if let Some(mut pivot) = Self::tank_wheel_pivot_ref(cx, tank_widget, index)
                .borrow_mut::<XrNode>()
            {
                pivot.set_pos(cx, local_pose.position);
                pivot.set_rot(cx, vec3f(0.0, steering, 0.0));
            }
            if let Some(mut wheel) =
                Self::tank_wheel_mesh_ref(cx, tank_widget, index).borrow_mut::<IcoSphere>()
            {
                wheel.set_radius(cx, Self::tank_physics_wheel_radius_meters());
                wheel.set_rot(cx, vec3f(spin, 0.0, 0.0));
            }
        }
    }

    fn tank_body_visual_lift(tank_body: &XrRuntimeBodyState) -> f32 {
        let mut wheel_y_sum = 0.0;
        let mut wheel_count = 0.0;
        for index in 0..TANK_WHEEL_COUNT {
            if let Some(local_pose) = tank_body.linked_support_local_poses[index]
                .or_else(|| Self::tank_physics_wheel_local_pose(index))
            {
                wheel_y_sum += local_pose.position.y;
                wheel_count += 1.0;
            }
        }
        let average_wheel_y = if wheel_count > 0.0 {
            wheel_y_sum / wheel_count
        } else {
            TANK_WHEEL_VERTICAL_OFFSET_METERS
        };
        (Self::tank_physics_body_mount_lift_meters()
            + (TANK_WHEEL_VERTICAL_OFFSET_METERS - average_wheel_y)
                * TANK_BODY_VISUAL_SUSPENSION_RESPONSE)
            .clamp(
                TANK_BODY_VISUAL_LIFT_MIN_METERS,
                TANK_BODY_VISUAL_LIFT_MAX_METERS,
            )
    }

    fn rotation_quat(rot: Vec3f) -> Quat {
        let x = Quat::from_axis_angle(vec3f(1.0, 0.0, 0.0), rot.x);
        let y = Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), rot.y);
        let z = Quat::from_axis_angle(vec3f(0.0, 0.0, 1.0), rot.z);
        Quat::multiply(&z, &Quat::multiply(&y, &x))
    }

    #[cfg(test)]
    fn quat_to_rot(quat: Quat) -> Vec3f {
        let x = (2.0 * (quat.w * quat.x - quat.y * quat.z))
            .atan2(1.0 - 2.0 * (quat.x * quat.x + quat.y * quat.y));
        let y = (2.0 * (quat.x * quat.z + quat.w * quat.y))
            .clamp(-1.0, 1.0)
            .asin();
        let z = (2.0 * (quat.w * quat.z - quat.x * quat.y))
            .atan2(1.0 - 2.0 * (quat.y * quat.y + quat.z * quat.z));
        vec3f(x, y, z)
    }

    fn transform_basis_with_node(
        parent_pos: Vec3f,
        parent_ori: Quat,
        parent_scale: Vec3f,
        node: &XrNode,
    ) -> (Vec3f, Quat, Vec3f) {
        let local_pos = vec3f(
            node.pos().x * parent_scale.x,
            node.pos().y * parent_scale.y,
            node.pos().z * parent_scale.z,
        );
        let rotated_pos = parent_ori.rotate_vec3(&local_pos);
        let orientation = Quat::multiply(&Self::rotation_quat(node.rot()), &parent_ori);
        let scale = vec3f(
            parent_scale.x * node.scale().x,
            parent_scale.y * node.scale().y,
            parent_scale.z * node.scale().z,
        );
        (parent_pos + rotated_pos, orientation, scale)
    }

    fn transform_pose_with_basis(
        parent_pos: Vec3f,
        parent_ori: Quat,
        parent_scale: Vec3f,
        local_pose: Pose,
    ) -> Pose {
        let scaled_pos = vec3f(
            local_pose.position.x * parent_scale.x,
            local_pose.position.y * parent_scale.y,
            local_pose.position.z * parent_scale.z,
        );
        Pose::new(
            Quat::multiply(&local_pose.orientation, &parent_ori),
            parent_pos + parent_ori.rotate_vec3(&scaled_pos),
        )
    }

    fn current_activity<H: TankSceneHost>(&self, host: &H, cx: &mut Cx) -> Option<XrActivityId> {
        host.ui_root()
            .widget(cx, ids!(scene_select))
            .borrow::<XrSelect>()
            .map(|select| select.activity_id())
    }

    fn is_tanks_scene_active<H: TankSceneHost>(&self, host: &H, cx: &mut Cx) -> bool {
        self.current_activity(host, cx) == Some(XrActivityId(live_id!(tanks_scene)))
    }

    fn tank_scene_spawn_basis<H: TankSceneHost>(
        &self,
        host: &H,
        cx: &mut Cx,
    ) -> Option<(Vec3f, Quat, Vec3f)> {
        let ui = host.ui_root();
        let scene_select = ui.widget(cx, ids!(scene_select));
        let (select_pos, select_ori, select_scale) =
            xr_widget_with_scene_node(&scene_select, |node| {
                (node.pos(), Self::rotation_quat(node.rot()), node.scale())
            })?;
        let tanks_scene = scene_select.widget(cx, ids!(tanks_scene));
        xr_widget_with_scene_node(&tanks_scene, |node| {
            Self::transform_basis_with_node(select_pos, select_ori, select_scale, node)
        })
    }

    fn collect_spawn_pool_widget_uids(widget: &WidgetRef, pool_uids: &mut Vec<WidgetUid>) {
        if !widget.visible() {
            return;
        }
        xr_widget_with_scene_node(widget, |node| {
            if node.spawn_pool() {
                pool_uids.push(widget.widget_uid());
            }
        });
        xr_widget_children(widget, &mut |_, child| {
            Self::collect_spawn_pool_widget_uids(&child, pool_uids)
        });
    }

    fn find_widget_by_uid(widget: &WidgetRef, target: WidgetUid) -> Option<WidgetRef> {
        if widget.widget_uid() == target {
            return Some(widget.clone());
        }
        let mut found = None;
        xr_widget_children(widget, &mut |_, child| {
            if found.is_none() {
                found = Self::find_widget_by_uid(&child, target);
            }
        });
        found
    }

    fn refresh_tank_pool<H: TankSceneHost>(&mut self, host: &H, cx: &mut Cx) {
        self.tank_pool_uids.clear();
        let tank_slots = host.ui_root().widget(cx, ids!(tank_slots));
        if tank_slots.borrow::<XrNode>().is_none() {
            return;
        }
        Self::collect_spawn_pool_widget_uids(&tank_slots, &mut self.tank_pool_uids);
    }

    fn local_tank_widget_uid<H: TankSceneHost>(
        &mut self,
        host: &H,
        cx: &mut Cx,
    ) -> Option<WidgetUid> {
        if self.tank_pool_uids.is_empty() {
            self.refresh_tank_pool(host, cx);
        }
        if let Some(primary) = self.primary_tank_widget_uid {
            if host.widget_is_local_shared_object(cx, primary) {
                return Some(primary);
            }
            let has_any_local_shared = self
                .tank_pool_uids
                .iter()
                .copied()
                .any(|widget_uid| host.widget_is_local_shared_object(cx, widget_uid));
            if !has_any_local_shared && host.runtime_body_state(primary).is_some() {
                return Some(primary);
            }
        }
        self.tank_pool_uids
            .iter()
            .copied()
            .find(|widget_uid| host.widget_is_local_shared_object(cx, *widget_uid))
    }

    fn local_tank_body_state<H: TankSceneHost>(
        &mut self,
        host: &H,
        cx: &mut Cx,
    ) -> Option<(WidgetUid, XrRuntimeBodyState)> {
        let tank_widget_uid = self.local_tank_widget_uid(host, cx)?;
        host.runtime_body_state(tank_widget_uid)
            .map(|body| (tank_widget_uid, body))
    }

    fn tank_widget_ref<H: TankSceneHost>(
        &self,
        host: &H,
        cx: &mut Cx,
        widget_uid: WidgetUid,
    ) -> Option<WidgetRef> {
        let tank_slots = host.ui_root().widget(cx, ids!(tank_slots));
        if tank_slots.borrow::<XrNode>().is_none() {
            return None;
        }
        Self::find_widget_by_uid(&tank_slots, widget_uid)
    }

    fn tank_spawn_pose<H: TankSceneHost>(&self, host: &H, cx: &mut Cx) -> Pose {
        let support_clearance = self.tank_spawn_support_clearance_meters(host, cx);
        let peer_id = host.local_peer_id(cx).unwrap_or_default();
        let hash = peer_id
            .0
            .wrapping_mul(0x9e37_79b9)
            .wrapping_add(0x7f4a_7c15);
        let angle = ((hash & 1023) as f32 / 1024.0) * std::f32::consts::TAU;
        let radius =
            TANK_SPAWN_RING_RADIUS_METERS + (((hash >> 10) & 63) as f32 / 63.0 - 0.5) * 0.015;
        let local_pose = Pose::new(
            Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), angle + std::f32::consts::PI),
            vec3f(
                angle.cos() * radius,
                support_clearance,
                angle.sin() * radius,
            ),
        );
        if let Some((scene_pos, scene_ori, scene_scale)) = self.tank_scene_spawn_basis(host, cx) {
            Self::transform_pose_with_basis(scene_pos, scene_ori, scene_scale, local_pose)
        } else {
            local_pose
        }
    }

    fn tank_reset_pose_from_controller<H: TankSceneHost>(
        &self,
        host: &H,
        cx: &mut Cx,
        controller: &XrController,
    ) -> Pose {
        let support_clearance = self.tank_spawn_support_clearance_meters(host, cx);
        let pose = controller.grip_pose;
        if !controller.active() || !pose.is_finite() {
            return self.tank_spawn_pose(host, cx);
        }
        let mut forward = pose.orientation.rotate_vec3(&vec3f(0.0, 0.0, 1.0));
        forward.y = 0.0;
        let yaw = if forward.length() > 1.0e-4 {
            forward = forward.normalize();
            forward.x.atan2(forward.z)
        } else {
            0.0
        };
        Pose::new(
            Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), yaw),
            pose.position + vec3f(0.0, support_clearance, 0.0),
        )
    }

    fn ensure_local_tank_spawned<H: TankSceneHost>(&mut self, host: &mut H, cx: &mut Cx) {
        if !self.is_tanks_scene_active(host, cx) {
            self.tank_active_hit_projectiles.clear();
            self.tank_spawn_requested = false;
            return;
        }
        if let Some((widget_uid, _)) = self.local_tank_body_state(host, cx) {
            self.primary_tank_widget_uid = Some(widget_uid);
            self.tank_spawn_requested = false;
            return;
        }
        let scene_ready = host
            .ui_root()
            .borrow::<XrRoot>()
            .is_some_and(|root| root.physics_scene_body_count() > 0);
        let shared_state_ready = host
            .ui_root()
            .widget(cx, ids!(xr_peer_sync))
            .borrow::<XrPeerSync>()
            .is_some_and(|peer_sync| {
                peer_sync.enabled()
                    && peer_sync.spawnable_activity() == Some(XrActivityId(live_id!(tanks_scene)))
            });
        if !scene_ready || !shared_state_ready || self.tank_spawn_requested {
            return;
        }
        if self.tank_pool_uids.is_empty() {
            self.refresh_tank_pool(host, cx);
        }
        let Some(widget_uid) = self
            .primary_tank_widget_uid
            .or_else(|| self.tank_pool_uids.first().copied())
        else {
            return;
        };
        let spawn_pose = self.tank_spawn_pose(host, cx);
        let widget_uid = host.emit_local_shared_body_spawn_exact(
            cx,
            XrBodySpawn {
                widget_uid,
                shadow: false,
                mode: XrSharedObjectMode::Dynamic,
                pose: spawn_pose,
                linvel: vec3f(0.0, 0.0, 0.0),
                angvel: vec3f(0.0, 0.0, 0.0),
            },
        );
        self.tank_spawn_requested = true;
        self.primary_tank_widget_uid = Some(widget_uid);
    }

    fn sync_tank_depth_mesh_focus<H: TankSceneHost>(&mut self, host: &mut H, cx: &mut Cx) {
        let focus_point = if self.is_tanks_scene_active(host, cx) {
            self.local_tank_body_state(host, cx)
                .map(|(_, body)| body.pose.position)
                .filter(|position| position.is_finite())
        } else {
            None
        };
        if focus_point.is_some() {
            if !host.depth_mesh_focus_cube_enabled() {
                host.toggle_depth_mesh_focus_cube(cx);
                self.tank_depth_focus_cube_auto_enabled = true;
            }
        } else if self.tank_depth_focus_cube_auto_enabled && host.depth_mesh_focus_cube_enabled() {
            host.toggle_depth_mesh_focus_cube(cx);
            self.tank_depth_focus_cube_auto_enabled = false;
        }
        host.set_depth_mesh_focus_point(cx, focus_point);
    }

    fn reset_local_tank<H: TankSceneHost>(&mut self, host: &mut H, cx: &mut Cx) -> bool {
        let spawn_pose = self.tank_spawn_pose(host, cx);
        self.reset_local_tank_at_pose(host, cx, spawn_pose)
    }

    fn reset_local_tank_at_pose<H: TankSceneHost>(
        &mut self,
        host: &mut H,
        cx: &mut Cx,
        spawn_pose: Pose,
    ) -> bool {
        if !self.is_tanks_scene_active(host, cx) {
            return false;
        }
        if self.tank_pool_uids.is_empty() {
            self.refresh_tank_pool(host, cx);
        }
        let Some(widget_uid) = self
            .local_tank_widget_uid(host, cx)
            .or(self.primary_tank_widget_uid)
            .or_else(|| self.tank_pool_uids.first().copied())
        else {
            return false;
        };
        self.tank_turret_yaw = 0.0;
        self.tank_turret_pitch = 0.0;
        self.tank_projectile_next_emit_at = None;
        self.tank_active_hit_projectiles.clear();
        self.tank_hit_flash_until = 0.0;
        let widget_uid = host.emit_local_shared_body_spawn_exact(
            cx,
            XrBodySpawn {
                widget_uid,
                shadow: false,
                mode: XrSharedObjectMode::Dynamic,
                pose: spawn_pose,
                linvel: vec3f(0.0, 0.0, 0.0),
                angvel: vec3f(0.0, 0.0, 0.0),
            },
        );
        self.tank_spawn_requested = true;
        self.primary_tank_widget_uid = Some(widget_uid);
        cx.redraw_all();
        true
    }

    fn refresh_tank_projectile_pool<H: TankSceneHost>(&mut self, host: &H, cx: &mut Cx) {
        self.tank_projectile_pool_uids.clear();
        let projectile_root = host.ui_root().widget(cx, ids!(tank_projectiles));
        if projectile_root.borrow::<XrNode>().is_none() {
            self.tank_projectile_cursor = 0;
            return;
        }
        Self::collect_spawn_pool_widget_uids(&projectile_root, &mut self.tank_projectile_pool_uids);
        if self.tank_projectile_pool_uids.is_empty() {
            self.tank_projectile_cursor = 0;
        } else {
            self.tank_projectile_cursor %= self.tank_projectile_pool_uids.len();
        }
    }

    fn next_tank_projectile_widget_uid<H: TankSceneHost>(
        &mut self,
        host: &H,
        cx: &mut Cx,
    ) -> Option<WidgetUid> {
        if self.tank_projectile_pool_uids.is_empty() {
            self.refresh_tank_projectile_pool(host, cx);
        }
        let len = self.tank_projectile_pool_uids.len();
        if len == 0 {
            return None;
        }
        let widget_uid = self.tank_projectile_pool_uids[self.tank_projectile_cursor % len];
        self.tank_projectile_cursor = (self.tank_projectile_cursor + 1) % len;
        Some(widget_uid)
    }

    fn sync_local_tank_widgets<H: TankSceneHost>(
        &mut self,
        host: &mut H,
        cx: &mut Cx,
        now: f64,
    ) {
        let ui = host.ui_root();
        let Some((tank_widget_uid, tank_body)) = self.local_tank_body_state(host, cx) else {
            ui.widget(cx, ids!(scene_status))
                .set_text(cx, TANK_SCENE_STATUS_TEXT);
            return;
        };
        let Some(tank_widget) = self.tank_widget_ref(host, cx, tank_widget_uid) else {
            return;
        };
        if let Some(mut body_mount) = tank_widget
            .widget(cx, ids!(tank_body_mount))
            .borrow_mut::<XrNode>()
        {
            body_mount.set_pos(cx, vec3f(0.0, Self::tank_body_visual_lift(&tank_body), 0.0));
        }
        if let Some(mut turret) = tank_widget
            .widget(cx, ids!(tank_turret_yaw))
            .borrow_mut::<XrNode>()
        {
            turret.set_rot(cx, vec3f(0.0, self.tank_turret_yaw, 0.0));
        }
        if let Some(mut barrel) = tank_widget
            .widget(cx, ids!(tank_barrel_pitch))
            .borrow_mut::<XrNode>()
        {
            barrel.set_rot(cx, vec3f(self.tank_turret_pitch, 0.0, 0.0));
        }
        if let Some(mut hull) = tank_widget.widget(cx, ids!(hull_block)).borrow_mut::<Cube>() {
            let color = if now < self.tank_hit_flash_until {
                vec4f(0.98, 0.36, 0.26, 1.0)
            } else {
                vec4f(0.4157, 0.5137, 0.2157, 1.0)
            };
            hull.set_color(cx, color);
        }
        self.sync_tank_wheel_widgets(cx, &tank_widget, &tank_body);
        let status = if now < self.tank_hit_flash_until {
            format!("Tank hit by a remote shell. {TANK_SCENE_STATUS_TEXT}")
        } else {
            TANK_SCENE_STATUS_TEXT.to_string()
        };
        ui.widget(cx, ids!(scene_status)).set_text(cx, &status);
    }

    fn update_tank_turret_with_controller<H: TankSceneHost>(
        &mut self,
        host: &mut H,
        cx: &mut Cx,
        controller: &XrController,
        dt: f32,
    ) {
        if self.local_tank_body_state(host, cx).is_none() {
            return;
        }
        let (pitch_input, yaw_input) = tank_stick_axes(controller.stick, self.tank_drive);
        let dt = dt.clamp(1.0 / 240.0, 0.1);
        self.tank_turret_yaw = (self.tank_turret_yaw + yaw_input * TANK_TURRET_YAW_SPEED_RADPS * dt)
            .rem_euclid(std::f32::consts::TAU);
        self.tank_turret_pitch = (self.tank_turret_pitch
            + pitch_input * TANK_TURRET_PITCH_SPEED_RADPS * dt)
            .clamp(TANK_TURRET_PITCH_MIN_RAD, TANK_TURRET_PITCH_MAX_RAD);
    }

    fn detect_local_tank_hits<H: TankSceneHost>(
        &mut self,
        host: &mut H,
        cx: &mut Cx,
        now: f64,
    ) {
        let Some(local_tank_widget_uid) = self.local_tank_widget_uid(host, cx) else {
            self.tank_active_hit_projectiles.clear();
            return;
        };
        let Some(local_authority) =
            host.shared_object_authority_for_widget(cx, local_tank_widget_uid)
        else {
            self.tank_active_hit_projectiles.clear();
            return;
        };
        if self.tank_projectile_pool_uids.is_empty() {
            self.refresh_tank_projectile_pool(host, cx);
        }
        let projectile_pool: HashSet<WidgetUid> =
            self.tank_projectile_pool_uids.iter().copied().collect();
        let Some(runtime_contacts) = host.runtime_contacts() else {
            return;
        };
        let mut active_projectiles = HashSet::new();
        for &(left, right) in runtime_contacts.iter() {
            let projectile_uid =
                if left == local_tank_widget_uid && projectile_pool.contains(&right) {
                    Some(right)
                } else if right == local_tank_widget_uid && projectile_pool.contains(&left) {
                    Some(left)
                } else {
                    None
                };
            let Some(projectile_uid) = projectile_uid else {
                continue;
            };
            let Some(projectile_authority) =
                host.shared_object_authority_for_widget(cx, projectile_uid)
            else {
                continue;
            };
            if projectile_authority == local_authority {
                continue;
            }
            if self.tank_active_hit_projectiles.insert(projectile_uid) {
                self.tank_hit_flash_until = now + TANK_HIT_FLASH_SECONDS;
                log!(
                    "tank hit: local authority {:08x} hit by projectile {:016x} from {:08x}",
                    local_authority.0,
                    projectile_uid.0,
                    projectile_authority.0
                );
            }
            active_projectiles.insert(projectile_uid);
        }
        self.tank_active_hit_projectiles = active_projectiles;
    }

    fn emit_tank_projectiles<H: TankSceneHost>(
        &mut self,
        host: &mut H,
        cx: &mut Cx,
        now: f64,
        fire_active: bool,
    ) {
        if !fire_active {
            self.tank_projectile_next_emit_at = None;
            return;
        }
        let Some((_, tank_body)) = self.local_tank_body_state(host, cx) else {
            self.tank_projectile_next_emit_at = None;
            return;
        };

        let interval = (1.0 / TANK_PROJECTILE_RATE_HZ).clamp(0.01, 10.0) as f64;
        let tank_orientation = tank_body.pose.orientation;
        let turret_orientation = Quat::multiply(
            &Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), self.tank_turret_yaw),
            &tank_orientation,
        );
        let barrel_orientation = Quat::multiply(
            &Quat::from_axis_angle(vec3f(1.0, 0.0, 0.0), self.tank_turret_pitch),
            &turret_orientation,
        );
        let tank_position = tank_body.pose.position;
        let tank_scale = tank_body.scale;
        let scale_local = |offset: Vec3f| {
            vec3f(
                offset.x * tank_scale.x,
                offset.y * tank_scale.y,
                offset.z * tank_scale.z,
            )
        };
        let turret_mount =
            tank_position + tank_orientation.rotate_vec3(&scale_local(vec3f(0.0, 0.08, 0.015)));
        let barrel_pivot =
            turret_mount + turret_orientation.rotate_vec3(&scale_local(vec3f(0.0, 0.0, 0.08)));
        let barrel_tip =
            barrel_pivot + barrel_orientation.rotate_vec3(&scale_local(vec3f(0.0, 0.0, 0.28)));
        let direction = barrel_orientation
            .rotate_vec3(&vec3f(0.0, 0.0, 1.0))
            .normalize();
        let projectile_radius =
            TANK_PROJECTILE_RADIUS_METERS * tank_scale.x.min(tank_scale.y).min(tank_scale.z).max(0.0001);
        let mut next_emit_at = self.tank_projectile_next_emit_at.unwrap_or(now);
        let mut emitted = 0usize;

        while now >= next_emit_at && emitted < TANK_PROJECTILE_MAX_EMITS_PER_UPDATE {
            let Some(widget_uid) = self.next_tank_projectile_widget_uid(host, cx) else {
                self.tank_projectile_next_emit_at = None;
                return;
            };
            let _ = host.emit_local_shared_body_spawn(
                cx,
                XrBodySpawn {
                    widget_uid,
                    shadow: false,
                    mode: XrSharedObjectMode::Dynamic,
                    pose: Pose::new(
                        barrel_orientation,
                        barrel_tip + direction * projectile_radius,
                    ),
                    linvel: tank_body.linvel + direction * TANK_PROJECTILE_SPEED_MPS,
                    angvel: vec3f(0.0, 0.0, 0.0),
                },
            );
            cx.redraw_all();
            next_emit_at += interval;
            emitted += 1;
        }

        self.tank_projectile_next_emit_at = Some(next_emit_at);
    }

    fn desktop_gamepad_tank_input(&self, cx: &mut Cx) -> Option<DesktopTankInput> {
        let mut best_input = None;
        let mut best_score = 0.0f32;
        for state in cx.game_input_states() {
            let GameInputState::Gamepad(gamepad) = state else {
                continue;
            };
            let input = DesktopTankInput {
                left_stick: vec2f(gamepad.left_stick.x as f32, gamepad.left_stick.y as f32),
                right_stick: vec2f(gamepad.right_stick.x as f32, gamepad.right_stick.y as f32),
                left_trigger: gamepad.left_trigger as f32,
                right_trigger: gamepad.right_trigger as f32,
                a: gamepad.a as f32,
                b: gamepad.b as f32,
                x: gamepad.x as f32,
            };
            let score = input.left_stick.length() * 2.0
                + input.right_stick.length()
                + input.left_trigger.max(input.right_trigger)
                + input.a.max(input.x)
                + input.b;
            if score > best_score {
                best_input = Some(input);
                best_score = score;
            }
        }
        best_input
    }

    fn drive_tank_with_controllers<H: TankSceneHost>(
        &mut self,
        host: &mut H,
        cx: &mut Cx,
        right_controller: &XrController,
        left_controller: &XrController,
    ) {
        let Some((tank_widget_uid, body)) = self.local_tank_body_state(host, cx) else {
            return;
        };
        let control = car_drive_command(
            tank_widget_uid,
            body.held_by,
            left_controller.stick,
            right_controller.trigger,
            left_controller.trigger,
            self.car_drive,
        );
        let forced_dynamic =
            control.is_some() && body.held_by.is_none() && (!body.dynamic_body || body.shadowed);
        if forced_dynamic {
            let widget_uid = host.emit_local_shared_body_spawn_exact(
                cx,
                XrBodySpawn {
                    widget_uid: tank_widget_uid,
                    shadow: false,
                    mode: XrSharedObjectMode::Dynamic,
                    pose: body.pose,
                    linvel: body.linvel,
                    angvel: body.angvel,
                },
            );
            self.primary_tank_widget_uid = Some(widget_uid);
            self.tank_spawn_requested = true;
        }
        if let Some(control) = control {
            host.apply_car_control(cx, control);
        }
    }

    fn drive_tank_for_update<H: TankSceneHost>(
        &mut self,
        host: &mut H,
        cx: &mut Cx,
        update: &XrUpdateEvent,
    ) {
        if update.clicked_b() {
            let spawn_pose =
                self.tank_reset_pose_from_controller(host, cx, &update.state.right_controller);
            self.reset_local_tank_at_pose(host, cx, spawn_pose);
            self.sync_local_tank_widgets(host, cx, update.state.time);
            return;
        }
        let dt = (update.state.time - update.last.time).clamp(1.0 / 240.0, 0.1) as f32;
        self.drive_tank_with_controllers(
            host,
            cx,
            &update.state.right_controller,
            &update.state.left_controller,
        );
        self.update_tank_turret_with_controller(host, cx, &update.state.right_controller, dt);
        self.emit_tank_projectiles(
            host,
            cx,
            update.state.time,
            update.state.left_controller.click_a()
                || update.state.left_controller.click_x()
                || update.state.right_controller.click_a()
                || update.state.right_controller.click_x(),
        );
        self.detect_local_tank_hits(host, cx, update.state.time);
        self.sync_local_tank_widgets(host, cx, update.state.time);
    }

    fn drive_tank_for_desktop_frame<H: TankSceneHost>(
        &mut self,
        host: &mut H,
        cx: &mut Cx,
        event: &NextFrameEvent,
    ) {
        let dt = self
            .last_desktop_tank_drive_at
            .map(|last| (event.time - last) as f32)
            .unwrap_or(1.0 / 60.0);
        self.last_desktop_tank_drive_at = Some(event.time);
        let Some(input) = self.desktop_gamepad_tank_input(cx) else {
            self.last_desktop_tank_reset_pressed = false;
            self.desktop_tank_drive_armed = false;
            self.emit_tank_projectiles(host, cx, event.time, false);
            self.detect_local_tank_hits(host, cx, event.time);
            self.sync_local_tank_widgets(host, cx, event.time);
            return;
        };
        if !self.desktop_tank_drive_armed {
            if Self::desktop_tank_input_is_neutral(input) {
                self.desktop_tank_drive_armed = true;
            } else {
                self.last_desktop_tank_reset_pressed = false;
                self.emit_tank_projectiles(host, cx, event.time, false);
                self.detect_local_tank_hits(host, cx, event.time);
                self.sync_local_tank_widgets(host, cx, event.time);
                return;
            }
        }
        let reset_pressed = input.b > 0.5;
        let reset_clicked = reset_pressed && !self.last_desktop_tank_reset_pressed;
        self.last_desktop_tank_reset_pressed = reset_pressed;
        if reset_clicked {
            self.reset_local_tank(host, cx);
            self.desktop_tank_drive_armed = false;
            self.sync_local_tank_widgets(host, cx, event.time);
            return;
        }
        let right_controller = XrController {
            stick: input.right_stick,
            trigger: input.right_trigger,
            buttons: if input.a > 0.5 {
                XrController::CLICK_A
            } else {
                0
            },
            ..XrController::default()
        };
        let left_controller = XrController {
            stick: input.left_stick,
            trigger: input.left_trigger,
            buttons: if input.x > 0.5 {
                XrController::CLICK_X
            } else {
                0
            },
            ..XrController::default()
        };
        self.drive_tank_with_controllers(host, cx, &right_controller, &left_controller);
        self.update_tank_turret_with_controller(host, cx, &right_controller, dt);
        self.emit_tank_projectiles(
            host,
            cx,
            event.time,
            left_controller.click_a()
                || left_controller.click_x()
                || right_controller.click_a()
                || right_controller.click_x(),
        );
        self.detect_local_tank_hits(host, cx, event.time);
        self.sync_local_tank_widgets(host, cx, event.time);
    }
}

impl Widget for Tank {
    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        args: ScriptValue,
    ) -> ScriptAsyncResult {
        self.node.script_call(vm, method, args)
    }

    fn script_result(&mut self, vm: &mut ScriptVm, id: ScriptAsyncId, result: ScriptValue) {
        self.node.script_result(vm, id, result);
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.node.handle_event(cx, event, scope);
    }

    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        self.node.draw_3d(cx, scope)
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }
}

impl Tank {
    pub fn pre_ui_event(&mut self, cx: &mut Cx, event: &Event) {
        let mut root = self.root_widget_ref(cx);
        self.runtime.handle_event(&mut root, cx, event);
    }

    pub fn post_ui_event(&mut self, cx: &mut Cx) {
        let mut root = self.root_widget_ref(cx);
        self.runtime.maintain(&mut root, cx);
    }
}

pub fn tank_drive_command(
    widget_uid: WidgetUid,
    pose: Pose,
    held_by: Option<XrSharedHand>,
    controller: &XrController,
    config: TankDriveConfig,
) -> Option<XrBodyDrive> {
    if held_by.is_some() {
        return None;
    }
    let (forward, turn) = tank_stick_axes(controller.stick, config);
    let body_forward = flat_forward(pose.orientation);
    let world_up = vec3f(0.0, 1.0, 0.0);
    let target_linvel = body_forward * (forward * config.max_speed_mps.max(0.0));
    let target_angvel =
        world_up * (turn * config.turn_gain.max(0.0) * config.max_yaw_speed_radps.max(0.0));
    Some(XrBodyDrive {
        widget_uid,
        target_linvel,
        target_angvel,
        max_linear_accel: config.max_linear_accel_mps2.max(0.0),
        max_angular_accel: config.max_angular_accel_radps2.max(0.0),
        preserve_vertical_linvel: true,
    })
}

pub fn tank_stick_axes(stick: Vec2f, config: TankDriveConfig) -> (f32, f32) {
    stick_deadzone_scaled_axes(stick, config.stick_deadzone, config.stick_response_exponent)
}

fn stick_deadzone_scaled_axes(stick: Vec2f, deadzone: f32, exponent: f32) -> (f32, f32) {
    (
        deadzone_scaled_axis(-stick.y, deadzone, exponent),
        deadzone_scaled_axis(-stick.x, deadzone, exponent),
    )
}

fn deadzone_scaled_axis(value: f32, deadzone: f32, exponent: f32) -> f32 {
    let deadzone = deadzone.clamp(0.0, 0.95);
    if !value.is_finite() {
        return 0.0;
    }
    let magnitude = value.abs();
    if magnitude <= deadzone {
        return 0.0;
    }
    let scaled = ((magnitude - deadzone) / (1.0 - deadzone)).clamp(0.0, 1.0);
    let response = scaled.powf(exponent.clamp(1.0, 3.0));
    response.copysign(value)
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

#[cfg(test)]
include!("../tests/obj/tank.rs");

#[cfg(test)]
mod tests {
    use super::*;

    fn quat_close(a: Quat, b: Quat, tolerance: f32) -> bool {
        let dot = a.dot(b).abs();
        (1.0 - dot) <= tolerance
    }

    #[test]
    fn quat_to_rot_round_trips_x_then_y_then_z_node_rotations() {
        for rotation in [
            vec3f(0.0, 0.0, 0.0),
            vec3f(0.35, 0.0, 0.0),
            vec3f(0.0, -0.42, 0.0),
            vec3f(0.0, 0.0, 0.61),
            vec3f(0.37, -0.48, 0.29),
            vec3f(-1.10, 0.43, 0.72),
        ] {
            let quat = TankSceneRuntime::rotation_quat(rotation);
            let recovered = TankSceneRuntime::quat_to_rot(quat);
            let roundtrip = TankSceneRuntime::rotation_quat(recovered);
            assert!(
                quat_close(quat, roundtrip, 1.0e-4),
                "wheel/node quaternion conversion should preserve mixed-axis orientation: rotation={rotation:?} recovered={recovered:?} quat={quat:?} roundtrip={roundtrip:?}",
            );
        }
    }
}
