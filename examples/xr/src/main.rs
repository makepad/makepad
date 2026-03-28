use makepad_widgets;

use makepad_widgets::makepad_draw::DrawVector;
use makepad_widgets::makepad_platform::{
    TextureFormat, TextureUpdated, XrDepthAlignHeightMap, XrDepthAlignSlicePreview,
};
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

    let DrawHeightMap = set_type_default() do #(DrawHeightMap::script_shader(vm)){
        ..mod.draw.DrawQuad
        height_texture: texture_2d(float)
        alpha: 0.96
        uv_min: vec2(0.0, 0.0)
        uv_max: vec2(1.0, 1.0)
        wall_band_start: 0.72

        pixel: fn() {
            let uv = self.uv_min + (self.uv_max - self.uv_min) * self.pos
            let sample = self.height_texture.sample(uv).x
            if sample <= 0.00001 {
                return #0000
            }
            let wall_mix = clamp(
                (sample - self.wall_band_start) / max(1.0 - self.wall_band_start, 0.0001),
                0.0,
                1.0
            )
            let lifted = pow(sample, mix(1.28, 0.72, wall_mix))
            let base = mix(
                vec3(0.05, 0.13, 0.19),
                vec3(0.16, 0.60, 0.66),
                clamp(lifted * 1.35, 0.0, 1.0)
            )
            let bright = mix(base, vec3(0.98, 0.92, 0.74), clamp(pow(lifted, 1.7), 0.0, 1.0))
            let color = mix(bright, vec3(1.0, 0.66, 0.22), wall_mix * 0.82)
            return Pal.premul(vec4(color, self.alpha))
        }
    }

    let AlignmentSlicePreviewBase = #(AlignmentSlicePreview::register_widget(vm))
    let AlignmentSlicePreview = set_type_default() do AlignmentSlicePreviewBase{
        width: Fill
        height: Fill
        draw_bg +: {
            color: #x09141d
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
            xr_people_debug := XrPeopleDebug{
                auto_alignment_enabled: true
            }

            control_strip := XrView{
                visible: false
                show_in_non_xr: true
                wrist_left: true
                logical_size: vec2(1220, 700)
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

                        depth_resolution_3_button := XrUiButton{
                            width: 64
                            text: "3 cm"
                            on_press: || ui.root.set_depth_voxel_size(0.03)
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
                        height: Fill
                        flow: Right
                        spacing: 12

                        View{
                            width: 430
                            height: Fill
                            flow: Down
                            spacing: 8

                            SolidView{
                                width: Fill
                                height: 32
                                padding: Inset{left: 10 right: 10 top: 7 bottom: 7}
                                draw_bg.color: #x0d1824

                                peer_sync_status_field := Label{
                                    width: Fill
                                    text: "AlignSync: off"
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

                        SolidView{
                            width: Fill
                            height: Fill
                            flow: Down
                            spacing: 8
                            padding: Inset{left: 10 right: 10 top: 10 bottom: 10}
                            draw_bg.color: #x0d1824

                            Label{
                                width: Fill
                                text: "Projected Height Map"
                                draw_text.color: #x9af7c4
                            }

                            alignment_slice_preview := AlignmentSlicePreview{}
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
#[repr(C)]
pub struct DrawHeightMap {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    alpha: f32,
    #[live]
    uv_min: Vec2f,
    #[live]
    uv_max: Vec2f,
    #[live]
    wall_band_start: f32,
}

#[derive(Script, ScriptHook, Widget)]
pub struct AlignmentSlicePreview {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[redraw]
    #[live]
    draw_map: DrawHeightMap,
    #[redraw]
    #[live]
    draw_vector: DrawVector,
    #[rust]
    area: Area,
    #[rust]
    height_texture: Option<Texture>,
    #[rust]
    remote_height_texture: Option<Texture>,
    #[rust]
    local_preview: Option<XrDepthAlignSlicePreview>,
    #[rust]
    remote_height_map: Option<XrDepthAlignHeightMap>,
}

impl AlignmentSlicePreview {
    const PAD: f32 = 14.0;
    const PANEL_GAP: f32 = 14.0;
    const GRID_DIVISIONS: usize = 4;
    const CUTOUT_STEPS: usize = 40;
    const MAP_ALPHA: f32 = 0.96;

    fn preview_square(rect: Rect) -> (f32, f32, f32) {
        let rect_x = rect.pos.x as f32;
        let rect_y = rect.pos.y as f32;
        let rect_w = rect.size.x as f32;
        let rect_h = rect.size.y as f32;
        let inner = (rect_w.min(rect_h) - Self::PAD * 2.0).max(1.0);
        let ox = rect_x + (rect_w - inner) * 0.5;
        let oy = rect_y + (rect_h - inner) * 0.5;
        (ox, oy, inner)
    }

    fn preview_panel_square(rect: Rect, panel_index: usize, panel_count: usize) -> (f32, f32, f32) {
        if panel_count <= 1 {
            return Self::preview_square(rect);
        }
        let rect_x = rect.pos.x as f32;
        let rect_y = rect.pos.y as f32;
        let rect_w = rect.size.x as f32;
        let rect_h = rect.size.y as f32;
        let total_gap = Self::PANEL_GAP * (panel_count.saturating_sub(1)) as f32;
        let inner =
            (((rect_w - total_gap) / panel_count.max(1) as f32).min(rect_h) - Self::PAD * 2.0)
                .max(1.0);
        let total_w = inner * panel_count as f32 + total_gap;
        let start_x = rect_x + (rect_w - total_w) * 0.5;
        let ox = start_x + panel_index as f32 * (inner + Self::PANEL_GAP);
        let oy = rect_y + (rect_h - inner) * 0.5;
        (ox, oy, inner)
    }

    fn set_remote_height_map(
        &mut self,
        cx: &mut Cx,
        remote_height_map: Option<XrDepthAlignHeightMap>,
    ) {
        if self.remote_height_map == remote_height_map {
            return;
        }
        self.remote_height_map = remote_height_map;
        if self.remote_height_map.is_none() {
            self.remote_height_texture = None;
        }
        self.area.redraw(cx);
    }

    fn set_local_preview(
        &mut self,
        cx: &mut Cx,
        local_preview: Option<XrDepthAlignSlicePreview>,
    ) {
        if self.local_preview == local_preview {
            return;
        }
        self.local_preview = local_preview;
        if self.local_preview.is_none() {
            self.height_texture = None;
        }
        self.area.redraw(cx);
    }

    fn height_map_extent_x(height_map: &XrDepthAlignHeightMap) -> f32 {
        height_map.extent_x_meters()
    }

    fn height_map_extent_z(height_map: &XrDepthAlignHeightMap) -> f32 {
        height_map.extent_z_meters()
    }

    fn preview_scale(height_map: &XrDepthAlignHeightMap, inner: f32) -> f32 {
        inner
            / Self::height_map_extent_x(height_map)
                .max(Self::height_map_extent_z(height_map))
                .max(1.0e-5)
    }

    fn height_map_draw_rect(
        height_map: &XrDepthAlignHeightMap,
        ox: f32,
        oy: f32,
        inner: f32,
    ) -> Rect {
        let extent_x = Self::height_map_extent_x(height_map);
        let extent_z = Self::height_map_extent_z(height_map);
        if extent_x <= 1.0e-5 || extent_z <= 1.0e-5 {
            return Rect {
                pos: dvec2(ox as f64, oy as f64),
                size: dvec2(inner.max(1.0) as f64, inner.max(1.0) as f64),
            };
        }
        let scale = Self::preview_scale(height_map, inner);
        let draw_w = extent_x * scale;
        let draw_h = extent_z * scale;
        Rect {
            pos: dvec2(
                (ox + (inner - draw_w) * 0.5) as f64,
                (oy + (inner - draw_h) * 0.5) as f64,
            ),
            size: dvec2(draw_w.max(1.0) as f64, draw_h.max(1.0) as f64),
        }
    }

    fn height_map_preview_point(
        height_map: &XrDepthAlignHeightMap,
        ox: f32,
        oy: f32,
        inner: f32,
        point: Vec2f,
    ) -> (f32, f32) {
        let rect = Self::height_map_draw_rect(height_map, ox, oy, inner);
        let extent_x = Self::height_map_extent_x(height_map).max(1.0e-5);
        let extent_z = Self::height_map_extent_z(height_map).max(1.0e-5);
        let nx = ((point.x - height_map.origin_x) / extent_x).clamp(0.0, 1.0);
        let nz = ((point.y - height_map.origin_z) / extent_z).clamp(0.0, 1.0);
        (
            rect.pos.x as f32 + nx * rect.size.x as f32,
            rect.pos.y as f32 + nz * rect.size.y as f32,
        )
    }

    fn map_preview_point(
        preview: &XrDepthAlignSlicePreview,
        ox: f32,
        oy: f32,
        inner: f32,
        point: Vec2f,
    ) -> (f32, f32) {
        // The height-map texture is uploaded row-major with z increasing downward on screen,
        // so the vector overlay must use the same orientation to line up with the image.
        Self::height_map_preview_point(&preview.height_map, ox, oy, inner, point)
    }

    fn draw_grid(
        &mut self,
        height_map: &XrDepthAlignHeightMap,
        ox: f32,
        oy: f32,
        inner: f32,
    ) {
        let rect = Self::height_map_draw_rect(height_map, ox, oy, inner);
        let ox = rect.pos.x as f32;
        let oy = rect.pos.y as f32;
        let inner_w = rect.size.x as f32;
        let inner_h = rect.size.y as f32;
        self.draw_vector.set_color_hex(0x163042, 1.0);
        for step in 0..=Self::GRID_DIVISIONS {
            let t = step as f32 / Self::GRID_DIVISIONS as f32;
            let px = ox + inner_w * t;
            let py = oy + inner_h * t;
            self.draw_vector.move_to(px, oy);
            self.draw_vector.line_to(px, oy + inner_h);
            self.draw_vector.move_to(ox, py);
            self.draw_vector.line_to(ox + inner_w, py);
        }
        self.draw_vector.stroke(1.0);

        self.draw_vector.set_color_hex(0x42657c, 1.0);
        self.draw_vector.rect(ox, oy, inner_w, inner_h);
        self.draw_vector.stroke(1.5);
    }

    fn preview_height_to_u8(value: u16) -> u8 {
        if value == 0 {
            0
        } else {
            let normalized = (value - 1) as f32 / 65534.0;
            1 + (normalized * 254.0).round() as u8
        }
    }

    fn ensure_height_texture(
        texture_slot: &mut Option<Texture>,
        cx: &mut Cx,
        height_map: &XrDepthAlignHeightMap,
    ) {
        let map_width = height_map.size_x as usize;
        let map_height = height_map.size_z as usize;
        if map_width == 0
            || map_height == 0
            || height_map.height_u16.len() != map_width * map_height
        {
            *texture_slot = None;
            return;
        }

        let needs_recreate = texture_slot.as_ref().is_none_or(|texture| {
            !matches!(
                texture.get_format(cx),
                TextureFormat::VecRu8 { width, height, .. }
                    if *width == map_width && *height == map_height
            )
        });

        let pixels = height_map
            .height_u16
            .iter()
            .map(|value| Self::preview_height_to_u8(*value))
            .collect::<Vec<_>>();

        if needs_recreate {
            *texture_slot = Some(Texture::new_with_format(
                cx,
                TextureFormat::VecRu8 {
                    width: map_width,
                    height: map_height,
                    data: Some(pixels),
                    unpack_row_length: None,
                    updated: TextureUpdated::Full,
                },
            ));
            return;
        }

        if let Some(texture) = texture_slot.as_ref() {
            let mut data = texture.take_vec_u8(cx);
            if data.len() != pixels.len() {
                data.resize(pixels.len(), 0);
            }
            data.copy_from_slice(&pixels);
            texture.put_back_vec_u8(cx, data, None);
        }
    }

    fn draw_origin_cross(
        &mut self,
        preview: &XrDepthAlignSlicePreview,
        ox: f32,
        oy: f32,
        inner: f32,
    ) {
        if preview.height_map.origin_x > 0.0
            || preview.height_map.origin_z > 0.0
            || preview.height_map.origin_x + Self::height_map_extent_x(&preview.height_map) < 0.0
            || preview.height_map.origin_z + Self::height_map_extent_z(&preview.height_map) < 0.0
        {
            return;
        }
        let (cx, cy) = Self::map_preview_point(preview, ox, oy, inner, vec2f(0.0, 0.0));
        self.draw_vector.set_color_hex(0xffcf6a, 1.0);
        self.draw_vector.move_to(cx - 6.0, cy);
        self.draw_vector.line_to(cx + 6.0, cy);
        self.draw_vector.move_to(cx, cy - 6.0);
        self.draw_vector.line_to(cx, cy + 6.0);
        self.draw_vector.stroke(1.2);
    }

    fn draw_cutout_ring(
        &mut self,
        preview: &XrDepthAlignSlicePreview,
        ox: f32,
        oy: f32,
        inner: f32,
    ) {
        let Some(center) = preview.cutout_center else {
            return;
        };
        let (cx, cy) = Self::map_preview_point(preview, ox, oy, inner, center);
        let radius = preview.cutout_radius_meters * Self::preview_scale(&preview.height_map, inner);
        self.draw_vector.set_color_hex(0xff8d62, 1.0);
        for step in 0..=Self::CUTOUT_STEPS {
            let angle = step as f32 / Self::CUTOUT_STEPS as f32 * std::f32::consts::TAU;
            let px = cx + angle.cos() * radius;
            let py = cy + angle.sin() * radius;
            if step == 0 {
                self.draw_vector.move_to(px, py);
            } else {
                self.draw_vector.line_to(px, py);
            }
        }
        self.draw_vector.stroke(1.2);
    }

    fn draw_cutout_heading(
        &mut self,
        preview: &XrDepthAlignSlicePreview,
        ox: f32,
        oy: f32,
        inner: f32,
    ) {
        let (Some(center), Some(forward)) = (preview.cutout_center, preview.cutout_forward) else {
            return;
        };
        let forward_len = forward.length();
        if forward_len <= 1.0e-5 {
            return;
        }
        let (cx, cy) = Self::map_preview_point(preview, ox, oy, inner, center);
        let scale = Self::preview_scale(&preview.height_map, inner);
        let dir = vec2f(forward.x, forward.y) * forward_len.recip();
        let start = vec2f(cx, cy) + dir * (preview.cutout_radius_meters * scale + 4.0);
        let tip = start + dir * 28.0;
        let side = vec2f(-dir.y, dir.x);
        let left = tip - dir * 8.0 + side * 5.0;
        let right = tip - dir * 8.0 - side * 5.0;
        self.draw_vector.set_color_hex(0xffe07a, 1.0);
        self.draw_vector.move_to(start.x, start.y);
        self.draw_vector.line_to(tip.x, tip.y);
        self.draw_vector.move_to(left.x, left.y);
        self.draw_vector.line_to(tip.x, tip.y);
        self.draw_vector.line_to(right.x, right.y);
        self.draw_vector.stroke(1.8);
    }

    fn draw_cutout_ring_in_local_frame(
        &mut self,
        preview: &XrDepthAlignSlicePreview,
        center: Vec2f,
        radius_meters: f32,
        ox: f32,
        oy: f32,
        inner: f32,
        color_hex: u32,
    ) {
        let (cx, cy) = Self::map_preview_point(preview, ox, oy, inner, center);
        let radius = radius_meters * Self::preview_scale(&preview.height_map, inner);
        self.draw_vector.set_color_hex(color_hex, 1.0);
        for step in 0..=Self::CUTOUT_STEPS {
            let angle = step as f32 / Self::CUTOUT_STEPS as f32 * std::f32::consts::TAU;
            let px = cx + angle.cos() * radius;
            let py = cy + angle.sin() * radius;
            if step == 0 {
                self.draw_vector.move_to(px, py);
            } else {
                self.draw_vector.line_to(px, py);
            }
        }
        self.draw_vector.stroke(1.2);
    }

    fn draw_height_map_rect(
        &mut self,
        cx: &mut Cx2d,
        texture: &Texture,
        rect: Rect,
        alpha: f32,
        uv_min: Vec2f,
        uv_max: Vec2f,
    ) {
        self.draw_map.alpha = alpha;
        self.draw_map.uv_min = uv_min;
        self.draw_map.uv_max = uv_max;
        self.draw_map.draw_vars.set_texture(0, texture);
        self.draw_map.draw_abs(cx, rect);
    }

    fn remote_overlay_draw_params(
        preview: &XrDepthAlignSlicePreview,
        remote_height_map: &XrDepthAlignHeightMap,
        ox: f32,
        oy: f32,
        inner: f32,
    ) -> Option<(Rect, Vec2f, Vec2f)> {
        let local_map = &preview.height_map;
        let local_extent_x = Self::height_map_extent_x(local_map);
        let local_extent_z = Self::height_map_extent_z(local_map);
        let remote_extent_x = Self::height_map_extent_x(remote_height_map);
        let remote_extent_z = Self::height_map_extent_z(remote_height_map);
        if local_extent_x <= 1.0e-5
            || local_extent_z <= 1.0e-5
            || remote_extent_x <= 1.0e-5
            || remote_extent_z <= 1.0e-5
        {
            return None;
        }

        let draw_min_x = local_map.origin_x.max(remote_height_map.origin_x);
        let draw_min_z = local_map.origin_z.max(remote_height_map.origin_z);
        let draw_max_x = (local_map.origin_x + local_extent_x)
            .min(remote_height_map.origin_x + remote_extent_x);
        let draw_max_z = (local_map.origin_z + local_extent_z)
            .min(remote_height_map.origin_z + remote_extent_z);
        if draw_max_x <= draw_min_x || draw_max_z <= draw_min_z {
            return None;
        }

        let (screen_min_x, screen_min_y) = Self::height_map_preview_point(
            local_map,
            ox,
            oy,
            inner,
            vec2f(draw_min_x, draw_min_z),
        );
        let (screen_max_x, screen_max_y) = Self::height_map_preview_point(
            local_map,
            ox,
            oy,
            inner,
            vec2f(draw_max_x, draw_max_z),
        );
        let uv_min = vec2f(
            ((draw_min_x - remote_height_map.origin_x) / remote_extent_x).clamp(0.0, 1.0),
            ((draw_min_z - remote_height_map.origin_z) / remote_extent_z).clamp(0.0, 1.0),
        );
        let uv_max = vec2f(
            ((draw_max_x - remote_height_map.origin_x) / remote_extent_x).clamp(0.0, 1.0),
            ((draw_max_z - remote_height_map.origin_z) / remote_extent_z).clamp(0.0, 1.0),
        );
        Some((
            Rect {
                pos: dvec2(screen_min_x as f64, screen_min_y as f64),
                size: dvec2(
                    (screen_max_x - screen_min_x).max(1.0) as f64,
                    (screen_max_y - screen_min_y).max(1.0) as f64,
                ),
            },
            uv_min,
            uv_max,
        ))
    }
}

impl Widget for AlignmentSlicePreview {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        self.draw_bg.draw_abs(cx, rect);
        self.area = self.draw_bg.area();

        let panel_count = if self.remote_height_map.is_some() { 2 } else { 1 };
        let (local_ox, local_oy, local_inner) = Self::preview_panel_square(rect, 0, panel_count);
        let remote_panel = (panel_count > 1).then(|| Self::preview_panel_square(rect, 1, panel_count));
        if let Some(preview) = self.local_preview.clone() {
            Self::ensure_height_texture(&mut self.height_texture, cx.cx, &preview.height_map);
            if let Some(texture) = self.height_texture.as_ref().cloned() {
                self.draw_height_map_rect(
                    cx,
                    &texture,
                    Self::height_map_draw_rect(&preview.height_map, local_ox, local_oy, local_inner),
                    Self::MAP_ALPHA,
                    vec2f(0.0, 0.0),
                    vec2f(1.0, 1.0),
                );
            } else {
                self.draw_map.draw_vars.empty_texture(0);
            }
            if let (Some(remote_height_map), Some((remote_ox, remote_oy, remote_inner))) =
                (self.remote_height_map.as_ref(), remote_panel)
            {
                let panel_rect = Self::remote_overlay_draw_params(
                    &preview,
                    remote_height_map,
                    remote_ox,
                    remote_oy,
                    remote_inner,
                );
                if let Some((rect, uv_min, uv_max)) = panel_rect {
                    Self::ensure_height_texture(
                        &mut self.remote_height_texture,
                        cx.cx,
                        remote_height_map,
                    );
                    if let Some(texture) = self.remote_height_texture.as_ref().cloned() {
                        self.draw_height_map_rect(
                            cx,
                            &texture,
                            rect,
                            Self::MAP_ALPHA,
                            uv_min,
                            uv_max,
                        );
                    }
                }
            }

            self.draw_vector.begin();
            self.draw_grid(&preview.height_map, local_ox, local_oy, local_inner);
            self.draw_origin_cross(&preview, local_ox, local_oy, local_inner);
            self.draw_cutout_ring(&preview, local_ox, local_oy, local_inner);
            self.draw_cutout_heading(&preview, local_ox, local_oy, local_inner);
            if let Some((remote_ox, remote_oy, remote_inner)) = remote_panel {
                self.draw_grid(&preview.height_map, remote_ox, remote_oy, remote_inner);
                self.draw_origin_cross(&preview, remote_ox, remote_oy, remote_inner);
                let remote_cutout = self.remote_height_map.as_ref().and_then(|height_map| {
                    height_map
                        .player_cutout_center
                        .map(|center| (center, height_map.player_cutout_radius_meters))
                });
                if let Some((center, radius_meters)) = remote_cutout {
                    self.draw_cutout_ring_in_local_frame(
                        &preview,
                        center,
                        radius_meters,
                        remote_ox,
                        remote_oy,
                        remote_inner,
                        0x9ed5ff,
                    );
                }
            }
            self.draw_vector.end(cx);
        } else {
            self.draw_map.draw_vars.empty_texture(0);
            self.draw_vector.begin();
            let empty_map = XrDepthAlignHeightMap::default();
            self.draw_grid(&empty_map, local_ox, local_oy, local_inner);
            if let Some((remote_ox, remote_oy, remote_inner)) = remote_panel {
                self.draw_grid(&empty_map, remote_ox, remote_oy, remote_inner);
            }
            self.draw_vector.end(cx);
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if matches!(event, Event::Signal | Event::XrUpdate(_)) {
            self.area.redraw(cx);
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
        let (
            peer_sync_status_text,
            network_status_text,
            alignment_debug_text,
            alignment_state_text,
            peer_scene_text,
            local_slice_preview,
            remote_height_map,
        ) = self
            .ui
            .widget(cx, ids!(xr_people_debug))
            .borrow::<XrPeopleDebug>()
            .map(|people_debug| {
                (
                    people_debug.status_text().to_string(),
                    people_debug.network_status_text().to_string(),
                    people_debug.alignment_debug_text().to_string(),
                    people_debug.alignment_state_text().to_string(),
                    people_debug.peer_scene_text().to_string(),
                    people_debug.local_slice_preview(),
                    people_debug.aligned_peer_height_map(),
                )
            })
            .unwrap_or_else(|| {
                (
                    "AlignSync: unavailable".to_string(),
                    "Network: unavailable".to_string(),
                    "AlignDbg: unavailable".to_string(),
                    "AlignState: unavailable".to_string(),
                    "PeerMap: unavailable".to_string(),
                    None,
                    None,
                )
            });
        if let Some(mut preview) = self
            .ui
            .widget(cx, ids!(alignment_slice_preview))
            .borrow_mut::<AlignmentSlicePreview>()
        {
            preview.set_local_preview(cx, local_slice_preview);
            preview.set_remote_height_map(cx, remote_height_map);
        }
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

        let tsdf_memory_mb = cx
            .xr_depth_mesh()
            .latest_tsdf_snapshot()
            .as_ref()
            .map(|snapshot| snapshot.grid.heap_bytes() as f64 / 1_000_000.0)
            .unwrap_or(0.0);
        let (depth_frames_seen, depth_frames_dropped) = cx
            .xr_depth_mesh()
            .state()
            .read()
            .ok()
            .map(|state| {
                (state.stats.frames_seen, state.stats.frames_dropped)
            })
            .unwrap_or((0, 0));
        let depth_frames_kept = depth_frames_seen.saturating_sub(depth_frames_dropped);
        let depth_drop_percent = if depth_frames_seen > 0 {
            depth_frames_dropped as f64 * 100.0 / depth_frames_seen as f64
        } else {
            0.0
        };
        let xr_runtime_text = match (
            cx.xr_render_scale(),
            cx.xr_display_refresh_rate_hz(),
            cx.xr_effective_frame_rate_hz(),
            cx.xr_gpu_frame_time_ms(),
        ) {
            (Some(scale), Some(refresh_hz), Some(effective_hz), Some(gpu_ms)) => format!(
                "Depth: {:.0} cm | TSDF {:.1} MB | in {} keep {} drop {} ({:.0}%) | XR scale: {:.2} | refresh {:.1} Hz | cadence {:.1} Hz | GPU {:.2} ms",
                cx.xr_depth_mesh().voxel_size_meters() * 100.0,
                tsdf_memory_mb,
                depth_frames_seen,
                depth_frames_kept,
                depth_frames_dropped,
                depth_drop_percent,
                scale,
                refresh_hz,
                effective_hz,
                gpu_ms
            ),
            (Some(scale), Some(refresh_hz), Some(effective_hz), None) => format!(
                "Depth: {:.0} cm | TSDF {:.1} MB | in {} keep {} drop {} ({:.0}%) | XR scale: {:.2} | refresh {:.1} Hz | cadence {:.1} Hz | GPU waiting",
                cx.xr_depth_mesh().voxel_size_meters() * 100.0,
                tsdf_memory_mb,
                depth_frames_seen,
                depth_frames_kept,
                depth_frames_dropped,
                depth_drop_percent,
                scale,
                refresh_hz,
                effective_hz
            ),
            (Some(scale), Some(refresh_hz), None, Some(gpu_ms)) => format!(
                "Depth: {:.0} cm | TSDF {:.1} MB | in {} keep {} drop {} ({:.0}%) | XR scale: {:.2} | refresh {:.1} Hz | cadence waiting | GPU {:.2} ms",
                cx.xr_depth_mesh().voxel_size_meters() * 100.0,
                tsdf_memory_mb,
                depth_frames_seen,
                depth_frames_kept,
                depth_frames_dropped,
                depth_drop_percent,
                scale,
                refresh_hz,
                gpu_ms
            ),
            (Some(scale), Some(refresh_hz), None, None) => format!(
                "Depth: {:.0} cm | TSDF {:.1} MB | in {} keep {} drop {} ({:.0}%) | XR scale: {:.2} | refresh {:.1} Hz | cadence waiting | GPU waiting",
                cx.xr_depth_mesh().voxel_size_meters() * 100.0,
                tsdf_memory_mb,
                depth_frames_seen,
                depth_frames_kept,
                depth_frames_dropped,
                depth_drop_percent,
                scale,
                refresh_hz
            ),
            (Some(scale), None, _, Some(gpu_ms)) => {
                format!(
                    "Depth: {:.0} cm | TSDF {:.1} MB | in {} keep {} drop {} ({:.0}%) | XR scale: {:.2} | refresh waiting | GPU {:.2} ms",
                    cx.xr_depth_mesh().voxel_size_meters() * 100.0,
                    tsdf_memory_mb,
                    depth_frames_seen,
                    depth_frames_kept,
                    depth_frames_dropped,
                    depth_drop_percent,
                    scale,
                    gpu_ms
                )
            }
            (Some(scale), None, _, None) => {
                format!(
                    "Depth: {:.0} cm | TSDF {:.1} MB | in {} keep {} drop {} ({:.0}%) | XR scale: {:.2} | refresh waiting | GPU waiting",
                    cx.xr_depth_mesh().voxel_size_meters() * 100.0,
                    tsdf_memory_mb,
                    depth_frames_seen,
                    depth_frames_kept,
                    depth_frames_dropped,
                    depth_drop_percent,
                    scale
                )
            }
            (None, _, _, _) => format!(
                "Depth: {:.0} cm | TSDF {:.1} MB | in {} keep {} drop {} ({:.0}%) | XR render scale: not active",
                cx.xr_depth_mesh().voxel_size_meters() * 100.0,
                tsdf_memory_mb
                ,
                depth_frames_seen,
                depth_frames_kept,
                depth_frames_dropped,
                depth_drop_percent
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
