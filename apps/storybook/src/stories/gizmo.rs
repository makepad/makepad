//! The 3D gizmo page: a transform gizmo moving a box in a small wireframe
//! scene under an orbit camera, the orientation gizmo in its corner, and the
//! orientation gizmo on its own in both of its shapes.
use crate::makepad_widgets::gizmo::makepad_gizmo::{
    frame_of_view, look_from, mat_translation, orbit_view, transform_point, view_distance,
    view_from_frame, GizmoCamera, GizmoPrim, GizmoRect, UpAxis, ViewDir,
};
use crate::makepad_widgets::gizmo::*;
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.storybook.StoryGizmoWorldBase = #(StoryGizmoWorld::register_widget(vm))
    /** The scene under the gizmo: a ground grid and a box, drawn as lines
     * through the same camera the gizmos are handed. A drag on it orbits. */
    mod.storybook.StoryGizmoWorld = set_type_default() do mod.storybook.StoryGizmoWorldBase{
        width: Fill
        height: Fill
        draw_bg +: {color: #x1b1d23}
    }

    mod.storybook.StoryGizmoSceneBase = #(StoryGizmoScene::register_widget(vm))
    /** A viewport: the scene, the transform gizmo over all of it, the
     * orientation gizmo in the corner and a readout at the foot. Every layer
     * is named, so a variant overrides layers rather than adding them. */
    mod.storybook.StoryGizmoScene = set_type_default() do mod.storybook.StoryGizmoSceneBase{
        width: Fill
        height: 460.
        flow: Overlay
        world := mod.storybook.StoryGizmoWorld{}
        subject := Gizmo3d{}
        corner := View{
            width: Fill
            height: Fill
            align: Align{x: 1.0 y: 0.0}
            padding: Inset{top: 10. right: 10.}
            cube := ViewCube{}
        }
        foot := View{
            width: Fill
            height: Fill
            align: Align{x: 0.0 y: 1.0}
            padding: Inset{left: 12. bottom: 10.}
            status := Label{
                text: "Drag a handle to move the box; drag the ground to orbit."
                draw_text +: {color: #xc4c8d0}
            }
        }
    }

    mod.stories.Gizmo3dOverview = StoryPage{
        StoryNote{text: "Two gizmos that sit over a 3D view and know nothing about it but its matrices. Gizmo3d moves, turns and scales one model matrix; ViewCube turns the camera. The scene below is a grid and a box drawn as lines, so the page needs no 3D renderer: an app lays both widgets over whatever it draws."}

        StoryHeading{text: "Over a scene"}
        StoryNote{text: "The gizmo takes the pointer only over a handle, so a drag anywhere else orbits the camera as it would without it; Shift and a vertical drag moves the camera in and out. Arrows move along an axis, squares in a plane, the centre dot in the screen plane. Rings turn about an axis and the outer ring about the line of sight. Boxes scale along the box's own axes and the centre dot scales all three. Escape during a drag puts the box back. Ctrl (Cmd on macOS) inverts snapping while it is held."}
        scene := mod.storybook.StoryGizmoScene{}
        StoryRow{
            reset := Button{text: "Reset the box"}
            front := Button{text: "Look from the front"}
        }

        StoryHeading{text: "The orientation gizmo on its own"}
        StoryNote{text: "A cube whose faces, edges and corners are all targets, and the compact form: a ball per axis end, the positive ones labelled and solid, the negative ones hollow, the near ones larger. A click turns to that view and a second click on it turns to the opposite one; a drag orbits. The right two are Z-up, as a CAD app would have them. Unwired, each turns only its own picture."}
        StoryRow{
            ViewCube{}
            ViewCube{shape: ViewCubeShape.Balls}
            ViewCube{up: ViewCubeUp.Z}
            ViewCube{up: ViewCubeUp.Z shape: ViewCubeShape.Balls width: 140 height: 140}
        }
    }
}

const FOV_Y: f32 = 40.0;
const NEAR: f32 = 0.05;
const FAR: f32 = 200.0;

fn initial_view() -> Mat4f {
    look_from(vec3(1.0, 0.75, 1.35), vec3(0.0, 0.0, 0.0), 6.0, UpAxis::Y)
}

fn initial_model() -> Mat4f {
    Mat4f::translation(vec3(0.0, 0.5, 0.0))
}

/// A world segment on screen, cut at the near side of the camera, or None
/// when it is wholly behind it.
fn project_segment(cam: &GizmoCamera, a: Vec3f, b: Vec3f) -> Option<(Vec2f, Vec2f)> {
    let eps = 0.02;
    let wa = cam.clip(a).w;
    let wb = cam.clip(b).w;
    if wa < eps && wb < eps {
        return None;
    }
    let (a, b) = if wa < eps {
        (a + (b - a) * ((eps - wa) / (wb - wa)), b)
    } else if wb < eps {
        (a, b + (a - b) * ((eps - wb) / (wa - wb)))
    } else {
        (a, b)
    };
    Some((cam.project(a)?, cam.project(b)?))
}

/// The box's corners: bit 0 is x, bit 1 y and bit 2 z.
const FACES: [[usize; 4]; 6] = [
    [0, 2, 6, 4],
    [1, 3, 7, 5],
    [0, 1, 5, 4],
    [2, 3, 7, 6],
    [0, 1, 3, 2],
    [4, 5, 7, 6],
];

/// The scene as lines and faces: the ground grid, then the box with its
/// faces toward the camera tinted and the edges behind it faint.
fn scene_prims(cam: &GizmoCamera, model: &Mat4f) -> Vec<GizmoPrim> {
    let mut out = Vec::new();
    let grid = vec4(0.55, 0.58, 0.66, 0.16);
    for i in -5..=5 {
        let f = i as f32;
        let (cx_, cz) = if i == 0 {
            (vec4(0.93, 0.29, 0.31, 0.55), vec4(0.27, 0.54, 0.96, 0.55))
        } else {
            (grid, grid)
        };
        if let Some((a, b)) = project_segment(cam, vec3(-5.0, 0.0, f), vec3(5.0, 0.0, f)) {
            out.push(GizmoPrim::line(a, b, 1.0, cx_));
        }
        if let Some((a, b)) = project_segment(cam, vec3(f, 0.0, -5.0), vec3(f, 0.0, 5.0)) {
            out.push(GizmoPrim::line(a, b, 1.0, cz));
        }
    }

    let corners: Vec<Vec3f> = (0..8)
        .map(|k| {
            let p = vec3(
                (k & 1) as f32 - 0.5,
                ((k >> 1) & 1) as f32 - 0.5,
                ((k >> 2) & 1) as f32 - 0.5,
            );
            transform_point(model, p)
        })
        .collect();
    let center = mat_translation(model);
    let mut visible = [false; 6];
    for (n, face) in FACES.iter().enumerate() {
        let [a, b, c, d] = face.map(|k| corners[k]);
        let mid = (a + b + c + d) * 0.25;
        let mut normal = Vec3f::cross(b - a, d - a);
        if normal.dot(mid - center) < 0.0 {
            normal = normal * -1.0;
        }
        let facing = normal.normalize().dot(cam.to_camera(mid));
        if facing <= 0.0 {
            continue;
        }
        visible[n] = true;
        let pts: Vec<Vec2f> = face
            .iter()
            .filter_map(|k| cam.project(corners[*k]))
            .collect();
        if pts.len() == 4 {
            let tint = vec4(0.55, 0.64, 0.80, 0.14 + 0.22 * facing);
            out.push(GizmoPrim::Quad {
                pts: [pts[0], pts[1], pts[2], pts[3]],
                fill: tint,
                width: 0.0,
                color: tint,
            });
        }
    }
    for k in 0..8usize {
        for bit in [1usize, 2, 4] {
            if k & bit != 0 {
                continue;
            }
            let j = k | bit;
            let front = FACES
                .iter()
                .enumerate()
                .any(|(n, f)| visible[n] && f.contains(&k) && f.contains(&j));
            let color = if front {
                vec4(0.92, 0.94, 0.98, 0.95)
            } else {
                vec4(0.92, 0.94, 0.98, 0.22)
            };
            if let Some((a, b)) = project_segment(cam, corners[k], corners[j]) {
                out.push(GizmoPrim::line(a, b, if front { 1.6 } else { 1.0 }, color));
            }
        }
    }
    out
}

/// The scene: draws the grid and the box, and orbits on a drag.
#[derive(Script, ScriptHook, Widget)]
pub struct StoryGizmoWorld {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_line: DrawGizmoStroke,
    #[live]
    draw_disc: DrawGizmoDisc,
    #[live]
    draw_face: DrawGizmoFace,
    #[live]
    draw_text: DrawText,
    #[rust]
    view: Mat4f,
    #[rust]
    model: Mat4f,
    #[rust]
    last: Option<DVec2>,
}

impl Widget for StoryGizmoWorld {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => cx.set_cursor(MouseCursor::Grab),
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                self.last = Some(fe.abs);
                cx.set_cursor(MouseCursor::Grabbing);
            }
            Hit::FingerMove(fe) => {
                let Some(last) = self.last else {
                    return;
                };
                let d = fe.abs - last;
                self.last = Some(fe.abs);
                let pivot = vec3(0.0, 0.0, 0.0);
                let view = if fe.modifiers.shift {
                    // In and out along the line to the pivot.
                    let dist = view_distance(&self.view, pivot);
                    let dist = (dist * 1.01f32.powf(d.y as f32)).clamp(1.5, 60.0);
                    let (right, up, back, _) = frame_of_view(&self.view);
                    view_from_frame(right, up, back, pivot + back * dist)
                } else {
                    orbit_view(
                        &self.view,
                        pivot,
                        vec3(0.0, 1.0, 0.0),
                        -(d.x as f32) * 0.01,
                        -(d.y as f32) * 0.01,
                    )
                };
                cx.widget_action(self.uid, GizmoAction::ViewChanged(view));
            }
            Hit::FingerUp(_) => {
                self.last = None;
                cx.set_cursor(MouseCursor::Grab);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = self.draw_bg.draw_walk(cx, walk);
        let viewport = GizmoRect::new(
            rect.pos.x as f32,
            rect.pos.y as f32,
            rect.size.x as f32,
            rect.size.y as f32,
        );
        let proj = Mat4f::perspective(FOV_Y, viewport.w / viewport.h.max(1.0), NEAR, FAR);
        let cam = GizmoCamera::new(self.view, proj, viewport);
        let prims = scene_prims(&cam, &self.model);
        paint_gizmo_prims(
            cx,
            &prims,
            &mut self.draw_line,
            &mut self.draw_disc,
            &mut self.draw_face,
            &mut self.draw_text,
        );
        DrawStep::done()
    }
}

