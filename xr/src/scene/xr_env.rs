use super::xr_node::{
    XrBodyKind, XrDrawScopeData, XrHandInfluencePoint, XrNode, XrRuntimeBodyState,
    XR_HAND_INFLUENCE_POINTS_PER_HAND, XR_HAND_INFLUENCE_POINT_COUNT,
};
use crate::prelude::*;
use crate::util::{
    depth_debug_mesh::DebugDepthMeshChunk, depth_debug_mesh_worker::XrDepthDebugMeshWorker,
};
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
use std::{
    collections::{HashMap, HashSet, VecDeque},
    rc::Rc,
    sync::Arc,
};

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
        geom: vertex_buffer(geom.DepthMeshVertex, geom.DepthMeshGeom)

        v_barycentric: varying(vec3f)

        edge_distance_px: fn(bary: vec3f) -> f32 {
            let bary_fw = vec3(
                length(vec2(dFdx(bary.x), dFdy(bary.x))),
                length(vec2(dFdx(bary.y), dFdy(bary.y))),
                length(vec2(dFdx(bary.z), dFdy(bary.z)))
            );
            return min(
                bary.x / max(bary_fw.x, 0.00001),
                min(
                    bary.y / max(bary_fw.y, 0.00001),
                    bary.z / max(bary_fw.z, 0.00001)
                )
            )
        }

        wire_band_alpha: fn(bary: vec3f) -> f32 {
            let edge_px = self.edge_distance_px(bary);
            let inner = smoothstep(self.wire_inner_px - 0.75, self.wire_inner_px + 0.75, edge_px);
            let outer = 1.0 - smoothstep(self.wire_outer_px - 0.75, self.wire_outer_px + 0.75, edge_px);
            return clamp(inner * outer, 0.0, 1.0)
        }

        vertex: fn() {
            let world = vec4(
                self.geom.pos.x,
                self.geom.pos.y,
                self.geom.pos.z,
                1.0
            );
            let view = self.draw_pass.camera_view * world;
            let biased_view = vec4(view.x, view.y, view.z + self.depth_bias, view.w);
            self.v_barycentric = vec3(
                self.geom.barycentric.x,
                self.geom.barycentric.y,
                self.geom.barycentric.z
            );
            self.vertex_pos = self.draw_pass.camera_projection * biased_view;
        }

        pixel: fn() {
            let wire_alpha = self.base_color.w * self.wire_band_alpha(self.v_barycentric);
            return vec4(
                self.base_color.x * wire_alpha,
                self.base_color.y * wire_alpha,
                self.base_color.z * wire_alpha,
                wire_alpha
            );
        }

        fragment: fn() {
            self.fb0 = self.pixel();
        }
    }

    mod.widgets.XrEnv = set_type_default() do #(XrEnv::script_component(vm)){
        draw_cube: mod.draw.DrawCube{}
        draw_depth_mesh: mod.draw.DrawDepthMeshBasic{
            alpha_blend: true
            base_color: vec4(0.60, 0.62, 0.66, 0.95)
            depth_bias: 0.006
            wire_outer_px: 2.4
            wire_inner_px: 0.7
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
const XR_DEPTH_SURFACE_MESH_REQUEST_MOVE_METERS: f32 = 0.20;
const XR_DEPTH_SURFACE_MESH_REQUEST_ROTATE_DEGREES: f32 = 12.0;
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

#[derive(Clone, Copy, Debug, Default)]
struct XrPhysicsMetrics {
    compute_ms: f64,
    tsdf_query_ms: f64,
    rapier_step_ms: f64,
    depth_query_surface_count: usize,
}

struct XrPhysicsRuntime {
    worker: Option<XrPhysicsWorker>,
    runtime_bodies: Rc<HashMap<WidgetUid, XrRuntimeBodyState>>,
    root_pose: Option<Pose>,
    scene_dirty: bool,
    revision: u64,
    metrics: XrPhysicsMetrics,
}

impl Default for XrPhysicsRuntime {
    fn default() -> Self {
        Self {
            worker: None,
            runtime_bodies: Rc::new(HashMap::new()),
            root_pose: None,
            scene_dirty: true,
            revision: 0,
            metrics: XrPhysicsMetrics::default(),
        }
    }
}

struct XrDepthRuntime {
    surface_mesh_generation: u64,
    surface_mesh_update_sequence: u64,
    requested_snapshot_grid: Option<Arc<SparseTsdGridReadSnapshot>>,
    requested_head_pose: Option<Pose>,
    snapshot_grid: Option<Arc<SparseTsdGridReadSnapshot>>,
    visible_request_id: u64,
    visible_chunks: HashSet<ChunkKey>,
    mesh_chunks: HashMap<ChunkKey, (Geometry, DepthSurfaceMeshChunkHandle)>,
    pending_upserts: VecDeque<(u64, DebugDepthMeshChunk)>,
    query_hit_geometry: Option<Geometry>,
    surface_mesh_worker: Option<XrDepthDebugMeshWorker>,
    query_retained_hits: HashMap<u64, RetainedDepthQueryHit>,
}

impl Default for XrDepthRuntime {
    fn default() -> Self {
        Self {
            surface_mesh_generation: 0,
            surface_mesh_update_sequence: 0,
            requested_snapshot_grid: None,
            requested_head_pose: None,
            snapshot_grid: None,
            visible_request_id: 0,
            visible_chunks: HashSet::new(),
            mesh_chunks: HashMap::new(),
            pending_upserts: VecDeque::new(),
            query_hit_geometry: None,
            surface_mesh_worker: None,
            query_retained_hits: HashMap::new(),
        }
    }
}

struct XrPassthroughRuntime {
    camera_choice: Option<XrPassthroughCameraChoice>,
    camera_textures: Option<XrPassthroughCameraTextures>,
    camera_video: VideoYuvMetadata,
    camera_permission: Option<PermissionStatus>,
    camera_source_size: Vec2f,
    camera_playback_requested: bool,
    camera_failed: bool,
    camera_has_frame: bool,
    env_face_quad: Option<Geometry>,
    env_cube: Option<XrPassthroughEnvCube>,
}

impl Default for XrPassthroughRuntime {
    fn default() -> Self {
        Self {
            camera_choice: None,
            camera_textures: None,
            camera_video: VideoYuvMetadata::disabled(),
            camera_permission: None,
            camera_source_size: Vec2f::default(),
            camera_playback_requested: false,
            camera_failed: false,
            camera_has_frame: false,
            env_face_quad: None,
            env_cube: None,
        }
    }
}

#[derive(Default)]
struct XrHandSystem;

#[derive(Default)]
struct XrWorld {
    last_xr_state: Option<Rc<XrState>>,
    depth: XrDepthRuntime,
    passthrough: XrPassthroughRuntime,
    physics: XrPhysicsRuntime,
    hands: XrHandSystem,
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
    world: XrWorld,

    // Physics (moved from XrScene)
    #[live(9.81)]
    pub gravity: f32,
    #[rust(0.25)]
    physics_time_scale: f32,
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
        self.world.physics.metrics.compute_ms
    }

    pub(crate) fn physics_tsdf_query_ms(&self) -> f64 {
        self.world.physics.metrics.tsdf_query_ms
    }

    pub(crate) fn physics_rapier_step_ms(&self) -> f64 {
        self.world.physics.metrics.rapier_step_ms
    }

    pub(crate) fn physics_time_scale(&self) -> f32 {
        self.physics_time_scale
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
        self.world.physics.metrics.depth_query_surface_count
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

    fn depth_surface_mesh_request_pose_changed(previous: Pose, next: Pose) -> bool {
        if (next.position - previous.position).length() >= XR_DEPTH_SURFACE_MESH_REQUEST_MOVE_METERS
        {
            return true;
        }
        let mut previous_forward = previous.orientation.rotate_vec3(&vec3f(0.0, 0.0, -1.0));
        let mut next_forward = next.orientation.rotate_vec3(&vec3f(0.0, 0.0, -1.0));
        if previous_forward.length() <= 1.0e-4 || next_forward.length() <= 1.0e-4 {
            return false;
        }
        previous_forward = previous_forward.normalize();
        next_forward = next_forward.normalize();
        let cos_threshold = XR_DEPTH_SURFACE_MESH_REQUEST_ROTATE_DEGREES
            .to_radians()
            .cos();
        previous_forward.dot(next_forward) <= cos_threshold
    }

    fn prepare_depth_mesh(&mut self, cx: &mut Cx2d, state: &XrState) {
        self.draw_depth_mesh.draw_vars.options.depth_write = false;
        self.world.depth.poll_surface_mesh_worker(cx);
        if !self.depth_mesh_visible() {
            self.world.depth.clear_surface_mesh();
            return;
        }
        let Some(snapshot) = cx.cx.xr_tsdf().latest_tsdf_snapshot() else {
            self.world.depth.clear_surface_mesh();
            return;
        };
        let snapshot_unchanged = self
            .world
            .depth
            .requested_snapshot_grid
            .as_ref()
            .is_some_and(|previous| Arc::ptr_eq(previous, &snapshot.grid));
        let pose_unchanged = self
            .world
            .depth
            .requested_head_pose
            .map(|previous| {
                !Self::depth_surface_mesh_request_pose_changed(previous, state.head_pose)
            })
            .unwrap_or(false);
        if snapshot_unchanged && pose_unchanged {
            return;
        }
        self.world
            .depth
            .ensure_surface_mesh_worker()
            .request_snapshot(snapshot.clone(), state.head_pose);
        self.world.depth.requested_snapshot_grid = Some(snapshot.grid.clone());
        self.world.depth.requested_head_pose = Some(state.head_pose);
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

    fn draw_scope_hand_influence_points(
        &self,
        state: Option<&XrState>,
    ) -> [Option<XrHandInfluencePoint>; XR_HAND_INFLUENCE_POINT_COUNT] {
        self.world.hands.draw_scope_hand_influence_points(state)
    }

    pub(crate) fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        match event {
            Event::XrUpdate(update) => {
                self.world.last_xr_state = Some(update.state.clone());
                self.world
                    .passthrough
                    .sync_camera(cx, Self::passthrough_video_id());
            }
            Event::PermissionResult(result) if result.permission == Permission::HeadsetCamera => {
                self.world.passthrough.camera_permission = Some(result.status);
                self.world
                    .passthrough
                    .sync_camera(cx, Self::passthrough_video_id());
                cx.redraw_all();
            }
            Event::VideoInputs(ev) => {
                self.world.passthrough.camera_failed = false;
                self.world.passthrough.camera_choice = XrPassthroughRuntime::pick_camera_choice(ev);
                if self.world.passthrough.camera_choice.is_none() {
                    crate::warning!("XR passthrough camera: no suitable camera choice found");
                }
                self.world
                    .passthrough
                    .sync_camera(cx, Self::passthrough_video_id());
                cx.redraw_all();
            }
            Event::VideoYuvTexturesReady(ev) if ev.video_id == Self::passthrough_video_id() => {
                if let Some(textures) = self.world.passthrough.camera_textures.as_mut() {
                    textures.tex_y = Some(ev.tex_y.clone());
                    textures.tex_u = Some(ev.tex_u.clone());
                    textures.tex_v = Some(ev.tex_v.clone());
                }
                cx.redraw_all();
            }
            Event::VideoTextureUpdated(ev) if ev.video_id == Self::passthrough_video_id() => {
                self.world.passthrough.camera_video = ev.yuv;
                self.world.passthrough.camera_has_frame = true;
                cx.redraw_all();
            }
            Event::VideoPlaybackPrepared(ev) if ev.video_id == Self::passthrough_video_id() => {
                self.world.passthrough.camera_source_size =
                    vec2f(ev.video_width as f32, ev.video_height as f32);
                cx.redraw_all();
            }
            Event::VideoPlaybackResourcesReleased(ev)
                if ev.video_id == Self::passthrough_video_id() =>
            {
                self.world.passthrough.reset_camera_state();
                cx.redraw_all();
            }
            Event::VideoDecodingError(ev) if ev.video_id == Self::passthrough_video_id() => {
                crate::warning!("XR passthrough camera error: {}", ev.error);
                self.world.passthrough.camera_playback_requested = false;
                self.world.passthrough.camera_failed = true;
                self.world.passthrough.camera_has_frame = false;
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
        let (root_pos, root_ori) = if let Some(root_pose) = self.world.physics.root_pose {
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
        self.world
            .physics
            .worker
            .get_or_insert_with(|| XrPhysicsWorker::new(cx.xr_tsdf()))
    }

    fn apply_physics_worker_result(&mut self, result: XrPhysicsWorkerResult) -> bool {
        if result.revision != self.world.physics.revision {
            return false;
        }
        self.world.physics.runtime_bodies = Rc::new(result.runtime_bodies);
        if let Some(retained_hits) = result.depth_query_retained_hits {
            self.world.depth.query_retained_hits = retained_hits;
        }
        self.world.physics.metrics = XrPhysicsMetrics {
            compute_ms: result.physics_compute_ms,
            tsdf_query_ms: result.physics_tsdf_query_ms,
            rapier_step_ms: result.physics_rapier_step_ms,
            depth_query_surface_count: result.physics_depth_query_surface_count,
        };
        true
    }

    fn poll_physics_worker(&mut self, cx: &mut Cx) {
        let mut applied = false;
        while let Some(result) = self
            .world
            .physics
            .worker
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
        self.world.depth.query_retained_hits.clear();
        self.world.physics.revision = self.world.physics.revision.saturating_add(1);
        let revision = self.world.physics.revision;
        let gravity = self.gravity;
        self.ensure_physics_worker(cx)
            .request_rebuild(revision, gravity, cubes);
        self.world.physics.scene_dirty = false;
    }

    pub fn ensure_physics(&mut self, cx: &mut Cx, children: &[(LiveId, WidgetRef)]) {
        self.poll_physics_worker(cx);
        if self.world.physics.scene_dirty || self.world.physics.worker.is_none() {
            self.request_physics_rebuild(cx, children);
            cx.redraw_all();
        }
    }

    pub fn spawn_body(&mut self, cx: &mut Cx, spawn: XrBodySpawn) {
        self.poll_physics_worker(cx);
        if self.world.physics.scene_dirty || self.world.physics.worker.is_none() {
            return;
        }
        let revision = self.world.physics.revision;
        self.ensure_physics_worker(cx)
            .request_body_spawn(revision, spawn);
    }

    pub fn mark_scene_dirty(&mut self) {
        self.world.physics.scene_dirty = true;
    }

    pub fn set_root_pose(&mut self, cx: &mut Cx, pose: Option<Pose>) {
        if self.world.physics.root_pose == pose {
            return;
        }
        self.world.physics.root_pose = pose;
        self.world.physics.scene_dirty = true;
        cx.redraw_all();
    }

    #[allow(dead_code)]
    fn has_dynamic_bodies(&self) -> bool {
        !self.world.physics.runtime_bodies.is_empty()
    }

    pub fn step_physics(&mut self, cx: &mut Cx) {
        self.poll_physics_worker(cx);
        let revision = self.world.physics.revision;
        let physics_time_scale = self.physics_time_scale;
        let include_retained_hits = self.depth_query_hits_visible();
        let (left_hand, right_hand) = self
            .world
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
        self.world.physics.revision = self.world.physics.revision.saturating_add(1);
        if let Some(worker) = self.world.physics.worker.as_mut() {
            worker.request_reset(self.world.physics.revision);
        }
        self.world.physics.metrics = XrPhysicsMetrics::default();
        self.world.depth.query_retained_hits.clear();
        Rc::make_mut(&mut self.world.physics.runtime_bodies).clear();
        self.world.physics.scene_dirty = true;
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
        let state = self.world.last_xr_state.clone();
        if let Some(state) = state.as_deref() {
            if self.depth_debug_enabled() {
                self.prepare_depth_mesh(cx, state);
                let show_depth_mesh = self.depth_mesh_visible();
                let show_depth_query_hits = self.depth_query_hits_visible();
                if show_depth_mesh {
                    self.world.depth.poll_surface_mesh_worker(cx);
                }
                self.world.depth.draw_surface_mesh(
                    &mut self.draw_depth_mesh,
                    cx,
                    show_depth_mesh,
                    show_depth_query_hits,
                );
            }

            if XR_RENDER_HAND_GEOMETRY {
                self.prepare_pbr(cx);
                self.draw_hand(cx, &state.left_hand, None, true);
                self.draw_hand(cx, &state.right_hand, None, false);
            }
        }

        let env_texture = if self.env_cube {
            state.as_deref().and_then(|state| {
                self.world.passthrough.render_env_cube(
                    &mut self.draw_passthrough_env_face,
                    cx,
                    state,
                )
            })
        } else {
            None
        };

        XrDrawScopeData {
            runtime_bodies: self.world.physics.runtime_bodies.clone(),
            tracking_from_content: Mat4f::identity(),
            content_from_tracking: Mat4f::identity(),
            env_texture,
            camera_texture: self
                .world
                .passthrough
                .camera_textures
                .as_ref()
                .map(|textures| textures.camera.clone()),
            camera_source_size: self.world.passthrough.camera_source_size,
            camera_rotation_steps: self.world.passthrough.camera_video.rotation_steps,
            camera_center_offset_uv: self.world.passthrough.camera_center_offset_uv(),
            camera_enabled: self.world.passthrough.camera_has_frame && state.is_some(),
            hand_influence_points: self.draw_scope_hand_influence_points(state.as_deref()),
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
    #[live(0.006)]
    pub depth_bias: f32,
    #[live(2.4)]
    pub wire_outer_px: f32,
    #[live(0.7)]
    pub wire_inner_px: f32,
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
