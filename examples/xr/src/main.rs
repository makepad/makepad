pub use makepad_widgets;

use makepad_physics::{PhysicsOp, PhysicsWorld, RigidBody};
use makepad_widgets::makepad_platform::permission::{Permission, PermissionStatus};
use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.XrSceneBase = #(XrScene::register_widget(vm))
    mod.widgets.XrScene = set_type_default() do mod.widgets.XrSceneBase{
        draw_cube +: {}
        draw_pbr +: {
            light_dir: vec3(0.35, 0.8, 0.45)
            light_color: vec3(1.0, 1.0, 1.0)
            ambient: 0.25
            spec_power: 128.0
            spec_strength: 0.9
            env_intensity: 1.8
            vertex: fn() {
                let local_uv = vec2(self.geom.ny_nz_uv.z, self.geom.ny_nz_uv.w);
                let local_pos_src = vec3(self.geom.pos_nx.x, self.geom.pos_nx.y, self.geom.pos_nx.z);
                let displacement = self.get_vertex_displacement(local_uv, local_pos_src);
                let local_scale = self.local_scale;
                let scaled_local_pos = vec3(
                    (local_pos_src.x + displacement.x) * local_scale.x,
                    (local_pos_src.y + displacement.y) * local_scale.y,
                    (local_pos_src.z + displacement.z) * local_scale.z
                );
                let safe_scale = vec3(
                    max(abs(local_scale.x), 0.000001),
                    max(abs(local_scale.y), 0.000001),
                    max(abs(local_scale.z), 0.000001)
                );
                let model_view = self.draw_list.view_transform * self.model_matrix;
                let model_pos = model_view * vec4(
                    scaled_local_pos.x,
                    scaled_local_pos.y,
                    scaled_local_pos.z,
                    1.0
                );
                let model_n = model_view * vec4(
                    self.geom.pos_nx.w / safe_scale.x,
                    self.geom.ny_nz_uv.x / safe_scale.y,
                    self.geom.ny_nz_uv.y / safe_scale.z,
                    0.0
                );
                let model_t = model_view * vec4(
                    self.geom.tangent.x / safe_scale.x,
                    self.geom.tangent.y / safe_scale.y,
                    self.geom.tangent.z / safe_scale.z,
                    0.0
                );

                self.v_world = vec3(model_pos.x, model_pos.y, model_pos.z);
                self.v_normal = normalize(vec3(model_n.x, model_n.y, model_n.z));
                self.v_tangent = vec4(normalize(vec3(model_t.x, model_t.y, model_t.z)), self.geom.tangent.w);
                self.v_uv = local_uv;
                self.v_color = self.geom.color;

                self.v_world_clip = vec4(model_pos.x, model_pos.y, model_pos.z + self.draw_call.zbias, 1.0);
                let view_pos = self.draw_pass.camera_view * self.v_world_clip;
                self.v_view_pos = vec3(view_pos.x, view_pos.y, view_pos.z);
                self.vertex_pos = self.draw_pass.camera_projection * view_pos;
            }
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1280, 820)
                body +: {
                    phase_view := AdaptiveView{
                        width: Fill
                        height: Fill
                        retain_unused_variants: false

                        Preflight := View{
                            width: Fill
                            height: Fill
                            flow: Down
                            align: Align{x: 0.5 y: 0.5}
                            padding: Inset{left: 36 right: 36 top: 36 bottom: 36}
                            spacing: 14
                            show_bg: true
                            draw_bg +: {
                                color_top: uniform(#x0b1422)
                                color_bottom: uniform(#x051018)
                                color_glow: uniform(#x1b4663)
                                pixel: fn() {
                                    let uv = self.pos;
                                    let base = mix(self.color_top, self.color_bottom, uv.y);
                                    let glow = smoothstep(0.72, 0.0, length(uv - vec2(0.18, 0.24)));
                                    return mix(base, self.color_glow, glow * 0.24);
                                }
                            }

                            panel := RoundedView{
                                width: 560
                                height: Fit
                                flow: Down
                                spacing: 10
                                padding: Inset{left: 22 right: 22 top: 20 bottom: 20}
                                draw_bg.color: #x09131cdd
                                draw_bg.radius: 16.0

                                title := H1{
                                    text: "XR Preflight"
                                    draw_text.color: #xeff7ff
                                }

                                detail_label := Label{
                                    width: Fill
                                    text: "Allow Quest scene access here before starting XR. The passthrough depth path uses Meta's scene permission for environment depth and occlusion."
                                    draw_text.color: #xb8c8d8
                                }

                                View{
                                    width: Fill
                                    height: Fit
                                    flow: Right
                                    spacing: 10

                                    allow_button := Button{
                                        width: Fill
                                        text: "Allow Quest Scene Access"
                                    }

                                    start_xr_button := Button{
                                        width: Fill
                                        text: "Start XR"
                                    }
                                }

                                status_label := Label{
                                    width: Fill
                                    text: "Checking startup requirements."
                                    draw_text.color: #x8fe4d6
                                }
                            }
                        }

                        XrRuntime := View{
                            width: 0
                            height: 0
                        }
                    }
                }
            }

            xr_scene := mod.widgets.XrScene{}
        }
    }
}