/// The viewport: owns the camera and the box's model matrix, and hands them
/// to the scene and to both gizmos whenever either changes.
#[derive(Script, ScriptHook, Widget)]
pub struct StoryGizmoScene {
    #[deref]
    view: View,
    #[rust]
    camera: Mat4f,
    #[rust]
    model: Mat4f,
    #[rust]
    ready: bool,
}

impl StoryGizmoScene {
    fn push(&mut self, cx: &mut Cx) {
        let (camera, model) = (self.camera, self.model);
        let world = self.view.widget(cx, ids!(world));
        if let Some(mut w) = world.borrow_mut::<StoryGizmoWorld>() {
            w.view = camera;
            w.model = model;
        }
        world.redraw(cx);
        let gizmo = self.view.gizmo3d(cx, ids!(subject));
        gizmo.set_perspective(cx, camera, FOV_Y, NEAR, FAR);
        if !gizmo.is_dragging() {
            gizmo.set_model(cx, model);
        }
        self.view.view_cube(cx, ids!(cube)).set_view(cx, camera);
        let t = mat_translation(&model);
        self.view.label(cx, ids!(status)).set_text(
            cx,
            &format!("box at  x {:.2}   y {:.2}   z {:.2}", t.x, t.y, t.z),
        );
    }

    fn ensure_ready(&mut self, cx: &mut Cx) {
        if !self.ready {
            self.ready = true;
            self.camera = initial_view();
            self.model = initial_model();
            self.push(cx);
        }
    }

