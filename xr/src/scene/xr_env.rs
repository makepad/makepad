use crate::xr_node::{
    XrBodyKind, XrDrawScopeData, XrHandInfluencePoint, XrNode, XrRuntimeBodyState,
    XR_HAND_INFLUENCE_POINTS_PER_HAND, XR_HAND_INFLUENCE_POINT_COUNT,
};
use crate::*;
use makepad_widgets::makepad_platform::{
    event::{CameraPreviewMode, VideoSource, VideoYuvMetadata},
    permission::{Permission, PermissionStatus},
    video::{VideoFormatId, VideoInputId, VideoInputsEvent, VideoPixelFormat},
};
use rapier3d::prelude::{
    BroadPhaseBvh, CCDSolver, ColliderBuilder, ColliderHandle, ColliderSet, ImpulseJointSet,
    IntegrationParameters, IslandManager, MultibodyJointSet, NarrowPhase, PhysicsPipeline,
    Pose as RapierPose, Real as RapierReal, RigidBodyBuilder, RigidBodyHandle, RigidBodySet,
    Rotation as RapierRotation, SharedShape, Vector as RapierVector,
};
use std::{collections::HashMap, rc::Rc, sync::Arc};

#[path = "xr_depth.rs"]
mod xr_depth;
#[path = "xr_hands.rs"]
mod xr_hands;
#[path = "xr_passthrough.rs"]
mod xr_passthrough;
#[path = "xr_physics.rs"]
mod xr_physics;
#[path = "xr_physics_worker.rs"]
mod xr_physics_worker;

pub(crate) use self::xr_physics::RapierScene;
use self::{
    xr_depth::{DepthSurfaceMeshChunkHandle, RetainedDepthQueryHit},
    xr_passthrough::{
        XrPassthroughCameraChoice, XrPassthroughCameraTextures, XrPassthroughEnvCube,
    },
    xr_physics_worker::{XrPhysicsWorker, XrPhysicsWorkerResult},
};
use crate::depth_debug_mesh_worker::XrDepthDebugMeshWorker;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.draw.DrawDepthMeshBasic = mod.std.set_type_default() do #(DrawDepthMeshBasic::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.IcoVertex, geom.IcoGeom)

        v_world: varying(vec3f)
        v_geom_normal: varying(vec3f)

        vertex: fn() {
            let world = vec4(
                self.geom.pos.x,
                self.geom.pos.y,
                self.geom.pos.z,
                1.0
            );
            let geom_normal = normalize(vec3(
                self.geom.normal.x,
                self.geom.normal.y,
                self.geom.normal.z
            ));
            let biased_world = vec4(world.xyz + geom_normal * self.normal_bias, 1.0);
            self.v_world = world.xyz;
            self.v_geom_normal = geom_normal;
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * biased_world);
        }

        pixel: fn() {
            let face_raw = cross(dFdx(self.v_world), dFdy(self.v_world));
            let face_len = length(face_raw);
            let geom_normal = normalize(self.v_geom_normal);
            let mut n = if face_len > 0.00001 {
                normalize(face_raw)
            } else {
                geom_normal
            };
            if dot(n, geom_normal) < 0.0 {
                n = -n;
            }
            let l = normalize(self.light_dir);
            let diffuse = max(dot(n, l), 0.0);
            let lit = self.ambient + diffuse * (1.0 - self.ambient);
            return vec4(self.base_color.xyz * lit, self.base_color.w);
        }

        fragment: fn() {
            self.fb0 = self.pixel();
        }
    }

    mod.widgets.XrEnv = set_type_default() do #(XrEnv::script_component(vm)){
        draw_cube: mod.draw.DrawCube{}
        draw_depth_mesh: mod.draw.DrawDepthMeshBasic{
            alpha_blend: false
            light_dir: vec3(0.28, 0.86, 0.42)
            ambient: 0.26
            normal_bias: 0.006
            base_color: vec4(0.76, 0.88, 0.98, 1.0)
        }
        draw_pbr: mod.draw.DrawPbr{
            light_dir: vec3(0.35, 0.8, 0.45)
            light_color: vec3(1.0, 1.0, 1.0)
            ambient: 0.04
            spec_power: 128.0
            spec_strength: 1.0
            env_intensity: 1.25
        }
        draw_passthrough_env_face: mod.draw.DrawPassthroughEnvFace{
            source_size: vec2(1280.0, 960.0)
            camera_enabled: 0.0
            rotation_steps: 0.0
            update_strength: 0.92
            face_index: 0.0
            bootstrap_mix: 1.0
            camera_fov_y_degrees: 92.0
            camera_projection_scale: 1.12
            camera_center_offset_uv: vec2(0.0, 0.0)
            camera_right: vec3(1.0, 0.0, 0.0)
            camera_up: vec3(0.0, 1.0, 0.0)
            camera_forward: vec3(0.0, 0.0, -1.0)
        }
    }
}