const CUBE_COLORS: &[[f32; 3]] = &[
    [0.90, 0.30, 0.25],
    [0.25, 0.75, 0.45],
    [0.30, 0.50, 0.90],
    [0.95, 0.75, 0.20],
    [0.80, 0.40, 0.85],
    [0.20, 0.80, 0.80],
    [0.95, 0.55, 0.25],
    [0.60, 0.85, 0.35],
];

const PLATFORM_COLOR: [f32; 3] = [0.10, 0.14, 0.18];
const XR_GRID_SIZE: usize = 5;
const XR_CUBE_HALF_EXTENT: f32 = 0.020;
const XR_CUBE_SPACING: f32 = 0.046;
const XR_CUBE_DENSITY: f32 = 900.0;
const XR_PLATFORM_HALF_WIDTH: f32 = 0.16;
const XR_PLATFORM_HALF_HEIGHT: f32 = 0.012;
const XR_PLATFORM_HALF_DEPTH: f32 = 0.10;
const XR_SCENE_DISTANCE: f32 = 0.72;
const XR_SCENE_DROP: f32 = 0.45;
const XR_PHYSICS_DT: f32 = 1.0 / 72.0;
const XR_HAND_IMPULSE_SCALE: f32 = 0.22;
const XR_HAND_PUSH_SCALE: f32 = 1.8;
const XR_HAND_MAX_BODY_IMPULSE: f32 = 0.65;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum AppPhase {
    #[default]
    Preflight,
    XrRuntime,
}

#[derive(Clone, Copy, Debug)]
struct HandCollider {
    prev: Vec3f,
    curr: Vec3f,
    radius: f32,
    strength: f32,
}

#[derive(Clone, Copy, Debug)]
struct SphereBoxContact {
    normal: Vec3f,
    penetration: f32,
}

#[derive(Script, ScriptHook, Widget)]
pub struct XrScene {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[live]
    draw_cube: DrawCube,
    #[redraw]
    #[live]
    draw_pbr: DrawPbr,
    #[rust]
    world: Option<PhysicsWorld>,
    #[rust]
    pending_ops: Vec<PhysicsOp>,
    #[rust]
    scene_pose: Option<Pose>,
    #[rust]
    body_mesh: Option<usize>,
}

impl XrScene {
    fn draw_pose_box(
        &mut self,
        cx: &mut Cx2d,
        pose: Pose,
        size: Vec3f,
        color: Vec4f,
        depth_clip: f32,
    ) {
        self.draw_cube.transform = pose.to_mat4();
        self.draw_cube.cube_pos = vec3(0.0, 0.0, 0.0);
        self.draw_cube.cube_size = size;
        self.draw_cube.color = color;
        self.draw_cube.depth_clip = depth_clip;
        self.draw_cube.draw(cx);
    }

    fn draw_forward_box(
        &mut self,
        cx: &mut Cx2d,
        pose: Pose,
        size: Vec3f,
        forward_offset: f32,
        color: Vec4f,
        depth_clip: f32,
    ) {
        self.draw_cube.transform = pose.to_mat4();
        self.draw_cube.cube_pos = vec3(0.0, 0.0, forward_offset);
        self.draw_cube.cube_size = size;
        self.draw_cube.color = color;
        self.draw_cube.depth_clip = depth_clip;
        self.draw_cube.draw(cx);
    }