    /// Put the box back where it started.
    pub fn reset(&mut self, cx: &mut Cx) {
        self.ensure_ready(cx);
        self.model = initial_model();
        self.push(cx);
    }

    /// Turn the camera through the view cube, as a click on its face would.
    pub fn look_from_front(&mut self, cx: &mut Cx) {
        self.ensure_ready(cx);
        self.view
            .view_cube(cx, ids!(cube))
            .snap_to(cx, ViewDir::POS_Z);
    }
}

impl Widget for StoryGizmoScene {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ensure_ready(cx);
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let actions = cx.capture_actions(|cx| self.view.handle_event(cx, event, scope));
        if actions.is_empty() {
            return;
        }
        let gizmo = self.view.gizmo3d(cx, ids!(subject));
        let mut changed = false;
        if let Some(model) = gizmo.changed(&actions) {
            self.model = model;
            changed = true;
        }
        let from_cube = self.view.view_cube(cx, ids!(cube)).view_changed(&actions);
        let world_uid = self.view.widget(cx, ids!(world)).widget_uid();
        let from_world = actions
            .filter_widget_actions_cast::<GizmoAction>(world_uid)
            .filter_map(|a| match a {
                GizmoAction::ViewChanged(v) => Some(v),
                _ => None,
            })
            .last();
        if let Some(view) = from_world.or(from_cube) {
            self.camera = view;
            changed = true;
        }
        if changed {
            self.push(cx);
        }
        cx.extend_actions(actions);
    }
}

