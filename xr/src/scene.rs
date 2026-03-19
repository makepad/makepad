use super::passthrough_cube::DrawPassthroughCubeAtlas;
use self::{
    depth::{DepthSurfaceMeshChunkHandle, RetainedDepthQueryHit},
    passthrough::{
        XrPassthroughCameraChoice, XrPassthroughCameraTextures, XrPassthroughEnvAtlas,
    },
    physics::{makepad_pose, RapierScene},
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
use std::collections::HashMap;

#[path = "depth.rs"]
mod depth;
#[path = "hands.rs"]
mod hands;
#[path = "passthrough.rs"]
mod passthrough;
#[path = "physics.rs"]
mod physics;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom
    use mod.prelude.widgets.*
    use mod.widgets.*

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
        draw_passthrough_cube_atlas +: {
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

const XR_SCENE_FORWARD_OFFSET: f32 = 0.55;
const XR_SCENE_VERTICAL_OFFSET: f32 = 0.30;
const XR_SCENE_HEAD_HEIGHT_SCALE: f32 = 0.5;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum XrDepthDebugMode {
    Passthrough,
}

impl Default for XrDepthDebugMode {
    fn default() -> Self {
        Self::Passthrough
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
    #[redraw]
    #[live]
    draw_pbr_refractive: DrawPbrRefractive,
    #[redraw]
    #[live]
    draw_depth_mesh: DrawDepthMeshBasic,
    #[redraw]
    #[live]
    draw_passthrough_cube_atlas: DrawPassthroughCubeAtlas,
    #[rust]
    scene: Option<RapierScene>,
    #[rust]
    depth_surface_mesh_generation: u64,
    #[rust]
    depth_surface_mesh_update_sequence: u64,
    #[rust]
    depth_surface_mesh_chunks: HashMap<(i32, i32, i32), (Geometry, DepthSurfaceMeshChunkHandle)>,
    #[rust]
    depth_surface_mesh_upload_count: usize,
    #[rust]
    depth_query_retained_hits: HashMap<u64, RetainedDepthQueryHit>,
    #[rust]
    depth_debug_mode: XrDepthDebugMode,
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
}

impl XrScene {
    pub fn reset_requested(update: &XrUpdateEvent) -> bool {
        update.clicked_menu()
    }

    fn depth_debug_enabled(&self) -> bool {
        let _ = self.depth_debug_mode;
        false
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

    fn prepare_depth_mesh(&mut self, cx: &mut Cx2d) {
        self.draw_depth_mesh.draw_vars.options.depth_write = true;
        let _ = cx;
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

    pub fn default_scene_center(state: &XrState) -> Vec3f {
        let mut forward = state.vec_in_head_space(vec3(0.0, 0.0, -1.0)) - state.head_pose.position;
        forward.y = 0.0;
        if forward.length() <= 1.0e-4 {
            forward = vec3f(0.0, 0.0, -1.0);
        } else {
            forward = forward.normalize();
        }
        vec3f(
            state.head_pose.position.x,
            state.head_pose.position.y * XR_SCENE_HEAD_HEIGHT_SCALE,
            state.head_pose.position.z,
        ) + forward * XR_SCENE_FORWARD_OFFSET
            + vec3f(0.0, XR_SCENE_VERTICAL_OFFSET, 0.0)
    }

    pub fn ensure_scene(&mut self, state: &XrState) -> bool {
        if self.scene.is_some() {
            return false;
        }
        self.scene = Some(RapierScene::new(Self::default_scene_center(state)));
        true
    }

    pub fn dynamic_box_count(&self) -> usize {
        self.scene.as_ref().map(|scene| scene.cubes.len()).unwrap_or(0)
    }

    pub fn spawn_dynamic_box(&mut self, pose: Pose, half_extents: Vec3f) {
        if let Some(scene) = self.scene.as_mut() {
            scene.spawn_dynamic_box(pose, half_extents);
        }
    }

    pub fn spawn_fixed_box(&mut self, pose: Pose, half_extents: Vec3f, friction: f32) {
        if let Some(scene) = self.scene.as_mut() {
            scene.spawn_fixed_box(pose, half_extents, friction);
        }
    }

    pub fn dynamic_box_states(&self) -> Vec<(Pose, Vec3f)> {
        self.scene
            .as_ref()
            .map(|scene| {
                scene
                    .cubes
                    .iter()
                    .filter_map(|cube| {
                        scene
                            .bodies
                            .get(cube.body)
                            .map(|body| (makepad_pose(body.position()), cube.half_extents))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn draw_rounded_cube(
        &mut self,
        cx: &mut Cx2d,
        pose: Pose,
        half_extents: Vec3f,
        radius: f32,
        color: Vec4f,
        roughness: f32,
    ) {
        self.prepare_pbr(cx);
        self.draw_pbr_rounded_cube(cx, pose, half_extents, radius, color, roughness);
    }

    pub fn draw_pose_boxes<I>(&mut self, cx: &mut Cx2d, boxes: I)
    where
        I: IntoIterator<Item = (Pose, Vec3f, Vec4f, f32)>,
    {
        self.draw_cube.begin_many_instances(cx);
        for (pose, size, color, depth_clip) in boxes {
            self.draw_pose_box(cx, pose, size, color, depth_clip);
        }
        self.draw_cube.end_many_instances(cx);
    }

    fn reset_scene(&mut self, cx: &mut Cx, state: &XrState) {
        self.depth_debug_mode = XrDepthDebugMode::Passthrough;
        if let Some(atlas) = self.passthrough_env_atlas.as_mut() {
            atlas.reset_state();
        }
        self.clear_depth_surface_mesh();
        let _ = self.scene.take();
        self.sync_passthrough_camera(cx);
        let _ = state;
    }
}

impl Widget for XrScene {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event {
            Event::XrUpdate(e) => {
                if Self::reset_requested(e) {
                    self.reset_scene(cx, &e.state);
                }
                self.ensure_scene(&e.state);
                self.sync_hands(&e.state);
                self.sync_depth_query_surfaces(cx);
                if let Some(scene) = self.scene.as_mut() {
                    scene.step();
                }
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
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }

    fn draw_3d(&mut self, cx: &mut Cx3d, _scope: &mut Scope) -> DrawStep {
        let Some(state) = cx.draw_event.xr_state.as_ref() else {
            return DrawStep::done();
        };

        let cx = &mut Cx2d::new(cx.cx);
        if self.depth_debug_enabled() {
            self.prepare_depth_mesh(cx);
            self.sync_depth_surface_mesh(cx);
            self.draw_depth_surface_mesh(cx);
        }

        if XR_RENDER_HAND_GEOMETRY {
            self.prepare_pbr(cx);
            let left_colliders = self
                .scene
                .as_ref()
                .map(|scene| Self::collect_live_hand_colliders(scene, &scene.left_hand));
            let right_colliders = self
                .scene
                .as_ref()
                .map(|scene| Self::collect_live_hand_colliders(scene, &scene.right_hand));
            self.draw_hand(cx, &state.left_hand, left_colliders.as_deref(), true);
            self.draw_hand(cx, &state.right_hand, right_colliders.as_deref(), false);
        }

        DrawStep::done()
    }
}
