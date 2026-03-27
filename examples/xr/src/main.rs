use makepad_widgets;

use makepad_widgets::*;
use makepad_xr::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let Block = Cube{
        size: vec3(0.160, 0.082, 0.075)
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

    let XrUiButton = mod.widgets.ButtonFlat{
        draw_bg +: {
            border_size: 0.0
            border_radius: 10.0
            pixel: fn() {
                let fill = self.color
                    .mix(self.color_focus, self.focus)
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_down, self.down)
                    .mix(self.color_disabled, self.disabled);
                return Pal.premul(fill)
            }
        }
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
                        for row in 0..20 {
                            for col in 0..8 {
                                let offset = if row % 2 == 0 {0.0} else {0.08}
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
                                    pos: vec3(-0.46 + col * 0.16 + offset, 0.028 + row * 0.084, -0.10)
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
                            size: vec3(0.78, 0.08, 0.92)
                            corner_radius: 0.02
                            roughness: 0.92
                            metallic: 0.0
                            color: #x212c39
                            pos: vec3(0.05, -0.06, -0.10)
                        }

                        Cube{
                            body: mod.widgets.XrBodyKind.Fixed
                            size: vec3(0.06, 0.60, 0.92)
                            corner_radius: 0.02
                            roughness: 0.84
                            metallic: 0.0
                            color: #x19232e
                            pos: vec3(-0.31, 0.20, -0.10)
                        }

                        Cube{
                            body: mod.widgets.XrBodyKind.Fixed
                            size: vec3(0.06, 0.60, 0.92)
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
                            pos: vec3(0.05, 0.20, -0.53)
                        }

                        Cube{
                            body: mod.widgets.XrBodyKind.Fixed
                            size: vec3(0.94, 0.24, 0.06)
                            corner_radius: 0.02
                            roughness: 0.86
                            metallic: 0.0
                            color: #x223140
                            pos: vec3(0.05, 0.02, 0.33)
                        }

                        for layer in 0..4 {
                            for row in 0..8 {
                                for col in 0..5 {
                                    let diffuse = #xa0a4aa
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
                                        diffuse: diffuse
                                        color: color
                                        pos: vec3(-0.118 + col * 0.084, 0.04 + layer * 0.082, -0.436 + row * 0.084)
                                    }
                                }
                            }
                        }
                    }
                }

                ico_shoot_scene := Shooter{
                    pos: vec3(0.0, -0.16, 0.0)
                    scale: vec3(0.62, 0.62, 0.62)
                    projectile_emit_rate_hz: 14.0
                    projectile_emit_speed_mps: 15.0
                    on_render: ||{
                        Platform{
                            pos: vec3(0.05, -0.06, -0.12)
                            size: vec3(1.52, 0.08, 0.52)
                        }

                        Cube{
                            body: mod.widgets.XrBodyKind.Fixed
                            size: vec3(1.10, 0.52, 0.06)
                            corner_radius: 0.02
                            roughness: 0.86
                            metallic: 0.0
                            color: #x1c2834
                            pos: vec3(0.05, 0.20, -0.42)
                        }

                        for index in 0..160 {
                            let diffuse = #xa0a4aa
                            let color = if index % 6 == 0 {
                                #xff6f59
                            } else if index % 6 == 1 {
                                #x46d39a
                            } else if index % 6 == 2 {
                                #x66a9ff
                            } else if index % 6 == 3 {
                                #xffc857
                            } else if index % 6 == 4 {
                                #xff8a4c
                            } else {
                                #xd58cff
                            }
                            IcoSphere{
                                projectile_pool: true
                                density: 0.75
                                friction: 0.48
                                restitution: 0.04
                                radius: 0.080
                                diffuse: diffuse
                                color: color
                                pos: vec3(-2.4, -6.0 - index * 0.004, 0.0)
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
            xr_people_debug := XrPeopleDebug{}

            control_strip := XrView{
                visible: false
                show_in_non_xr: true
                wrist_left: true
                logical_size: vec2(920, 468)
                pixel_scale: 0.000215
                dpi_factor: 2.0
                SolidView{
                    width: Fill
                    height: Fill
                    flow: Down
                    padding: 16
                    spacing: 12
                    draw_bg.color: #x162331ee

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
                            text: "Quest scenes: 160 faceted icos, blocks, fingertip shooter, tree, refraction."
                            draw_text.color: #xb8c8d8
                        }
                    }

                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8

                        test_scene_button := XrUiButton{
                            width: 88
                            text: "XR Test"
                            on_press: || ui.scene_select.test_scene()
                        }

                        ico_box_scene_button := XrUiButton{
                            width: 72
                            text: "Icos"
                            on_press: || ui.scene_select.ico_box_scene()
                        }

                        ico_shoot_scene_button := XrUiButton{
                            width: 84
                            text: "Shooter"
                            on_press: || ui.scene_select.ico_shoot_scene()
                        }

                        block_scene_button := XrUiButton{
                            width: 88
                            text: "Blocks"
                            on_press: || ui.scene_select.block_scene()
                        }

                        helmet_scene_button := XrUiButton{
                            width: 88
                            text: "Helmet"
                            on_press: || ui.scene_select.helmet_scene()
                        }

                        tree_scene_button := XrUiButton{
                            width: 88
                            text: "Tree"
                            on_press: || ui.scene_select.tree_scene()
                        }

                        refraction_scene_button := XrUiButton{
                            width: 104
                            text: "Refraction"
                            on_press: || ui.scene_select.refraction_scene()
                        }
                    }

                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{y: 0.5}

                        physics_scale_label := Label{
                            width: 104
                            text: "Physics Speed"
                            draw_text.color: #xe8f4ff
                        }

                        physics_scale_025_button := XrUiButton{
                            width: 58
                            text: "0.25"
                            on_press: || ui.root.set_physics_time_scale(0.25)
                        }

                        physics_scale_05_button := XrUiButton{
                            width: 58
                            text: "0.5"
                            on_press: || ui.root.set_physics_time_scale(0.5)
                        }

                        physics_scale_10_button := XrUiButton{
                            width: 58
                            text: "1.0"
                            on_press: || ui.root.set_physics_time_scale(1.0)
                        }
                    }

                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 12
                        align: Align{y: 0.5}

                        depth_toggle_button := XrUiButton{
                            width: 132
                            text: "Toggle Env Mesh"
                            on_press: || ui.root.set_depth(!ui.root.depth_mesh_visible())
                        }

                        query_hits_toggle_button := XrUiButton{
                            width: 148
                            text: "Toggle Query Hits"
                            on_press: || ui.root.set_depth_query_hits(!ui.root.depth_query_hits_visible())
                        }

                        scene_status := Label{
                            width: Fill
                            text: "Default scene: 160 faceted icosahedra with sphere colliders. Shooter mode fires pooled icos from the main index fingertip."
                            draw_text.color: #xe8f4ff
                        }
                    }

                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{y: 0.5}

                        depth_resolution_label := Label{
                            width: 104
                            text: "Depth Voxel"
                            draw_text.color: #xe8f4ff
                        }

                        depth_resolution_5_button := XrUiButton{
                            width: 64
                            text: "5 cm"
                            on_press: || ui.root.set_depth_voxel_size(0.05)
                        }

                        depth_resolution_10_button := XrUiButton{
                            width: 72
                            text: "10 cm"
                            on_press: || ui.root.set_depth_voxel_size(0.10)
                        }
                    }

                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{y: 0.5}

                        render_scale_label := Label{
                            width: 104
                            text: "Render Scale"
                            draw_text.color: #xe8f4ff
                        }

                        scale_08_button := XrUiButton{
                            width: 58
                            text: "0.8x"
                            on_press: || ui.root.set_render_scale(0.8)
                        }

                        scale_10_button := XrUiButton{
                            width: 58
                            text: "1.0x"
                            on_press: || ui.root.set_render_scale(1.0)
                        }

                        scale_12_button := XrUiButton{
                            width: 58
                            text: "1.2x"
                            on_press: || ui.root.set_render_scale(1.2)
                        }

                        scale_14_button := XrUiButton{
                            width: 58
                            text: "1.4x"
                            on_press: || ui.root.set_render_scale(1.4)
                        }

                        scale_15_button := XrUiButton{
                            width: 58
                            text: "1.5x"
                            on_press: || ui.root.set_render_scale(1.5)
                        }
                    }

                    View{
                        width: Fill
                        height: Fit
                        flow: Down
                        spacing: 8

                        SolidView{
                            width: Fill
                            height: 32
                            padding: Inset{left: 10 right: 10 top: 7 bottom: 7}
                            draw_bg.color: #x0d1824

                            peer_sync_status_field := Label{
                                width: Fill
                                text: "Peers: off"
                                draw_text.color: #xe8f4ff
                            }
                        }

                        SolidView{
                            width: Fill
                            height: 32
                            padding: Inset{left: 10 right: 10 top: 7 bottom: 7}
                            draw_bg.color: #x0d1824

                            network_status_field := Label{
                                width: Fill
                                text: "Network: off"
                                draw_text.color: #x9ec8e8
                            }
                        }

                        SolidView{
                            width: Fill
                            height: 32
                            padding: Inset{left: 10 right: 10 top: 7 bottom: 7}
                            draw_bg.color: #x0d1824

                            peer_scene_field := Label{
                                width: Fill
                                text: "PeerScene: off"
                                draw_text.color: #xffd29a
                            }
                        }

                        SolidView{
                            width: Fill
                            height: 32
                            padding: Inset{left: 10 right: 10 top: 7 bottom: 7}
                            draw_bg.color: #x0d1824

                            alignment_state_field := Label{
                                width: Fill
                                text: "AlignState: off"
                                draw_text.color: #xfff1ab
                            }
                        }

                        SolidView{
                            width: Fill
                            height: 32
                            padding: Inset{left: 10 right: 10 top: 7 bottom: 7}
                            draw_bg.color: #x0d1824

                            alignment_debug_field := Label{
                                width: Fill
                                text: "AlignDbg: off"
                                draw_text.color: #xffd29a
                            }
                        }

                        SolidView{
                            width: Fill
                            height: 32
                            padding: Inset{left: 10 right: 10 top: 7 bottom: 7}
                            draw_bg.color: #x0d1824

                            plane_scan_field := Label{
                                width: Fill
                                text: "PlaneScan: off"
                                draw_text.color: #x9af7c4
                            }
                        }

                        SolidView{
                            width: Fill
                            height: 32
                            padding: Inset{left: 10 right: 10 top: 7 bottom: 7}
                            draw_bg.color: #x0d1824

                            physics_geom_field := Label{
                                width: Fill
                                text: "Physics geometry: waiting for frame"
                                draw_text.color: #xe8f4ff
                            }
                        }

                        SolidView{
                            width: Fill
                            height: 32
                            padding: Inset{left: 10 right: 10 top: 7 bottom: 7}
                            draw_bg.color: #x0d1824

                            physics_timing_field := Label{
                                width: Fill
                                text: "Physics compute: waiting for frame"
                                draw_text.color: #xe8f4ff
                            }
                        }

                        SolidView{
                            width: Fill
                            height: 32
                            padding: Inset{left: 10 right: 10 top: 7 bottom: 7}
                            draw_bg.color: #x0d1824

                            frame_cpu_field := Label{
                                width: Fill
                                text: "CPU frame: waiting for frame"
                                draw_text.color: #xe8f4ff
                            }
                        }

                        SolidView{
                            width: Fill
                            height: 32
                            padding: Inset{left: 10 right: 10 top: 7 bottom: 7}
                            draw_bg.color: #x0d1824

                            xr_runtime_field := Label{
                                width: Fill
                                text: "XR render scale: waiting for XR session"
                                draw_text.color: #xe8f4ff
                            }
                        }
                    }
                }
            }

            wrist_toggle := XrView{
                visible: false
                mode: mod.widgets.XrViewMode.StuckToWrist
                wrist_left: true
                logical_size: vec2(112, 104)
                pixel_scale: 0.00030
                dpi_factor: 1.6
                depth_scale: 120.0
                SolidView{
                    width: Fill
                    height: Fill
                    flow: Down
                    padding: 6
                    spacing: 4
                    draw_bg.color: #x0f1b27ee
                    draw_bg.border_radius: 18.0

                    wrist_menu_button := XrUiButton{
                        width: Fill
                        height: 40
                        text: "Menu"
                        draw_bg.border_radius: 12.0
                        on_press: || ui.control_strip.toggle_visible_next_to_wrist()
                    }

                    wrist_reset_button := XrUiButton{
                        width: Fill
                        height: 40
                        text: "Reset"
                        draw_bg.border_radius: 12.0
                        on_press: || ui.root.reset_physics()
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
    network_started: bool,
    #[rust]
    last_physics_geometry_text: String,
    #[rust]
    last_physics_timing_text: String,
    #[rust]
    last_frame_cpu_text: String,
    #[rust]
    last_xr_runtime_text: String,
    #[rust]
    last_peer_sync_status_text: String,
    #[rust]
    last_network_status_text: String,
    #[rust]
    last_peer_scene_text: String,
    #[rust]
    last_alignment_state_text: String,
    #[rust]
    last_alignment_debug_text: String,
    #[rust]
    last_plane_scan_text: String,
}

impl App {
    fn ensure_network_started(&mut self, cx: &mut Cx) {
        if self.network_started {
            return;
        }
        if let Some(mut people_debug) = self
            .ui
            .widget(cx, ids!(xr_people_debug))
            .borrow_mut::<XrPeopleDebug>()
        {
            people_debug.set_enabled(cx, true);
            self.network_started = true;
        }
    }

    fn refresh_debug_fields(&mut self, cx: &mut Cx) {
        let (
            surface_count,
            vertex_count,
            triangle_count,
            compute_ms,
            physics_time_scale,
            step_dt_ms,
            frame_cpu_ms,
            frame_update_cpu_ms,
            frame_draw_cpu_ms,
        ) = if let Some(root) = self.ui.borrow::<XrRoot>() {
            (
                root.physics_depth_query_surface_count(),
                root.physics_depth_query_vertex_count(),
                root.physics_depth_query_triangle_count(),
                root.physics_compute_ms(),
                root.physics_time_scale(),
                root.physics_step_dt_ms(),
                root.frame_cpu_ms(),
                root.frame_update_cpu_ms(),
                root.frame_draw_cpu_ms(),
            )
        } else {
            return;
        };
        let peer_sync_status_text = self
            .ui
            .widget(cx, ids!(xr_people_debug))
            .borrow::<XrPeopleDebug>()
            .map(|people_debug| people_debug.status_text().to_string())
            .unwrap_or_else(|| "Peers: unavailable".to_string());
        let network_status_text = self
            .ui
            .widget(cx, ids!(xr_people_debug))
            .borrow::<XrPeopleDebug>()
            .map(|people_debug| people_debug.network_status_text().to_string())
            .unwrap_or_else(|| "Network: unavailable".to_string());
        let alignment_debug_text = self
            .ui
            .widget(cx, ids!(xr_people_debug))
            .borrow::<XrPeopleDebug>()
            .map(|people_debug| people_debug.alignment_debug_text().to_string())
            .unwrap_or_else(|| "AlignDbg: unavailable".to_string());
        let alignment_state_text = self
            .ui
            .widget(cx, ids!(xr_people_debug))
            .borrow::<XrPeopleDebug>()
            .map(|people_debug| people_debug.alignment_state_text().to_string())
            .unwrap_or_else(|| "AlignState: unavailable".to_string());
        let peer_scene_text = self
            .ui
            .widget(cx, ids!(xr_people_debug))
            .borrow::<XrPeopleDebug>()
            .map(|people_debug| people_debug.peer_scene_text().to_string())
            .unwrap_or_else(|| "PeerScene: unavailable".to_string());
        let plane_scan_text = self
            .ui
            .widget(cx, ids!(xr_people_debug))
            .borrow::<XrPeopleDebug>()
            .map(|people_debug| people_debug.plane_scan_text().to_string())
            .unwrap_or_else(|| "PlaneScan: unavailable".to_string());

        if self.last_peer_sync_status_text != peer_sync_status_text {
            self.ui
                .widget(cx, ids!(peer_sync_status_field))
                .set_text(cx, &peer_sync_status_text);
            self.last_peer_sync_status_text = peer_sync_status_text;
        }
        if self.last_network_status_text != network_status_text {
            self.ui
                .widget(cx, ids!(network_status_field))
                .set_text(cx, &network_status_text);
            self.last_network_status_text = network_status_text;
        }
        if self.last_alignment_debug_text != alignment_debug_text {
            self.ui
                .widget(cx, ids!(alignment_debug_field))
                .set_text(cx, &alignment_debug_text);
            self.last_alignment_debug_text = alignment_debug_text;
        }
        if self.last_alignment_state_text != alignment_state_text {
            self.ui
                .widget(cx, ids!(alignment_state_field))
                .set_text(cx, &alignment_state_text);
            self.last_alignment_state_text = alignment_state_text;
        }
        if self.last_peer_scene_text != peer_scene_text {
            self.ui
                .widget(cx, ids!(peer_scene_field))
                .set_text(cx, &peer_scene_text);
            self.last_peer_scene_text = peer_scene_text;
        }
        if self.last_plane_scan_text != plane_scan_text {
            self.ui
                .widget(cx, ids!(plane_scan_field))
                .set_text(cx, &plane_scan_text);
            self.last_plane_scan_text = plane_scan_text;
        }

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

        let timing_text = if step_dt_ms > 0.0 {
            format!(
                "Physics compute: {:.2} ms | tick {:.2} ms ({:.0} Hz) | sim {:.2}x",
                compute_ms,
                step_dt_ms,
                1000.0 / step_dt_ms,
                physics_time_scale
            )
        } else {
            format!(
                "Physics compute: {:.2} ms | sim {:.2}x",
                compute_ms, physics_time_scale
            )
        };
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

        let xr_runtime_text = match (
            cx.xr_render_scale(),
            cx.xr_display_refresh_rate_hz(),
            cx.xr_effective_frame_rate_hz(),
            cx.xr_gpu_frame_time_ms(),
        ) {
            (Some(scale), Some(refresh_hz), Some(effective_hz), Some(gpu_ms)) => format!(
                "Depth: {:.0} cm | XR scale: {:.2} | refresh {:.1} Hz | cadence {:.1} Hz | GPU {:.2} ms",
                cx.xr_depth_mesh().voxel_size_meters() * 100.0,
                scale,
                refresh_hz,
                effective_hz,
                gpu_ms
            ),
            (Some(scale), Some(refresh_hz), Some(effective_hz), None) => format!(
                "Depth: {:.0} cm | XR scale: {:.2} | refresh {:.1} Hz | cadence {:.1} Hz | GPU waiting",
                cx.xr_depth_mesh().voxel_size_meters() * 100.0,
                scale,
                refresh_hz,
                effective_hz
            ),
            (Some(scale), Some(refresh_hz), None, Some(gpu_ms)) => format!(
                "Depth: {:.0} cm | XR scale: {:.2} | refresh {:.1} Hz | cadence waiting | GPU {:.2} ms",
                cx.xr_depth_mesh().voxel_size_meters() * 100.0,
                scale,
                refresh_hz,
                gpu_ms
            ),
            (Some(scale), Some(refresh_hz), None, None) => format!(
                "Depth: {:.0} cm | XR scale: {:.2} | refresh {:.1} Hz | cadence waiting | GPU waiting",
                cx.xr_depth_mesh().voxel_size_meters() * 100.0,
                scale,
                refresh_hz
            ),
            (Some(scale), None, _, Some(gpu_ms)) => {
                format!(
                    "Depth: {:.0} cm | XR scale: {:.2} | refresh waiting | GPU {:.2} ms",
                    cx.xr_depth_mesh().voxel_size_meters() * 100.0,
                    scale,
                    gpu_ms
                )
            }
            (Some(scale), None, _, None) => {
                format!(
                    "Depth: {:.0} cm | XR scale: {:.2} | refresh waiting | GPU waiting",
                    cx.xr_depth_mesh().voxel_size_meters() * 100.0,
                    scale
                )
            }
            (None, _, _, _) => format!(
                "Depth: {:.0} cm | XR render scale: not active",
                cx.xr_depth_mesh().voxel_size_meters() * 100.0
            ),
        };
        if self.last_xr_runtime_text != xr_runtime_text {
            self.ui
                .widget(cx, ids!(xr_runtime_field))
                .set_text(cx, &xr_runtime_text);
            self.last_xr_runtime_text = xr_runtime_text;
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
        if matches!(event, Event::Startup) {
            self.ensure_network_started(cx);
        }
        self.refresh_debug_fields(cx);
    }
}