    fn draw_hand(&mut self, cx: &mut Cx2d, hand: &XrHand, is_left: bool) {
        if !hand.in_view() {
            return;
        }

        let joint_color = if is_left {
            vec4(0.22, 0.78, 1.0, 1.0)
        } else {
            vec4(1.0, 0.68, 0.30, 1.0)
        };
        let tip_color = if is_left {
            vec4(0.42, 0.98, 1.0, 1.0)
        } else {
            vec4(1.0, 0.86, 0.44, 1.0)
        };

        for joint in &hand.joints {
            self.draw_pose_box(cx, *joint, vec3(0.011, 0.011, 0.016), joint_color, 0.0);
        }

        for finger_index in 0..XrHand::END_KNUCKLES.len() {
            if !hand.tip_active(finger_index) {
                continue;
            }
            let tip_len = hand.tips[finger_index].max(0.006);
            self.draw_forward_box(
                cx,
                hand.joints[XrHand::END_KNUCKLES[finger_index]],
                vec3(0.007, 0.007, 0.018 + tip_len * 0.6),
                -0.014 - tip_len * 0.3,
                tip_color,
                0.0,
            );
        }
    }

    fn ensure_scene(&mut self, state: &XrState) -> bool {
        if self.scene_pose.is_some() {
            return false;
        }

        let mut forward = state.vec_in_head_space(vec3(0.0, 0.0, -1.0)) - state.head_pose.position;
        forward.y = 0.0;
        if forward.length() <= 1.0e-4 {
            forward = vec3f(0.0, 0.0, -1.0);
        } else {
            forward = forward.normalize();
        }
        let right = Vec3f::cross(forward, vec3f(0.0, 1.0, 0.0)).normalize();
        let center = state.vec_in_head_space(vec3(0.0, -XR_SCENE_DROP, -XR_SCENE_DISTANCE));

        let scene_pose = Pose::new(Quat::look_rotation(forward, vec3f(0.0, 1.0, 0.0)), center);
        log!(
            "XR physics wall spawned at ({:.2}, {:.2}, {:.2})",
            scene_pose.position.x,
            scene_pose.position.y,
            scene_pose.position.z
        );

        let mut world = PhysicsWorld::new(vec3f(0.0, -9.81, 0.0), XR_PHYSICS_DT);
        world.ground_y = center.y;
        let mut spawn_ops = Vec::new();
        let half = vec3f(
            XR_CUBE_HALF_EXTENT,
            XR_CUBE_HALF_EXTENT,
            XR_CUBE_HALF_EXTENT,
        );
        let center_offset = (XR_GRID_SIZE as f32 - 1.0) * 0.5;
        for row in 0..XR_GRID_SIZE {
            for col in 0..XR_GRID_SIZE {
                spawn_ops.push(PhysicsOp::SpawnDynamic {
                    position: center
                        + right * ((col as f32 - center_offset) * XR_CUBE_SPACING)
                        + vec3f(0.0, XR_CUBE_HALF_EXTENT + row as f32 * XR_CUBE_SPACING, 0.0),
                    half_extents: half,
                    velocity: Vec3f::default(),
                    density: XR_CUBE_DENSITY,
                });
            }
        }
        world.step(&spawn_ops);
        self.scene_pose = Some(scene_pose);
        self.world = Some(world);
        true
    }

    fn hand_tip_world(hand: &XrHand, finger_index: usize) -> Vec3f {
        let tip_len = hand.tips[finger_index].max(0.0);
        hand.joints[XrHand::END_KNUCKLES[finger_index]]
            .to_mat4()
            .transform_vec4(vec4(0.0, 0.0, -tip_len, 1.0))
            .to_vec3f()
    }

