use crate::cube::Cube;
use crate::gltf::Gltf;
use crate::refractive_cube::RefractiveCube;
use crate::tree::Tree;
use crate::xr_node::{XrBodyKind, XrNode, XrRuntimeBodyState, XrDrawScopeData};
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
use std::{collections::HashMap, rc::Rc};

#[path = "xr_depth.rs"]
mod xr_depth;
#[path = "xr_hands.rs"]
mod xr_hands;
#[path = "xr_passthrough.rs"]
mod xr_passthrough;
#[path = "xr_physics.rs"]
mod xr_physics;

pub(crate) use self::xr_physics::{makepad_pose, RapierScene};
use self::{
    xr_depth::{DepthSurfaceMeshChunkHandle, RetainedDepthQueryHit},
    xr_passthrough::{
        XrPassthroughCameraChoice, XrPassthroughCameraTextures, XrPassthroughEnvAtlas,
    },
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

    mod.widgets.XrEnv = set_type_default() do #(XrEnv::script_component(vm)){
        draw_cube: mod.draw.DrawCube{}
        draw_depth_mesh: mod.draw.DrawDepthMeshBasic{
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
        draw_passthrough_env_atlas: mod.draw.DrawPassthroughEnvAtlas{
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

#[derive(Script, ScriptHook)]
pub struct XrEnv {
    #[live]
    draw_cube: DrawCube,
    #[live]
    draw_pbr: DrawPbr,
    #[live]
    draw_depth_mesh: DrawDepthMeshBasic,
    #[live]
    draw_passthrough_env_atlas: DrawPassthroughEnvAtlas,
    #[live(false)]
    depth_mesh: bool,
    #[live(false)]
    env_cube: bool,
    #[rust]
    last_xr_state: Option<Rc<XrState>>,
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

    // Physics (moved from XrScene)
    #[live(9.81)]
    pub gravity: f32,
    #[rust]
    scene: Option<RapierScene>,
    #[rust]
    runtime_bodies: Rc<HashMap<WidgetUid, XrRuntimeBodyState>>,
    #[rust(true)]
    scene_dirty: bool,
    #[rust]
    next_frame: NextFrame,
}

impl XrEnv {
    pub(crate) fn depth_mesh_visible(&self) -> bool {
        self.depth_mesh
    }

    pub(crate) fn set_depth_mesh_visible(&mut self, visible: bool) {
        self.depth_mesh = visible;
    }

    pub(crate) fn toggle_depth_mesh_visible(&mut self) -> bool {
        self.depth_mesh = !self.depth_mesh;
        self.depth_mesh
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

    fn pointer_tip_world(hand: &XrHand) -> Option<Vec3f> {
        if !hand.in_view() || !hand.tip_active(XrHand::INDEX_TIP) {
            return None;
        }
        let tip_len = hand.tips[XrHand::INDEX_TIP].max(0.0);
        Some(
            hand.joints[XrHand::INDEX_KNUCKLE3]
                .to_mat4()
                .transform_vec4(vec4(0.0, 0.0, -tip_len, 1.0))
                .to_vec3f(),
        )
    }

    fn draw_scope_pointer_tips(state: Option<&XrState>) -> [Option<Vec3f>; 2] {
        let Some(state) = state else {
            return [None, None];
        };
        [
            Self::pointer_tip_world(&state.left_hand),
            Self::pointer_tip_world(&state.right_hand),
        ]
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
        if let Some(cube) = widget.borrow::<Cube>() {
            let node = cube.node();
            let (pos, ori, scale) = Self::transform_with_node(parent_pos, parent_ori, parent_scale, node);
            let half = cube.half_extents();
            cubes.push(CollectedXrCube {
                uid: cube.widget_uid(),
                body_kind: node.body_kind(),
                pose: Pose::new(ori, pos),
                scale,
                half_extents: vec3f(half.x * scale.x, half.y * scale.y, half.z * scale.z),
                density: node.density(),
                friction: node.friction(),
                restitution: node.restitution(),
            });
            let (pos, ori, scale) = (pos, ori, scale);
            drop(cube);
            widget.children(&mut |_, child| Self::collect_cubes_from_widget(&child, pos, ori, scale, cubes));
            return;
        }
        if let Some(cube) = widget.borrow::<RefractiveCube>() {
            let node = cube.node();
            let (pos, ori, scale) = Self::transform_with_node(parent_pos, parent_ori, parent_scale, node);
            let half = cube.half_extents();
            cubes.push(CollectedXrCube {
                uid: cube.widget_uid(),
                body_kind: node.body_kind(),
                pose: Pose::new(ori, pos),
                scale,
                half_extents: vec3f(half.x * scale.x, half.y * scale.y, half.z * scale.z),
                density: node.density(),
                friction: node.friction(),
                restitution: node.restitution(),
            });
            let (pos, ori, scale) = (pos, ori, scale);
            drop(cube);
            widget.children(&mut |_, child| Self::collect_cubes_from_widget(&child, pos, ori, scale, cubes));
            return;
        }
        if let Some(gltf) = widget.borrow::<Gltf>() {
            let node = gltf.node();
            let (pos, ori, scale) = Self::transform_with_node(parent_pos, parent_ori, parent_scale, node);
            let half = node.physics_half_extents();
            if node.body_kind() != XrBodyKind::Disabled && (half.x > 0.0 || half.y > 0.0 || half.z > 0.0) {
                cubes.push(CollectedXrCube {
                    uid: gltf.widget_uid(),
                    body_kind: node.body_kind(),
                    pose: Pose::new(ori, pos),
                    scale,
                    half_extents: vec3f(half.x * scale.x, half.y * scale.y, half.z * scale.z),
                    density: node.density(),
                    friction: node.friction(),
                    restitution: node.restitution(),
                });
            }
            drop(gltf);
            widget.children(&mut |_, child| Self::collect_cubes_from_widget(&child, pos, ori, scale, cubes));
            return;
        }
        if let Some(tree) = widget.borrow::<Tree>() {
            let node = tree.node();
            let (pos, ori, scale) = Self::transform_with_node(parent_pos, parent_ori, parent_scale, node);
            let half = node.physics_half_extents();
            if node.body_kind() != XrBodyKind::Disabled && (half.x > 0.0 || half.y > 0.0 || half.z > 0.0) {
                cubes.push(CollectedXrCube {
                    uid: tree.widget_uid(),
                    body_kind: node.body_kind(),
                    pose: Pose::new(ori, pos),
                    scale,
                    half_extents: vec3f(half.x * scale.x, half.y * scale.y, half.z * scale.z),
                    density: node.density(),
                    friction: node.friction(),
                    restitution: node.restitution(),
                });
            }
            drop(tree);
            widget.children(&mut |_, child| Self::collect_cubes_from_widget(&child, pos, ori, scale, cubes));
            return;
        }
        if let Some(node) = widget.borrow::<XrNode>() {
            let (pos, ori, scale) = Self::transform_with_node(parent_pos, parent_ori, parent_scale, &node);
            let half = node.physics_half_extents();
            if node.body_kind() != XrBodyKind::Disabled && (half.x > 0.0 || half.y > 0.0 || half.z > 0.0) {
                cubes.push(CollectedXrCube {
                    uid: node.widget_uid(),
                    body_kind: node.body_kind(),
                    pose: Pose::new(ori, pos),
                    scale,
                    half_extents: vec3f(half.x * scale.x, half.y * scale.y, half.z * scale.z),
                    density: node.density(),
                    friction: node.friction(),
                    restitution: node.restitution(),
                });
            }
            drop(node);
            widget.children(&mut |_, child| Self::collect_cubes_from_widget(&child, pos, ori, scale, cubes));
            return;
        }
        widget.children(&mut |_, child| Self::collect_cubes_from_widget(&child, parent_pos, parent_ori, parent_scale, cubes));
    }

    fn collect_cubes_from_children(&self, children: &[(LiveId, WidgetRef)]) -> Vec<CollectedXrCube> {
        let mut cubes = Vec::new();
        let root_pos = vec3f(0.0, 0.0, 0.0);
        let root_ori = Quat::default();
        let root_scale = vec3f(1.0, 1.0, 1.0);
        for (_, child) in children {
            Self::collect_cubes_from_widget(child, root_pos, root_ori, root_scale, &mut cubes);
        }
        cubes
    }

    fn rebuild_physics_scene(&mut self, children: &[(LiveId, WidgetRef)]) {
        let cubes = self.collect_cubes_from_children(children);
        let mut scene = RapierScene::new(self.gravity);
        for cube in cubes {
            match cube.body_kind {
                XrBodyKind::Disabled => {}
                XrBodyKind::Dynamic => scene.spawn_dynamic_box(
                    cube.uid, cube.pose, cube.half_extents, cube.scale,
                    cube.density, cube.friction, cube.restitution,
                ),
                XrBodyKind::Fixed => scene.spawn_fixed_box(
                    cube.uid, cube.pose, cube.half_extents, cube.scale,
                    cube.friction, cube.restitution,
                ),
            }
        }
        self.scene = Some(scene);
        self.scene_dirty = false;
        self.sync_runtime_bodies();
    }

    fn sync_runtime_bodies(&mut self) {
        let runtime_bodies = Rc::make_mut(&mut self.runtime_bodies);
        runtime_bodies.clear();
        let Some(scene) = self.scene.as_ref() else { return };
        for cube in &scene.cubes {
            if let Some(body) = scene.bodies.get(cube.body) {
                runtime_bodies.insert(
                    cube.widget_uid,
                    XrRuntimeBodyState {
                        pose: makepad_pose(body.position()),
                        scale: cube.scale,
                    },
                );
            }
        }
    }

    pub fn ensure_physics(&mut self, cx: &mut Cx, children: &[(LiveId, WidgetRef)]) {
        if self.scene_dirty || self.scene.is_none() {
            self.rebuild_physics_scene(children);
            cx.redraw_all();
        }
    }

    pub fn mark_scene_dirty(&mut self) {
        self.scene_dirty = true;
    }

    fn has_dynamic_bodies(&self) -> bool {
        self.scene.as_ref().map(|scene| {
            scene.cubes.iter().any(|cube| matches!(cube.body_kind, XrBodyKind::Dynamic))
        }).unwrap_or(false)
    }

    pub fn step_physics(&mut self, cx: &mut Cx) {
        if let Some(scene) = self.scene.as_mut() {
            scene.step();
        }
        self.sync_runtime_bodies();
        cx.redraw_all();
    }

    pub fn reset_physics(&mut self, cx: &mut Cx) {
        self.scene = None;
        Rc::make_mut(&mut self.runtime_bodies).clear();
        self.scene_dirty = true;
        cx.redraw_all();
    }

    pub(crate) fn runtime_scene_ref(&self) -> Option<&RapierScene> {
        self.scene.as_ref()
    }

    pub(crate) fn runtime_scene_mut(&mut self) -> Option<&mut RapierScene> {
        self.scene.as_mut()
    }

    // --- New API for XrRoot ---

    pub fn prepare_and_draw(&mut self, cx: &mut Cx2d) -> XrDrawScopeData {
        let state = self.last_xr_state.clone();
        if let Some(state) = state.as_deref() {
            if self.depth_debug_enabled() {
                self.prepare_depth_mesh(cx);
                self.sync_depth_surface_mesh(cx);
                self.draw_depth_surface_mesh(cx);
            }

            if XR_RENDER_HAND_GEOMETRY {
                self.prepare_pbr(cx);
                let left_colliders = self.scene.as_ref()
                    .map(|scene| Self::collect_live_hand_colliders(scene, &scene.left_hand));
                let right_colliders = self.scene.as_ref()
                    .map(|scene| Self::collect_live_hand_colliders(scene, &scene.right_hand));
                self.draw_hand(cx, &state.left_hand, left_colliders.as_deref(), true);
                self.draw_hand(cx, &state.right_hand, right_colliders.as_deref(), false);
            }
        }

        let env_texture = if self.env_cube {
            state.as_deref().and_then(|state| self.render_passthrough_env_atlas(cx, state))
        } else {
            None
        };

        XrDrawScopeData {
            runtime_bodies: self.runtime_bodies.clone(),
            env_texture,
            camera_texture: self.passthrough_camera_textures.as_ref()
                .map(|textures| textures.camera.clone()),
            camera_source_size: self.passthrough_camera_source_size,
            camera_rotation_steps: self.passthrough_camera_video.rotation_steps,
            camera_center_offset_uv: self.passthrough_camera_center_offset_uv(),
            camera_enabled: self.passthrough_camera_has_frame && state.is_some(),
            pointer_tips: Self::draw_scope_pointer_tips(state.as_deref()),
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
