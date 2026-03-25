use crate::{
    IcoSphere,
    xr_env::{makepad_pose, RapierScene},
    xr_node::{XrBodyKind, XrNode, XrRuntimeBodyState},
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
    is_sphere: bool,
    density: f32,
    friction: f32,
    restitution: f32,
}

#[derive(Script, ScriptHook, Widget)]
pub struct XrScene {
    #[live]
    physics: XrPhysics,
    #[rust]
    scene: Option<RapierScene>,
    #[rust]
    runtime_bodies: Rc<HashMap<WidgetUid, XrRuntimeBodyState>>,
    #[rust(true)]
    scene_dirty: bool,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    scene_root_pose: Option<Pose>,
    #[cast]
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

    fn scene_root_transform_state(&self) -> XrTransformState {
        if let Some(pose) = self.scene_root_pose {
            XrTransformState {
                position: pose.position,
                orientation: pose.orientation,
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
        let Some(node) = widget.cast_inner::<XrNode>() else {
            widget.children(&mut |_, child| Self::collect_cubes_from_widget(&child, parent, cubes));
            return;
        };

        let is_sphere = widget.borrow::<IcoSphere>().is_some();
        let world = Self::transform_with_node(parent, &node);
        let half_extents = node.physics_half_extents();
        let should_push = node.body_kind() != XrBodyKind::Disabled
            && (half_extents.x > 0.0 || half_extents.y > 0.0 || half_extents.z > 0.0);

        if should_push {
            cubes.push(CollectedXrCube {
                uid: widget.widget_uid(),
                body_kind: node.body_kind(),
                pose: Pose::new(world.orientation, world.position),
                scale: world.scale,
                half_extents: vec3f(
                    half_extents.x * world.scale.x,
                    half_extents.y * world.scale.y,
                    half_extents.z * world.scale.z,
                ),
                is_sphere,
                density: node.density(),
                friction: node.friction(),
                restitution: node.restitution(),
            });
        }

        drop(node);
        widget.children(&mut |_, child| Self::collect_cubes_from_widget(&child, world, cubes));
    }

    fn collect_rendered_cubes(&self) -> Vec<CollectedXrCube> {
        let mut cubes = Vec::new();
        let root = self.scene_root_transform_state();
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
                XrBodyKind::Dynamic => {
                    if cube.is_sphere {
                        scene.spawn_dynamic_sphere(
                            cube.uid,
                            cube.pose,
                            cube.half_extents.x.min(cube.half_extents.y).min(cube.half_extents.z),
                            cube.scale,
                            cube.density,
                            cube.friction,
                            cube.restitution,
                        );
                    } else {
                        scene.spawn_dynamic_box(
                            cube.uid,
                            cube.pose,
                            cube.half_extents,
                            cube.scale,
                            cube.density,
                            cube.friction,
                            cube.restitution,
                        );
                    }
                }
                XrBodyKind::Fixed => {
                    if cube.is_sphere {
                        scene.spawn_fixed_sphere(
                            cube.uid,
                            cube.pose,
                            cube.half_extents.x.min(cube.half_extents.y).min(cube.half_extents.z),
                            cube.scale,
                            cube.friction,
                            cube.restitution,
                        );
                    } else {
                        scene.spawn_fixed_box(
                            cube.uid,
                            cube.pose,
                            cube.half_extents,
                            cube.scale,
                            cube.friction,
                            cube.restitution,
                        );
                    }
                }
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

    pub(crate) fn set_root_pose(&mut self, cx: &mut Cx, pose: Option<Pose>) {
        if self.scene_root_pose == pose {
            return;
        }
        self.scene_root_pose = pose;
        self.scene_dirty = true;
        self.redraw(cx);
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
                cx.with_vm(|vm| {
                    let _ = self.node.script_call(vm, live_id!(render), NIL);
                });
                self.next_frame = cx.new_next_frame();
                self.ensure_runtime_scene(cx);
            }
            Event::NextFrame(ne) if self.next_frame.is_event(event).is_some() => {
                if !cx.in_xr_mode() && self.should_preview_step() {
                    if let Some(scene) = self.scene.as_mut() {
                        scene.step();
                    }
                    self.sync_runtime_bodies();
                    self.redraw(cx);
                }
                self.next_frame = cx.new_next_frame();
                let _ = ne;
            }
            Event::XrUpdate(update) => {
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
    }

    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        self.ensure_runtime_scene(cx.cx);
        self.node.draw_3d_all(cx, scope);
        DrawStep::done()
    }
}