    fn append_hand_colliders(colliders: &mut Vec<HandCollider>, hand: &XrHand, last_hand: &XrHand) {
        if !hand.in_view() {
            return;
        }

        let last_visible = last_hand.in_view();
        for (joint_index, radius, strength) in [
            (XrHand::CENTER, 0.045, 0.70),
            (XrHand::WRIST, 0.032, 0.40),
            (XrHand::THUMB_KNUCKLE2, 0.020, 0.80),
            (XrHand::INDEX_KNUCKLE3, 0.022, 1.00),
            (XrHand::MIDDLE_KNUCKLE3, 0.022, 0.95),
            (XrHand::RING_KNUCKLE3, 0.020, 0.80),
            (XrHand::LITTLE_KNUCKLE3, 0.018, 0.70),
        ] {
            let curr = hand.joints[joint_index].position;
            let prev_pose = if last_visible {
                last_hand.joints[joint_index]
            } else {
                hand.joints[joint_index]
            };
            let prev = prev_pose.position;
            colliders.push(HandCollider {
                prev,
                curr,
                radius,
                strength,
            });
        }

        for finger_index in 0..XrHand::END_KNUCKLES.len() {
            if !hand.tip_active(finger_index) {
                continue;
            }
            let curr = Self::hand_tip_world(hand, finger_index);
            let prev = if last_visible && last_hand.tip_active(finger_index) {
                Self::hand_tip_world(last_hand, finger_index)
            } else {
                curr
            };
            colliders.push(HandCollider {
                prev,
                curr,
                radius: 0.018,
                strength: 1.10,
            });
        }
    }

    fn queue_hand_impulses(&mut self, state: &XrState, last: &XrState) {
        let Some(world) = self.world.as_ref() else {
            return;
        };

        let dt = ((state.time - last.time) as f32).max(1.0 / 180.0);
        let mut colliders = Vec::new();
        Self::append_hand_colliders(&mut colliders, &state.left_hand, &last.left_hand);
        Self::append_hand_colliders(&mut colliders, &state.right_hand, &last.right_hand);
        if colliders.is_empty() {
            return;
        }

        let mut accumulated = vec![Vec3f::default(); world.bodies.len()];
        for (body_index, body) in world.bodies.iter().enumerate() {
            if !body.is_dynamic() {
                continue;
            }
            for collider in &colliders {
                let Some(contact) = sphere_box_contact(collider.curr, body, collider.radius) else {
                    continue;
                };
                let hand_velocity = (collider.curr - collider.prev) / dt;
                let mut impulse = hand_velocity * (XR_HAND_IMPULSE_SCALE * collider.strength)
                    + contact.normal
                        * (contact.penetration * XR_HAND_PUSH_SCALE * collider.strength);
                let impulse_len = impulse.length();
                if impulse_len <= 1.0e-4 {
                    continue;
                }
                let max_impulse = XR_HAND_MAX_BODY_IMPULSE * collider.strength;
                if impulse_len > max_impulse {
                    impulse = impulse * (max_impulse / impulse_len);
                }
                accumulated[body_index] += impulse;
            }
        }

        for (body_index, impulse) in accumulated.into_iter().enumerate() {
            let impulse_len = impulse.length();
            if impulse_len <= 1.0e-4 {
                continue;
            }
            let capped = if impulse_len > XR_HAND_MAX_BODY_IMPULSE {
                impulse * (XR_HAND_MAX_BODY_IMPULSE / impulse_len)
            } else {
                impulse
            };
            self.pending_ops.push(PhysicsOp::ApplyImpulse {
                body: body_index,
                impulse: capped,
            });
        }
    }

    fn configure_draw_pbr(&mut self, cx: &mut Cx2d, state: &XrState) {
        self.draw_pbr.set_use_pass_camera(true);
        self.draw_pbr.camera_pos = state.head_pose.position;
        self.draw_pbr.set_depth_write(true);
        self.draw_pbr.set_depth_clip(1.0);
        self.draw_pbr.set_base_color_texture(None);
        self.draw_pbr.set_metal_roughness_texture(None);
        self.draw_pbr.set_normal_texture(None);
        self.draw_pbr.set_occlusion_texture(None);
        self.draw_pbr.set_emissive_texture(None);
        let env_tex = self.draw_pbr.default_env_texture(cx);
        self.draw_pbr.set_env_texture(Some(env_tex));
    }

