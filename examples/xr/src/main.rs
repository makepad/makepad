pub use makepad_widgets;

use makepad_widgets::*;
use makepad_xr::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let Block = Cube{
        size: vec3(0.145, 0.082, 0.075)
        corner_radius: 0.018
        roughness: 0.28
        metallic: 0.02
    }

    let Platform = Cube{
        body: mod.widgets.XrBodyKind.Fixed
        size: vec3(1.45, 0.08, 0.44)
        corner_radius: 0.022
        roughness: 0.82
        metallic: 0.0
        color: #x2b3643
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1400, 900)
                pass.clear_color: #x0b1118
                body +: {
                    xr_root := XrRoot{
                        control_2d: @block_ctrl
                        control_xr: @block_ctrl
                        scene: @tree_scene
                        env_cube: true
                        depth_mesh: false

                        block_ctrl := View{
                            width: Fill
                            height: Fit
                            flow: Down
                            spacing: 12

                            title := H1{
                                text: "XR Blocks"
                                draw_text.color: #xeff7ff
                            }

                            detail := Label{
                                width: Fill
                                text: "Pick a scene directly. Reset rebuilds only the active scene."
                                draw_text.color: #xb8c8d8
                            }

                            reset := Button{
                                width: Fill
                                text: "Reset Scene"
                            }

                            show_blocks := Button{
                                width: Fill
                                text: "Blocks"
                            }

                            show_helmet := Button{
                                width: Fill
                                text: "Helmet"
                            }

                            show_tree := Button{
                                width: Fill
                                text: "Tree"
                            }

                            show_refraction := Button{
                                width: Fill
                                text: "Refraction"
                            }

                            depth_toggle := Button{
                                width: Fill
                                text: "Toggle Depth Mesh"
                            }
                        }

                        block_scene := XrScene{
                            physics: XrPhysics{gravity: 9.8}
                            camera_fov_y: 26.0
                            camera_distance: 3.6
                            preview_aspect_fill: true
                            on_render: ||{
                                Platform{pos: vec3(0.05, -0.06, -0.10)}
                                for row in 0..8 {
                                    for col in 0..8 {
                                        let offset = if row % 2 == 0 {0.0} else {0.0725}
                                        let color = if (row + col) % 6 == 0 {
                                            #xff5a4f
                                        } else if (row + col) % 6 == 1 {
                                            #x3ecf8e
                                        } else if (row + col) % 6 == 2 {
                                            #x57a1ff
                                        } else if (row + col) % 6 == 3 {
                                            #xffc857
                                        } else if (row + col) % 6 == 4 {
                                            #xff8f3f
                                        } else {
                                            #xd16dff
                                        }
                                        Block{
                                            pos: vec3(-0.46 + col * 0.145 + offset, 0.028 + row * 0.084, -0.10)
                                            color: color
                                        }
                                    }
                                }
                            }
                        }

                        helmet_scene := XrScene{
                            physics: XrPhysics{gravity: 9.8}
                            camera_fov_y: 26.0
                            camera_distance: 4.0
                            preview_aspect_fill: true
                            on_render: ||{
                                Platform{pos: vec3(0.05, -0.06, -0.10)}
                                for row in 0..1 {
                                    for col in 0..1 {
                                        Gltf{
                                            body: mod.widgets.XrBodyKind.Dynamic
                                            physics_size: vec3(0.17, 0.21, 0.17)
                                            density: 0.9
                                            friction: 0.7
                                            restitution: 0.08
                                            pos: vec3(-0.23 + col * 0.22 + if row % 2 == 0 {0.0} else {0.08}, 0.08 + row * 0.22, -0.10)
                                            src: crate_resource("self://resources/DamagedHelmet.glb")
                                            mesh_scale: vec3(0.38, 0.38, 0.38)
                                            mesh_rotation: vec3(0.0, 1.5708, 0.0)
                                            mesh_position: vec3(0.0, 0.32, 0.0)
                                        }
                                    }
                                }
                            }
                        }

                        tree_scene := XrScene{
                            physics: XrPhysics{gravity: 9.8}
                            camera_fov_y: 24.0
                            camera_distance: 6.2
                            preview_aspect_fill: true
                            on_render: ||{
                                Platform{pos: vec3(0.05, -0.06, -0.10)}
                                fractal_tree := FractalTree{
                                    body: mod.widgets.XrBodyKind.Fixed
                                    physics_size: vec3(0.34, 0.92, 0.34)
                                    pos: vec3(0.05, -0.02, -0.10)
                                }
                            }
                        }

                        refraction_scene := XrScene{
                            physics: XrPhysics{gravity: 9.8}
                            camera_fov_y: 26.0
                            camera_distance: 3.6
                            preview_aspect_fill: true
                            on_render: ||{
                                Platform{pos: vec3(0.05, -0.06, -0.10)}
                                for row in 0..4 {
                                    for col in 0..4 {
                                        let offset = if row % 2 == 0 {0.0} else {0.06}
                                        RefractiveCube{
                                            pos: vec3(-0.22 + col * 0.12 + offset, 0.05 + row * 0.11, -0.10)
                                            size: vec3(0.115, 0.105, 0.085)
                                            color: vec4(0.82, 0.93, 1.0, 0.12)
                                            focus_distance: 1.6
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl App {
    fn button_clicked(&self, cx: &Cx, actions: &Actions, button_id: LiveId) -> bool {
        self.ui
            .widget_flood(cx, &[button_id])
            .borrow::<Button>()
            .is_some_and(|button| button.clicked(actions))
    }

    fn xr_root_widget(&self, cx: &Cx) -> WidgetRef {
        let path_body = self.ui.widget(cx, ids!(main_window.body.xr_root));
        if path_body.borrow::<XrRoot>().is_some() {
            return path_body;
        }

        let direct = self.ui.widget(cx, ids!(xr_root));
        if direct.borrow::<XrRoot>().is_some() {
            return direct;
        }

        let flood = self.ui.widget_flood(cx, ids!(xr_root));
        if flood.borrow::<XrRoot>().is_some() {
            return flood;
        }

        WidgetRef::empty()
    }

    fn call_xr_root(&self, cx: &mut Cx, method: LiveId) -> ScriptAsyncResult {
        let xr_root = self.xr_root_widget(cx);
        if xr_root.borrow::<XrRoot>().is_none() {
            return ScriptAsyncResult::MethodNotFound;
        }
        cx.with_vm(|vm| xr_root.script_call(vm, method, NIL))
    }

    fn select_scene(&self, cx: &mut Cx, scene_id: LiveId) -> ScriptAsyncResult {
        let xr_root = self.xr_root_widget(cx);
        if xr_root.borrow::<XrRoot>().is_none() {
            return ScriptAsyncResult::MethodNotFound;
        }
        cx.with_vm(|vm| xr_root.script_call(vm, live_id!(select_scene), ScriptValue::from_id(scene_id)))
    }

    fn sync_depth_toggle_label(&self, cx: &mut Cx, visible: bool) {
        let label = if visible {
            "Hide Depth Mesh"
        } else {
            "Show Depth Mesh"
        };
        let button = self.ui.widget_flood(cx, ids!(depth_toggle));
        button.set_text(cx, label);
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.button_clicked(cx, actions, live_id!(reset)) {
            let _ = self.call_xr_root(cx, live_id!(render_scene));
        }

        if self.button_clicked(cx, actions, live_id!(show_blocks)) {
            let _ = self.select_scene(cx, live_id!(block_scene));
        }

        if self.button_clicked(cx, actions, live_id!(show_helmet)) {
            let _ = self.select_scene(cx, live_id!(helmet_scene));
        }

        if self.button_clicked(cx, actions, live_id!(show_tree)) {
            let _ = self.select_scene(cx, live_id!(tree_scene));
        }

        if self.button_clicked(cx, actions, live_id!(show_refraction)) {
            let _ = self.select_scene(cx, live_id!(refraction_scene));
        }

        if self.button_clicked(cx, actions, live_id!(depth_toggle)) {
            if let ScriptAsyncResult::Return(value) =
                self.call_xr_root(cx, live_id!(toggle_depth_mesh))
            {
                if let Some(visible) = value.as_bool() {
                    self.sync_depth_toggle_label(cx, visible);
                }
            }
        }
    }
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