const XR_SIMULATION_DT: f32 = 1.0 / 120.0;
const XR_ENABLE_HAND_PHYSICS: bool = true;
const XR_ENABLE_DEPTH_QUERY_PHYSICS: bool = true;
const XR_RENDER_HAND_GEOMETRY: bool = false;
const XR_PASSTHROUGH_QUAD_DISTANCE: f32 = 0.78;
const XR_PASSTHROUGH_QUAD_WORLD_OFFSET_Y: f32 = -0.145;
const XR_PASSTHROUGH_QUAD_WORLD_OFFSET_X: f32 = 0.0;
const XR_PASSTHROUGH_ENV_FACE_SIZE: usize = 512;
const XR_PASSTHROUGH_ENV_CAMERA_FOV_Y_DEGREES: f32 = 92.0;
const XR_PASSTHROUGH_ENV_CAMERA_PROJECTION_SCALE: f32 = 0.6825;
const XR_PASSTHROUGH_CAMERA_EXPOSURE: f32 = 0.68;
const XR_PASSTHROUGH_ENV_UPDATE_STRENGTH: f32 = 0.92;
#[allow(dead_code)]
const XR_DEPTH_QUERY_MAX_DISTANCE: f32 = 0.12;
const XR_DEPTH_QUERY_FRICTION: f32 = 0.9;
#[allow(dead_code)]
const XR_DEPTH_QUERY_LOOKAHEAD_SECONDS: f32 = 0.18;
#[allow(dead_code)]
const XR_DEPTH_QUERY_MAX_LOOKAHEAD_DISTANCE: f32 = 0.32;
const XR_DEPTH_QUERY_SURFACES_PER_BODY: usize = 2;
const XR_DEPTH_QUERY_IMPACT_ENABLE_SPEED_MIN: f32 = 0.35;
const XR_DEPTH_QUERY_IMPACT_ENABLE_APPROACH_SPEED_MIN: f32 = 0.18;
const XR_DEPTH_QUERY_SUPPORT_REFRESH_SPEED_MIN: f32 = 0.30;
const XR_DEPTH_QUERY_SUPPORT_REFRESH_EDGE_MARGIN_SCALE: f32 = 0.45;
const XR_DEPTH_QUERY_SUPPORT_REFRESH_EDGE_MARGIN_MIN: f32 = 0.012;
const XR_DEPTH_QUERY_SUPPORT_REFRESH_EDGE_MARGIN_MAX: f32 = 0.04;
#[allow(dead_code)]
const XR_DEPTH_QUERY_STICKY_KEEP_MARGIN: f32 = 0.015;
#[allow(dead_code)]
const XR_DEPTH_QUERY_FINGERPRINT_QUANTIZATION_METERS: f32 = 0.01;
#[allow(dead_code)]
const XR_DEPTH_QUERY_HIT_MISS_GRACE_FRAMES: u8 = 3;
const XR_HAND_COLLIDER_SLOTS_PER_HAND: usize = 25;
const XR_HAND_COLLIDER_FRICTION: f32 = 0.8;
const XR_HAND_PLATE_HALF_WIDTH: f32 = 0.045;
const XR_HAND_PLATE_HALF_HEIGHT: f32 = 0.005;
const XR_HAND_PLATE_HALF_DEPTH: f32 = 0.028;
const XR_HAND_PLATE_FORWARD_OFFSET: f32 = 0.004;
const XR_HAND_TIP_RADIUS_SCALE: f32 = 0.72;
const XR_BODY_LINEAR_DAMPING: f32 = 1.5;
const XR_BODY_ANGULAR_DAMPING: f32 = 6.0;
const XR_BODY_ADDITIONAL_SOLVER_ITERATIONS: usize = 4;
const XR_BODY_SLEEP_ANGULAR_THRESHOLD: f32 = 2.0;
const XR_BODY_SLEEP_TIME: f32 = 0.35;
const XR_BODY_SNAP_SLEEP_LINEAR_SPEED: f32 = 0.03;
const XR_BODY_SNAP_SLEEP_ANGULAR_SPEED: f32 = 1.0;
const XR_PBR_FACE_SUBDIVISIONS: usize = 1;
const XR_PBR_CORNER_SEGMENTS: usize = 3;
const XR_PBR_HAND_CAPSULE_SUBDIVISIONS: usize = 8;
const XR_PBR_HAND_SPHERE_SUBDIVISIONS: usize = 8;