    fn ensure_body_mesh(&mut self, cx: &mut Cx2d) -> Option<usize> {
        if let Some(mesh) = self.body_mesh {
            return Some(mesh);
        }
        match self.draw_pbr.upload_uniform_rounded_cube_mesh(
            cx,
            XR_CUBE_HALF_EXTENT,
            0.015,
            1,
            4,
        ) {
            Ok(mesh) => {
                self.body_mesh = Some(mesh);
                Some(mesh)
            }
            Err(err) => {
                log!("XR rounded cube mesh upload failed: {}", err);
                None
            }
        }
    }

    fn draw_platform(&mut self, cx: &mut Cx2d, scene_matrix: &Mat4f) {
        let platform_pose = Pose::new(Quat::default(), vec3f(0.0, -XR_PLATFORM_HALF_HEIGHT, 0.0));
        self.draw_pbr
            .set_transform(Mat4f::mul(scene_matrix, &platform_pose.to_mat4()));
        self.draw_pbr.set_base_color_factor(vec4(
            PLATFORM_COLOR[0],
            PLATFORM_COLOR[1],
            PLATFORM_COLOR[2],
            1.0,
        ));
        self.draw_pbr.set_metal_roughness(0.0, 0.82);
        let _ = self.draw_pbr.draw_rounded_cube(
            cx,
            vec3(
                XR_PLATFORM_HALF_WIDTH,
                XR_PLATFORM_HALF_HEIGHT,
                XR_PLATFORM_HALF_DEPTH,
            ),
            0.028,
            1,
            4,
        );
    }

    fn draw_bodies(&mut self, cx: &mut Cx2d, _scene_matrix: &Mat4f) {
        let Some(body_mesh) = self.ensure_body_mesh(cx) else {
            return;
        };
        let Some(world) = self.world.as_ref() else {
            return;
        };

        self.draw_pbr.set_metal_roughness(0.0, 0.52);
        for (i, body) in world.bodies.iter().enumerate() {
            if !body.is_dynamic() {
                continue;
            }
            let color = CUBE_COLORS[i % CUBE_COLORS.len()];
            self.draw_pbr.set_transform(body.pose.to_mat4());
            self.draw_pbr
                .set_base_color_factor(vec4(color[0], color[1], color[2], 1.0));
            let _ = self.draw_pbr.draw_mesh(cx, body_mesh);
        }
    }
}

impl Widget for XrScene {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Event::XrUpdate(e) = event {
            let just_initialized = self.ensure_scene(&e.state);
            if !just_initialized {
                self.queue_hand_impulses(&e.state, &e.last);
            }
            if let Some(world) = &mut self.world {
                world.step(&self.pending_ops);
            }
            self.pending_ops.clear();
            self.redraw(cx);
        }
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }

    fn draw_3d(&mut self, cx: &mut Cx3d, _scope: &mut Scope) -> DrawStep {
        let Some(state) = cx.draw_event.xr_state.as_ref() else {
            return DrawStep::done();
        };

        let cx = &mut Cx2d::new(cx.cx);
        self.ensure_scene(state);
        self.draw_hand(cx, &state.left_hand, true);
        self.draw_hand(cx, &state.right_hand, false);

        let Some(scene_pose) = self.scene_pose else {
            return DrawStep::done();
        };
        let scene_matrix = scene_pose.to_mat4();

        self.configure_draw_pbr(cx, state);
        self.draw_platform(cx, &scene_matrix);
        self.draw_bodies(cx, &scene_matrix);

        DrawStep::done()
    }
}

