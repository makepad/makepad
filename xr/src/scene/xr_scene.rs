use crate::{
    cube::Cube,
    gltf::Gltf,
    refractive_cube::RefractiveCube,
    scene_draw::SceneState3D,
    tree::Tree,
    xr_env::{makepad_pose, RapierScene},
    xr_node::{XrBodyKind, XrDrawScopeData, XrNode, XrRuntimeBodyState},
};
use makepad_widgets::*;
use std::{collections::HashMap, rc::Rc};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.XrPhysics = set_type_default() do #(XrPhysics::script_component(vm))
    mod.widgets.XrSceneBase = #(XrScene::register_widget(vm))

    mod.widgets.XrScene = set_type_default() do mod.widgets.XrSceneBase{
        width: Fill
        height: Fill
        physics: mod.widgets.XrPhysics{}
        draw_bg +: {
            color: #x171d26
            draw_depth: -99.0
        }
    }
}

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
    #[area]
    #[rust]
    area: Area,
    #[rust]
    scene: Option<RapierScene>,
    #[rust]
    runtime_bodies: Rc<HashMap<WidgetUid, XrRuntimeBodyState>>,
    #[rust(true)]
    scene_dirty: bool,
    #[rust]
    last_xr_state: Option<Rc<XrState>>,
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
    #[live(1.0)]
    camera_aspect_ratio_tweak: f32,
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
    #[live(false)]
    xr_anchor_to_head: bool,
    #[live(0.78)]
    xr_anchor_forward_offset: f32,
    #[live(vec3(0.0, -0.26, 0.0))]
    xr_anchor_position_offset: Vec3f,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    preview_color_texture: Option<Texture>,
    #[rust]
    preview_depth_texture: Option<Texture>,
    #[rust]
    xr_draw_logged: bool,
    #[rust(0u32)]
    xr_anchor_log_count: u32,
    #[rust(0u32)]
    preview_draw_log_count: u32,
    #[rust(false)]
    preview_resources_logged: bool,
    #[rust]
    xr_anchor_pose: Pose,
    #[rust(false)]
    xr_anchor_initialized: bool,
    #[deref]
    node: XrNode,
}

impl XrScene {
    pub fn reset_requested(update: &XrUpdateEvent) -> bool {
        update.clicked_menu()
    }