#[derive(Clone, Copy)]
struct CollectedXrCube {
    uid: WidgetUid,
    body_kind: XrBodyKind,
    projectile_pool: bool,
    pose: Pose,
    scale: Vec3f,
    half_extents: Vec3f,
    is_sphere: bool,
    density: f32,
    friction: f32,
    restitution: f32,
}

#[derive(Script, ScriptHook)]
pub struct XrEnv {
    #[live]
    draw_cube: DrawCube,
    #[live]
    draw_pbr: DrawPbr,
    #[live]
    draw_depth_mesh: DrawDepthMeshBasic,
    #[live]
    draw_passthrough_env_face: DrawPassthroughEnvFace,
    #[live(false)]
    depth_mesh: bool,
    #[live(false)]
    depth_query_hits: bool,
    #[live(false)]
    env_cube: bool,
    #[rust]
    last_xr_state: Option<Rc<XrState>>,
    #[rust]
    depth_surface_mesh_generation: u64,
    #[rust]
    depth_surface_mesh_update_sequence: u64,
    #[rust]
    depth_surface_mesh_requested_snapshot_grid: Option<Arc<SparseTsdGridReadSnapshot>>,
    #[rust]
    depth_surface_mesh_snapshot_grid: Option<Arc<SparseTsdGridReadSnapshot>>,
    #[rust]
    depth_surface_mesh_chunks: HashMap<(i32, i32, i32), (Geometry, DepthSurfaceMeshChunkHandle)>,
    #[rust]
    depth_query_hit_geometry: Option<Geometry>,
    #[rust]
    depth_surface_mesh_upload_count: usize,
    #[rust]
    depth_surface_mesh_worker: Option<XrDepthDebugMeshWorker>,
    #[allow(dead_code)]
    #[rust]
    depth_query_retained_hits: HashMap<u64, RetainedDepthQueryHit>,
    #[rust]
    passthrough_camera_choice: Option<XrPassthroughCameraChoice>,
    #[rust]
    passthrough_camera_textures: Option<XrPassthroughCameraTextures>,
    #[rust]
    passthrough_camera_video: VideoYuvMetadata,
    #[rust]
    passthrough_camera_permission: Option<PermissionStatus>,
    #[rust]
    passthrough_camera_source_size: Vec2f,
    #[rust]
    passthrough_camera_playback_requested: bool,
    #[rust]
    passthrough_camera_failed: bool,
    #[rust]
    passthrough_camera_has_frame: bool,
    #[rust]
    passthrough_env_face_quad: Option<Geometry>,
    #[rust]
    passthrough_env_cube: Option<XrPassthroughEnvCube>,

    // Physics (moved from XrScene)
    #[live(9.81)]
    pub gravity: f32,
    #[rust(0.25)]
    physics_time_scale: f32,
    #[rust]
    physics_worker: Option<XrPhysicsWorker>,
    #[rust]
    runtime_bodies: Rc<HashMap<WidgetUid, XrRuntimeBodyState>>,
    #[rust]
    root_pose: Option<Pose>,
    #[rust(true)]
    scene_dirty: bool,
    #[rust]
    physics_revision: u64,
    #[rust]
    physics_compute_ms: f64,
    #[rust]
    physics_step_dt_ms: f64,
    #[rust]
    physics_depth_query_surface_count: usize,
    #[rust]
    physics_depth_query_vertex_count: usize,
    #[rust]
    physics_depth_query_triangle_count: usize,
    #[allow(dead_code)]
    #[rust]
    next_frame: NextFrame,
}

