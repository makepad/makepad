pub use makepad_widgets;

use makepad_widgets::*;
use makepad_xr::XrScene as EngineXrScene;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.ExampleXrSceneBase = #(ExampleXrScene::register_widget(vm))
    mod.widgets.ExampleXrScene = set_type_default() do mod.widgets.ExampleXrSceneBase{
        scene := mod.widgets.XrScene{}
        cube_half_extents: vec3(0.12, 0.12, 0.12)
        cube_color: vec4(0.88, 0.52, 0.26, 1.0)
        cube_corner_radius: 0.018
        cube_roughness: 0.42
    }

    let active_scene = "gltf"

    fn select_scene(scene_id, label) {
        active_scene = scene_id
        ui.scene_label.set_text(label)
        ui.desktop_scene.render()
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            on_startup: ||{
                ui.desktop_scene.render()
            }

            main_window := Window{
                window.inner_size: vec2(1400, 900)
                pass.clear_color: #x0b1118
                body +: {
                    main := View{
                        width: Fill
                        height: Fill
                        flow: Right

                        sidebar := RoundedView{
                            width: 320
                            height: Fill
                            flow: Down
                            spacing: 14
                            padding: Inset{left: 20 right: 20 top: 22 bottom: 20}
                            draw_bg.color: #x0d1520
                            draw_bg.radius: 0.0

                            title := H1{
                                text: "XR Demo"
                                draw_text.color: #xeff7ff
                            }

                            intro := Label{
                                width: Fill
                                text: "Desktop mode stays in 2D UI with the scene viewport on the right. Drag to orbit, scroll to zoom, and click the physics stack to kick cubes."
                                draw_text.color: #xb8c8d8
                            }

                            scene_label := Label{
                                width: Fill
                                text: "Single GLTF object"
                                draw_text.color: #x8fe4d6
                            }

                            show_gltf := Button{
                                width: Fill
                                text: "GLTF"
                                on_click: || select_scene("gltf", "Single GLTF object")
                            }

                            show_splat := Button{
                                width: Fill
                                text: "Splat Trio"
                                on_click: || select_scene("splat", "Helmet plus two splat assets")
                            }

                            show_physics := Button{
                                width: Fill
                                text: "Physics Cubes"
                                on_click: || select_scene("physics", "Interactive physics cube stack")
                            }

                            filler := Filler{}

                            xr_note := Label{
                                width: Fill
                                text: "XR presentation stays on the separate XR scene path when headset support is available."
                                draw_text.color: #x6f8191
                            }
                        }

                        gutter := SolidView{
                            width: 1
                            height: Fill
                            draw_bg.color: #x1a2633
                        }

                        viewport := View{
                            width: Fill
                            height: Fill

                            desktop_scene := Scene3D{
                                width: Fill
                                height: Fill
                                animating: true
                                spin_speed: 0.0
                                camera_fov_y: 46.0
                                camera_distance: 5.8
                                camera_near: 0.02
                                camera_far: 400.0
                                camera_target: vec3(0.0, 0.35, 0.0)
                                draw_bg +: {
                                    color: #x111823
                                    draw_depth: -400.0
                                }

                                scene_root := Node3D{
                                    on_render: ||{
                                        if active_scene == "gltf" {
                                            ground := Grid3D{
                                                size: 16.0
                                                position: vec3(0.0, -1.25, 0.0)
                                                color: vec4(0.56, 0.58, 0.61, 1.0)
                                            }
                                            model := Gltf3D{
                                                src: crate_resource("self://resources/DamagedHelmet.glb")
                                                env_src: crate_resource("self://resources/royal_esplanade_4k.jpg")
                                                scale: vec3(0.82, 0.82, 0.82)
                                                rotation: vec3(0.0, 0.45, 0.0)
                                            }
                                        }
                                        else if active_scene == "splat" {
                                            ground := Grid3D{
                                                size: 18.0
                                                position: vec3(0.0, -1.25, 0.0)
                                                color: vec4(0.56, 0.58, 0.61, 1.0)
                                            }
                                            helmet := Gltf3D{
                                                src: crate_resource("self://resources/DamagedHelmet.glb")
                                                env_src: crate_resource("self://resources/royal_esplanade_4k.jpg")
                                                position: vec3(0.0, -0.1, 0.0)
                                                rotation: vec3(0.0, 1.2, 0.0)
                                                scale: vec3(0.38, 0.38, 0.38)
                                            }
                                            sog := ViewSplat{
                                                src: crate_resource("self://../../local/toy-cat.sog")
                                                position: vec3(-1.8, 0.0, 0.0)
                                                scale: vec3(1.0, -1.0, 1.0)
                                                normalize_fit: 2.3
                                                max_splats: 0
                                                radius_scale: 1.1
                                                min_radius: 0.0012
                                            }
                                            ply := ViewSplat{
                                                src: crate_resource("self://../../local/biker.ply")
                                                position: vec3(1.8, -0.1, 0.0)
                                                scale: vec3(1.0, -1.0, 1.0)
                                                normalize_fit: 2.0
                                                max_splats: 0
                                                radius_scale: 1.1
                                                min_radius: 0.0012
                                            }
                                        }
                                        else {
                                            simulation := PhysicsWorld3D{}
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            xr_scene := mod.widgets.ExampleXrScene{}
        }
    }
}

const XR_CUBE_FORWARD_OFFSET: f32 = 0.55;
const XR_CUBE_VIEW_OFFSET_Y: f32 = -0.12;

#[derive(Script, ScriptHook, Widget)]
pub struct ExampleXrScene {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    scene: EngineXrScene,
    #[live(vec3(0.12, 0.12, 0.12))]
    cube_half_extents: Vec3f,
    #[live(vec4(0.88, 0.52, 0.26, 1.0))]
    cube_color: Vec4f,
    #[live(0.018)]
    cube_corner_radius: f32,
    #[live(0.42)]
    cube_roughness: f32,
    #[rust]
    cube_pose: Pose,
    #[rust]
    cube_pose_valid: bool,
}

impl ExampleXrScene {
    fn scene_forward(state: &XrState) -> Vec3f {
        let mut forward = state.vec_in_head_space(vec3(0.0, 0.0, -1.0)) - state.head_pose.position;
        forward.y = 0.0;
        if forward.length() <= 1.0e-4 {
            vec3f(0.0, 0.0, -1.0)
        } else {
            forward.normalize()
        }
    }

    fn compute_cube_pose(state: &XrState) -> Pose {
        let forward = Self::scene_forward(state);
        let orientation = Quat::look_rotation(forward, vec3f(0.0, 1.0, 0.0));
        Pose::new(
            orientation,
            state.head_pose.position
                + forward * XR_CUBE_FORWARD_OFFSET
                + vec3f(0.0, XR_CUBE_VIEW_OFFSET_Y, 0.0),
        )
    }

    fn reset_cube_anchor(&mut self, state: &XrState) {
        self.cube_pose = Self::compute_cube_pose(state);
        self.cube_pose_valid = true;
    }

    fn ensure_cube_anchor(&mut self, state: &XrState) {
        if self.scene.ensure_scene(state) || !self.cube_pose_valid {
            self.reset_cube_anchor(state);
        }
    }
}

impl Widget for ExampleXrScene {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.scene.handle_event(cx, event, scope);
        if let Event::XrUpdate(update) = event {
            if EngineXrScene::reset_requested(update) {
                self.reset_cube_anchor(&update.state);
            }
            self.ensure_cube_anchor(&update.state);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.scene.draw_walk(cx, scope, walk)
    }

    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        let _ = self.scene.draw_3d(cx, scope);
        let Some(state) = cx.draw_event.xr_state.as_ref() else {
            return DrawStep::done();
        };

        self.ensure_cube_anchor(state);
        let cx = &mut Cx2d::new(cx.cx);
        self.scene.draw_rounded_cube(
            cx,
            self.cube_pose,
            self.cube_half_extents,
            self.cube_corner_radius,
            self.cube_color,
            self.cube_roughness,
        );
        DrawStep::done()
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl MatchEvent for App {
    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        makepad_xr::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