fn sphere_box_contact(center: Vec3f, body: &RigidBody, radius: f32) -> Option<SphereBoxContact> {
    let local_center = body
        .pose
        .orientation
        .invert()
        .rotate_vec3(&(center - body.pose.position));
    let he = body.half_extents;
    let closest = vec3f(
        local_center.x.clamp(-he.x, he.x),
        local_center.y.clamp(-he.y, he.y),
        local_center.z.clamp(-he.z, he.z),
    );
    let delta = local_center - closest;
    let dist_sq = delta.dot(delta);
    if dist_sq > radius * radius {
        return None;
    }

    let (normal_local, penetration) = if dist_sq > 1.0e-6 {
        let dist = dist_sq.sqrt();
        (delta * (1.0 / dist), radius - dist)
    } else {
        let dx = he.x - local_center.x.abs();
        let dy = he.y - local_center.y.abs();
        let dz = he.z - local_center.z.abs();
        if dx <= dy && dx <= dz {
            (
                vec3f(if local_center.x >= 0.0 { 1.0 } else { -1.0 }, 0.0, 0.0),
                radius + dx,
            )
        } else if dy <= dz {
            (
                vec3f(0.0, if local_center.y >= 0.0 { 1.0 } else { -1.0 }, 0.0),
                radius + dy,
            )
        } else {
            (
                vec3f(0.0, 0.0, if local_center.z >= 0.0 { 1.0 } else { -1.0 }),
                radius + dz,
            )
        }
    };

    Some(SphereBoxContact {
        normal: body.pose.orientation.rotate_vec3(&normal_local).normalize(),
        penetration: penetration.max(0.0),
    })
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    phase: AppPhase,
    #[rust]
    scene_access: Option<PermissionStatus>,
    #[rust]
    pending_scene_access_check: Option<i32>,
    #[rust]
    pending_scene_access_request: Option<i32>,
    #[rust]
    ui_refresh_next_frame: Option<NextFrame>,
    #[rust]
    xr_start_next_frame: Option<NextFrame>,
}

impl App {
    fn is_android_preflight() -> bool {
        cfg!(target_os = "android")
    }

    fn scene_access_granted(&self) -> bool {
        !Self::is_android_preflight()
            || matches!(self.scene_access, Some(PermissionStatus::Granted))
    }

    fn phase_variant(&self) -> LiveId {
        match self.phase {
            AppPhase::Preflight => live_id!(Preflight),
            AppPhase::XrRuntime => live_id!(XrRuntime),
        }
    }

    fn apply_phase(&mut self, cx: &mut Cx) {
        let phase_variant = self.phase_variant();
        self.ui
            .adaptive_view(cx, ids!(phase_view))
            .set_variant_selector(move |_cx, _parent_size| phase_variant);
        cx.redraw_all();
    }

    fn schedule_ui_refresh(&mut self, cx: &mut Cx) {
        self.ui_refresh_next_frame = Some(cx.new_next_frame());
        cx.redraw_all();
    }