impl XrEnv {
    pub(crate) fn depth_mesh_visible(&self) -> bool {
        self.depth_mesh
    }

    pub(crate) fn depth_query_hits_visible(&self) -> bool {
        self.depth_query_hits
    }

    #[allow(dead_code)]
    pub(crate) fn set_depth_mesh_visible(&mut self, visible: bool) {
        self.depth_mesh = visible;
    }

    #[allow(dead_code)]
    pub(crate) fn set_depth_query_hits_visible(&mut self, visible: bool) {
        self.depth_query_hits = visible;
    }

    pub(crate) fn toggle_depth_mesh_visible(&mut self) -> bool {
        self.depth_mesh = !self.depth_mesh;
        self.depth_mesh
    }

    pub(crate) fn toggle_depth_query_hits_visible(&mut self) -> bool {
        self.depth_query_hits = !self.depth_query_hits;
        self.depth_query_hits
    }

    pub(crate) fn physics_compute_ms(&self) -> f64 {
        self.physics_compute_ms
    }

    pub(crate) fn physics_time_scale(&self) -> f32 {
        self.physics_time_scale
    }

    pub(crate) fn physics_step_dt_ms(&self) -> f64 {
        self.physics_step_dt_ms
    }

    pub(crate) fn set_physics_time_scale(&mut self, cx: &mut Cx, scale: f32) -> f32 {
        let scale = scale.clamp(0.1, 1.0);
        if (self.physics_time_scale - scale).abs() <= f32::EPSILON {
            return self.physics_time_scale;
        }
        self.physics_time_scale = scale;
        cx.redraw_all();
        self.physics_time_scale
    }

    pub(crate) fn physics_depth_query_surface_count(&self) -> usize {
        self.physics_depth_query_surface_count
    }

    pub(crate) fn physics_depth_query_vertex_count(&self) -> usize {
        self.physics_depth_query_vertex_count
    }

    pub(crate) fn physics_depth_query_triangle_count(&self) -> usize {
        self.physics_depth_query_triangle_count
    }

    fn passthrough_video_id() -> LiveId {
        live_id!(xr_passthrough_camera)
    }

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

    fn prepare_pbr(&mut self, cx: &mut Cx2d) {
        Self::prepare_draw_pbr_common(&mut self.draw_pbr, cx);
    }

    fn prepare_draw_pbr_common(draw_pbr: &mut DrawPbr, cx: &mut Cx2d) {
        draw_pbr.begin();
        draw_pbr.set_depth_clip(1.0);
        draw_pbr.set_base_color_texture(None);
        draw_pbr.set_metal_roughness_texture(None);
        draw_pbr.set_normal_texture(None);
        draw_pbr.set_occlusion_texture(None);
        draw_pbr.set_emissive_texture(None);
        let env_tex = draw_pbr.default_env_texture(cx);
        draw_pbr.set_env_texture(Some(env_tex));
    }

    fn prepare_depth_mesh(&mut self, cx: &mut Cx2d) {
        self.draw_depth_mesh.draw_vars.options.depth_write = true;
        self.poll_depth_surface_mesh_worker(cx);
        if !self.depth_mesh_visible() {
            self.clear_depth_surface_mesh();
            return;
        }
        let Some(snapshot) = cx.cx.xr_tsdf().latest_tsdf_snapshot() else {
            self.clear_depth_surface_mesh();
            return;
        };
        if self
            .depth_surface_mesh_requested_snapshot_grid
            .as_ref()
            .is_some_and(|previous| Arc::ptr_eq(previous, &snapshot.grid))
        {
            return;
        }
        self.ensure_depth_surface_mesh_worker()
            .request_snapshot(snapshot.clone());
        self.depth_surface_mesh_requested_snapshot_grid = Some(snapshot.grid.clone());
    }

