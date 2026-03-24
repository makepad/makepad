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

    let TestPedestal = Cube{
        body: mod.widgets.XrBodyKind.Fixed
        size: vec3(0.28, 0.18, 0.28)
        corner_radius: 0.026
        roughness: 0.18
        metallic: 0.04
    }

    startup() do #(App::script_component(vm)){
        ui:  XrRoot{
            window.inner_size: vec2(1400, 900)
            pass.clear_color: #x0b1118
            camera.fov_y: 52.0
            camera.distance: 2.8
            env.gravity: 9.8
            env.env_cube: true
            env.depth_mesh: false

            scene_select := XrSelect{
                pos: vec3(0.0, -0.02, -0.62)
                active_child: @block_scene

                test_scene := XrNode{
                    on_render: ||{
                        Cube{
                            body: mod.widgets.XrBodyKind.Fixed
                            size: vec3(1.65, 0.08, 1.18)
                            corner_radius: 0.04
                            roughness: 0.92
                            metallic: 0.0
                            color: #x243444
                            pos: vec3(0.42, -0.22, -0.72)
                        }

                        Cube{
                            body: mod.widgets.XrBodyKind.Fixed
                            size: vec3(0.18, 0.72, 1.16)
                            corner_radius: 0.04
                            roughness: 0.88
                            metallic: 0.0
                            color: #x1c2733
                            pos: vec3(1.20, 0.10, -0.72)
                        }

                        Cube{
                            body: mod.widgets.XrBodyKind.Fixed
                            size: vec3(1.62, 0.72, 0.18)
                            corner_radius: 0.04
                            roughness: 0.88
                            metallic: 0.0
                            color: #x1a2430
                            pos: vec3(0.42, 0.10, -1.22)
                        }

                        TestPedestal{
                            pos: vec3(0.05, -0.05, -0.76)
                            color: #xff6a4d
                        }

                        TestPedestal{
                            pos: vec3(0.42, 0.02, -0.76)
                            color: #x58d68d
                            size: vec3(0.24, 0.32, 0.24)
                        }

                        TestPedestal{
                            pos: vec3(0.78, -0.01, -0.76)
                            color: #x68a8ff
                            size: vec3(0.24, 0.26, 0.24)
                        }

                        Cube{
                            body: mod.widgets.XrBodyKind.Fixed
                            size: vec3(0.24, 0.24, 0.24)
                            corner_radius: 0.024
                            roughness: 0.12
                            metallic: 0.02
                            color: #xffff7a
                            pos: vec3(0.42, 0.34, -0.76)
                        }

                        Cube{
                            body: mod.widgets.XrBodyKind.Fixed
                            size: vec3(0.16, 0.82, 0.16)
                            corner_radius: 0.03
                            roughness: 0.22
                            metallic: 0.04
                            color: #xff8a54
                            pos: vec3(0.42, 0.34, -1.02)
                        }
                    }
                }

                block_scene := XrNode{
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

                helmet_scene := XrNode{
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

                tree_scene := XrNode{
                    on_render: ||{
                        Platform{pos: vec3(0.05, -0.06, -0.10)}
                        fractal_tree := FractalTree{
                            body: mod.widgets.XrBodyKind.Fixed
                            physics_size: vec3(0.34, 0.92, 0.34)
                            pos: vec3(0.05, -0.02, -0.10)
                            child_scale: 0.57735026
                            length_scale_0: 0.60
                            length_scale_1: 1.78
                            length_scale_2: 1.88
                            length_scale_3: 0.97
                            length_scale_4: 1.03
                            length_scale_rest: 1.08
                            branch_split_angle: 0.70
                            branch_yaw_step: 2.0943952
                            branch_yaw_phase_step: 1.0471976
                        }
                    }
                }

                refraction_scene := XrNode{
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

            control_strip := XrView{
                pos: vec3(0.0, 0.72, -0.88)
                logical_size: vec2(760, 156)
                pixel_scale: 0.00078
                dpi_factor: 2.0
                RoundedView{
                    width: Fill
                    height: Fill
                    flow: Down
                    padding: 16
                    spacing: 12
                    draw_bg.color: #x162331ee
                    draw_bg.border_radius: 18.0

                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 12
                        align: Align{y: 0.5}

                        title := Label{
                            text: "XR Scene Picker"
                            draw_text.color: #xeff7ff
                            draw_text.text_style.font_size: 18.0
                        }

                        detail := Label{
                            width: Fill
                            text: "Desktop XR preview with script-driven scene switching."
                            draw_text.color: #xb8c8d8
                        }
                    }

                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8

                        test_scene_button := Button{
                            width: 106
                            text: "XR Test"
                            on_press: || ui.scene_select.test_scene()
                        }

                        block_scene_button := Button{
                            width: 106
                            text: "Blocks"
                            on_press: || ui.scene_select.block_scene()
                        }

                        helmet_scene_button := Button{
                            width: 106
                            text: "Helmet"
                            on_press: || ui.scene_select.helmet_scene()
                        }

                        tree_scene_button := Button{
                            width: 106
                            text: "Tree"
                            on_press: || ui.scene_select.tree_scene()
                        }

                        refraction_scene_button := Button{
                            width: 120
                            text: "Refraction"
                            on_press: || ui.scene_select.refraction_scene()
                        }
                    }

                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 12
                        align: Align{y: 0.5}

                        depth_toggle_button := Button{
                            width: 170
                            text: "Toggle Depth Mesh"
                            on_press: || ui.depth_toggle()
                        }

                        scene_status := Label{
                            width: Fill
                            text: "Use the buttons above to switch XR scenes."
                            draw_text.color: #xe8f4ff
                        }
                    }
                }
            }

            xr_permissions := mod.widgets.XrPermissionsFlow{}
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        makepad_xr::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
