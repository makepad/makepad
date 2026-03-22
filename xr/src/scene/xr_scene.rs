use super::passthrough_env::DrawPassthroughEnvAtlas;
use self::{
    xr_depth::{DepthSurfaceMeshChunkHandle, RetainedDepthQueryHit},
    xr_passthrough::{
        XrPassthroughCameraChoice, XrPassthroughCameraTextures, XrPassthroughEnvAtlas,
    },
    xr_physics::{makepad_pose, RapierScene},
};
use crate::{
    cube::Cube,
    refractive_cube::RefractiveCube,
    scene_draw::SceneState3D,
    xr_node::{XrBodyKind, XrDrawScopeData, XrNode, XrRuntimeBodyState},
    xr_root::xr_root_options_from_scope,
};
use makepad_widgets::makepad_platform::{
    event::{CameraPreviewMode, VideoSource, VideoYuvMetadata},
    permission::{Permission, PermissionStatus},
    video::{VideoFormatId, VideoInputId, VideoInputsEvent, VideoPixelFormat},
};
use makepad_widgets::*;
use rapier3d::prelude::{
    BroadPhaseBvh, CCDSolver, ColliderBuilder, ColliderHandle, ColliderSet, ImpulseJointSet,
    IntegrationParameters, IslandManager, MultibodyJointSet, NarrowPhase, PhysicsPipeline,
    Pose as RapierPose, Real as RapierReal, RigidBodyBuilder, RigidBodyHandle, RigidBodySet,
    Rotation as RapierRotation, SharedShape, Vector as RapierVector,
};
use std::{collections::HashMap, rc::Rc};