    fn draw_pbr_rounded_cube(
        &mut self,
        cx: &mut Cx2d,
        pose: Pose,
        half_extents: Vec3f,
        radius: f32,
        color: Vec4f,
        roughness: f32,
    ) {
        self.draw_pbr.set_transform(pose.to_mat4());
        self.draw_pbr.set_base_color_factor(color);
        self.draw_pbr.set_metal_roughness(0.0, roughness);
        let _ = self.draw_pbr.draw_rounded_cube(
            cx,
            half_extents,
            radius,
            XR_PBR_FACE_SUBDIVISIONS,
            XR_PBR_CORNER_SEGMENTS,
        );
    }

    fn draw_pbr_capsule(
        &mut self,
        cx: &mut Cx2d,
        pose: Pose,
        radius: f32,
        half_height: f32,
        color: Vec4f,
        roughness: f32,
    ) {
        self.draw_pbr.set_transform(pose.to_mat4());
        self.draw_pbr.set_base_color_factor(color);
        self.draw_pbr.set_metal_roughness(0.0, roughness);
        let _ =
            self.draw_pbr
                .draw_capsule(cx, radius, half_height, XR_PBR_HAND_CAPSULE_SUBDIVISIONS);
    }

    fn draw_pbr_sphere(
        &mut self,
        cx: &mut Cx2d,
        center: Vec3f,
        radius: f32,
        color: Vec4f,
        roughness: f32,
    ) {
        self.draw_pbr
            .set_transform(Pose::new(Quat::default(), center).to_mat4());
        self.draw_pbr.set_base_color_factor(color);
        self.draw_pbr.set_metal_roughness(0.0, roughness);
        let _ = self
            .draw_pbr
            .draw_sphere(cx, radius, XR_PBR_HAND_SPHERE_SUBDIVISIONS);
    }

    fn hand_influence_tip_world(hand: &XrHand, tip: usize) -> Option<Vec3f> {
        if !hand.in_view() || !hand.tip_active(tip) {
            return None;
        }
        Some(match tip {
            XrHand::THUMB_TIP => hand.tip_pos_thumb(),
            XrHand::INDEX_TIP => hand.tip_pos_index(),
            XrHand::MIDDLE_TIP => hand.tip_pos_middle(),
            XrHand::RING_TIP => hand.tip_pos_ring(),
            XrHand::LITTLE_TIP => hand.tip_pos_little(),
            _ => hand.tip_pos_index(),
        })
    }

    fn hand_influence_point(
        pos: Vec3f,
        gain_scale: f32,
        radius_scale: f32,
    ) -> XrHandInfluencePoint {
        XrHandInfluencePoint {
            pos,
            gain_scale,
            radius_scale,
        }
    }

    fn palm_world(hand: &XrHand) -> Option<Vec3f> {
        if !hand.in_view() {
            return None;
        }
        let center = hand.joints[XrHand::CENTER].position;
        let wrist = hand.joints[XrHand::WRIST].position;
        let thumb = hand.joints[XrHand::THUMB_BASE].position;
        let index = hand.joints[XrHand::INDEX_BASE].position;
        let middle = hand.joints[XrHand::MIDDLE_BASE].position;
        let ring = hand.joints[XrHand::RING_BASE].position;
        let little = hand.joints[XrHand::LITTLE_BASE].position;
        Some(
            center * 0.28
                + wrist * 0.10
                + thumb * 0.12
                + index * 0.13
                + middle * 0.18
                + ring * 0.11
                + little * 0.08,
        )
    }

    fn write_hand_influence_points(hand: &XrHand, target: &mut [Option<XrHandInfluencePoint>]) {
        debug_assert_eq!(target.len(), XR_HAND_INFLUENCE_POINTS_PER_HAND);
        target[0] = Self::hand_influence_tip_world(hand, XrHand::THUMB_TIP)
            .map(|pos| Self::hand_influence_point(pos, 0.72, 0.92));
        target[1] = Self::hand_influence_tip_world(hand, XrHand::INDEX_TIP)
            .map(|pos| Self::hand_influence_point(pos, 1.00, 1.00));
        target[2] = Self::hand_influence_tip_world(hand, XrHand::MIDDLE_TIP)
            .map(|pos| Self::hand_influence_point(pos, 0.96, 1.00));
        target[3] = Self::hand_influence_tip_world(hand, XrHand::RING_TIP)
            .map(|pos| Self::hand_influence_point(pos, 0.82, 0.94));
        target[4] = Self::hand_influence_tip_world(hand, XrHand::LITTLE_TIP)
            .map(|pos| Self::hand_influence_point(pos, 0.68, 0.88));
        target[5] = Self::palm_world(hand).map(|pos| Self::hand_influence_point(pos, 1.30, 2.40));
    }

