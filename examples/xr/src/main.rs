pub use makepad_widgets;

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
use std::{
    collections::{hash_map::DefaultHasher, HashMap, HashSet},
    hash::{Hash, Hasher},
};

app_main!(App);

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

    set_type_default() do #(DrawXrPassthroughQuad::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.PbrVertex, geom.PbrGeom)

        camera_texture: texture_video()

        v_uv: varying(vec2f)

        vertex: fn() {
            let world = vec4(
                self.geom.pos_nx.x,
                self.geom.pos_nx.y,
                self.geom.pos_nx.z,
                1.0
            );
            self.v_uv = self.geom.ny_nz_uv.zw;
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * world);
        }

        sample_camera_rgb: fn(coord: vec2f) -> vec3f {
            if self.camera_enabled > 0.5 {
                let coord_90 = vec2(1.0 - coord.y, coord.x);
                let coord_180 = vec2(1.0 - coord.x, 1.0 - coord.y);
                let coord_270 = vec2(coord.y, 1.0 - coord.x);
                let is_90 = step(0.5, self.rotation_steps) * step(self.rotation_steps, 1.5);
                let is_180 = step(1.5, self.rotation_steps) * step(self.rotation_steps, 2.5);
                let is_270 = step(2.5, self.rotation_steps);
                let is_0 = 1.0 - is_90 - is_180 - is_270;
                let sample_coord = coord * is_0 + coord_90 * is_90 + coord_180 * is_180 + coord_270 * is_270;
                let sample = self.camera_texture.sample_video(sample_coord).xyz;
                // Quest's external sampler path appears to expose the camera sample as U,Y,V.
                let y = (sample.y * 255.0 - 16.0) / 219.0;
                let u = (sample.x * 255.0 - 128.0) / 224.0;
                let v = (sample.z * 255.0 - 128.0) / 224.0;
                let r = y + 1.8556 * u;
                let g = y - 0.1873 * u - 0.4681 * v;
                let b = y + 1.5748 * v;
                return vec3(clamp(r, 0.0, 1.0), clamp(g, 0.0, 1.0), clamp(b, 0.0, 1.0));
            }

            return vec3(0.0, 0.0, 0.0);
        }

        frosted_offset: fn(seed: vec2f, scale: f32) -> vec2f {
            let texel = self.scatter_pixels / max(self.source_size, vec2(1.0, 1.0));
            let ox = Math.random_2d(seed + vec2(3.17, 9.41)) - 0.5;
            let oy = Math.random_2d(seed.yx + vec2(5.93, 1.27)) - 0.5;
            return vec2(ox, oy) * texel * scale;
        }

        pixel: fn() {
            let uv = clamp(self.v_uv, vec2(0.0, 0.0), vec2(1.0, 1.0));
            let seed = uv * self.source_size;
            let center = self.sample_camera_rgb(uv);
            let blur = center * 0.34
                + self.sample_camera_rgb(clamp(uv + self.frosted_offset(seed, 1.0), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.17
                + self.sample_camera_rgb(clamp(uv + self.frosted_offset(seed + vec2(17.0, 11.0), 1.4), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.17
                + self.sample_camera_rgb(clamp(uv + self.frosted_offset(seed + vec2(31.0, 7.0), 1.9), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.16
                + self.sample_camera_rgb(clamp(uv + self.frosted_offset(seed + vec2(13.0, 29.0), 2.3), vec2(0.0, 0.0), vec2(1.0, 1.0))) * 0.16;

            let frosted = mix(center, blur, self.frost_mix) * self.tint_color.xyz;
            let alpha = self.tint_color.w;
            return vec4(frosted * alpha, alpha);
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
        draw_passthrough_quad +: {
            source_size: vec2(1280.0, 960.0)
            tint_color: vec4(1.0, 1.0, 1.0, 1.0)
            frost_mix: 0.84
            scatter_pixels: 2.6
            camera_enabled: 0.0
            rotation_steps: 0.0
            biplanar: 0.0
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
const XR_CUBE_HALF_EXTENT: f32 = 0.020;
const XR_PLATFORM_HALF_WIDTH: f32 = 0.64;
const XR_PLATFORM_HALF_HEIGHT: f32 = 0.006;
const XR_PLATFORM_HALF_DEPTH: f32 = 0.16;
const XR_SCENE_FORWARD_OFFSET: f32 = 0.55;
const XR_SCENE_VERTICAL_OFFSET: f32 = 0.30;
const XR_SCENE_HEAD_HEIGHT_SCALE: f32 = 0.5;
const XR_SIMULATION_DT: f32 = 1.0 / 120.0;
const XR_ENABLE_HAND_PHYSICS: bool = true;
const XR_ENABLE_DEPTH_QUERY_PHYSICS: bool = true;
const XR_RENDER_HAND_GEOMETRY: bool = false;
const XR_PASSTHROUGH_QUAD_DISTANCE: f32 = 0.78;
const XR_PASSTHROUGH_QUAD_HEIGHT: f32 = 0.42;
const XR_PASSTHROUGH_QUAD_WORLD_OFFSET_Y: f32 = -0.145;
const XR_PASSTHROUGH_QUAD_WORLD_OFFSET_X: f32 = 0.0;
const XR_PASSTHROUGH_ENV_ATLAS_WIDTH: usize = 2048;
const XR_PASSTHROUGH_ENV_ATLAS_HEIGHT: usize = 1024;
const XR_PASSTHROUGH_ENV_CAMERA_FOV_Y_DEGREES: f32 = 92.0;
const XR_PASSTHROUGH_ENV_CAMERA_PROJECTION_SCALE: f32 = 0.6825;
const XR_PASSTHROUGH_CAMERA_EXPOSURE: f32 = 0.68;
const XR_PASSTHROUGH_ENV_UPDATE_STRENGTH: f32 = 0.92;
const XR_PASSTHROUGH_CUBE_HALF_EXTENT: f32 = 0.085;
const XR_PASSTHROUGH_CUBE_CORNER_RADIUS: f32 = 0.018;
const XR_PASSTHROUGH_CUBE_DISTANCE: f32 = 0.60;
const XR_PASSTHROUGH_CUBE_VERTICAL_OFFSET: f32 = -0.02;
const XR_PASSTHROUGH_CUBE_SPACING: f32 = 0.22;
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
const XR_PLATFORM_SPAWN_GAP: f32 = 0.0;
const XR_WALL_BRICK_HALF_WIDTH: f32 = XR_CUBE_HALF_EXTENT * 2.0;
const XR_WALL_BRICK_HALF_HEIGHT: f32 = XR_CUBE_HALF_EXTENT;
const XR_WALL_BRICK_HALF_DEPTH: f32 = XR_CUBE_HALF_EXTENT;
const XR_WALL_FULL_ROW_BRICKS: usize = 12;
const XR_WALL_SHORT_ROW_BRICKS: usize = 11;
const XR_WALL_ROWS: usize = 12;
const XR_WALL_ROTATION_Y: f32 = std::f32::consts::FRAC_PI_2;
const XR_PLATFORM_ROUND_RADIUS: f32 = 0.005;
const XR_PBR_FACE_SUBDIVISIONS: usize = 1;
const XR_PBR_CORNER_SEGMENTS: usize = 3;
const XR_PBR_HAND_CAPSULE_SUBDIVISIONS: usize = 8;
const XR_PBR_HAND_SPHERE_SUBDIVISIONS: usize = 8;
const XR_BRICK_VISUAL_SCALE: f32 = 0.98;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum AppPhase {
    #[default]
    Preflight,
    XrRuntime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum XrDepthDebugMode {
    Passthrough,
}

impl Default for XrDepthDebugMode {
    fn default() -> Self {
        Self::Passthrough
    }
}

#[derive(Clone)]
struct XrPassthroughCameraChoice {
    input_id: VideoInputId,
    format_id: VideoFormatId,
    width: usize,
    height: usize,
}

#[derive(Clone)]
struct XrPassthroughCameraTextures {
    camera: Texture,
    tex_y: Option<Texture>,
    tex_u: Option<Texture>,
    tex_v: Option<Texture>,
}

#[derive(Clone, Copy)]
struct XrPassthroughQuadPlacement {
    center: Vec3f,
    right: Vec3f,
    up: Vec3f,
    normal: Vec3f,
}

#[derive(Clone, Copy, Debug)]
enum HandCollider {
    Capsule { a: Vec3f, b: Vec3f, radius: f32 },
    Ball { center: Vec3f, radius: f32 },
    Box { pose: Pose, half_extents: Vec3f },
}

#[derive(Clone, Copy)]
struct PhysicsCube {
    body: RigidBodyHandle,
    collider: ColliderHandle,
    half_extents: Vec3f,
    color_index: usize,
}

#[derive(Clone, Copy)]
struct HandColliderBody {
    body: RigidBodyHandle,
    collider: ColliderHandle,
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

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawXrPassthroughQuad {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub source_size: Vec2f,
    #[live]
    pub tint_color: Vec4f,
    #[live]
    pub frost_mix: f32,
    #[live]
    pub scatter_pixels: f32,
    #[live]
    pub camera_enabled: f32,
    #[live]
    pub rotation_steps: f32,
    #[live]
    pub biplanar: f32,
    #[live]
    pub yuv_enabled: f32,
}

impl DrawXrPassthroughQuad {
    fn draw_geometry(&mut self, cx: &mut Cx2d, geometry_id: GeometryId) {
        self.draw_vars.append_group_id = cx.draw_call_group_background().0;
        self.draw_vars.geometry_id = Some(geometry_id);
        if cx.new_draw_call(&self.draw_vars).is_some() && self.draw_vars.can_instance() {
            let new_area = cx.add_aligned_instance(&self.draw_vars);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }
}

struct XrPassthroughEnvAtlas {
    pass: DrawPass,
    draw_list: DrawList2d,
    ping: Texture,
    pong: Texture,
    ping_is_current: bool,
    initialized: bool,
    pending_swap: bool,
}

impl XrPassthroughEnvAtlas {
    fn new(cx: &mut Cx) -> Self {
        let atlas_width = XR_PASSTHROUGH_ENV_ATLAS_WIDTH;
        let atlas_height = XR_PASSTHROUGH_ENV_ATLAS_HEIGHT;
        let ping = Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 {
                size: TextureSize::Fixed {
                    width: atlas_width,
                    height: atlas_height,
                },
                initial: true,
            },
        );
        let pong = Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 {
                size: TextureSize::Fixed {
                    width: atlas_width,
                    height: atlas_height,
                },
                initial: true,
            },
        );
        let pass = DrawPass::new_with_name(cx, "xr_passthrough_env_atlas");
        pass.set_size(cx, dvec2(atlas_width as f64, atlas_height as f64));
        Self {
            pass,
            draw_list: DrawList2d::new(cx),
            ping,
            pong,
            ping_is_current: true,
            initialized: false,
            pending_swap: false,
        }
    }

    fn current_texture(&self) -> &Texture {
        if self.ping_is_current {
            &self.ping
        } else {
            &self.pong
        }
    }

    fn target_texture(&self) -> &Texture {
        if self.ping_is_current {
            &self.pong
        } else {
            &self.ping
        }
    }

    fn finish_frame(&mut self) {
        self.ping_is_current = !self.ping_is_current;
        self.initialized = true;
        self.pending_swap = false;
    }
}

#[derive(Clone, Copy)]
struct DepthSurfaceMeshChunkHandle {
    geometry_id: GeometryId,
    fingerprint: u64,
}

#[derive(Clone, Copy)]
struct DepthQuerySurfaceCollider {
    collider: ColliderHandle,
    fingerprint: u64,
}

#[derive(Clone, Copy)]
enum DepthQuerySurfaceShape {
    Triangle([Vec3f; 3]),
    Quad([Vec3f; 4]),
}

#[derive(Clone, Copy)]
struct DepthQuerySurfaceTarget {
    shape: DepthQuerySurfaceShape,
    fingerprint: u64,
}

#[derive(Clone, Copy)]
struct DepthQuerySurfaceCandidate {
    key: u64,
    distance: f32,
    shape: DepthQuerySurfaceShape,
    fingerprint: u64,
}

#[derive(Clone)]
struct RetainedDepthQueryHit {
    hit: XrDepthMeshQueryHit,
    misses_left: u8,
}

impl RetainedDepthQueryHit {
    fn new(hit: XrDepthMeshQueryHit) -> Self {
        Self {
            hit,
            misses_left: XR_DEPTH_QUERY_HIT_MISS_GRACE_FRAMES,
        }
    }

    fn reuse_result(&mut self) -> Option<XrDepthMeshQueryResult> {
        if self.misses_left == 0 {
            return None;
        }
        self.misses_left -= 1;
        Some(XrDepthMeshQueryResult::Hit(self.hit.clone()))
    }
}

struct RapierScene {
    gravity: RapierVector,
    integration_parameters: IntegrationParameters,
    pipeline: PhysicsPipeline,
    islands: IslandManager,
    broad_phase: BroadPhaseBvh,
    narrow_phase: NarrowPhase,
    bodies: RigidBodySet,
    colliders: ColliderSet,
    impulse_joints: ImpulseJointSet,
    multibody_joints: MultibodyJointSet,
    ccd_solver: CCDSolver,
    cubes: Vec<PhysicsCube>,
    depth_query_surfaces: Vec<DepthQuerySurfaceCollider>,
    left_hand: Vec<HandColliderBody>,
    right_hand: Vec<HandColliderBody>,
    platform_pose: Pose,
}

fn rapier_vec3(v: Vec3f) -> RapierVector {
    RapierVector::new(v.x, v.y, v.z)
}

fn rapier_rotation(q: Quat) -> RapierRotation {
    RapierRotation::from_xyzw(q.x, q.y, q.z, q.w)
}

fn rapier_pose(pose: Pose) -> RapierPose {
    RapierPose::from_parts(
        rapier_vec3(pose.position),
        rapier_rotation(pose.orientation),
    )
}

fn makepad_pose(pose: &RapierPose) -> Pose {
    Pose::new(
        Quat {
            x: pose.rotation.x,
            y: pose.rotation.y,
            z: pose.rotation.z,
            w: pose.rotation.w,
        },
        vec3f(pose.translation.x, pose.translation.y, pose.translation.z),
    )
}

fn capsule_pose(a: Vec3f, b: Vec3f) -> (RapierPose, RapierReal) {
    let delta = b - a;
    let length = delta.length();
    let rotation = if length > 1.0e-4 {
        RapierRotation::from_rotation_arc(RapierVector::Y, rapier_vec3(delta * (1.0 / length)))
    } else {
        RapierRotation::IDENTITY
    };
    (
        RapierPose::from_parts(rapier_vec3((a + b) * 0.5), rotation),
        (length * 0.5).max(0.0005),
    )
}

fn quantize_depth_query_value(value: f32) -> i32 {
    (value / XR_DEPTH_QUERY_FINGERPRINT_QUANTIZATION_METERS).round() as i32
}

fn depth_query_triangle_fingerprint(triangle: [Vec3f; 3]) -> u64 {
    let mut vertices = triangle.map(|vertex| {
        [
            quantize_depth_query_value(vertex.x),
            quantize_depth_query_value(vertex.y),
            quantize_depth_query_value(vertex.z),
        ]
    });
    vertices.sort_unstable();
    let mut hasher = DefaultHasher::new();
    vertices.hash(&mut hasher);
    hasher.finish()
}

fn depth_query_quad_fingerprint(quad: [Vec3f; 4]) -> u64 {
    let mut vertices = quad.map(|vertex| {
        [
            quantize_depth_query_value(vertex.x),
            quantize_depth_query_value(vertex.y),
            quantize_depth_query_value(vertex.z),
        ]
    });
    vertices.sort_unstable();
    let mut hasher = DefaultHasher::new();
    vertices.hash(&mut hasher);
    hasher.finish()
}

fn depth_query_patch_is_degenerate(patch: [Vec3f; 4]) -> bool {
    let epsilon = 1.0e-4;
    (patch[1] - patch[0]).length() <= epsilon
        && (patch[2] - patch[0]).length() <= epsilon
        && (patch[3] - patch[0]).length() <= epsilon
}

fn depth_query_surface_candidate(
    key: u64,
    distance: f32,
    from_planar_patch: bool,
    triangle: [Vec3f; 3],
    patch: [Vec3f; 4],
) -> DepthQuerySurfaceCandidate {
    let (shape, fingerprint) = if from_planar_patch && !depth_query_patch_is_degenerate(patch) {
        (
            DepthQuerySurfaceShape::Quad(patch),
            depth_query_quad_fingerprint(patch),
        )
    } else {
        (
            DepthQuerySurfaceShape::Triangle(triangle),
            depth_query_triangle_fingerprint(triangle),
        )
    };
    DepthQuerySurfaceCandidate {
        key,
        distance,
        shape,
        fingerprint,
    }
}

fn extend_depth_query_surface_candidates(
    candidates: &mut Vec<DepthQuerySurfaceCandidate>,
    hit: &XrDepthMeshQueryHit,
) {
    candidates.push(depth_query_surface_candidate(
        hit.key,
        hit.distance,
        hit.from_planar_patch,
        hit.triangle,
        hit.patch,
    ));
    candidates.extend(hit.additional_hits.iter().map(|extra| {
        depth_query_surface_candidate(
            hit.key,
            extra.distance,
            extra.from_planar_patch,
            extra.triangle,
            extra.patch,
        )
    }));
}

fn build_depth_query_surface_targets(
    results: &[XrDepthMeshQueryResult],
) -> Vec<DepthQuerySurfaceTarget> {
    let mut hits = Vec::new();
    for result in results {
        if let XrDepthMeshQueryResult::Hit(hit) = result {
            extend_depth_query_surface_candidates(&mut hits, hit);
        }
    }
    hits.sort_by(|a, b| {
        matches!(b.shape, DepthQuerySurfaceShape::Quad(_))
            .cmp(&matches!(a.shape, DepthQuerySurfaceShape::Quad(_)))
            .then_with(|| {
                a.distance
                    .partial_cmp(&b.distance)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.key.cmp(&b.key))
    });

    let mut seen = HashSet::new();
    let mut targets = Vec::with_capacity(XR_DEPTH_QUERY_SHARED_SURFACE_POOL_SIZE);
    for hit in hits {
        if !seen.insert(hit.fingerprint) {
            continue;
        }
        targets.push(DepthQuerySurfaceTarget {
            shape: hit.shape,
            fingerprint: hit.fingerprint,
        });
        if targets.len() >= XR_DEPTH_QUERY_SHARED_SURFACE_POOL_SIZE {
            break;
        }
    }
    targets
}

impl RapierScene {
    fn spawn_dynamic_box(&mut self, pose: Pose, half_extents: Vec3f) {
        let body = self.bodies.insert(
            RigidBodyBuilder::dynamic()
                .pose(rapier_pose(pose))
                .ccd_enabled(true)
                .linear_damping(XR_BODY_LINEAR_DAMPING)
                .angular_damping(XR_BODY_ANGULAR_DAMPING)
                .additional_solver_iterations(XR_BODY_ADDITIONAL_SOLVER_ITERATIONS),
        );
        if let Some(rigid_body) = self.bodies.get_mut(body) {
            let activation = rigid_body.activation_mut();
            activation.angular_threshold = XR_BODY_SLEEP_ANGULAR_THRESHOLD;
            activation.time_until_sleep = XR_BODY_SLEEP_TIME;
        }
        let collider = self.colliders.insert_with_parent(
            ColliderBuilder::cuboid(half_extents.x, half_extents.y, half_extents.z)
                .density(1.0)
                .friction(0.8)
                .restitution(0.0),
            body,
            &mut self.bodies,
        );
        self.cubes.push(PhysicsCube {
            body,
            collider,
            half_extents,
            color_index: self.cubes.len() % CUBE_COLORS.len(),
        });
    }

    fn new(center: Vec3f) -> Self {
        let scene_rotation = Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), XR_WALL_ROTATION_Y);
        let platform_center = center + vec3f(0.0, -XR_PLATFORM_HALF_HEIGHT, 0.0);
        let mut scene = Self {
            gravity: RapierVector::new(0.0, -9.81, 0.0),
            integration_parameters: IntegrationParameters {
                dt: XR_SIMULATION_DT,
                ..IntegrationParameters::default()
            },
            pipeline: PhysicsPipeline::new(),
            islands: IslandManager::new(),
            broad_phase: BroadPhaseBvh::new(),
            narrow_phase: NarrowPhase::new(),
            bodies: RigidBodySet::new(),
            colliders: ColliderSet::new(),
            impulse_joints: ImpulseJointSet::new(),
            multibody_joints: MultibodyJointSet::new(),
            ccd_solver: CCDSolver::new(),
            cubes: Vec::new(),
            depth_query_surfaces: Vec::new(),
            left_hand: Vec::new(),
            right_hand: Vec::new(),
            platform_pose: Pose::new(scene_rotation, platform_center),
        };

        let platform = scene
            .bodies
            .insert(RigidBodyBuilder::fixed().pose(rapier_pose(scene.platform_pose)));
        scene.colliders.insert_with_parent(
            ColliderBuilder::cuboid(
                XR_PLATFORM_HALF_WIDTH,
                XR_PLATFORM_HALF_HEIGHT,
                XR_PLATFORM_HALF_DEPTH,
            )
            .friction(0.9),
            platform,
            &mut scene.bodies,
        );

        // Invisible floor at XR ground level (y=0).
        let floor = scene.bodies.insert(RigidBodyBuilder::fixed().build());
        scene.colliders.insert_with_parent(
            ColliderBuilder::new(SharedShape::halfspace(RapierVector::new(0.0, 1.0, 0.0)))
                .friction(0.9),
            floor,
            &mut scene.bodies,
        );
        let brick_half_extents = vec3f(
            XR_WALL_BRICK_HALF_WIDTH,
            XR_WALL_BRICK_HALF_HEIGHT,
            XR_WALL_BRICK_HALF_DEPTH,
        );
        let brick_width = XR_WALL_BRICK_HALF_WIDTH * 2.0 + XR_PLATFORM_SPAWN_GAP;
        let brick_height = XR_WALL_BRICK_HALF_HEIGHT * 2.0 + XR_PLATFORM_SPAWN_GAP;
        let wall_origin = center;
        let row_axis = vec3f(0.0, 0.0, 1.0);
        let brick_rotation = scene_rotation;
        for row in 0..XR_WALL_ROWS {
            let bricks_in_row = if row % 2 == 0 {
                XR_WALL_FULL_ROW_BRICKS
            } else {
                XR_WALL_SHORT_ROW_BRICKS
            };
            let row_center_offset = (bricks_in_row as f32 - 1.0) * 0.5;
            for brick in 0..bricks_in_row {
                let brick_center = wall_origin
                    + row_axis * ((brick as f32 - row_center_offset) * brick_width)
                    + vec3f(
                        0.0,
                        XR_WALL_BRICK_HALF_HEIGHT
                            + XR_PLATFORM_SPAWN_GAP
                            + row as f32 * brick_height,
                        0.0,
                    );
                scene
                    .spawn_dynamic_box(Pose::new(brick_rotation, brick_center), brick_half_extents);
            }
        }

        if XR_ENABLE_DEPTH_QUERY_PHYSICS {
            scene.depth_query_surfaces = (0..XR_DEPTH_QUERY_SHARED_SURFACE_POOL_SIZE)
                .map(|_| scene.spawn_depth_query_surface())
                .collect();
        }
        if XR_ENABLE_HAND_PHYSICS {
            scene.left_hand = scene.spawn_hand_colliders(XR_HAND_COLLIDER_SLOTS_PER_HAND);
            scene.right_hand = scene.spawn_hand_colliders(XR_HAND_COLLIDER_SLOTS_PER_HAND);
        }
        scene.step();
        scene
    }

    fn spawn_hand_colliders(&mut self, count: usize) -> Vec<HandColliderBody> {
        let mut result = Vec::with_capacity(count);
        for _ in 0..count {
            let body = self
                .bodies
                .insert(RigidBodyBuilder::kinematic_position_based().pose(RapierPose::IDENTITY));
            let collider = self.colliders.insert_with_parent(
                ColliderBuilder::capsule_y(0.01, 0.01)
                    .friction(XR_HAND_COLLIDER_FRICTION)
                    .restitution(0.0),
                body,
                &mut self.bodies,
            );
            if let Some(collider) = self.colliders.get_mut(collider) {
                collider.set_enabled(false);
            }
            result.push(HandColliderBody { body, collider });
        }
        result
    }

    fn spawn_depth_query_surface(&mut self) -> DepthQuerySurfaceCollider {
        let body = self.bodies.insert(RigidBodyBuilder::fixed().build());
        let collider = self.colliders.insert_with_parent(
            ColliderBuilder::triangle(
                RapierVector::new(0.0, -1000.0, 0.0),
                RapierVector::new(0.0, -1000.0, 0.01),
                RapierVector::new(0.01, -1000.0, 0.0),
            )
            .friction(XR_DEPTH_QUERY_FRICTION),
            body,
            &mut self.bodies,
        );
        if let Some(collider) = self.colliders.get_mut(collider) {
            collider.set_enabled(false);
        }
        DepthQuerySurfaceCollider {
            collider,
            fingerprint: 0,
        }
    }

    fn sync_hand_bodies(
        bodies: &[HandColliderBody],
        colliders: &[HandCollider],
        rigid_bodies: &mut RigidBodySet,
        collider_set: &mut ColliderSet,
    ) {
        for (index, slot) in bodies.iter().enumerate() {
            let active = index < colliders.len();
            if active {
                let (target_pose, shape) = match colliders[index] {
                    HandCollider::Capsule { a, b, radius } => {
                        let (target_pose, half_height) = capsule_pose(a, b);
                        (target_pose, SharedShape::capsule_y(half_height, radius))
                    }
                    HandCollider::Ball { center, radius } => (
                        RapierPose::from_parts(rapier_vec3(center), RapierRotation::IDENTITY),
                        SharedShape::ball(radius),
                    ),
                    HandCollider::Box { pose, half_extents } => (
                        rapier_pose(pose),
                        SharedShape::cuboid(half_extents.x, half_extents.y, half_extents.z),
                    ),
                };
                let was_enabled = collider_set
                    .get(slot.collider)
                    .map(|collider| collider.is_enabled())
                    .unwrap_or(false);
                if let Some(collider) = collider_set.get_mut(slot.collider) {
                    collider.set_shape(shape);
                    collider.set_enabled(true);
                }
                if let Some(body) = rigid_bodies.get_mut(slot.body) {
                    if !was_enabled {
                        // Reset the body pose on reacquire so tracking loss doesn't inject a huge velocity spike.
                        body.set_position(target_pose, false);
                    }
                    body.set_next_kinematic_position(target_pose);
                }
            } else if let Some(collider) = collider_set.get_mut(slot.collider) {
                collider.set_enabled(false);
            }
        }
    }

    fn step(&mut self) {
        self.pipeline.step(
            self.gravity,
            &self.integration_parameters,
            &mut self.islands,
            &mut self.broad_phase,
            &mut self.narrow_phase,
            &mut self.bodies,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            &mut self.ccd_solver,
            &(),
            &(),
        );
        self.settle_resting_bodies();
    }

    fn settle_resting_bodies(&mut self) {
        let linear_speed_sq = XR_BODY_SNAP_SLEEP_LINEAR_SPEED * XR_BODY_SNAP_SLEEP_LINEAR_SPEED;
        let angular_speed_sq = XR_BODY_SNAP_SLEEP_ANGULAR_SPEED * XR_BODY_SNAP_SLEEP_ANGULAR_SPEED;
        let mut to_sleep = Vec::new();

        for cube in &self.cubes {
            let has_active_contact = self
                .narrow_phase
                .contact_pairs_with(cube.collider)
                .any(|pair| pair.has_any_active_contact());
            if !has_active_contact {
                continue;
            }

            let Some(body) = self.bodies.get(cube.body) else {
                continue;
            };
            if body.is_sleeping() {
                continue;
            }

            let linvel = body.linvel();
            let angvel = body.angvel();
            let linvel_sq = linvel.x * linvel.x + linvel.y * linvel.y + linvel.z * linvel.z;
            let angvel_sq = angvel.x * angvel.x + angvel.y * angvel.y + angvel.z * angvel.z;
            if linvel_sq <= linear_speed_sq && angvel_sq <= angular_speed_sq {
                to_sleep.push(cube.body);
            }
        }

        for handle in to_sleep {
            if let Some(body) = self.bodies.get_mut(handle) {
                body.set_linvel(RapierVector::ZERO, false);
                body.set_angvel(RapierVector::ZERO, false);
            }
        }
    }

    fn depth_query_key(index: usize) -> u64 {
        index as u64 + 1
    }

    fn clear_depth_query_surfaces(&mut self) {
        for surface in &mut self.depth_query_surfaces {
            if let Some(collider) = self.colliders.get_mut(surface.collider) {
                collider.set_enabled(false);
            }
            surface.fingerprint = 0;
        }
    }

    fn sync_depth_query_surface_pool(&mut self, targets: &[DepthQuerySurfaceTarget]) {
        for (surface, target) in self.depth_query_surfaces.iter_mut().zip(targets.iter()) {
            if surface.fingerprint != target.fingerprint {
                if let Some(collider) = self.colliders.get_mut(surface.collider) {
                    let shape = match target.shape {
                        DepthQuerySurfaceShape::Triangle(triangle) => SharedShape::triangle(
                            rapier_vec3(triangle[0]),
                            rapier_vec3(triangle[1]),
                            rapier_vec3(triangle[2]),
                        ),
                        DepthQuerySurfaceShape::Quad(quad) => SharedShape::trimesh(
                            vec![
                                rapier_vec3(quad[0]),
                                rapier_vec3(quad[1]),
                                rapier_vec3(quad[2]),
                                rapier_vec3(quad[3]),
                            ],
                            vec![[0, 1, 2], [0, 2, 3]],
                        )
                        .unwrap_or_else(|_| {
                            SharedShape::triangle(
                                rapier_vec3(quad[0]),
                                rapier_vec3(quad[1]),
                                rapier_vec3(quad[2]),
                            )
                        }),
                    };
                    collider.set_shape(shape);
                }
                surface.fingerprint = target.fingerprint;
            }
            if let Some(collider) = self.colliders.get_mut(surface.collider) {
                collider.set_enabled(true);
            }
        }
        for surface in self.depth_query_surfaces.iter_mut().skip(targets.len()) {
            if let Some(collider) = self.colliders.get_mut(surface.collider) {
                collider.set_enabled(false);
            }
            surface.fingerprint = 0;
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
    draw_passthrough_quad: DrawXrPassthroughQuad,
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
    passthrough_camera_quad: Option<Geometry>,
    #[rust]
    passthrough_env_atlas_quad: Option<Geometry>,
    #[rust]
    passthrough_quad_placement: Option<XrPassthroughQuadPlacement>,
    #[rust]
    passthrough_env_atlas: Option<XrPassthroughEnvAtlas>,
    #[rust]
    passthrough_cube_poses: [Option<Pose>; 4],
}

impl XrScene {
    fn current_passthrough_quad_placement(
        &self,
        state: &XrState,
    ) -> (XrPassthroughQuadPlacement, f32, f32) {
        let source_size = self.passthrough_camera_source_size;
        let aspect = if source_size.y > 1.0 {
            source_size.x / source_size.y
        } else {
            4.0 / 3.0
        };
        let half_height = XR_PASSTHROUGH_QUAD_DISTANCE
            * (XR_PASSTHROUGH_ENV_CAMERA_FOV_Y_DEGREES.to_radians() * 0.5).tan()
            * XR_PASSTHROUGH_ENV_CAMERA_PROJECTION_SCALE;
        let half_width = half_height * aspect;

        let (head, right, up, forward) = Self::current_head_basis(state);
        let center = head
            + forward * XR_PASSTHROUGH_QUAD_DISTANCE
            + right * XR_PASSTHROUGH_QUAD_WORLD_OFFSET_X
            + up * XR_PASSTHROUGH_QUAD_WORLD_OFFSET_Y;
        (
            XrPassthroughQuadPlacement {
                center,
                right,
                up,
                normal: -forward,
            },
            half_width,
            half_height,
        )
    }

    fn passthrough_camera_center_offset_uv(&self) -> Vec2f {
        let source_size = self.passthrough_camera_source_size;
        let aspect = if source_size.y > 1.0 {
            source_size.x / source_size.y
        } else {
            4.0 / 3.0
        };
        let half_height = XR_PASSTHROUGH_QUAD_DISTANCE
            * (XR_PASSTHROUGH_ENV_CAMERA_FOV_Y_DEGREES.to_radians() * 0.5).tan()
            * XR_PASSTHROUGH_ENV_CAMERA_PROJECTION_SCALE;
        let half_width = half_height * aspect;
        vec2f(
            -XR_PASSTHROUGH_QUAD_WORLD_OFFSET_X / (2.0 * half_width.max(0.0001)),
            XR_PASSTHROUGH_QUAD_WORLD_OFFSET_Y / (2.0 * half_height.max(0.0001)),
        )
    }

    fn depth_debug_enabled(&self) -> bool {
        let _ = self.depth_debug_mode;
        false
    }

    fn passthrough_debug_enabled(&self) -> bool {
        let _ = self.depth_debug_mode;
        true
    }

    fn passthrough_video_id() -> LiveId {
        live_id!(xr_passthrough_camera)
    }

    fn pick_passthrough_camera_choice(ev: &VideoInputsEvent) -> Option<XrPassthroughCameraChoice> {
        fn better(a: &makepad_widgets::makepad_platform::video::VideoFormat, b: &makepad_widgets::makepad_platform::video::VideoFormat) -> bool {
            let a_is_preferred_square = a.width == 1280 && a.height == 1280;
            let b_is_preferred_square = b.width == 1280 && b.height == 1280;
            if a_is_preferred_square != b_is_preferred_square {
                return a_is_preferred_square;
            }

            let a_fits_cap = a.width <= 1920 && a.height <= 1920;
            let b_fits_cap = b.width <= 1920 && b.height <= 1920;
            if a_fits_cap != b_fits_cap {
                return a_fits_cap;
            }

            let a_is_square = a.width == a.height;
            let b_is_square = b.width == b.height;
            if a_is_square != b_is_square {
                return a_is_square;
            }
            let a_pixels = a.width * a.height;
            let b_pixels = b.width * b.height;
            if a_pixels != b_pixels {
                return a_pixels > b_pixels;
            }
            a.frame_rate.unwrap_or(0.0) > b.frame_rate.unwrap_or(0.0)
        }

        let desc = ev
            .descs
            .iter()
            .find(|desc| desc.name == "Back Camera")
            .or_else(|| ev.descs.iter().find(|desc| desc.name == "External Camera"))
            .or_else(|| ev.descs.first())?;

        let mut best = None;
        for format in &desc.formats {
            if format.pixel_format != VideoPixelFormat::YUV420 {
                continue;
            }
            if best.as_ref().is_none_or(|current| better(format, current)) {
                best = Some(*format);
            }
        }

        let format = best?;
        Some(XrPassthroughCameraChoice {
            input_id: desc.input_id,
            format_id: format.format_id,
            width: format.width,
            height: format.height,
        })
    }

    fn reset_passthrough_camera_state(&mut self) {
        self.passthrough_camera_playback_requested = false;
        self.passthrough_camera_failed = false;
        self.passthrough_camera_textures = None;
        self.passthrough_camera_video = VideoYuvMetadata::disabled();
        self.passthrough_camera_has_frame = false;
        self.passthrough_camera_quad = None;
        self.passthrough_quad_placement = None;
    }

    fn stop_passthrough_camera(&mut self, cx: &mut Cx) {
        cx.cancel_pending_camera_playback(Self::passthrough_video_id());
        if self.passthrough_camera_playback_requested || self.passthrough_camera_textures.is_some()
        {
            cx.cleanup_video_playback_resources(Self::passthrough_video_id());
        }
        self.reset_passthrough_camera_state();
    }

    fn sync_passthrough_camera(&mut self, cx: &mut Cx) {
        if !self.passthrough_debug_enabled() {
            self.stop_passthrough_camera(cx);
            return;
        }

        if matches!(
            self.passthrough_camera_permission,
            Some(PermissionStatus::DeniedCanRetry) | Some(PermissionStatus::DeniedPermanent)
        ) {
            crate::warning!(
                "XR passthrough camera: sync blocked by permission state {:?}",
                self.passthrough_camera_permission
            );
            return;
        }

        let Some(choice) = self.passthrough_camera_choice.clone() else {
            crate::warning!("XR passthrough camera: sync waiting for camera choice");
            return;
        };

        self.passthrough_camera_source_size = vec2f(choice.width as f32, choice.height as f32);
        if self.passthrough_camera_textures.is_none() {
            self.passthrough_camera_textures = Some(XrPassthroughCameraTextures {
                camera: Texture::new_with_format(cx, TextureFormat::VideoExternal),
                tex_y: None,
                tex_u: None,
                tex_v: None,
            });
        }
        if self.passthrough_camera_failed {
            return;
        }
        if self.passthrough_camera_playback_requested {
            return;
        }

        cx.prepare_headset_camera_playback(
            Self::passthrough_video_id(),
            VideoSource::Camera(choice.input_id, choice.format_id),
            CameraPreviewMode::Texture,
            0,
            self.passthrough_camera_textures
                .as_ref()
                .map(|textures| textures.camera.texture_id())
                .unwrap_or_default(),
            false,
            false,
        );
        self.passthrough_camera_playback_requested = true;
    }

    fn upsert_passthrough_quad_geometry(&mut self, cx: &mut Cx2d, state: &XrState) -> Option<GeometryId> {
        let (placement, half_width, half_height) = self.current_passthrough_quad_placement(state);

        let corners = [
            placement.center - placement.right * half_width + placement.up * half_height,
            placement.center + placement.right * half_width + placement.up * half_height,
            placement.center + placement.right * half_width - placement.up * half_height,
            placement.center - placement.right * half_width - placement.up * half_height,
        ];

        let tangent = [placement.right.x, placement.right.y, placement.right.z, 1.0];
        let color = [1.0, 1.0, 1.0, 1.0];
        let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let mut vertices = Vec::with_capacity(4 * 16);
        for (corner, uv) in corners.iter().zip(uvs.iter()) {
            vertices.extend_from_slice(&[
                corner.x,
                corner.y,
                corner.z,
                placement.normal.x,
                placement.normal.y,
                placement.normal.z,
                uv[0],
                uv[1],
                color[0],
                color[1],
                color[2],
                color[3],
                tangent[0],
                tangent[1],
                tangent[2],
                tangent[3],
            ]);
        }
        let indices = vec![0, 1, 2, 2, 3, 0, 0, 2, 1, 0, 3, 2];

        let geometry = self
            .passthrough_camera_quad
            .get_or_insert_with(|| Geometry::new(cx.cx.cx));
        geometry.update(cx.cx.cx, indices, vertices);
        Some(geometry.geometry_id())
    }

    fn upsert_passthrough_env_atlas_geometry(
        &mut self,
        cx: &mut Cx2d,
        width: f64,
        height: f64,
    ) -> GeometryId {
        let corners = [
            [0.0f32, 0.0f32, 0.0f32],
            [width as f32, 0.0f32, 0.0f32],
            [width as f32, height as f32, 0.0f32],
            [0.0f32, height as f32, 0.0f32],
        ];
        let normal = [0.0, 0.0, 1.0];
        let tangent = [1.0, 0.0, 0.0, 1.0];
        let color = [1.0, 1.0, 1.0, 1.0];
        let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let mut vertices = Vec::with_capacity(4 * 16);
        for (corner, uv) in corners.iter().zip(uvs.iter()) {
            vertices.extend_from_slice(&[
                corner[0],
                corner[1],
                corner[2],
                normal[0],
                normal[1],
                normal[2],
                uv[0],
                uv[1],
                color[0],
                color[1],
                color[2],
                color[3],
                tangent[0],
                tangent[1],
                tangent[2],
                tangent[3],
            ]);
        }
        let indices = vec![0, 1, 2, 2, 3, 0, 0, 2, 1, 0, 3, 2];
        let geometry = self
            .passthrough_env_atlas_quad
            .get_or_insert_with(|| Geometry::new(cx.cx.cx));
        geometry.update(cx.cx.cx, indices, vertices);
        geometry.geometry_id()
    }

    fn draw_passthrough_camera_quad(&mut self, cx: &mut Cx2d, state: &XrState) {
        if !self.passthrough_debug_enabled() {
            return;
        }
        let Some(textures) = self.passthrough_camera_textures.clone() else {
            return;
        };
        let Some(geometry_id) = self.upsert_passthrough_quad_geometry(cx, state) else {
            return;
        };

        self.draw_passthrough_quad.draw_vars.options.depth_write = false;
        self.draw_passthrough_quad.source_size = self.passthrough_camera_source_size;
        self.draw_passthrough_quad.frost_mix = 0.0;
        self.draw_passthrough_quad.tint_color = vec4f(1.0, 1.0, 1.0, 0.42);
        self.draw_passthrough_quad.camera_enabled = if self.passthrough_camera_has_frame {
            1.0
        } else {
            0.0
        };
        self.draw_passthrough_quad.rotation_steps = self.passthrough_camera_video.rotation_steps;
        self.draw_passthrough_quad.biplanar = self.passthrough_camera_video.shader_biplanar();
        self.draw_passthrough_quad.yuv_enabled = if self.passthrough_camera_has_frame {
            self.passthrough_camera_video.shader_enabled()
        } else {
            -1.0
        };
        self.draw_passthrough_quad
            .draw_vars
            .set_texture(0, &textures.camera);
        self.draw_passthrough_quad.draw_geometry(cx, geometry_id);
    }

    fn current_head_basis(state: &XrState) -> (Vec3f, Vec3f, Vec3f, Vec3f) {
        let head = state.head_pose.position;
        let right = (state.vec_in_head_space(vec3(1.0, 0.0, 0.0)) - head).normalize();
        let up = (state.vec_in_head_space(vec3(0.0, 1.0, 0.0)) - head).normalize();
        let forward = (state.vec_in_head_space(vec3(0.0, 0.0, -1.0)) - head).normalize();
        (head, right, up, forward)
    }

    fn render_passthrough_env_atlas(&mut self, cx: &mut Cx2d, state: &XrState) -> Option<Texture> {
        let source_size = self.passthrough_camera_source_size;
        let rotation_steps = self.passthrough_camera_video.rotation_steps;
        let camera_enabled = if self.passthrough_camera_has_frame {
            1.0
        } else {
            0.0
        };
        let camera_texture = self
            .passthrough_camera_textures
            .as_ref()
            .map(|textures| textures.camera.clone())?;
        let atlas_width = XR_PASSTHROUGH_ENV_ATLAS_WIDTH as f64;
        let atlas_height = XR_PASSTHROUGH_ENV_ATLAS_HEIGHT as f64;
        let (_, camera_right, camera_up, camera_forward) = Self::current_head_basis(state);
        let camera_center_offset_uv = self.passthrough_camera_center_offset_uv();
        let geometry_id = self.upsert_passthrough_env_atlas_geometry(cx, atlas_width, atlas_height);

        let Self {
            passthrough_env_atlas,
            draw_passthrough_cube_atlas,
            ..
        } = self;
        let atlas =
            passthrough_env_atlas.get_or_insert_with(|| XrPassthroughEnvAtlas::new(cx.cx.cx));
        if atlas.pending_swap {
            atlas.finish_frame();
        }
        atlas.pass.set_size(cx.cx.cx, dvec2(atlas_width, atlas_height));
        let previous_texture = atlas.current_texture().clone();
        let display_texture = atlas.initialized.then_some(previous_texture.clone());
        let target_texture = atlas.target_texture().clone();
        let bootstrap_mix = if atlas.initialized { 0.0 } else { 1.0 };

        atlas.pass.set_color_texture(
            cx.cx.cx,
            &target_texture,
            DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 1.0)),
        );

        cx.make_child_pass(&atlas.pass);
        cx.begin_pass(&atlas.pass, Some(1.0));
        atlas.draw_list.begin_always(cx);

        draw_passthrough_cube_atlas.draw_vars.options.depth_write = false;
        draw_passthrough_cube_atlas.source_size = source_size;
        draw_passthrough_cube_atlas.camera_enabled = camera_enabled;
        draw_passthrough_cube_atlas.rotation_steps = rotation_steps;
        draw_passthrough_cube_atlas.bootstrap_mix = bootstrap_mix;
        draw_passthrough_cube_atlas.update_strength = XR_PASSTHROUGH_ENV_UPDATE_STRENGTH;
        draw_passthrough_cube_atlas.camera_fov_y_degrees =
            XR_PASSTHROUGH_ENV_CAMERA_FOV_Y_DEGREES;
        draw_passthrough_cube_atlas.camera_projection_scale =
            XR_PASSTHROUGH_ENV_CAMERA_PROJECTION_SCALE;
        draw_passthrough_cube_atlas.camera_exposure = XR_PASSTHROUGH_CAMERA_EXPOSURE;
        draw_passthrough_cube_atlas.camera_center_offset_uv = camera_center_offset_uv;
        draw_passthrough_cube_atlas.camera_right = camera_right;
        draw_passthrough_cube_atlas.camera_up = camera_up;
        draw_passthrough_cube_atlas.camera_forward = camera_forward;
        draw_passthrough_cube_atlas
            .draw_vars
            .set_texture(0, &camera_texture);
        draw_passthrough_cube_atlas
            .draw_vars
            .set_texture(1, &previous_texture);
        draw_passthrough_cube_atlas.draw_geometry(cx, geometry_id);

        atlas.draw_list.end(cx);
        cx.end_pass(&atlas.pass);
        atlas.pending_swap = true;
        display_texture
    }

    fn passthrough_cube_pose(state: &XrState, horizontal_offset: f32) -> Pose {
        let (head, right, _, forward) = Self::current_head_basis(state);
        let center = head
            + forward * XR_PASSTHROUGH_CUBE_DISTANCE
            + right * horizontal_offset
            + vec3f(0.0, XR_PASSTHROUGH_CUBE_VERTICAL_OFFSET, 0.0);
        let flat_forward = vec3f(forward.x, 0.0, forward.z);
        let facing = if flat_forward.length() > 1.0e-4 {
            flat_forward.normalize()
        } else {
            forward
        };
        Pose::new(Quat::look_rotation(facing, vec3(0.0, 1.0, 0.0)), center)
    }

    fn passthrough_cube_horizontal_offset(slot: usize) -> f32 {
        (slot as f32 - 1.5) * XR_PASSTHROUGH_CUBE_SPACING
    }

    fn ensure_passthrough_cube_pose(&mut self, state: &XrState, slot: usize) -> Pose {
        if let Some(pose) = self.passthrough_cube_poses[slot] {
            return pose;
        }

        let pose = Self::passthrough_cube_pose(state, Self::passthrough_cube_horizontal_offset(slot));
        self.passthrough_cube_poses[slot] = Some(pose);
        pose
    }

    fn draw_reflective_passthrough_cube(
        &mut self,
        cx: &mut Cx2d,
        state: &XrState,
        env_atlas: Option<Texture>,
        slot: usize,
        base_color: Vec4f,
        metallic: f32,
        roughness: f32,
        spec_strength: f32,
        env_intensity: f32,
        ambient: f32,
        light_color: Vec3f,
    ) {
        self.prepare_pbr(cx);
        self.draw_pbr.camera_pos = state.head_pose.position;
        if let Some(env_atlas) = env_atlas {
            self.draw_pbr.set_env_texture(None);
            self.draw_pbr.set_env_atlas_texture(Some(env_atlas));
        } else {
            let env_tex = self.draw_pbr.default_env_texture(cx);
            self.draw_pbr.set_env_texture(Some(env_tex));
            self.draw_pbr.set_env_atlas_texture(None);
        }
        self.draw_pbr.ambient = ambient;
        self.draw_pbr.spec_strength = spec_strength;
        self.draw_pbr.env_intensity = env_intensity;
        self.draw_pbr.light_color = light_color;
        self.draw_pbr.set_base_color_factor(base_color);
        self.draw_pbr.set_metal_roughness(metallic, roughness);
        let cube_pose = self.ensure_passthrough_cube_pose(state, slot);
        self.draw_pbr.set_transform(cube_pose.to_mat4());
        let _ = self.draw_pbr.draw_rounded_cube(
            cx,
            vec3(
                XR_PASSTHROUGH_CUBE_HALF_EXTENT,
                XR_PASSTHROUGH_CUBE_HALF_EXTENT,
                XR_PASSTHROUGH_CUBE_HALF_EXTENT,
            ),
            XR_PASSTHROUGH_CUBE_CORNER_RADIUS,
            XR_PBR_FACE_SUBDIVISIONS,
            XR_PBR_CORNER_SEGMENTS,
        );
    }

    fn draw_refractive_passthrough_cube(
        &mut self,
        cx: &mut Cx2d,
        state: &XrState,
        env_atlas: Option<Texture>,
        slot: usize,
        base_color: Vec4f,
        roughness: f32,
        spec_strength: f32,
        env_intensity: f32,
        focus_distance: f32,
    ) {
        self.prepare_refractive_pbr(cx);
        self.draw_pbr_refractive.source_size = self.passthrough_camera_source_size;
        self.draw_pbr_refractive.camera_pos = state.head_pose.position;
        self.draw_pbr_refractive.camera_enabled = if self.passthrough_camera_has_frame {
            1.0
        } else {
            0.0
        };
        self.draw_pbr_refractive.rotation_steps = self.passthrough_camera_video.rotation_steps;
        self.draw_pbr_refractive.camera_fov_y_degrees =
            XR_PASSTHROUGH_ENV_CAMERA_FOV_Y_DEGREES;
        self.draw_pbr_refractive.camera_projection_scale =
            XR_PASSTHROUGH_ENV_CAMERA_PROJECTION_SCALE;
        self.draw_pbr_refractive.camera_exposure = XR_PASSTHROUGH_CAMERA_EXPOSURE;
        self.draw_pbr_refractive.camera_center_offset_uv =
            self.passthrough_camera_center_offset_uv();
        let cube_pose = self.ensure_passthrough_cube_pose(state, slot);
        let cube_transform = cube_pose.to_mat4();
        self.draw_pbr_refractive.object_center = cube_pose.position;
        self.draw_pbr_refractive.object_right = cube_transform
            .transform_vec4(vec4f(1.0, 0.0, 0.0, 0.0))
            .to_vec3f()
            .normalize();
        self.draw_pbr_refractive.object_up = cube_transform
            .transform_vec4(vec4f(0.0, 1.0, 0.0, 0.0))
            .to_vec3f()
            .normalize();
        self.draw_pbr_refractive.object_forward = cube_transform
            .transform_vec4(vec4f(0.0, 0.0, 1.0, 0.0))
            .to_vec3f()
            .normalize();
        self.draw_pbr_refractive.object_half_extents = vec3f(
            XR_PASSTHROUGH_CUBE_HALF_EXTENT,
            XR_PASSTHROUGH_CUBE_HALF_EXTENT,
            XR_PASSTHROUGH_CUBE_HALF_EXTENT,
        );
        self.draw_pbr_refractive.object_corner_radius = XR_PASSTHROUGH_CUBE_CORNER_RADIUS;
        self.draw_pbr_refractive.transmission_focus_distance = focus_distance;
        self.draw_pbr_refractive.set_depth_write(true);
        self.draw_pbr_refractive.set_camera_texture(
            self.passthrough_camera_textures
                .as_ref()
                .map(|textures| textures.camera.clone()),
        );
        if let Some(env_atlas) = env_atlas {
            self.draw_pbr_refractive.set_env_texture(None);
            self.draw_pbr_refractive
                .set_env_atlas_texture(Some(env_atlas));
        } else {
            let env_tex = self.draw_pbr_refractive.default_env_texture(cx);
            self.draw_pbr_refractive.set_env_texture(Some(env_tex));
            self.draw_pbr_refractive.set_env_atlas_texture(None);
        }
        self.draw_pbr_refractive.ambient = 0.002;
        self.draw_pbr_refractive.spec_strength = spec_strength;
        self.draw_pbr_refractive.env_intensity = env_intensity;
        self.draw_pbr_refractive.light_color = vec3(0.10, 0.10, 0.10);
        self.draw_pbr_refractive.set_base_color_factor(base_color);
        self.draw_pbr_refractive.set_metal_roughness(0.0, roughness);
        self.draw_pbr_refractive.set_transform(cube_transform);
        let _ = self.draw_pbr_refractive.draw_rounded_cube(
            cx,
            vec3(
                XR_PASSTHROUGH_CUBE_HALF_EXTENT,
                XR_PASSTHROUGH_CUBE_HALF_EXTENT,
                XR_PASSTHROUGH_CUBE_HALF_EXTENT,
            ),
            XR_PASSTHROUGH_CUBE_CORNER_RADIUS,
            XR_PBR_FACE_SUBDIVISIONS,
            XR_PBR_CORNER_SEGMENTS,
        );
    }

    fn draw_passthrough_probe_plate(&mut self, cx: &mut Cx2d, state: &XrState) {
        let (placement, half_width, half_height) = self.current_passthrough_quad_placement(state);
        let pose = Pose::new(
            Quat::look_rotation(-placement.normal, placement.up),
            placement.center - placement.normal * 0.0015,
        );
        self.draw_pose_box(
            cx,
            pose,
            vec3(half_width, half_height, 0.001),
            vec4(0.20, 0.55, 0.95, 1.0),
            1.0,
        );
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

    fn clear_depth_surface_mesh(&mut self) {
        self.depth_surface_mesh_generation = 0;
        self.depth_surface_mesh_update_sequence = 0;
        self.depth_surface_mesh_chunks.clear();
        self.depth_surface_mesh_upload_count = 0;
    }

    fn upsert_depth_surface_mesh_chunk(&mut self, cx: &mut Cx2d, chunk: &XrDepthMeshChunk) {
        let key = (chunk.chunk_key.x, chunk.chunk_key.y, chunk.chunk_key.z);
        if self
            .depth_surface_mesh_chunks
            .get(&key)
            .map(|gpu_chunk| gpu_chunk.1.fingerprint == chunk.fingerprint)
            .unwrap_or(false)
        {
            return;
        }

        let vertices = pack_depth_mesh_vertices(chunk);
        if let Some((geometry, handle)) = self.depth_surface_mesh_chunks.get_mut(&key) {
            geometry.update(cx.cx.cx, chunk.indices.clone(), vertices);
            *handle = DepthSurfaceMeshChunkHandle {
                geometry_id: geometry.geometry_id(),
                fingerprint: chunk.fingerprint,
            };
        } else {
            let geometry = Geometry::new(cx.cx.cx);
            geometry.update(cx.cx.cx, chunk.indices.clone(), vertices);
            let handle = DepthSurfaceMeshChunkHandle {
                geometry_id: geometry.geometry_id(),
                fingerprint: chunk.fingerprint,
            };
            self.depth_surface_mesh_chunks
                .insert(key, (geometry, handle));
            self.depth_surface_mesh_upload_count =
                self.depth_surface_mesh_upload_count.saturating_add(1);
        }
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

    fn pose_point_world(pose: Pose, local: Vec3f) -> Vec3f {
        pose.to_mat4().transform_vec4(local.to_vec4()).to_vec3f()
    }

    fn append_capsule_collider(colliders: &mut Vec<HandCollider>, a: Vec3f, b: Vec3f, radius: f32) {
        colliders.push(HandCollider::Capsule { a, b, radius });
    }

    fn append_ball_collider(colliders: &mut Vec<HandCollider>, center: Vec3f, radius: f32) {
        colliders.push(HandCollider::Ball { center, radius });
    }

    fn append_box_collider(colliders: &mut Vec<HandCollider>, pose: Pose, half_extents: Vec3f) {
        colliders.push(HandCollider::Box { pose, half_extents });
    }

    fn hand_plate_pose(hand: &XrHand) -> Pose {
        let palm_pose = hand.joints[XrHand::CENTER];
        Pose::new(
            palm_pose.orientation,
            Self::pose_point_world(palm_pose, vec3f(0.0, 0.0, XR_HAND_PLATE_FORWARD_OFFSET)),
        )
    }

    fn hand_tip_world(hand: &XrHand, finger_index: usize) -> Vec3f {
        let tip_len = hand.tips[finger_index].max(0.0);
        hand.joints[XrHand::END_KNUCKLES[finger_index]]
            .to_mat4()
            .transform_vec4(vec4(0.0, 0.0, -tip_len, 1.0))
            .to_vec3f()
    }

    fn append_finger_chain_colliders(
        colliders: &mut Vec<HandCollider>,
        hand: &XrHand,
        chain: &[usize],
        tip_index: usize,
        radius: f32,
    ) {
        for segment in chain.windows(2) {
            Self::append_capsule_collider(
                colliders,
                hand.joints[segment[0]].position,
                hand.joints[segment[1]].position,
                radius,
            );
        }
        if hand.tip_active(tip_index) {
            let end_joint = *chain.last().unwrap_or(&XrHand::CENTER);
            Self::append_capsule_collider(
                colliders,
                hand.joints[end_joint].position,
                Self::hand_tip_world(hand, tip_index),
                radius * 0.85,
            );
        }
    }

    fn append_fingertip_collider(
        colliders: &mut Vec<HandCollider>,
        hand: &XrHand,
        tip_index: usize,
        radius: f32,
    ) {
        if hand.tip_active(tip_index) {
            Self::append_ball_collider(colliders, Self::hand_tip_world(hand, tip_index), radius);
        }
    }

    fn build_hand_colliders(hand: &XrHand) -> Vec<HandCollider> {
        let mut colliders = Vec::with_capacity(XR_HAND_COLLIDER_SLOTS_PER_HAND);
        if !hand.in_view() {
            return colliders;
        }

        Self::append_box_collider(
            &mut colliders,
            Self::hand_plate_pose(hand),
            vec3f(
                XR_HAND_PLATE_HALF_WIDTH,
                XR_HAND_PLATE_HALF_HEIGHT,
                XR_HAND_PLATE_HALF_DEPTH,
            ),
        );

        Self::append_finger_chain_colliders(
            &mut colliders,
            hand,
            &[
                XrHand::THUMB_BASE,
                XrHand::THUMB_KNUCKLE1,
                XrHand::THUMB_KNUCKLE2,
            ],
            XrHand::THUMB_TIP,
            0.015,
        );
        Self::append_fingertip_collider(
            &mut colliders,
            hand,
            XrHand::THUMB_TIP,
            0.015 * XR_HAND_TIP_RADIUS_SCALE,
        );
        Self::append_finger_chain_colliders(
            &mut colliders,
            hand,
            &[
                XrHand::INDEX_BASE,
                XrHand::INDEX_KNUCKLE1,
                XrHand::INDEX_KNUCKLE2,
                XrHand::INDEX_KNUCKLE3,
            ],
            XrHand::INDEX_TIP,
            0.014,
        );
        Self::append_fingertip_collider(
            &mut colliders,
            hand,
            XrHand::INDEX_TIP,
            0.014 * XR_HAND_TIP_RADIUS_SCALE,
        );
        Self::append_finger_chain_colliders(
            &mut colliders,
            hand,
            &[
                XrHand::MIDDLE_BASE,
                XrHand::MIDDLE_KNUCKLE1,
                XrHand::MIDDLE_KNUCKLE2,
                XrHand::MIDDLE_KNUCKLE3,
            ],
            XrHand::MIDDLE_TIP,
            0.015,
        );
        Self::append_fingertip_collider(
            &mut colliders,
            hand,
            XrHand::MIDDLE_TIP,
            0.015 * XR_HAND_TIP_RADIUS_SCALE,
        );
        Self::append_finger_chain_colliders(
            &mut colliders,
            hand,
            &[
                XrHand::RING_BASE,
                XrHand::RING_KNUCKLE1,
                XrHand::RING_KNUCKLE2,
                XrHand::RING_KNUCKLE3,
            ],
            XrHand::RING_TIP,
            0.014,
        );
        Self::append_fingertip_collider(
            &mut colliders,
            hand,
            XrHand::RING_TIP,
            0.014 * XR_HAND_TIP_RADIUS_SCALE,
        );
        Self::append_finger_chain_colliders(
            &mut colliders,
            hand,
            &[
                XrHand::LITTLE_BASE,
                XrHand::LITTLE_KNUCKLE1,
                XrHand::LITTLE_KNUCKLE2,
                XrHand::LITTLE_KNUCKLE3,
            ],
            XrHand::LITTLE_TIP,
            0.013,
        );
        Self::append_fingertip_collider(
            &mut colliders,
            hand,
            XrHand::LITTLE_TIP,
            0.013 * XR_HAND_TIP_RADIUS_SCALE,
        );

        colliders
    }

    fn collect_live_hand_colliders(
        scene: &RapierScene,
        slots: &[HandColliderBody],
    ) -> Vec<HandCollider> {
        let mut colliders = Vec::with_capacity(slots.len());
        for slot in slots {
            let Some(collider) = scene.colliders.get(slot.collider) else {
                continue;
            };
            if !collider.is_enabled() {
                continue;
            }

            let pose = makepad_pose(collider.position());
            let shape = collider.shape();
            if let Some(capsule) = shape.as_capsule() {
                colliders.push(HandCollider::Capsule {
                    a: Self::pose_point_world(
                        pose,
                        vec3f(
                            capsule.segment.a.x,
                            capsule.segment.a.y,
                            capsule.segment.a.z,
                        ),
                    ),
                    b: Self::pose_point_world(
                        pose,
                        vec3f(
                            capsule.segment.b.x,
                            capsule.segment.b.y,
                            capsule.segment.b.z,
                        ),
                    ),
                    radius: capsule.radius,
                });
            } else if let Some(ball) = shape.as_ball() {
                colliders.push(HandCollider::Ball {
                    center: pose.position,
                    radius: ball.radius,
                });
            } else if let Some(cuboid) = shape.as_cuboid() {
                colliders.push(HandCollider::Box {
                    pose,
                    half_extents: vec3f(
                        cuboid.half_extents.x,
                        cuboid.half_extents.y,
                        cuboid.half_extents.z,
                    ),
                });
            }
        }
        colliders
    }

    fn draw_hand_shapes(&mut self, cx: &mut Cx2d, colliders: &[HandCollider], is_left: bool) {
        let color = if is_left {
            vec4(0.18, 0.72, 1.0, 1.0)
        } else {
            vec4(1.0, 0.62, 0.20, 1.0)
        };
        for collider in colliders {
            match collider {
                HandCollider::Capsule { a, b, radius } => {
                    let (pose, half_height) = capsule_pose(*a, *b);
                    self.draw_pbr_capsule(
                        cx,
                        makepad_pose(&pose),
                        *radius,
                        half_height,
                        color,
                        0.58,
                    );
                }
                HandCollider::Ball { center, radius } => {
                    self.draw_pbr_sphere(cx, *center, *radius, color, 0.56);
                }
                HandCollider::Box { pose, half_extents } => {
                    self.draw_pbr_rounded_cube(cx, *pose, *half_extents, 0.0, color, 0.60);
                }
            }
        }
    }

    fn draw_hand(
        &mut self,
        cx: &mut Cx2d,
        hand: &XrHand,
        physics_colliders: Option<&[HandCollider]>,
        is_left: bool,
    ) {
        if !XR_RENDER_HAND_GEOMETRY || !hand.in_view() {
            return;
        }

        let joint_color = if is_left {
            vec4(0.22, 0.78, 1.0, 1.0)
        } else {
            vec4(1.0, 0.68, 0.30, 1.0)
        };
        let raw_colliders;
        let colliders = if let Some(physics_colliders) = physics_colliders {
            physics_colliders
        } else {
            raw_colliders = Self::build_hand_colliders(hand);
            &raw_colliders
        };
        self.draw_hand_shapes(cx, colliders, is_left);

        self.draw_cube.begin_many_instances(cx);
        for joint in &hand.joints {
            self.draw_pose_box(cx, *joint, vec3(0.011, 0.011, 0.016), joint_color, 0.0);
        }
        self.draw_cube.end_many_instances(cx);
    }

    fn ensure_scene(&mut self, state: &XrState) -> bool {
        if self.scene.is_some() {
            return false;
        }

        let mut forward = state.vec_in_head_space(vec3(0.0, 0.0, -1.0)) - state.head_pose.position;
        forward.y = 0.0;
        if forward.length() <= 1.0e-4 {
            forward = vec3f(0.0, 0.0, -1.0);
        } else {
            forward = forward.normalize();
        }
        let center = vec3f(
            state.head_pose.position.x,
            state.head_pose.position.y * XR_SCENE_HEAD_HEIGHT_SCALE,
            state.head_pose.position.z,
        ) + forward * XR_SCENE_FORWARD_OFFSET
            + vec3f(0.0, XR_SCENE_VERTICAL_OFFSET, 0.0);

        self.scene = Some(RapierScene::new(center));
        true
    }

    fn reset_scene(&mut self, cx: &mut Cx, state: &XrState) {
        self.depth_debug_mode = XrDepthDebugMode::Passthrough;
        self.passthrough_quad_placement = None;
        self.passthrough_camera_quad = None;
        self.passthrough_cube_poses.fill(None);
        if let Some(atlas) = self.passthrough_env_atlas.as_mut() {
            atlas.ping_is_current = true;
            atlas.initialized = false;
            atlas.pending_swap = false;
        }
        self.clear_depth_surface_mesh();
        let _ = self.scene.take();
        self.sync_passthrough_camera(cx);
        let _ = state;
    }

    fn sync_hands(&mut self, state: &XrState) {
        if !XR_ENABLE_HAND_PHYSICS {
            return;
        }

        let Some(scene) = self.scene.as_mut() else {
            return;
        };

        let left = Self::build_hand_colliders(&state.left_hand);
        let right = Self::build_hand_colliders(&state.right_hand);
        let RapierScene {
            bodies,
            colliders,
            left_hand,
            right_hand,
            ..
        } = scene;
        RapierScene::sync_hand_bodies(left_hand, &left, bodies, colliders);
        RapierScene::sync_hand_bodies(right_hand, &right, bodies, colliders);
    }

    fn resolve_depth_query_result(
        retained_hits: &mut HashMap<u64, RetainedDepthQueryHit>,
        key: u64,
        latest_result: Option<XrDepthMeshQueryResult>,
        expired_retained_keys: &mut Vec<u64>,
    ) -> Option<XrDepthMeshQueryResult> {
        match latest_result {
            Some(XrDepthMeshQueryResult::Hit(hit)) => {
                retained_hits.insert(key, RetainedDepthQueryHit::new(hit.clone()));
                Some(XrDepthMeshQueryResult::Hit(hit))
            }
            Some(XrDepthMeshQueryResult::Miss { .. }) | None => retained_hits
                .get_mut(&key)
                .and_then(|retained| retained.reuse_result())
                .or_else(|| {
                    if retained_hits.contains_key(&key) {
                        expired_retained_keys.push(key);
                    }
                    None
                }),
        }
    }

    fn build_depth_query_request(
        key: u64,
        pose: Pose,
        velocity: Vec3f,
        half_extents: Vec3f,
    ) -> XrDepthMeshQuery {
        let mut lookahead = velocity.scale(XR_DEPTH_QUERY_LOOKAHEAD_SECONDS);
        let lookahead_length = lookahead.length();
        if lookahead_length > XR_DEPTH_QUERY_MAX_LOOKAHEAD_DISTANCE && lookahead_length > 1.0e-6 {
            lookahead =
                lookahead.scale(XR_DEPTH_QUERY_MAX_LOOKAHEAD_DISTANCE / lookahead_length);
        }
        XrDepthMeshQuery {
            key,
            center: pose.position,
            predicted_center: pose.position + lookahead,
            velocity,
            radius: half_extents.length(),
            max_distance: XR_DEPTH_QUERY_MAX_DISTANCE,
            include_planar_patches: false,
        }
    }

    fn sync_depth_query_surfaces(&mut self, cx: &mut Cx) {
        if !XR_ENABLE_DEPTH_QUERY_PHYSICS {
            return;
        }
        let Some(scene) = self.scene.as_mut() else {
            return;
        };
        let depth_mesh = cx.xr_depth_mesh();
        let mut clear_keys = Vec::new();
        let mut query_requests = Vec::new();
        let mut query_results = Vec::new();
        let mut expired_retained_keys = Vec::new();
        let retained_hits = &mut self.depth_query_retained_hits;

        for (index, cube) in scene.cubes.iter().enumerate() {
            let key = RapierScene::depth_query_key(index);
            let Some(body) = scene.bodies.get(cube.body) else {
                clear_keys.push(key);
                continue;
            };

            if let Some(result) = Self::resolve_depth_query_result(
                retained_hits,
                key,
                depth_mesh.latest_query_result(key),
                &mut expired_retained_keys,
            ) {
                query_results.push(result);
            }

            if body.is_sleeping() {
                continue;
            }

            let pose = makepad_pose(body.position());
            let linvel = body.linvel();
            let velocity = vec3f(linvel.x, linvel.y, linvel.z);
            query_requests.push(Self::build_depth_query_request(
                key,
                pose,
                velocity,
                cube.half_extents,
            ));
        }

        for key in clear_keys {
            depth_mesh.clear_query(key);
            retained_hits.remove(&key);
        }
        for key in expired_retained_keys {
            retained_hits.remove(&key);
        }

        for query in query_requests {
            let _ = depth_mesh.submit_query(query);
        }

        let targets = build_depth_query_surface_targets(&query_results);
        scene.sync_depth_query_surface_pool(&targets);
    }

    fn sync_depth_surface_mesh(&mut self, cx: &mut Cx2d) {
        if !self.depth_debug_enabled() {
            return;
        }

        let Some(depth_mesh) = cx.cx.xr_depth_mesh().latest_mesh() else {
            self.clear_depth_surface_mesh();
            return;
        };
        let previous_mesh_generation = self.depth_surface_mesh_generation;
        let previous_update_sequence = self.depth_surface_mesh_update_sequence;
        if self.depth_surface_mesh_generation == depth_mesh.mesh_generation
            && self.depth_surface_mesh_update_sequence == depth_mesh.update_sequence
        {
            return;
        }

        self.depth_surface_mesh_generation = depth_mesh.mesh_generation;
        self.depth_surface_mesh_update_sequence = depth_mesh.update_sequence;
        if depth_mesh.mesh_chunks.is_empty() {
            self.clear_depth_surface_mesh();
            return;
        }

        let active_chunk_count = depth_mesh.mesh_chunks.len();
        if self.depth_surface_mesh_upload_count > active_chunk_count.saturating_mul(3) + 64 {
            self.clear_depth_surface_mesh();
        }

        let needs_full_resync = previous_mesh_generation == 0
            || self.depth_surface_mesh_chunks.is_empty()
            || depth_mesh.update_sequence != previous_update_sequence.saturating_add(1);

        if needs_full_resync {
            let mut desired_keys = HashSet::with_capacity(depth_mesh.mesh_chunks.len());
            for chunk in &depth_mesh.mesh_chunks {
                desired_keys.insert((chunk.chunk_key.x, chunk.chunk_key.y, chunk.chunk_key.z));
                self.upsert_depth_surface_mesh_chunk(cx, chunk);
            }
            self.depth_surface_mesh_chunks
                .retain(|key, _| desired_keys.contains(key));
            return;
        }

        for key in &depth_mesh.removed_chunk_keys {
            self.depth_surface_mesh_chunks
                .remove(&(key.x, key.y, key.z));
        }
        for key in &depth_mesh.dirty_chunk_keys {
            if let Some(chunk) = depth_mesh
                .mesh_chunks
                .iter()
                .find(|chunk| chunk.chunk_key == *key)
            {
                self.upsert_depth_surface_mesh_chunk(cx, chunk);
            }
        }
    }

    fn draw_platform(&mut self, cx: &mut Cx2d) {
        let Some(scene) = self.scene.as_ref() else {
            return;
        };

        self.draw_pbr_rounded_cube(
            cx,
            scene.platform_pose,
            vec3f(
                XR_PLATFORM_HALF_WIDTH,
                XR_PLATFORM_HALF_HEIGHT,
                XR_PLATFORM_HALF_DEPTH,
            ),
            XR_PLATFORM_ROUND_RADIUS,
            vec4(PLATFORM_COLOR[0], PLATFORM_COLOR[1], PLATFORM_COLOR[2], 1.0),
            0.85,
        );
    }

    fn draw_bodies(&mut self, cx: &mut Cx2d) {
        let Some(scene) = self.scene.as_ref() else {
            return;
        };

        let cubes: Vec<_> = scene
            .cubes
            .iter()
            .filter_map(|cube| {
                scene.bodies.get(cube.body).map(|body| {
                    let phys_pose = makepad_pose(body.position());
                    let visual_pose = Pose::new(phys_pose.orientation, phys_pose.position);
                    (visual_pose, cube.half_extents, cube.color_index)
                })
            })
            .collect();

        self.draw_cube.begin_many_instances(cx);
        for (pose, half_extents, color_index) in cubes {
            let color = CUBE_COLORS[color_index];
            self.draw_pose_box(
                cx,
                pose,
                vec3(
                    half_extents.x * 2.0 * XR_BRICK_VISUAL_SCALE,
                    half_extents.y * 2.0 * XR_BRICK_VISUAL_SCALE,
                    half_extents.z * 2.0 * XR_BRICK_VISUAL_SCALE,
                ),
                vec4(color[0], color[1], color[2], 1.0),
                1.0,
            );
        }
        self.draw_cube.end_many_instances(cx);
    }

    fn draw_depth_surface_mesh(&mut self, cx: &mut Cx2d) {
        if !self.depth_debug_enabled() {
            return;
        }
        if self.depth_surface_mesh_chunks.is_empty() {
            return;
        }
        self.draw_depth_mesh.base_color = vec4(0.76, 0.88, 0.98, 1.0);
        let mut chunk_handles: Vec<_> = self
            .depth_surface_mesh_chunks
            .iter()
            .map(|(key, chunk)| (*key, chunk.1.geometry_id))
            .collect();
        chunk_handles.sort_by_key(|(key, _)| *key);
        for (_, geometry_id) in chunk_handles {
            self.draw_depth_mesh.draw_geometry(cx, geometry_id);
        }
    }

}

fn pack_depth_mesh_vertices(chunk: &XrDepthMeshChunk) -> Vec<f32> {
    const FLOATS_PER_VERTEX: usize = 16;
    let mut vertices = Vec::with_capacity(chunk.vertices.len() * FLOATS_PER_VERTEX);
    for (position, normal) in chunk.vertices.iter().zip(chunk.normals.iter()) {
        vertices.extend_from_slice(&[
            position.x, position.y, position.z, normal.x, normal.y, normal.z, 0.0, 0.0, 1.0, 1.0,
            1.0, 1.0, 1.0, 0.0, 0.0, 1.0,
        ]);
    }
    vertices
}

impl Widget for XrScene {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event {
            Event::XrUpdate(e) => {
                if e.clicked_menu() || e.menu_pressed() {
                    self.reset_scene(cx, &e.state);
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
        let env_atlas = self.render_passthrough_env_atlas(cx, state);
        self.draw_reflective_passthrough_cube(
            cx,
            state,
            env_atlas.clone(),
            0,
            vec4(0.72, 0.12, 0.10, 1.0),
            1.0,
            0.72,
            0.22,
            0.18,
            0.010,
            vec3(0.07, 0.05, 0.05),
        );
        self.draw_reflective_passthrough_cube(
            cx,
            state,
            env_atlas.clone(),
            1,
            vec4(0.96, 0.97, 0.99, 1.0),
            1.0,
            0.12,
            0.95,
            0.82,
            0.006,
            vec3(0.08, 0.08, 0.08),
        );
        self.draw_refractive_passthrough_cube(
            cx,
            state,
            env_atlas.clone(),
            2,
            vec4(0.58, 0.82, 1.0, 1.0),
            0.05,
            0.90,
            0.96,
            1.8,
        );
        self.draw_refractive_passthrough_cube(
            cx,
            state,
            env_atlas,
            3,
            vec4(0.26, 0.56, 1.0, 1.0),
            0.09,
            1.06,
            1.02,
            1.15,
        );

        DrawStep::done()
    }
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
    headset_camera: Option<PermissionStatus>,
    #[rust]
    pending_scene_access_check: Option<i32>,
    #[rust]
    pending_headset_camera_check: Option<i32>,
    #[rust]
    pending_scene_access_request: Option<i32>,
    #[rust]
    pending_headset_camera_request: Option<i32>,
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

    fn headset_camera_granted(&self) -> bool {
        !Self::is_android_preflight()
            || matches!(self.headset_camera, Some(PermissionStatus::Granted))
    }

    fn xr_permissions_ready(&self) -> bool {
        self.scene_access_granted() && self.headset_camera_granted()
    }

    fn permission_checks_pending(&self) -> bool {
        self.pending_scene_access_check.is_some() || self.pending_headset_camera_check.is_some()
    }

    fn permission_requests_pending(&self) -> bool {
        self.pending_scene_access_request.is_some() || self.pending_headset_camera_request.is_some()
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
        if self.permission_checks_pending() {
            "Checking Quest Permissions..."
        } else if self.permission_requests_pending() {
            "Waiting for Quest Permissions..."
        } else if self.xr_permissions_ready() {
            "Re-check Quest Permissions"
        } else {
            "Allow Quest Permissions"
        }
    }

    fn detail_text(&self) -> &'static str {
        if !Self::is_android_preflight() {
            "This build can start XR directly from the splash screen."
        } else if self.xr_permissions_ready() {
            "Quest scene access and headset camera are granted. Start XR when you are ready."
        } else if !self.scene_access_granted() {
            "Allow Quest scene access before starting XR. This unlocks environment depth and passthrough occlusion."
        } else if !self.headset_camera_granted() {
            "Allow Quest headset camera access before starting XR. This unlocks the passthrough texture overlay."
        } else {
            "Allow Quest permissions before starting XR."
        }
    }

    fn status_text(&self) -> &'static str {
        if self.permission_checks_pending() {
            "Checking current Quest permission status."
        } else if self.permission_requests_pending() {
            "Approve the Quest permission dialog to continue."
        } else if !Self::is_android_preflight() {
            "XR is ready to launch from this splash screen."
        } else if self.xr_permissions_ready() {
            "Quest scene access and headset camera granted."
        } else if !self.scene_access_granted() {
            "Quest scene access has not been granted yet."
        } else if !self.headset_camera_granted() {
            "Quest headset camera permission has not been granted yet."
        } else {
            "Quest permissions are incomplete."
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
                && !self.permission_checks_pending()
                && !self.permission_requests_pending(),
        );
        self.ui
            .widget(cx, ids!(allow_button))
            .set_text(cx, self.allow_button_text());

        self.ui
            .button(cx, ids!(start_xr_button))
            .set_enabled(cx, self.xr_permissions_ready());
    }

    fn begin_scene_access_check(&mut self, cx: &mut Cx) {
        if self.phase != AppPhase::Preflight
            || !Self::is_android_preflight()
            || self.pending_scene_access_check.is_some()
        {
            return;
        }
        self.pending_scene_access_check = Some(cx.check_permission(Permission::SceneAccess));
        self.schedule_ui_refresh(cx);
    }

    fn begin_headset_camera_check(&mut self, cx: &mut Cx) {
        if self.phase != AppPhase::Preflight
            || !Self::is_android_preflight()
            || self.pending_headset_camera_check.is_some()
        {
            return;
        }
        self.pending_headset_camera_check = Some(cx.check_permission(Permission::HeadsetCamera));
        self.schedule_ui_refresh(cx);
    }

    fn request_scene_access(&mut self, cx: &mut Cx) {
        if self.phase != AppPhase::Preflight
            || !Self::is_android_preflight()
            || self.pending_scene_access_check.is_some()
            || self.pending_scene_access_request.is_some()
        {
            return;
        }
        self.pending_scene_access_request = Some(cx.request_permission(Permission::SceneAccess));
        self.schedule_ui_refresh(cx);
    }

    fn request_headset_camera(&mut self, cx: &mut Cx) {
        if self.phase != AppPhase::Preflight
            || !Self::is_android_preflight()
            || self.pending_headset_camera_check.is_some()
            || self.pending_headset_camera_request.is_some()
        {
            return;
        }
        self.pending_headset_camera_request = Some(cx.request_permission(Permission::HeadsetCamera));
        self.schedule_ui_refresh(cx);
    }

    fn begin_preflight_permission_checks(&mut self, cx: &mut Cx) {
        self.begin_scene_access_check(cx);
        self.begin_headset_camera_check(cx);
    }

    fn request_next_missing_permission(&mut self, cx: &mut Cx) {
        if !self.scene_access_granted() {
            self.request_scene_access(cx);
        } else if !self.headset_camera_granted() {
            self.request_headset_camera(cx);
        } else {
            self.begin_preflight_permission_checks(cx);
        }
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
        if self.phase != AppPhase::Preflight || !self.xr_permissions_ready() {
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
            self.headset_camera = Some(PermissionStatus::Granted);
            self.maybe_start_xr_on_ready(cx);
            return;
        }
        self.apply_phase(cx);
        self.schedule_ui_refresh(cx);
        self.begin_preflight_permission_checks(cx);
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(allow_button)).clicked(actions) {
            self.request_next_missing_permission(cx);
        }

        if self.ui.button(cx, ids!(start_xr_button)).clicked(actions) && self.xr_permissions_ready()
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
                let was_request = self.pending_scene_access_request == Some(result.request_id);
                if self.pending_scene_access_check == Some(result.request_id) {
                    self.pending_scene_access_check = None;
                } else if self.pending_scene_access_request == Some(result.request_id) {
                    self.pending_scene_access_request = None;
                } else {
                    return;
                }
                self.scene_access = Some(result.status);
                if was_request
                    && result.status == PermissionStatus::Granted
                    && !self.headset_camera_granted()
                {
                    self.request_next_missing_permission(cx);
                } else if !self.maybe_start_xr_on_ready(cx) {
                    self.schedule_ui_refresh(cx);
                }
            }
            Event::PermissionResult(result) if result.permission == Permission::HeadsetCamera => {
                let was_request = self.pending_headset_camera_request == Some(result.request_id);
                if self.pending_headset_camera_check == Some(result.request_id) {
                    self.pending_headset_camera_check = None;
                } else if self.pending_headset_camera_request == Some(result.request_id) {
                    self.pending_headset_camera_request = None;
                } else {
                    return;
                }
                self.headset_camera = Some(result.status);
                if was_request
                    && result.status == PermissionStatus::Granted
                    && !self.scene_access_granted()
                {
                    self.request_next_missing_permission(cx);
                } else if !self.maybe_start_xr_on_ready(cx) {
                    self.schedule_ui_refresh(cx);
                }
            }
            Event::Resume => {
                if self.phase == AppPhase::Preflight
                    && Self::is_android_preflight()
                    && !self.permission_requests_pending()
                {
                    self.begin_preflight_permission_checks(cx);
                }
            }
            _ => {}
        }
    }
}