    fn allow_button_text(&self) -> &'static str {
        if self.pending_scene_access_check.is_some() {
            "Checking Quest Scene Access..."
        } else if self.pending_scene_access_request.is_some() {
            "Waiting for Quest Permission..."
        } else if matches!(self.scene_access, Some(PermissionStatus::Granted)) {
            "Re-check Quest Scene Access"
        } else {
            "Allow Quest Scene Access"
        }
    }

    fn detail_text(&self) -> &'static str {
        if !Self::is_android_preflight() {
            "This build can start XR directly from the splash screen."
        } else {
            match self.scene_access {
                Some(PermissionStatus::Granted) => {
                    "Quest scene access is granted. Start XR when you are ready."
                }
                Some(PermissionStatus::DeniedCanRetry) => {
                    "Quest scene access was denied. Use the allow button to ask again."
                }
                Some(PermissionStatus::DeniedPermanent) => {
                    "Quest scene access was denied again. Retry is still available here, but Android may require system settings before the dialog reappears."
                }
                Some(PermissionStatus::NotDetermined) | None => {
                    "Allow Quest scene access before starting XR. This unlocks environment depth and passthrough occlusion."
                }
            }
        }
    }

    fn status_text(&self) -> &'static str {
        if self.pending_scene_access_check.is_some() {
            "Checking current Quest permission status."
        } else if self.pending_scene_access_request.is_some() {
            "Approve the Quest permission dialog to continue."
        } else if !Self::is_android_preflight() {
            "XR is ready to launch from this splash screen."
        } else {
            match self.scene_access {
                Some(PermissionStatus::Granted) => "Quest scene access granted.",
                Some(PermissionStatus::DeniedCanRetry) => {
                    "Quest scene access denied. You can request it again."
                }
                Some(PermissionStatus::DeniedPermanent) => {
                    "Quest scene access denied. Retry may require Android settings."
                }
                Some(PermissionStatus::NotDetermined) | None => {
                    "Quest scene access has not been granted yet."
                }
            }
        }
    }

    fn refresh_preflight_ui(&mut self, cx: &mut Cx) {
        if self.phase != AppPhase::Preflight {
            return;
        }
        self.ui
            .label(cx, ids!(detail_label))
            .set_text(cx, self.detail_text());
        self.ui
            .label(cx, ids!(status_label))
            .set_text(cx, self.status_text());

        let allow_button = self.ui.button(cx, ids!(allow_button));
        allow_button.set_visible(cx, Self::is_android_preflight());
        allow_button.set_enabled(
            cx,
            Self::is_android_preflight()
                && self.pending_scene_access_check.is_none()
                && self.pending_scene_access_request.is_none(),
        );
        self.ui
            .widget(cx, ids!(allow_button))
            .set_text(cx, self.allow_button_text());

        self.ui
            .button(cx, ids!(start_xr_button))
            .set_enabled(cx, self.scene_access_granted());
    }

    fn begin_scene_access_check(&mut self, cx: &mut Cx) {
        if !Self::is_android_preflight() || self.pending_scene_access_check.is_some() {
            return;
        }
        self.pending_scene_access_check = Some(cx.check_permission(Permission::SceneAccess));
        self.schedule_ui_refresh(cx);
    }

    fn request_scene_access(&mut self, cx: &mut Cx) {
        if !Self::is_android_preflight()
            || self.pending_scene_access_check.is_some()
            || self.pending_scene_access_request.is_some()
        {
            return;
        }
        self.pending_scene_access_request = Some(cx.request_permission(Permission::SceneAccess));
        self.schedule_ui_refresh(cx);
    }

    fn begin_xr_runtime(&mut self, cx: &mut Cx) {
        if self.phase == AppPhase::XrRuntime {
            return;
        }
        self.phase = AppPhase::XrRuntime;
        self.apply_phase(cx);
        self.xr_start_next_frame = Some(cx.new_next_frame());
    }

    fn maybe_start_xr_on_ready(&mut self, cx: &mut Cx) -> bool {
        if self.phase != AppPhase::Preflight || !self.scene_access_granted() {
            return false;
        }
        self.begin_xr_runtime(cx);
        true
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.phase = AppPhase::Preflight;
        if !Self::is_android_preflight() {
            self.scene_access = Some(PermissionStatus::Granted);
            self.maybe_start_xr_on_ready(cx);
            return;
        }
        self.apply_phase(cx);
        self.schedule_ui_refresh(cx);
        self.begin_scene_access_check(cx);
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(allow_button)).clicked(actions) {
            self.request_scene_access(cx);
        }

        if self.ui.button(cx, ids!(start_xr_button)).clicked(actions) && self.scene_access_granted()
        {
            self.begin_xr_runtime(cx);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());

        match event {
            Event::NextFrame(ne) => {
                if self
                    .ui_refresh_next_frame
                    .is_some_and(|next_frame| ne.set.contains(&next_frame))
                {
                    self.ui_refresh_next_frame = None;
                    self.refresh_preflight_ui(cx);
                }

                if self
                    .xr_start_next_frame
                    .is_some_and(|next_frame| ne.set.contains(&next_frame))
                {
                    self.xr_start_next_frame = None;
                    cx.xr_start_presenting();
                }
            }
            Event::PermissionResult(result) if result.permission == Permission::SceneAccess => {
                if self.pending_scene_access_check == Some(result.request_id) {
                    self.pending_scene_access_check = None;
                } else if self.pending_scene_access_request == Some(result.request_id) {
                    self.pending_scene_access_request = None;
                } else {
                    return;
                }
                self.scene_access = Some(result.status);
                if !self.maybe_start_xr_on_ready(cx) {
                    self.schedule_ui_refresh(cx);
                }
            }
            Event::Resume => {
                if Self::is_android_preflight() && self.pending_scene_access_request.is_none() {
                    self.begin_scene_access_check(cx);
                }
            }
            _ => {}
        }
    }
}