    fn draw_scope_hand_influence_points(
        state: Option<&XrState>,
    ) -> [Option<XrHandInfluencePoint>; XR_HAND_INFLUENCE_POINT_COUNT] {
        let mut points = [None; XR_HAND_INFLUENCE_POINT_COUNT];
        let Some(state) = state else {
            return points;
        };
        let (left_points, right_points) = points.split_at_mut(XR_HAND_INFLUENCE_POINTS_PER_HAND);
        Self::write_hand_influence_points(&state.left_hand, left_points);
        Self::write_hand_influence_points(&state.right_hand, right_points);
        points
    }

    pub(crate) fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        match event {
            Event::XrUpdate(update) => {
                self.last_xr_state = Some(update.state.clone());
                self.sync_passthrough_camera(cx);
            }
            Event::PermissionResult(result) if result.permission == Permission::HeadsetCamera => {
                self.passthrough_camera_permission = Some(result.status);
                self.sync_passthrough_camera(cx);
                cx.redraw_all();
            }
            Event::VideoInputs(ev) => {
                self.passthrough_camera_failed = false;
                self.passthrough_camera_choice = Self::pick_passthrough_camera_choice(ev);
                if self.passthrough_camera_choice.is_none() {
                    crate::warning!("XR passthrough camera: no suitable camera choice found");
                }
                self.sync_passthrough_camera(cx);
                cx.redraw_all();
            }
            Event::VideoYuvTexturesReady(ev) if ev.video_id == Self::passthrough_video_id() => {
                if let Some(textures) = self.passthrough_camera_textures.as_mut() {
                    textures.tex_y = Some(ev.tex_y.clone());
                    textures.tex_u = Some(ev.tex_u.clone());
                    textures.tex_v = Some(ev.tex_v.clone());
                }
                cx.redraw_all();
            }
            Event::VideoTextureUpdated(ev) if ev.video_id == Self::passthrough_video_id() => {
                self.passthrough_camera_video = ev.yuv;
                self.passthrough_camera_has_frame = true;
                cx.redraw_all();
            }
            Event::VideoPlaybackPrepared(ev) if ev.video_id == Self::passthrough_video_id() => {
                self.passthrough_camera_source_size =
                    vec2f(ev.video_width as f32, ev.video_height as f32);
                cx.redraw_all();
            }
            Event::VideoPlaybackResourcesReleased(ev)
                if ev.video_id == Self::passthrough_video_id() =>
            {
                self.reset_passthrough_camera_state();
                cx.redraw_all();
            }
            Event::VideoDecodingError(ev) if ev.video_id == Self::passthrough_video_id() => {
                crate::warning!("XR passthrough camera error: {}", ev.error);
                self.passthrough_camera_playback_requested = false;
                self.passthrough_camera_failed = true;
                self.passthrough_camera_has_frame = false;
                cx.redraw_all();
            }
            _ => {}
        }
    }

    // --- Physics management (moved from XrScene) ---

    fn rotation_quat(rot: Vec3f) -> Quat {
        let x = Quat::from_axis_angle(vec3f(1.0, 0.0, 0.0), rot.x);
        let y = Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), rot.y);
        let z = Quat::from_axis_angle(vec3f(0.0, 0.0, 1.0), rot.z);
        Quat::multiply(&z, &Quat::multiply(&y, &x))
    }

    fn transform_with_node(
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

    fn collect_cubes_from_widget(
        widget: &WidgetRef,
        parent_pos: Vec3f,
        parent_ori: Quat,
        parent_scale: Vec3f,
        cubes: &mut Vec<CollectedXrCube>,
    ) {
        if !widget.visible() {
            return;
        }
        let Some(node) = widget.cast_inner::<XrNode>() else {
            widget.children(&mut |_, child| {
                Self::collect_cubes_from_widget(&child, parent_pos, parent_ori, parent_scale, cubes)
            });
            return;
        };

        let is_sphere = widget.borrow::<IcoSphere>().is_some();
        let (pos, ori, scale) =
            Self::transform_with_node(parent_pos, parent_ori, parent_scale, &node);
        let half = node.physics_half_extents();
        let should_push = node.body_kind() != XrBodyKind::Disabled
            && (half.x > 0.0 || half.y > 0.0 || half.z > 0.0);

        if should_push {
            cubes.push(CollectedXrCube {
                uid: widget.widget_uid(),
                body_kind: node.body_kind(),
                projectile_pool: node.projectile_pool(),
                pose: Pose::new(ori, pos),
                scale,
                half_extents: vec3f(half.x * scale.x, half.y * scale.y, half.z * scale.z),
                is_sphere,
                density: node.density(),
                friction: node.friction(),
                restitution: node.restitution(),
            });
        }

        drop(node);
        widget.children(&mut |_, child| {
            Self::collect_cubes_from_widget(&child, pos, ori, scale, cubes)
        });
    }

    fn collect_cubes_from_children(
        &self,
        children: &[(LiveId, WidgetRef)],
    ) -> Vec<CollectedXrCube> {
        let mut cubes = Vec::new();
        let (root_pos, root_ori) = if let Some(root_pose) = self.root_pose {
            (root_pose.position, root_pose.orientation)
        } else {
            (vec3f(0.0, 0.0, 0.0), Quat::default())
        };
        let root_scale = vec3f(1.0, 1.0, 1.0);
        for (_, child) in children {
            Self::collect_cubes_from_widget(child, root_pos, root_ori, root_scale, &mut cubes);
        }
        cubes
    }

    fn ensure_physics_worker(&mut self, cx: &mut Cx) -> &mut XrPhysicsWorker {
        self.physics_worker
            .get_or_insert_with(|| XrPhysicsWorker::new(cx.xr_tsdf()))
    }

    fn apply_physics_worker_result(&mut self, result: XrPhysicsWorkerResult) -> bool {
        if result.revision != self.physics_revision {
            return false;
        }
        self.runtime_bodies = Rc::new(result.runtime_bodies);
        if let Some(retained_hits) = result.depth_query_retained_hits {
            self.depth_query_retained_hits = retained_hits;
        }
        self.physics_compute_ms = result.physics_compute_ms;
        self.physics_step_dt_ms = result.physics_step_dt_ms;
        self.physics_depth_query_surface_count = result.physics_depth_query_surface_count;
        self.physics_depth_query_vertex_count = result.physics_depth_query_vertex_count;
        self.physics_depth_query_triangle_count = result.physics_depth_query_triangle_count;
        true
    }

    fn poll_physics_worker(&mut self, cx: &mut Cx) {
        let mut applied = false;
        while let Some(result) = self
            .physics_worker
            .as_mut()
            .and_then(|worker| worker.take_latest_result())
        {
            applied |= self.apply_physics_worker_result(result);
        }
        if applied {
            cx.redraw_all();
        }
    }

    fn request_physics_rebuild(&mut self, cx: &mut Cx, children: &[(LiveId, WidgetRef)]) {
        let cubes = self.collect_cubes_from_children(children);
        self.depth_query_retained_hits.clear();
        self.physics_revision = self.physics_revision.saturating_add(1);
        let revision = self.physics_revision;
        let gravity = self.gravity;
        self.ensure_physics_worker(cx)
            .request_rebuild(revision, gravity, cubes);
        self.scene_dirty = false;
    }

    pub fn ensure_physics(&mut self, cx: &mut Cx, children: &[(LiveId, WidgetRef)]) {
        self.poll_physics_worker(cx);
        if self.scene_dirty || self.physics_worker.is_none() {
            self.request_physics_rebuild(cx, children);
            cx.redraw_all();
        }
    }

    pub fn spawn_body(&mut self, cx: &mut Cx, spawn: XrBodySpawn) {
        self.poll_physics_worker(cx);
        if self.scene_dirty || self.physics_worker.is_none() {
            return;
        }
        let revision = self.physics_revision;
        self.ensure_physics_worker(cx)
            .request_body_spawn(revision, spawn);
    }

    pub fn mark_scene_dirty(&mut self) {
        self.scene_dirty = true;
    }

    pub fn set_root_pose(&mut self, cx: &mut Cx, pose: Option<Pose>) {
        if self.root_pose == pose {
            return;
        }
        self.root_pose = pose;
        self.scene_dirty = true;
        cx.redraw_all();
    }

    #[allow(dead_code)]
    fn has_dynamic_bodies(&self) -> bool {
        !self.runtime_bodies.is_empty()
    }

    pub fn step_physics(&mut self, cx: &mut Cx) {
        self.poll_physics_worker(cx);
        let revision = self.physics_revision;
        let physics_time_scale = self.physics_time_scale;
        let include_retained_hits = self.depth_query_hits_visible();
        let (left_hand, right_hand) = self
            .last_xr_state
            .as_deref()
            .map(|state| (state.left_hand.clone(), state.right_hand.clone()))
            .unwrap_or_else(|| (XrHand::default(), XrHand::default()));
        self.ensure_physics_worker(cx).request_step(
            revision,
            left_hand,
            right_hand,
            physics_time_scale,
            include_retained_hits,
        );
    }

    pub fn reset_physics(&mut self, cx: &mut Cx) {
        self.physics_revision = self.physics_revision.saturating_add(1);
        if let Some(worker) = self.physics_worker.as_mut() {
            worker.request_reset(self.physics_revision);
        }
        self.physics_compute_ms = 0.0;
        self.physics_step_dt_ms = 0.0;
        self.physics_depth_query_surface_count = 0;
        self.physics_depth_query_vertex_count = 0;
        self.physics_depth_query_triangle_count = 0;
        self.depth_query_retained_hits.clear();
        Rc::make_mut(&mut self.runtime_bodies).clear();
        self.scene_dirty = true;
        cx.redraw_all();
    }

    #[allow(dead_code)]
    pub(crate) fn runtime_scene_ref(&self) -> Option<&RapierScene> {
        None
    }

    #[allow(dead_code)]
    pub(crate) fn runtime_scene_mut(&mut self) -> Option<&mut RapierScene> {
        None
    }

    // --- New API for XrRoot ---

    pub fn prepare_and_draw(&mut self, cx: &mut Cx2d) -> XrDrawScopeData {
        let state = self.last_xr_state.clone();
        if let Some(state) = state.as_deref() {
            if self.depth_debug_enabled() {
                self.prepare_depth_mesh(cx);
                if self.depth_mesh_visible() {
                    self.sync_depth_surface_mesh(cx);
                }
                self.draw_depth_surface_mesh(cx);
            }

            if XR_RENDER_HAND_GEOMETRY {
                self.prepare_pbr(cx);
                self.draw_hand(cx, &state.left_hand, None, true);
                self.draw_hand(cx, &state.right_hand, None, false);
            }
        }

        let env_texture = if self.env_cube {
            state
                .as_deref()
                .and_then(|state| self.render_passthrough_env_cube(cx, state))
        } else {
            None
        };

        XrDrawScopeData {
            runtime_bodies: self.runtime_bodies.clone(),
            tracking_from_content: Mat4f::identity(),
            content_from_tracking: Mat4f::identity(),
            env_texture,
            camera_texture: self
                .passthrough_camera_textures
                .as_ref()
                .map(|textures| textures.camera.clone()),
            camera_source_size: self.passthrough_camera_source_size,
            camera_rotation_steps: self.passthrough_camera_video.rotation_steps,
            camera_center_offset_uv: self.passthrough_camera_center_offset_uv(),
            camera_enabled: self.passthrough_camera_has_frame && state.is_some(),
            hand_influence_points: Self::draw_scope_hand_influence_points(state.as_deref()),
        }
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawDepthMeshBasic {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub base_color: Vec4f,
    #[live]
    pub light_dir: Vec3f,
    #[live(0.006)]
    pub normal_bias: f32,
    #[live(0.26)]
    pub ambient: f32,
}

impl DrawDepthMeshBasic {
    fn draw_geometry(&mut self, cx: &mut Cx2d, geometry_id: GeometryId) {
        self.draw_vars.append_group_id = cx.draw_call_group_background().0;
        self.draw_vars.geometry_id = Some(geometry_id);
        if cx.new_draw_call(&self.draw_vars).is_some() && self.draw_vars.can_instance() {
            let new_area = cx.add_aligned_instance(&self.draw_vars);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }
}