fn gizmo_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let scene = root.widget(cx, ids!(scene));
    if root.button(cx, ids!(reset)).clicked(actions) {
        if let Some(mut scene) = scene.borrow_mut::<StoryGizmoScene>() {
            scene.reset(cx);
        }
    }
    if root.button(cx, ids!(front)).clicked(actions) {
        if let Some(mut scene) = scene.borrow_mut::<StoryGizmoScene>() {
            scene.look_from_front(cx);
        }
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "inputs/gizmo3d/overview",
    category: "Inputs",
    component: "Gizmo3d",
    also: &["ViewCube"],
    name: "3D Gizmo",
    dsl: "Gizmo3dOverview",
    added: "2026-09-25",
    tags: &[
        "3d", "transform", "gizmo", "manipulator", "translate", "rotate", "scale", "view cube",
        "orientation", "camera", "orbit", "new",
    ],
    doc: "# Gizmo3d and ViewCube

Two widgets that sit over a 3D view and know nothing about it but its matrices. **Gizmo3d** moves, turns and scales one model matrix. **ViewCube** turns the camera. The app draws the scene however it likes and lays both over it in an `Overlay` flow, the `Gizmo3d` filling the view so its viewport is the view's.

## Wiring

- `set_perspective(cx, view, fov_y_deg, near, far)` hands the gizmo the camera, completing the projection with the widget's own aspect; `set_camera(cx, view, proj)` takes a projection matrix outright, perspective or orthographic. `set_model(cx, model)` is the matrix it manipulates.
- A drag reports `GizmoAction::DragStart`, then `Changed(model)` as it moves, then `DragEnd`. `changed(actions)` on the ref returns the latest model. Escape during a drag reports the model the drag started from, and ends it.
- `ViewCube::set_view(cx, view)` hands the orientation gizmo the camera; `set_pivot` is the point it orbits and looks at (the origin unless set). A click or a drag reports `ViewChanged(view)`, which `view_changed(actions)` returns. Move the camera, then hand the new view to both gizmos.

## The transform gizmo

- **Translate**: an arrow per axis, a square per plane and a centre dot that moves in the screen plane. **Rotate**: a ring per axis, only its half facing the camera, and an outer ring about the line of sight. **Scale**: a box per local axis and a centre dot for all three. **Universal**: the translate handles, the rings further out and the scale boxes between them.
- `space` is `World` or `Local` for translate and rotate. Scale always follows the model's own axes.
- It only takes the pointer over a handle: its hit test is its pick, so presses anywhere else reach the scene underneath and the app's camera keeps working around it.
- An axis seen end-on fades and stops being pickable, and so does a plane seen edge-on. Arrows and boxes point along the half of their axis facing the camera, frozen for the length of a drag so nothing jumps under the pointer.
- Snapping: `snap` with `snap_translate` (world units per axis of the space), `snap_rotate` (degrees) and `snap_scale` (a factor step away from one). The primary modifier inverts `snap` while held. A zero step never snaps.
- While dragging, the other handles fade, a move leaves a line back to where it began, a turn fills the angle it swept, and a readout under the gizmo shows the value (`show_readout`).

## The orientation gizmo

- `shape: Cube` is a labelled cube whose faces, edges and corners are all targets: 26 views. `shape: Balls` is the compact form: a ball per axis end, positive ones solid and labelled, negative ones hollow, near ones larger.
- A click turns the camera to look from there at the pivot, over `snap_seconds`; a click on the view already showing turns to the opposite one. A drag orbits: yaw about the world's up axis, pitch about the camera's right, `orbit_speed` radians per point.
- `up: Y` or `up: Z` names the faces and keeps views from above and below the right way round.

## Underneath

The picking, dragging, snapping and pictures are pure math in `makepad-gizmo`, re-exported as `makepad_widgets::gizmo::makepad_gizmo`: `Gizmo::hover`, `begin_drag`, `drag`, `end_drag`, `cancel_drag` and `geometry`, and `ViewCube::hover`, `snap_view` and `geometry`, with `orbit_view`, `look_from` and `interpolate_view` for cameras. The pictures are screen-space primitives — lines, circles, arcs, quads, text — which `paint_gizmo_prims` draws with three SDF shaders. This page draws its scene with the same primitives.",
    subject: "subject",
    feature: None,
    controls: &[
        Control {
            label: "Mode",
            target: "subject",
            kind: ControlKind::Choice {
                prop: "mode",
                options: &[
                    "Gizmo3dMode.Translate",
                    "Gizmo3dMode.Rotate",
                    "Gizmo3dMode.Scale",
                    "Gizmo3dMode.Universal",
                ],
                default: 3,
            },
        },
        Control {
            label: "Space",
            target: "subject",
            kind: ControlKind::Choice {
                prop: "space",
                options: &["Gizmo3dSpace.World", "Gizmo3dSpace.Local"],
                default: 0,
            },
        },
        Control { label: "Snap", target: "subject", kind: ControlKind::Bool { prop: "snap", default: false } },
        Control { label: "Move step", target: "subject", kind: ControlKind::Number { prop: "snap_translate", min: 0., max: 2., step: 0.05, default: 0.25 } },
        Control { label: "Turn step (deg)", target: "subject", kind: ControlKind::Number { prop: "snap_rotate", min: 0., max: 90., step: 1., default: 15. } },
        Control { label: "Scale step", target: "subject", kind: ControlKind::Number { prop: "snap_scale", min: 0., max: 1., step: 0.05, default: 0.1 } },
        Control { label: "Size", target: "subject", kind: ControlKind::Number { prop: "size", min: 40., max: 240., step: 1., default: 96. } },
        Control { label: "Readout", target: "subject", kind: ControlKind::Bool { prop: "show_readout", default: true } },
        Control {
            label: "Cube shape",
            target: "cube",
            kind: ControlKind::Choice {
                prop: "shape",
                options: &["ViewCubeShape.Cube", "ViewCubeShape.Balls"],
                default: 0,
            },
        },
    ],
    on_actions: Some(gizmo_actions),
}];
