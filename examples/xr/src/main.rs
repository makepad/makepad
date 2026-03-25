use makepad_widgets;

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
            camera.distance: 1.8
            env.gravity: 9.8
            env.env_cube: true
            env.depth_mesh: false

            scene_select := XrSelect{
                pos: vec3(0.0, -0.02, -0.62)
                scale: vec3(0.5, 0.5, 0.5)
                active_child: @ico_box_scene

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
                    pos: vec3(0.0, -0.16, 0.0)
                    scale: vec3(0.62, 0.62, 0.62)
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

                ico_box_scene := XrNode{
                    pos: vec3(0.0, -0.18, 0.0)
                    scale: vec3(0.62, 0.62, 0.62)
                    on_render: ||{
                        Cube{
                            body: mod.widgets.XrBodyKind.Fixed
                            size: vec3(0.78, 0.08, 0.58)
                            corner_radius: 0.02
                            roughness: 0.92
                            metallic: 0.0
                            color: #x212c39
                            pos: vec3(0.05, -0.06, -0.10)
                        }

                        Cube{
                            body: mod.widgets.XrBodyKind.Fixed
                            size: vec3(0.06, 0.60, 0.58)
                            corner_radius: 0.02
                            roughness: 0.84
                            metallic: 0.0
                            color: #x19232e
                            pos: vec3(-0.31, 0.20, -0.10)
                        }

                        Cube{
                            body: mod.widgets.XrBodyKind.Fixed
                            size: vec3(0.06, 0.60, 0.58)
                            corner_radius: 0.02
                            roughness: 0.84
                            metallic: 0.0
                            color: #x19232e
                            pos: vec3(0.41, 0.20, -0.10)
                        }

                        Cube{
                            body: mod.widgets.XrBodyKind.Fixed
                            size: vec3(0.78, 0.60, 0.06)
                            corner_radius: 0.02
                            roughness: 0.84
                            metallic: 0.0
                            color: #x17202a
                            pos: vec3(0.05, 0.20, -0.36)
                        }

                        Cube{
                            body: mod.widgets.XrBodyKind.Fixed
                            size: vec3(0.94, 0.24, 0.06)
                            corner_radius: 0.02
                            roughness: 0.86
                            metallic: 0.0
                            color: #x223140
                            pos: vec3(0.05, 0.02, 0.16)
                        }

                        for layer in 0..4 {
                            for row in 0..4 {
                                for col in 0..5 {
                                    let color = if (col + row * 2 + layer * 3) % 6 == 0 {
                                        #xff6f59
                                    } else if (col + row * 2 + layer * 3) % 6 == 1 {
                                        #x46d39a
                                    } else if (col + row * 2 + layer * 3) % 6 == 2 {
                                        #x66a9ff
                                    } else if (col + row * 2 + layer * 3) % 6 == 3 {
                                        #xffc857
                                    } else if (col + row * 2 + layer * 3) % 6 == 4 {
                                        #xff8a4c
                                    } else {
                                        #xd58cff
                                    }
                                    IcoSphere{
                                        density: 0.75
                                        friction: 0.48
                                        restitution: 0.03
                                        radius: 0.040
                                        color: color
                                        pos: vec3(-0.118 + col * 0.084, 0.04 + layer * 0.082, -0.268 + row * 0.084)
                                    }
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
                            scale: vec3(0.72, 0.72, 0.72)
                            child_scale: 0.57735026
                            length_scale_0: 0.60
                            length_scale_1: 1.78
                            length_scale_2: 1.88
                            length_scale_3: 0.97
                            length_scale_4: 1.03
                            length_scale_rest: 1.08
                            branch_split_angle: 0.58
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
                pos: vec3(0.0, 0.46, -0.84)
                logical_size: vec2(780, 268)
                pixel_scale: 0.00074
                dpi_factor: 2.0
                RoundedView{
                    width: Fill
                    height: Fill
                    flow: Down
                    padding: 16
                    spacing: 12
                    draw_bg.color: #x162331ee
                    draw_bg.border_radius: 16.0

                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 10
                        align: Align{y: 0.5}

                        title := Label{
                            text: "XR Scene Picker"
                            draw_text.color: #xeff7ff
                            draw_text.text_style.font_size: 18.0
                        }

                        detail := Label{
                            width: Fill
                            text: "Quest stress scene: 80 faceted icosahedron masses."
                            draw_text.color: #xb8c8d8
                        }
                    }

                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8

                        test_scene_button := Button{
                            width: 88
                            text: "XR Test"
                            on_press: || ui.scene_select.test_scene()
                        }

                        ico_box_scene_button := Button{
                            width: 72
                            text: "Icos"
                            on_press: || ui.scene_select.ico_box_scene()
                        }

                        block_scene_button := Button{
                            width: 88
                            text: "Blocks"
                            on_press: || ui.scene_select.block_scene()
                        }

                        helmet_scene_button := Button{
                            width: 88
                            text: "Helmet"
                            on_press: || ui.scene_select.helmet_scene()
                        }

                        tree_scene_button := Button{
                            width: 88
                            text: "Tree"
                            on_press: || ui.scene_select.tree_scene()
                        }

                        refraction_scene_button := Button{
                            width: 104
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
                            width: 132
                            text: "Toggle Env Mesh"
                            on_press: || ui.root.set_depth(!ui.root.depth_mesh_visible())
                        }

                        query_hits_toggle_button := Button{
                            width: 148
                            text: "Toggle Query Hits"
                            on_press: || ui.root.set_depth_query_hits(!ui.root.depth_query_hits_visible())
                        }

                        scene_status := Label{
                            width: Fill
                            text: "Default scene: 80 faceted icosahedra with sphere colliders."
                            draw_text.color: #xe8f4ff
                        }
                    }

                    View{
                        width: Fill
                        height: Fit
                        flow: Down
                        spacing: 8

                        physics_geom_field := TextInput{
                            width: Fill
                            height: 32
                            is_read_only: true
                            empty_text: "Physics geometry: waiting for frame"
                        }

                        physics_timing_field := TextInput{
                            width: Fill
                            height: 32
                            is_read_only: true
                            empty_text: "Physics compute: waiting for frame"
                        }

                        frame_cpu_field := TextInput{
                            width: Fill
                            height: 32
                            is_read_only: true
                            empty_text: "CPU frame: waiting for frame"
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
    #[rust]
    last_physics_geometry_text: String,
    #[rust]
    last_physics_timing_text: String,
    #[rust]
    last_frame_cpu_text: String,
}

impl App {
    fn refresh_debug_fields(&mut self, cx: &mut Cx) {
        let (
            surface_count,
            vertex_count,
            triangle_count,
            compute_ms,
            frame_cpu_ms,
            frame_update_cpu_ms,
            frame_draw_cpu_ms,
        ) =
            if let Some(root) = self.ui.borrow::<XrRoot>() {
                (
                    root.physics_depth_query_surface_count(),
                    root.physics_depth_query_vertex_count(),
                    root.physics_depth_query_triangle_count(),
                    root.physics_compute_ms(),
                    root.frame_cpu_ms(),
                    root.frame_update_cpu_ms(),
                    root.frame_draw_cpu_ms(),
                )
            } else {
                return;
            };

        let geometry_text = format!(
            "Physics geometry: {} planes, {} vertices, {} triangles",
            surface_count, vertex_count, triangle_count
        );
        if self.last_physics_geometry_text != geometry_text {
            self.ui
                .widget(cx, ids!(physics_geom_field))
                .set_text(cx, &geometry_text);
            self.last_physics_geometry_text = geometry_text;
        }

        let timing_text = format!("Physics compute: {:.2} ms", compute_ms);
        if self.last_physics_timing_text != timing_text {
            self.ui
                .widget(cx, ids!(physics_timing_field))
                .set_text(cx, &timing_text);
            self.last_physics_timing_text = timing_text;
        }

        let frame_cpu_text = format!(
            "CPU frame: {:.2} ms total | update {:.2} ms | draw {:.2} ms",
            frame_cpu_ms, frame_update_cpu_ms, frame_draw_cpu_ms
        );
        if self.last_frame_cpu_text != frame_cpu_text {
            self.ui
                .widget(cx, ids!(frame_cpu_field))
                .set_text(cx, &frame_cpu_text);
            self.last_frame_cpu_text = frame_cpu_text;
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
        self.ui.handle_event(cx, event, &mut Scope::empty());
        self.refresh_debug_fields(cx);
    }
}