#[path = "xr_depth.rs"]
mod xr_depth;
#[path = "xr_hands.rs"]
mod xr_hands;
#[path = "xr_passthrough.rs"]
mod xr_passthrough;
#[path = "xr_physics.rs"]
mod xr_physics;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.XrPhysics = set_type_default() do #(XrPhysics::script_component(vm))
    mod.widgets.XrSceneBase = #(XrScene::register_widget(vm))

    set_type_default() do #(DrawDepthMeshBasic::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.PbrVertex, geom.PbrGeom)

        v_world: varying(vec3f)
        v_normal: varying(vec3f)

        vertex: fn() {
            let world = vec4(
                self.geom.pos_nx.x,
                self.geom.pos_nx.y,
                self.geom.pos_nx.z,
                1.0
            );
            self.v_normal = normalize(vec3(
                self.geom.pos_nx.w,
                self.geom.ny_nz_uv.x,
                self.geom.ny_nz_uv.y
            ));
            let biased_world = vec4(world.xyz + self.v_normal * self.normal_bias, 1.0);
            self.v_world = biased_world.xyz;
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * biased_world);
        }

        pixel: fn() {
            let n = normalize(self.v_normal);
            let l = normalize(self.light_dir);
            let diffuse = max(dot(n, l), 0.0);
            let lit = self.ambient + diffuse * (1.0 - self.ambient);
            return vec4(self.base_color.xyz * lit, self.base_color.w);
        }

        fragment: fn() {
            self.fb0 = self.pixel();
        }
    }

    mod.widgets.XrScene = set_type_default() do mod.widgets.XrSceneBase{
        width: Fill
        height: Fill
        physics: mod.widgets.XrPhysics{}
        draw_bg +: {
            color: #x171d26
            draw_depth: -99.0
        }
        draw_cube +: {}
        draw_depth_mesh +: {
            light_dir: vec3(0.28, 0.86, 0.42)
            ambient: 0.26
            normal_bias: 0.006
            base_color: vec4(0.76, 0.88, 0.98, 1.0)
        }
        draw_pbr +: {
            light_dir: vec3(0.35, 0.8, 0.45)
            light_color: vec3(1.0, 1.0, 1.0)
            ambient: 0.04
            spec_power: 128.0
            spec_strength: 1.0
            env_intensity: 1.25
        }
        draw_pbr_refractive +: {
            light_dir: vec3(0.35, 0.8, 0.45)
            light_color: vec3(1.0, 1.0, 1.0)
            ambient: 0.02
            spec_power: 128.0
            spec_strength: 1.0
            env_intensity: 1.2
            source_size: vec2(1280.0, 960.0)
            camera_enabled: 0.0
            rotation_steps: 0.0
            camera_fov_y_degrees: 92.0
            camera_projection_scale: 1.12
            camera_center_offset_uv: vec2(0.0, 0.0)
            camera_world_pos: vec3(0.0, 0.0, 0.0)
            camera_right: vec3(1.0, 0.0, 0.0)
            camera_up: vec3(0.0, 1.0, 0.0)
            camera_forward: vec3(0.0, 0.0, -1.0)
            object_center: vec3(0.0, 0.0, 0.0)
            object_right: vec3(1.0, 0.0, 0.0)
            object_up: vec3(0.0, 1.0, 0.0)
            object_forward: vec3(0.0, 0.0, -1.0)
            object_half_extents: vec3(0.085, 0.085, 0.085)
            object_corner_radius: 0.018
            transmission_focus_distance: 1.8
        }
        draw_passthrough_env_atlas +: {
            source_size: vec2(1280.0, 960.0)
            camera_enabled: 0.0
            rotation_steps: 0.0
            bootstrap_mix: 1.0
            update_strength: 0.92
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
const XR_PASSTHROUGH_ENV_ATLAS_WIDTH: usize = 2048;
const XR_PASSTHROUGH_ENV_ATLAS_HEIGHT: usize = 1024;
const XR_PASSTHROUGH_ENV_CAMERA_FOV_Y_DEGREES: f32 = 92.0;
const XR_PASSTHROUGH_ENV_CAMERA_PROJECTION_SCALE: f32 = 0.6825;
const XR_PASSTHROUGH_CAMERA_EXPOSURE: f32 = 0.68;
const XR_PASSTHROUGH_ENV_UPDATE_STRENGTH: f32 = 0.92;
const XR_DEPTH_QUERY_MAX_DISTANCE: f32 = 0.12;
const XR_DEPTH_QUERY_FRICTION: f32 = 0.9;
const XR_DEPTH_QUERY_LOOKAHEAD_SECONDS: f32 = 0.18;
const XR_DEPTH_QUERY_MAX_LOOKAHEAD_DISTANCE: f32 = 0.32;
const XR_DEPTH_QUERY_SHARED_SURFACE_POOL_SIZE: usize = 48;
const XR_DEPTH_QUERY_FINGERPRINT_QUANTIZATION_METERS: f32 = 0.01;
const XR_DEPTH_QUERY_HIT_MISS_GRACE_FRAMES: u8 = 6;
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

#[derive(Script, ScriptHook, Clone, Copy)]
pub struct XrPhysics {
    #[live(9.81)]
    pub gravity: f32,
}

impl Default for XrPhysics {
    fn default() -> Self {
        Self { gravity: 9.81 }
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

#[derive(Clone, Copy)]
struct XrTransformState {
    position: Vec3f,
    orientation: Quat,
    scale: Vec3f,
}

impl Default for XrTransformState {
    fn default() -> Self {
        Self {
            position: vec3f(0.0, 0.0, 0.0),
            orientation: Quat::default(),
            scale: vec3f(1.0, 1.0, 1.0),
        }
    }
}

#[derive(Clone, Copy)]
struct CollectedXrCube {
    uid: WidgetUid,
    body_kind: XrBodyKind,
    pose: Pose,
    scale: Vec3f,
    half_extents: Vec3f,
    density: f32,
    friction: f32,
    restitution: f32,
}

#[derive(Script, ScriptHook, Widget)]
pub struct XrScene {
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[redraw]
    #[live]
    preview_image: DrawImage,
    #[redraw]
    #[live]
    draw_list_3d: DrawList2d,
    #[new]
    preview_pass: DrawPass,
    #[live]
    physics: XrPhysics,
    #[redraw]
    #[live]
    draw_cube: DrawCube,
    #[redraw]
    #[live]
    draw_pbr: DrawPbr,
    #[redraw]
    #[live]
    draw_pbr_refractive: DrawPbrRefractive,
    #[redraw]
    #[live]
    draw_depth_mesh: DrawDepthMeshBasic,
    #[redraw]
    #[live]
    draw_passthrough_env_atlas: DrawPassthroughEnvAtlas,
    #[area]
    #[rust]
    area: Area,
    #[rust]
    scene: Option<RapierScene>,
    #[rust]
    runtime_bodies: HashMap<WidgetUid, XrRuntimeBodyState>,
    #[rust(true)]
    scene_dirty: bool,
    #[rust]
    last_xr_state: Option<Rc<XrState>>,
    #[rust(false)]
    xr_depth_mesh_enabled: bool,
    #[rust]
    drag_last_abs: Option<DVec2>,
    #[rust(0.0)]
    orbit_yaw: f32,
    #[rust(0.45)]
    orbit_pitch: f32,
    #[live(28.0)]
    camera_fov_y: f32,
    #[live(3.4)]
    camera_distance: f32,
    #[live(0.25)]
    camera_distance_min: f32,
    #[live(30.0)]
    camera_distance_max: f32,
    #[live(0.08)]
    wheel_zoom_step: f32,
    #[live(1.3333334)]
    preview_aspect_ratio: f32,
    #[live(false)]
    preview_aspect_fill: bool,
    #[live(0.05)]
    camera_near: f32,
    #[live(200.0)]
    camera_far: f32,
    #[live(vec2(0.0, 1.0))]
    depth_range: Vec2f,
    #[live(0.0)]
    depth_forward_bias: f32,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    depth_surface_mesh_generation: u64,
    #[rust]
    depth_surface_mesh_update_sequence: u64,
    #[rust]
    depth_surface_mesh_chunks: HashMap<(i32, i32, i32), (Geometry, DepthSurfaceMeshChunkHandle)>,
    #[rust]
    depth_surface_mesh_upload_count: usize,
    #[rust]
    preview_color_texture: Option<Texture>,
    #[rust]
    preview_depth_texture: Option<Texture>,
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
    passthrough_env_atlas_quad: Option<Geometry>,
    #[rust]
    passthrough_env_atlas: Option<XrPassthroughEnvAtlas>,
    #[deref]
    node: XrNode,
}

impl XrScene {
    pub fn reset_requested(update: &XrUpdateEvent) -> bool {
        update.clicked_menu()
    }

    fn depth_debug_enabled(&self) -> bool {
        self.xr_depth_mesh_enabled
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
        self.draw_cube.set_use_pass_camera(true);
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

    fn prepare_refractive_pbr(&mut self, cx: &mut Cx2d) {
        Self::prepare_draw_pbr_common(&mut self.draw_pbr_refractive.draw_super, cx);
    }

    fn prepare_draw_pbr_common(draw_pbr: &mut DrawPbr, cx: &mut Cx2d) {
        draw_pbr.begin();
        draw_pbr.set_use_pass_camera(true);
        draw_pbr.set_depth_clip(1.0);
        draw_pbr.set_base_color_texture(None);
        draw_pbr.set_metal_roughness_texture(None);
        draw_pbr.set_normal_texture(None);
        draw_pbr.set_occlusion_texture(None);
        draw_pbr.set_emissive_texture(None);
        let env_tex = draw_pbr.default_env_texture(cx);
        draw_pbr.set_env_texture(Some(env_tex));
    }

    fn prepare_depth_mesh(&mut self, _cx: &mut Cx2d) {
        self.draw_depth_mesh.draw_vars.options.depth_write = true;
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

    fn reset_scene(&mut self, cx: &mut Cx) {
        if let Some(atlas) = self.passthrough_env_atlas.as_mut() {
            atlas.reset_state();
        }
        self.clear_depth_surface_mesh();
        self.scene = None;
        self.runtime_bodies.clear();
        self.scene_dirty = true;
        self.sync_passthrough_camera(cx);
    }

    fn should_preview_step(&self) -> bool {
        self.scene
            .as_ref()
            .map(|scene| {
                scene
                    .cubes
                    .iter()
                    .any(|cube| matches!(cube.body_kind, XrBodyKind::Dynamic))
            })
            .unwrap_or(false)
    }

    fn preview_scene_state(&mut self, rect: Rect, pass_size: Vec2d) -> Option<SceneState3D> {
        if rect.size.x <= 1.0 || rect.size.y <= 1.0 || pass_size.x <= 1.0 || pass_size.y <= 1.0 {
            return None;
        }

        let pass_w = pass_size.x.max(1.0) as f32;
        let pass_h = pass_size.y.max(1.0) as f32;
        let x0 = (2.0 * rect.pos.x as f32 / pass_w) - 1.0;
        let x1 = (2.0 * (rect.pos.x + rect.size.x) as f32 / pass_w) - 1.0;
        let y0 = 1.0 - (2.0 * rect.pos.y as f32 / pass_h);
        let y1 = 1.0 - (2.0 * (rect.pos.y + rect.size.y) as f32 / pass_h);
        let clip_ndc = vec4(x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1));

        let sx = (clip_ndc.z - clip_ndc.x) * 0.5;
        let sy = (clip_ndc.w - clip_ndc.y) * 0.5;
        let tx = (clip_ndc.z + clip_ndc.x) * 0.5;
        let ty = (clip_ndc.w + clip_ndc.y) * 0.5;
        let viewport = Mat4f {
            v: [
                sx, 0.0, 0.0, 0.0, 0.0, sy, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, tx, ty, 0.0, 1.0,
            ],
        };

        let viewport_w = ((clip_ndc.z - clip_ndc.x).abs() * 0.5 * pass_size.x as f32).max(1.0);
        let viewport_h = ((clip_ndc.w - clip_ndc.y).abs() * 0.5 * pass_size.y as f32).max(1.0);
        let aspect = (viewport_w / viewport_h).max(0.001);
        let preview_fov_y = self.camera_fov_y.clamp(1.0, 179.0);
        let projection = Mat4f::perspective(
            preview_fov_y,
            aspect,
            self.camera_near.max(0.001),
            self.camera_far.max(self.camera_near + 0.001),
        );
        let projection_viewport = Mat4f::mul(&viewport, &projection);
        let distance = self.camera_distance.clamp(
            self.camera_distance_min.max(0.01),
            self.camera_distance_max
                .max(self.camera_distance_min.max(0.01) + 0.01),
        );
        let cos_pitch = self.orbit_pitch.clamp(-1.45, 1.45).cos();
        let camera_pos = vec3(
            distance * self.orbit_yaw.sin() * cos_pitch,
            distance * self.orbit_pitch.sin(),
            distance * self.orbit_yaw.cos() * cos_pitch,
        );
        let view = Mat4f::look_at(camera_pos, vec3(0.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0));

        Some(SceneState3D {
            time: 0.0,
            camera_pos,
            view,
            projection,
            projection_viewport,
            clip_ndc,
            depth_range: self.depth_range,
            depth_forward_bias: self.depth_forward_bias,
            use_pass_camera: false,
            viewport_rect: rect,
        })
    }

    fn preview_gate_rect(&self, rect: Rect) -> Rect {
        let target_aspect = self.preview_aspect_ratio as f64;
        if target_aspect <= 0.0 || rect.size.x <= 1.0 || rect.size.y <= 1.0 {
            return rect;
        }

        let rect_aspect = (rect.size.x / rect.size.y).max(0.001);
        if self.preview_aspect_fill && rect_aspect > target_aspect {
            let height = rect.size.x / target_aspect;
            Rect {
                pos: dvec2(rect.pos.x, rect.pos.y + (rect.size.y - height) * 0.5),
                size: dvec2(rect.size.x, height),
            }
        } else if rect_aspect > target_aspect {
            let width = rect.size.y * target_aspect;
            Rect {
                pos: dvec2(rect.pos.x + (rect.size.x - width) * 0.5, rect.pos.y),
                size: dvec2(width, rect.size.y),
            }
        } else {
            let height = rect.size.x / target_aspect;
            Rect {
                pos: dvec2(rect.pos.x, rect.pos.y + (rect.size.y - height) * 0.5),
                size: dvec2(rect.size.x, height),
            }
        }
    }

    fn xr_scene_state(&self, state: &XrState) -> SceneState3D {
        SceneState3D {
            time: state.time,
            camera_pos: state.head_pose.position,
            view: Mat4f::identity(),
            projection: Mat4f::identity(),
            projection_viewport: Mat4f::identity(),
            clip_ndc: vec4(-1.0, -1.0, 1.0, 1.0),
            depth_range: self.depth_range,
            depth_forward_bias: self.depth_forward_bias,
            use_pass_camera: true,
            viewport_rect: Rect::default(),
        }
    }

    fn rotation_quat(rot: Vec3f) -> Quat {
        let x = Quat::from_axis_angle(vec3f(1.0, 0.0, 0.0), rot.x);
        let y = Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), rot.y);
        let z = Quat::from_axis_angle(vec3f(0.0, 0.0, 1.0), rot.z);
        Quat::multiply(&z, &Quat::multiply(&y, &x))
    }

    fn transform_with_node(parent: XrTransformState, node: &XrNode) -> XrTransformState {
        let local_pos = vec3f(
            node.pos().x * parent.scale.x,
            node.pos().y * parent.scale.y,
            node.pos().z * parent.scale.z,
        );
        let rotated_pos = parent.orientation.rotate_vec3(&local_pos);
        let orientation = Quat::multiply(&Self::rotation_quat(node.rot()), &parent.orientation);
        XrTransformState {
            position: parent.position + rotated_pos,
            orientation,
            scale: vec3f(
                parent.scale.x * node.scale().x,
                parent.scale.y * node.scale().y,
                parent.scale.z * node.scale().z,
            ),
        }
    }

    fn collect_cubes_from_widget(
        widget: &WidgetRef,
        parent: XrTransformState,
        cubes: &mut Vec<CollectedXrCube>,
    ) {
        if let Some(cube) = widget.borrow::<Cube>() {
            let node = cube.node();
            let world = Self::transform_with_node(parent, node);
            let half_extents = cube.half_extents();
            cubes.push(CollectedXrCube {
                uid: cube.widget_uid(),
                body_kind: node.body_kind(),
                pose: Pose::new(world.orientation, world.position),
                scale: world.scale,
                half_extents: vec3f(
                    half_extents.x * world.scale.x,
                    half_extents.y * world.scale.y,
                    half_extents.z * world.scale.z,
                ),
                density: node.density(),
                friction: node.friction(),
                restitution: node.restitution(),
            });
            let world = world;
            drop(cube);
            widget.children(&mut |_, child| Self::collect_cubes_from_widget(&child, world, cubes));
            return;
        }

        if let Some(cube) = widget.borrow::<RefractiveCube>() {
            let node = cube.node();
            let world = Self::transform_with_node(parent, node);
            let half_extents = cube.half_extents();
            cubes.push(CollectedXrCube {
                uid: cube.widget_uid(),
                body_kind: node.body_kind(),
                pose: Pose::new(world.orientation, world.position),
                scale: world.scale,
                half_extents: vec3f(
                    half_extents.x * world.scale.x,
                    half_extents.y * world.scale.y,
                    half_extents.z * world.scale.z,
                ),
                density: node.density(),
                friction: node.friction(),
                restitution: node.restitution(),
            });
            let world = world;
            drop(cube);
            widget.children(&mut |_, child| Self::collect_cubes_from_widget(&child, world, cubes));
            return;
        }

        if let Some(node) = widget.borrow::<XrNode>() {
            let world = Self::transform_with_node(parent, &node);
            let half_extents = node.physics_half_extents();
            if node.body_kind() != XrBodyKind::Disabled
                && (half_extents.x > 0.0 || half_extents.y > 0.0 || half_extents.z > 0.0)
            {
                cubes.push(CollectedXrCube {
                    uid: node.widget_uid(),
                    body_kind: node.body_kind(),
                    pose: Pose::new(world.orientation, world.position),
                    scale: world.scale,
                    half_extents: vec3f(
                        half_extents.x * world.scale.x,
                        half_extents.y * world.scale.y,
                        half_extents.z * world.scale.z,
                    ),
                    density: node.density(),
                    friction: node.friction(),
                    restitution: node.restitution(),
                });
            }
            drop(node);
            widget.children(&mut |_, child| Self::collect_cubes_from_widget(&child, world, cubes));
            return;
        }

        widget.children(&mut |_, child| Self::collect_cubes_from_widget(&child, parent, cubes));
    }

    fn collect_rendered_cubes(&self) -> Vec<CollectedXrCube> {
        let mut cubes = Vec::new();
        let root = XrTransformState::default();
        self.node
            .children(&mut |_, child| Self::collect_cubes_from_widget(&child, root, &mut cubes));
        cubes
    }

    fn sync_runtime_bodies(&mut self) {
        self.runtime_bodies.clear();
        let Some(scene) = self.scene.as_ref() else {
            return;
        };
        for cube in &scene.cubes {
            if let Some(body) = scene.bodies.get(cube.body) {
                self.runtime_bodies.insert(
                    cube.widget_uid,
                    XrRuntimeBodyState {
                        pose: makepad_pose(body.position()),
                        scale: cube.scale,
                    },
                );
            }
        }
    }

    fn rebuild_runtime_scene(&mut self, cx: &mut Cx) {
        let cubes = self.collect_rendered_cubes();
        let mut scene = RapierScene::new(self.physics.gravity);
        for cube in cubes {
            match cube.body_kind {
                XrBodyKind::Disabled => {}
                XrBodyKind::Dynamic => scene.spawn_dynamic_box(
                    cube.uid,
                    cube.pose,
                    cube.half_extents,
                    cube.scale,
                    cube.density,
                    cube.friction,
                    cube.restitution,
                ),
                XrBodyKind::Fixed => scene.spawn_fixed_box(
                    cube.uid,
                    cube.pose,
                    cube.half_extents,
                    cube.scale,
                    cube.friction,
                    cube.restitution,
                ),
            }
        }
        self.scene = Some(scene);
        self.scene_dirty = false;
        self.sync_runtime_bodies();
        self.redraw(cx);
    }

    fn ensure_runtime_scene(&mut self, cx: &mut Cx) {
        if self.scene_dirty || self.scene.is_none() {
            self.rebuild_runtime_scene(cx);
        }
    }

    fn ensure_preview_pass_resources(&mut self, cx: &mut Cx) {
        if self.preview_color_texture.is_none() {
            let texture = Texture::new_with_format(
                cx,
                TextureFormat::RenderBGRAu8 {
                    size: TextureSize::Auto,
                    initial: true,
                },
            );
            self.preview_pass.set_color_texture(
                cx,
                &texture,
                DrawPassClearColor::ClearWith(vec4(0.0902, 0.1137, 0.1490, 1.0)),
            );
            self.preview_color_texture = Some(texture);
        }
        if self.preview_depth_texture.is_none() {
            let texture = Texture::new_with_format(
                cx,
                TextureFormat::DepthD32 {
                    size: TextureSize::Auto,
                    initial: true,
                },
            );
            self.preview_pass
                .set_depth_texture(cx, &texture, DrawPassClearDepth::ClearWith(1.0));
            self.preview_depth_texture = Some(texture);
        }
    }

    fn update_preview_pass_camera(&self, cx: &mut Cx, scene_state: SceneState3D) {
        let camera_inv = scene_state.view.invert();
        let pass_uniforms = &mut cx.passes[self.preview_pass.draw_pass_id()].pass_uniforms;
        pass_uniforms.camera_projection = scene_state.projection;
        pass_uniforms.camera_projection_r = scene_state.projection;
        pass_uniforms.camera_view = scene_state.view;
        pass_uniforms.camera_view_r = scene_state.view;
        pass_uniforms.depth_projection = scene_state.projection;
        pass_uniforms.depth_projection_r = scene_state.projection;
        pass_uniforms.depth_view = scene_state.view;
        pass_uniforms.depth_view_r = scene_state.view;
        pass_uniforms.camera_inv = camera_inv;
        pass_uniforms.camera_inv_r = camera_inv;
    }
}

impl Widget for XrScene {
    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(render) || method == live_id!(render_scene) {
            self.scene_dirty = true;
            return self.node.script_call(vm, live_id!(render), args);
        }
        ScriptAsyncResult::MethodNotFound
    }

    fn script_result(&mut self, vm: &mut ScriptVm, id: ScriptAsyncId, result: ScriptValue) {
        self.node.script_result(vm, id, result);
        self.scene_dirty = true;
        vm.with_cx_mut(|cx| self.ensure_runtime_scene(cx));
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.node.handle_event(cx, event, scope);

        match event {
            Event::Startup => {
                self.next_frame = cx.new_next_frame();
                self.ensure_runtime_scene(cx);
            }
            Event::NextFrame(ne) if self.next_frame.is_event(event).is_some() => {
                if !cx.in_xr_mode() && self.should_preview_step() {
                    if let Some(scene) = self.scene.as_mut() {
                        scene.step();
                    }
                    self.sync_runtime_bodies();
                    self.area.redraw(cx);
                }
                self.next_frame = cx.new_next_frame();
                let _ = ne;
            }
            Event::XrUpdate(update) => {
                self.last_xr_state = Some(update.state.clone());
                if Self::reset_requested(update) {
                    self.reset_scene(cx);
                }
                self.ensure_runtime_scene(cx);
                self.sync_hands(&update.state);
                self.sync_depth_query_surfaces(cx);
                if let Some(scene) = self.scene.as_mut() {
                    scene.step();
                }
                self.sync_runtime_bodies();
                self.sync_passthrough_camera(cx);
                self.redraw(cx);
            }
            Event::PermissionResult(result) if result.permission == Permission::HeadsetCamera => {
                self.passthrough_camera_permission = Some(result.status);
                self.sync_passthrough_camera(cx);
                self.redraw(cx);
            }
            Event::VideoInputs(ev) => {
                self.passthrough_camera_failed = false;
                self.passthrough_camera_choice = Self::pick_passthrough_camera_choice(ev);
                if self.passthrough_camera_choice.is_none() {
                    crate::warning!("XR passthrough camera: no suitable camera choice found");
                }
                self.sync_passthrough_camera(cx);
                self.redraw(cx);
            }
            Event::VideoYuvTexturesReady(ev) if ev.video_id == Self::passthrough_video_id() => {
                if let Some(textures) = self.passthrough_camera_textures.as_mut() {
                    textures.tex_y = Some(ev.tex_y.clone());
                    textures.tex_u = Some(ev.tex_u.clone());
                    textures.tex_v = Some(ev.tex_v.clone());
                }
                self.redraw(cx);
            }
            Event::VideoTextureUpdated(ev) if ev.video_id == Self::passthrough_video_id() => {
                self.passthrough_camera_video = ev.yuv;
                self.passthrough_camera_has_frame = true;
                self.redraw(cx);
            }
            Event::VideoPlaybackPrepared(ev) if ev.video_id == Self::passthrough_video_id() => {
                self.passthrough_camera_source_size =
                    vec2f(ev.video_width as f32, ev.video_height as f32);
                self.redraw(cx);
            }
            Event::VideoPlaybackResourcesReleased(ev)
                if ev.video_id == Self::passthrough_video_id() =>
            {
                self.reset_passthrough_camera_state();
                self.redraw(cx);
            }
            Event::VideoDecodingError(ev) if ev.video_id == Self::passthrough_video_id() => {
                crate::warning!("XR passthrough camera error: {}", ev.error);
                self.passthrough_camera_playback_requested = false;
                self.passthrough_camera_failed = true;
                self.passthrough_camera_has_frame = false;
                self.redraw(cx);
            }
            _ => {}
        }

        match event.hits_with_capture_overload(cx, self.area, true) {
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.drag_last_abs = Some(fe.abs);
                cx.set_cursor(MouseCursor::Grabbing);
            }
            Hit::FingerMove(fe) => {
                if let Some(last_abs) = self.drag_last_abs {
                    let delta = fe.abs - last_abs;
                    let sensitivity = 0.01_f32;
                    self.orbit_yaw -= (delta.x as f32) * sensitivity;
                    self.orbit_pitch =
                        (self.orbit_pitch + (delta.y as f32) * sensitivity).clamp(-1.45, 1.45);
                    self.drag_last_abs = Some(fe.abs);
                    self.area.redraw(cx);
                }
            }
            Hit::FingerScroll(fs) => {
                let scroll = if fs.scroll.y.abs() > f64::EPSILON {
                    fs.scroll.y
                } else {
                    fs.scroll.x
                };
                let step = self.wheel_zoom_step.max(0.001);
                let zoom_factor = if scroll > 0.0 {
                    1.0 / (1.0 - step)
                } else {
                    1.0 - step
                };
                self.camera_distance = (self.camera_distance * zoom_factor).clamp(
                    self.camera_distance_min.max(0.01),
                    self.camera_distance_max
                        .max(self.camera_distance_min.max(0.01) + 0.01),
                );
                self.area.redraw(cx);
            }
            Hit::FingerUp(fe) => {
                if self.drag_last_abs.take().is_some() {
                    if fe.is_over {
                        cx.set_cursor(MouseCursor::Grab);
                    } else {
                        cx.set_cursor(MouseCursor::Default);
                    }
                }
            }
            Hit::FingerHoverIn(_) => {
                if self.drag_last_abs.is_some() {
                    cx.set_cursor(MouseCursor::Grabbing);
                } else {
                    cx.set_cursor(MouseCursor::Grab);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.drag_last_abs.is_none() {
                    cx.set_cursor(MouseCursor::Default);
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ensure_runtime_scene(cx.cx);
        let rect = cx.walk_turtle(walk);
        self.draw_bg.draw_abs(cx, rect);
        self.area = self.draw_bg.area();

        self.ensure_preview_pass_resources(cx.cx);
        cx.make_child_pass(&self.preview_pass);
        cx.set_pass_area(&self.preview_pass, self.area);
        cx.begin_pass(&self.preview_pass, None);

        let preview_pass_size = cx.current_pass_size();
        let preview_bounds = Rect {
            pos: dvec2(0.0, 0.0),
            size: preview_pass_size,
        };
        let preview_rect = self.preview_gate_rect(preview_bounds);

        if let Some(scene_state) = self.preview_scene_state(preview_rect, preview_pass_size) {
            self.update_preview_pass_camera(cx.cx, scene_state);
            let mut draw_scope = XrDrawScopeData {
                runtime_bodies: self.runtime_bodies.clone(),
                env_texture: None,
                camera_texture: None,
                camera_source_size: vec2f(1280.0, 960.0),
                camera_rotation_steps: 0.0,
                camera_center_offset_uv: vec2f(0.0, 0.0),
                camera_enabled: false,
            };
            self.draw_list_3d.begin_always(cx);
            let cx3d = &mut Cx3d::new(cx.cx);
            cx3d.begin_scene_3d(scene_state);
            let mut scene_scope = Scope::with_data(&mut draw_scope);
            self.node.draw_3d_all(cx3d, &mut scene_scope);
            cx3d.end_scene_3d();
            self.draw_list_3d.end(cx);
        }
        cx.end_pass(&self.preview_pass);

        if let Some(texture) = self.preview_color_texture.as_ref() {
            self.preview_image.draw_vars.set_texture(0, texture);
            self.preview_image.draw_abs(cx, rect);
            self.area = self.preview_image.area();
        }

        DrawStep::done()
    }

    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        self.ensure_runtime_scene(cx.cx);
        let Some(state) = self.last_xr_state.clone() else {
            return DrawStep::done();
        };
        let root_options = xr_root_options_from_scope(scope);
        self.xr_depth_mesh_enabled = root_options.depth_mesh;

        let cx2d = &mut Cx2d::new(cx.cx);
        if self.depth_debug_enabled() {
            self.prepare_depth_mesh(cx2d);
            self.sync_depth_surface_mesh(cx2d);
            self.draw_depth_surface_mesh(cx2d);
        }

        if XR_RENDER_HAND_GEOMETRY {
            self.prepare_pbr(cx2d);
            let left_colliders = self
                .scene
                .as_ref()
                .map(|scene| Self::collect_live_hand_colliders(scene, &scene.left_hand));
            let right_colliders = self
                .scene
                .as_ref()
                .map(|scene| Self::collect_live_hand_colliders(scene, &scene.right_hand));
            self.draw_hand(cx2d, &state.left_hand, left_colliders.as_deref(), true);
            self.draw_hand(cx2d, &state.right_hand, right_colliders.as_deref(), false);
        }

        let env_texture = if root_options.env_cube {
            self.render_passthrough_env_atlas(cx2d, state.as_ref())
        } else {
            None
        };

        let scene_state = self.xr_scene_state(state.as_ref());
        let mut draw_scope = XrDrawScopeData {
            runtime_bodies: self.runtime_bodies.clone(),
            env_texture,
            camera_texture: self
                .passthrough_camera_textures
                .as_ref()
                .map(|textures| textures.camera.clone()),
            camera_source_size: self.passthrough_camera_source_size,
            camera_rotation_steps: self.passthrough_camera_video.rotation_steps,
            camera_center_offset_uv: self.passthrough_camera_center_offset_uv(),
            camera_enabled: self.passthrough_camera_has_frame,
        };
        cx.begin_scene_3d(scene_state);
        let mut scene_scope = Scope::with_data(&mut draw_scope);
        self.node.draw_3d_all(cx, &mut scene_scope);
        cx.end_scene_3d();
        DrawStep::done()
    }
}