    fn reset_scene(&mut self, cx: &mut Cx) {
        self.scene = None;
        Rc::make_mut(&mut self.runtime_bodies).clear();
        self.scene_dirty = true;
        self.redraw(cx);
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

    fn preview_scene_state(&mut self, rect: Rect, pass_size: Vec2d, time: f64) -> Option<SceneState3D> {
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

        let aspect = ((pass_size.x / pass_size.y).max(0.001) as f32)
            * self.camera_aspect_ratio_tweak.max(0.01);
        let preview_fov_y = self.camera_fov_y.clamp(1.0, 179.0);
        let projection = Mat4f::perspective(
            preview_fov_y,
            aspect,
            self.camera_near.max(0.001),
            self.camera_far.max(self.camera_near + 0.001),
        );
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
            time,
            camera_pos,
            view,
            projection,
            clip_ndc,
            depth_range: self.depth_range,
            depth_forward_bias: self.depth_forward_bias,
            use_pass_camera: false,
            viewport_rect: rect,
        })
    }



    fn xr_scene_state(&self, state: &XrState) -> SceneState3D {
        SceneState3D {
            time: state.time,
            camera_pos: state.head_pose.position,
            view: Mat4f::identity(),
            projection: Mat4f::identity(),
            clip_ndc: vec4(-1.0, -1.0, 1.0, 1.0),
            depth_range: self.depth_range,
            depth_forward_bias: self.depth_forward_bias,
            use_pass_camera: true,
            viewport_rect: Rect::default(),
        }
    }

    fn xr_anchor_pose_from_state(&self, state: &XrState) -> Pose {
        let mut forward = state.head_pose.orientation.rotate_vec3(&vec3f(0.0, 0.0, -1.0));
        forward.y = 0.0;
        if forward.length() <= 1.0e-4 {
            forward = vec3f(0.0, 0.0, -1.0);
        } else {
            forward = forward.normalize();
        }

        let up = vec3f(0.0, 1.0, 0.0);
        let yaw_rotation = Quat::look_rotation(forward, up);
        let center = vec3f(0.0, state.head_pose.position.y, 0.0)
            + forward * self.xr_anchor_forward_offset.max(0.0)
            + yaw_rotation.rotate_vec3(&self.xr_anchor_position_offset);
        Pose::new(Quat::look_rotation(forward.scale(-1.0), up), center)
    }

    fn xr_should_reanchor_anchor(update: &XrUpdateEvent) -> bool {
        let position_delta = update.state.head_pose.position - update.last.head_pose.position;
        if position_delta.length() > 0.35 {
            return true;
        }

        let mut current_forward = update
            .state
            .head_pose
            .orientation
            .rotate_vec3(&vec3f(0.0, 0.0, -1.0));
        current_forward.y = 0.0;
        current_forward = if current_forward.length() <= 1.0e-4 {
            vec3f(0.0, 0.0, -1.0)
        } else {
            current_forward.normalize()
        };

        let mut last_forward = update
            .last
            .head_pose
            .orientation
            .rotate_vec3(&vec3f(0.0, 0.0, -1.0));
        last_forward.y = 0.0;
        last_forward = if last_forward.length() <= 1.0e-4 {
            vec3f(0.0, 0.0, -1.0)
        } else {
            last_forward.normalize()
        };

        current_forward.dot(last_forward).clamp(-1.0, 1.0) < 0.75
    }

    fn update_xr_anchor_pose(&mut self, state: &XrState) {
        if !self.xr_anchor_to_head {
            return;
        }
        self.xr_anchor_pose = self.xr_anchor_pose_from_state(state);
        self.xr_anchor_initialized = true;
        if self.xr_anchor_log_count < 8 {
            let mut forward = state.head_pose.orientation.rotate_vec3(&vec3f(0.0, 0.0, -1.0));
            forward.y = 0.0;
            forward = if forward.length() <= 1.0e-4 {
                vec3f(0.0, 0.0, -1.0)
            } else {
                forward.normalize()
            };
            crate::log!(
                "XrScene anchor[{}] head=({:.3},{:.3},{:.3}) forward=({:.3},{:.3},{:.3}) anchor_pos=({:.3},{:.3},{:.3}) anchor_quat=({:.3},{:.3},{:.3},{:.3}) offset=({:.3},{:.3},{:.3}) forward_offset={:.3}",
                self.xr_anchor_log_count,
                state.head_pose.position.x,
                state.head_pose.position.y,
                state.head_pose.position.z,
                forward.x,
                forward.y,
                forward.z,
                self.xr_anchor_pose.position.x,
                self.xr_anchor_pose.position.y,
                self.xr_anchor_pose.position.z,
                self.xr_anchor_pose.orientation.x,
                self.xr_anchor_pose.orientation.y,
                self.xr_anchor_pose.orientation.z,
                self.xr_anchor_pose.orientation.w,
                self.xr_anchor_position_offset.x,
                self.xr_anchor_position_offset.y,
                self.xr_anchor_position_offset.z,
                self.xr_anchor_forward_offset
            );
            self.xr_anchor_log_count += 1;
        }
    }

    fn xr_root_transform_state(&self) -> XrTransformState {
        if self.xr_anchor_to_head && self.xr_anchor_initialized {
            XrTransformState {
                position: self.xr_anchor_pose.position,
                orientation: self.xr_anchor_pose.orientation,
                scale: vec3f(1.0, 1.0, 1.0),
            }
        } else {
            XrTransformState::default()
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

        if let Some(gltf) = widget.borrow::<Gltf>() {
            let node = gltf.node();
            let world = Self::transform_with_node(parent, node);
            let half_extents = node.physics_half_extents();
            if node.body_kind() != XrBodyKind::Disabled
                && (half_extents.x > 0.0 || half_extents.y > 0.0 || half_extents.z > 0.0)
            {
                cubes.push(CollectedXrCube {
                    uid: gltf.widget_uid(),
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
            drop(gltf);
            widget.children(&mut |_, child| Self::collect_cubes_from_widget(&child, world, cubes));
            return;
        }

        if let Some(tree) = widget.borrow::<Tree>() {
            let node = tree.node();
            let world = Self::transform_with_node(parent, node);
            let half_extents = node.physics_half_extents();
            if node.body_kind() != XrBodyKind::Disabled
                && (half_extents.x > 0.0 || half_extents.y > 0.0 || half_extents.z > 0.0)
            {
                cubes.push(CollectedXrCube {
                    uid: tree.widget_uid(),
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
            drop(tree);
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
        let root = self.xr_root_transform_state();
        self.node
            .children(&mut |_, child| Self::collect_cubes_from_widget(&child, root, &mut cubes));
        cubes
    }

    fn sync_runtime_bodies(&mut self) {
        let runtime_bodies = Rc::make_mut(&mut self.runtime_bodies);
        runtime_bodies.clear();
        let Some(scene) = self.scene.as_ref() else {
            return;
        };
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

    fn rebuild_runtime_scene(&mut self, cx: &mut Cx) {
        let cubes = self.collect_rendered_cubes();
        let dynamic_count = cubes
            .iter()
            .filter(|cube| matches!(cube.body_kind, XrBodyKind::Dynamic))
            .count();
        let fixed_count = cubes
            .iter()
            .filter(|cube| matches!(cube.body_kind, XrBodyKind::Fixed))
            .count();
        crate::log!(
            "XrScene rebuild_runtime_scene cubes={} dynamic={} fixed={} child_count={}",
            cubes.len(),
            dynamic_count,
            fixed_count,
            self.node.child_count()
        );
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

    pub(crate) fn ensure_runtime_scene(&mut self, cx: &mut Cx) {
        if self.scene_dirty || self.scene.is_none() {
            self.rebuild_runtime_scene(cx);
        }
    }

    pub(crate) fn runtime_scene_mut(&mut self) -> Option<&mut RapierScene> {
        self.scene.as_mut()
    }

    pub(crate) fn runtime_scene_ref(&self) -> Option<&RapierScene> {
        self.scene.as_ref()
    }

    pub(crate) fn runtime_bodies_clone(&self) -> Rc<HashMap<WidgetUid, XrRuntimeBodyState>> {
        self.runtime_bodies.clone()
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
        if !self.preview_resources_logged
            && self.preview_color_texture.is_some()
            && self.preview_depth_texture.is_some()
        {
            self.preview_resources_logged = true;
            crate::log!("XrScene preview pass resources created");
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
        if !cx.in_xr_mode() {
            self.xr_anchor_initialized = false;
            self.xr_anchor_log_count = 0;
        }

        match event {
            Event::Startup => {
                crate::log!(
                    "XrScene startup child_count={} camera_distance={} fov_y={} xr_anchor_to_head={} anchor_offset=({:.3},{:.3},{:.3}) forward_offset={:.3}",
                    self.node.child_count(),
                    self.camera_distance,
                    self.camera_fov_y,
                    self.xr_anchor_to_head,
                    self.xr_anchor_position_offset.x,
                    self.xr_anchor_position_offset.y,
                    self.xr_anchor_position_offset.z,
                    self.xr_anchor_forward_offset
                );
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
                self.xr_draw_logged = false;
                if self.xr_anchor_to_head
                    && (!self.xr_anchor_initialized || Self::xr_should_reanchor_anchor(update))
                {
                    self.update_xr_anchor_pose(update.state.as_ref());
                    self.scene_dirty = true;
                }
                if Self::reset_requested(update) {
                    self.reset_scene(cx);
                }
                self.ensure_runtime_scene(cx);
                if let Some(scene) = self.scene.as_mut() {
                    scene.step();
                }
                self.sync_runtime_bodies();
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
        let preview_rect = Rect {
            pos: dvec2(0.0, 0.0),
            size: preview_pass_size,
        };
        let scene_state = self.preview_scene_state(preview_rect, preview_pass_size, cx.time());
        if self.preview_draw_log_count < 10 {
            crate::log!(
                "XrScene preview draw[{}] rect={}x{} pass_size={}x{} scene_state={} runtime_bodies={} child_count={} has_preview_color={} has_preview_depth={}",
                self.preview_draw_log_count,
                rect.size.x,
                rect.size.y,
                preview_pass_size.x,
                preview_pass_size.y,
                scene_state.is_some(),
                self.runtime_bodies.len(),
                self.node.child_count(),
                self.preview_color_texture.is_some(),
                self.preview_depth_texture.is_some()
            );
            self.preview_draw_log_count += 1;
        }

        if let Some(scene_state) = scene_state {
            self.update_preview_pass_camera(cx.cx, scene_state);
            let mut draw_scope = XrDrawScopeData {
                runtime_bodies: self.runtime_bodies.clone(),
                env_texture: None,
                camera_texture: None,
                camera_source_size: vec2f(1280.0, 960.0),
                camera_rotation_steps: 0.0,
                camera_center_offset_uv: vec2f(0.0, 0.0),
                camera_enabled: false,
                pointer_tips: [None, None],
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
            if !self.xr_draw_logged {
                self.xr_draw_logged = true;
                crate::log!("XrScene draw_3d skipped: no last_xr_state");
            }
            return DrawStep::done();
        };
        if !self.xr_draw_logged {
            self.xr_draw_logged = true;
            crate::log!(
                "XrScene draw_3d state time={} scene_dirty={} runtime_bodies={} child_count={} xr_anchor_to_head={} xr_anchor_initialized={} anchor_pos=({:.3},{:.3},{:.3})",
                state.time,
                self.scene_dirty,
                self.runtime_bodies.len(),
                self.node.child_count(),
                self.xr_anchor_to_head,
                self.xr_anchor_initialized,
                self.xr_anchor_pose.position.x,
                self.xr_anchor_pose.position.y,
                self.xr_anchor_pose.position.z
            );
        }

        let scene_state = self.xr_scene_state(state.as_ref());
        cx.begin_scene_3d(scene_state);
        let previous_world = if self.xr_anchor_to_head && self.xr_anchor_initialized {
            cx.set_scene_world_transform_3d(self.xr_anchor_pose.to_mat4())
        } else {
            None
        };
        self.node.draw_3d_all(cx, scope);
        if let Some(previous_world) = previous_world {
            let _ = cx.set_scene_world_transform_3d(previous_world);
        }
        cx.end_scene_3d();
        DrawStep::done()
    }
}
