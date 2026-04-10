pub use makepad_widgets_dll as makepad_widgets;

use makepad_widgets::*;
use makepad_xr::obj::Tank;
use makepad_xr::scene::*;
use std::fmt::Write as _;

app_main!(App);

const DEBUG_VIEW_REFRESH_INTERVAL_SECONDS: f64 = 1.0;

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

    let TankSlot = XrNode{
        body: mod.widgets.XrBodyKind.Dynamic
        depth_query_support: mod.widgets.XrDepthQuerySupportRig.FourWheels
        shared_object_policy: mod.widgets.XrSharedObjectPolicy.PooledOnDemand
        spawn_pool: true
        physics_size: vec3(0.29, 0.09, 0.41)
        density: 120.0
        friction: 1.35
        restitution: 0.02
        pos: vec3(-12.0, -12.0, 0.0)
        rot: vec3(0.0, 0.0, 0.0)

        tank_body_mount := XrNode{
            body: mod.widgets.XrBodyKind.Disabled
            shared_object_policy: mod.widgets.XrSharedObjectPolicy.None
            pos: vec3(0.0, 0.0, 0.0)

            hull_block := Cube{
                body: mod.widgets.XrBodyKind.Disabled
                shared_object_policy: mod.widgets.XrSharedObjectPolicy.None
                size: vec3(0.28, 0.09, 0.41)
                corner_radius: 0.03
                roughness: 0.58
                metallic: 0.03
                color: #x6a8337
            }

            tank_turret_yaw := XrNode{
                body: mod.widgets.XrBodyKind.Disabled
                shared_object_policy: mod.widgets.XrSharedObjectPolicy.None
                pos: vec3(0.0, 0.08, 0.015)
                turret_block := Cube{
                    body: mod.widgets.XrBodyKind.Disabled
                    shared_object_policy: mod.widgets.XrSharedObjectPolicy.None
                    size: vec3(0.12, 0.07, 0.17)
                    corner_radius: 0.026
                    roughness: 0.44
                    metallic: 0.02
                    color: #x8ca853
                }

                tank_barrel_pitch := XrNode{
                    body: mod.widgets.XrBodyKind.Disabled
                    shared_object_policy: mod.widgets.XrSharedObjectPolicy.None
                    pos: vec3(0.0, 0.0, 0.08)
                    barrel_block := Cube{
                        body: mod.widgets.XrBodyKind.Disabled
                        shared_object_policy: mod.widgets.XrSharedObjectPolicy.None
                        pos: vec3(0.0, 0.0, 0.14)
                        size: vec3(0.025, 0.025, 0.28)
                        corner_radius: 0.02
                        roughness: 0.30
                        metallic: 0.06
                        color: #x24291c
                    }
                }
            }
        }

        tank_wheel_0 := XrNode{
            body: mod.widgets.XrBodyKind.Disabled
            shared_object_policy: mod.widgets.XrSharedObjectPolicy.None
            pos: vec3(-0.113, -0.045, 0.189)
            wheel_mesh := IcoSphere{
                body: mod.widgets.XrBodyKind.Disabled
                shared_object_policy: mod.widgets.XrSharedObjectPolicy.None
                scale: vec3(0.44, 1.0, 1.0)
                radius: 0.117
                diffuse: #xc4c7ce
                color: #x18212b

                marker := Cube{
                    body: mod.widgets.XrBodyKind.Disabled
                    shared_object_policy: mod.widgets.XrSharedObjectPolicy.None
                    size: vec3(0.020, 0.020, 0.020)
                    pos: vec3(0.0, 0.072, 0.0)
                    corner_radius: 0.006
                    roughness: 0.22
                    metallic: 0.06
                    color: #xe8ebf2
                }
            }
        }

        tank_wheel_1 := XrNode{
            body: mod.widgets.XrBodyKind.Disabled
            shared_object_policy: mod.widgets.XrSharedObjectPolicy.None
            pos: vec3(-0.113, -0.045, -0.189)
            wheel_mesh := IcoSphere{
                body: mod.widgets.XrBodyKind.Disabled
                shared_object_policy: mod.widgets.XrSharedObjectPolicy.None
                scale: vec3(0.44, 1.0, 1.0)
                radius: 0.117
                diffuse: #xc4c7ce
                color: #x18212b

                marker := Cube{
                    body: mod.widgets.XrBodyKind.Disabled
                    shared_object_policy: mod.widgets.XrSharedObjectPolicy.None
                    size: vec3(0.020, 0.020, 0.020)
                    pos: vec3(0.0, 0.072, 0.0)
                    corner_radius: 0.006
                    roughness: 0.22
                    metallic: 0.06
                    color: #xe8ebf2
                }
            }
        }

        tank_wheel_2 := XrNode{
            body: mod.widgets.XrBodyKind.Disabled
            shared_object_policy: mod.widgets.XrSharedObjectPolicy.None
            pos: vec3(0.113, -0.045, 0.189)
            wheel_mesh := IcoSphere{
                body: mod.widgets.XrBodyKind.Disabled
                shared_object_policy: mod.widgets.XrSharedObjectPolicy.None
                scale: vec3(0.44, 1.0, 1.0)
                radius: 0.117
                diffuse: #xc4c7ce
                color: #x18212b

                marker := Cube{
                    body: mod.widgets.XrBodyKind.Disabled
                    shared_object_policy: mod.widgets.XrSharedObjectPolicy.None
                    size: vec3(0.020, 0.020, 0.020)
                    pos: vec3(0.0, 0.072, 0.0)
                    corner_radius: 0.006
                    roughness: 0.22
                    metallic: 0.06
                    color: #xe8ebf2
                }
            }
        }

        tank_wheel_3 := XrNode{
            body: mod.widgets.XrBodyKind.Disabled
            shared_object_policy: mod.widgets.XrSharedObjectPolicy.None
            pos: vec3(0.113, -0.045, -0.189)
            wheel_mesh := IcoSphere{
                body: mod.widgets.XrBodyKind.Disabled
                shared_object_policy: mod.widgets.XrSharedObjectPolicy.None
                scale: vec3(0.44, 1.0, 1.0)
                radius: 0.117
                diffuse: #xc4c7ce
                color: #x18212b

                marker := Cube{
                    body: mod.widgets.XrBodyKind.Disabled
                    shared_object_policy: mod.widgets.XrSharedObjectPolicy.None
                    size: vec3(0.020, 0.020, 0.020)
                    pos: vec3(0.0, 0.072, 0.0)
                    corner_radius: 0.006
                    roughness: 0.22
                    metallic: 0.06
                    color: #xe8ebf2
                }
            }
        }
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
            camera.fov_y: 38.0
            camera.desktop_target: vec3(0.05, 0.10, -0.72)
            camera.distance: 1.85
            env.gravity: 9.8
            env.env_cube: true
            env.depth_mesh: false

            scene_select := XrSelect{
                pos: vec3(0.0, -0.02, -0.62)
                scale: vec3(0.5, 0.5, 0.5)
                active_child: @tanks_scene

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

                        for index in 0..80 {
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
                                spawn_pool: true
                                shared_object_policy: mod.widgets.XrSharedObjectPolicy.PooledOnDemand
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

                tanks_scene := XrNode{
                    pos: vec3(0.0, -0.16, 0.0)
                    scale: vec3(0.62, 0.62, 0.62)
                    on_render: ||{
                        tank_controller := Tank{}

                        Platform{
                            pos: vec3(0.0, -0.06, 0.0)
                            size: vec3(0.48387095, 0.08, 0.48387095)
                            friction: 1.8
                            color: #x283544
                        }

                        Cube{
                            body: mod.widgets.XrBodyKind.Fixed
                            size: vec3(0.34, 0.05, 0.46)
                            corner_radius: 0.018
                            roughness: 0.78
                            metallic: 0.0
                            friction: 1.6
                            color: #x4b5f72
                            pos: vec3(0.30, 0.01, -0.06)
                            rot: vec3(0.24, 0.0, 0.0)
                        }

                        tank_slots := XrNode{
                            body: mod.widgets.XrBodyKind.Disabled
                            shared_object_policy: mod.widgets.XrSharedObjectPolicy.None
                            for index in 0..8 {
                                TankSlot{
                                    pos: vec3(-14.0 - index * 0.7, -8.0, 0.0)
                                }
                            }
                        }

                        tank_projectiles := XrNode{
                            body: mod.widgets.XrBodyKind.Disabled
                            shared_object_policy: mod.widgets.XrSharedObjectPolicy.None
                            for index in 0..48 {
                                IcoSphere{
                                    spawn_pool: true
                                    shared_object_policy: mod.widgets.XrSharedObjectPolicy.PooledOnDemand
                                    gravity_scale: 1.0
                                    density: 0.65
                                    friction: 0.08
                                    restitution: 0.01
                                    radius: 0.024
                                    diffuse: #xa0a4aa
                                    color: #xffc857
                                    pos: vec3(-12.0, -12.0 - index * 0.01, 0.0)
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
                                    shared_object_policy: mod.widgets.XrSharedObjectPolicy.BootstrapShared
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
            xr_peer_sync := XrPeerSync{
                auto_alignment_enabled: false
            }

            xr_scene_sync_controller := XrSceneSyncController{}

            control_strip := XrView{
                visible: false
                show_in_non_xr: true
                pos: vec3(0.05, 0.44, -0.78)
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

                        tanks_scene_button := XrUiButton{
                            width: 72
                            text: "Tanks"
                            on_press: || ui.scene_select.tanks_scene()
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

                        tank_depth_mesh_mode_button := XrUiButton{
                            width: 154
                            text: "Tank TSDF Mode"
                            on_press: || ui.root.toggle_depth_mesh_focus_cube()
                        }

                        scene_status := Label{
                            width: Fill
                            text: "Default scene: tank mode. Left stick steers, right trigger accelerates, left trigger reverses, right stick aims the turret, A/X fire shells, B resets the tank, and controller grip picks the tank up."
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

                        depth_resolution_2_button := XrUiButton{
                            width: 94
                            text: "2 cm fixed"
                            on_press: || ui.root.set_depth_voxel_size(0.02)
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
                            width: Fill{weight: 66.0}
                            height: Fill
                            flow: Down
                            spacing: 8

                            SolidView{
                                width: Fill
                                height: Fit
                                padding: Inset{left: 10 right: 10 top: 6 bottom: 6}
                                draw_bg.color: #x0d1824

                                debug_field := Label{
                                    width: Fill
                                    height: Fit
                                    text: "Waiting for debug stats..."
                                    draw_text.color: #xe8f4ff
                                    draw_text.flow: Flow.Right{wrap: true}
                                }
                            }
                        }

                        View{
                            width: Fill{weight: 34.0}
                            height: Fill
                        }
                    }
                }
            }

            wrist_toggle := XrView{
                visible: false
                mode: mod.widgets.XrViewMode.StuckToWrist
                wrist_left: true
                logical_size: vec2(112, 188)
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

                    wrist_pose_button := XrUiButton{
                        width: Fill
                        height: 40
                        text: "Pose"
                        draw_bg.border_radius: 12.0
                        on_press: || ui.root.reset_activity_pose()
                    }

                    wrist_sync_status := Label{
                        width: Fill
                        height: Fit
                        text: "P:0.0 X:0.0"
                        draw_text.color: #xe8f4ff
                        draw_text.flow: Flow.Right{wrap: true}
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
    last_debug_text: String,
    #[rust]
    last_debug_refresh_at: Option<f64>,
    #[rust]
    last_wrist_perf_text: String,
    #[rust]
    debug_text_scratch: String,
    #[rust]
    wrist_perf_text_scratch: String,
}

impl App {
    fn run_scene_sync_pre_event(&self, cx: &mut Cx, event: &Event) {
        let controller = self.ui.widget(cx, ids!(xr_scene_sync_controller));
        if let Some(mut controller) = controller.borrow_mut::<XrSceneSyncController>() {
            controller.pre_ui_event(cx, event);
        };
    }

    fn run_scene_sync_post_event(&self, cx: &mut Cx, event: &Event) {
        let controller = self.ui.widget(cx, ids!(xr_scene_sync_controller));
        if let Some(mut controller) = controller.borrow_mut::<XrSceneSyncController>() {
            controller.post_ui_event(cx, event);
        };
    }

    fn run_tank_pre_event(&self, cx: &mut Cx, event: &Event) {
        let tank = self.ui.widget(cx, ids!(tank_controller));
        if let Some(mut tank) = tank.borrow_mut::<Tank>() {
            tank.pre_ui_event(cx, event);
        };
    }

    fn run_tank_post_event(&self, cx: &mut Cx) {
        let tank = self.ui.widget(cx, ids!(tank_controller));
        if let Some(mut tank) = tank.borrow_mut::<Tank>() {
            tank.post_ui_event(cx);
        };
    }

    fn refresh_debug_fields(&mut self, cx: &mut Cx) {
        let now = Cx::time_now();
        if self
            .last_debug_refresh_at
            .is_some_and(|last| now - last < DEBUG_VIEW_REFRESH_INTERVAL_SECONDS)
        {
            return;
        }
        self.last_debug_refresh_at = Some(now);

        let mut draw_top_children_text = String::new();
        let (
            surface_count,
            compute_ms,
            query_ms,
            rapier_ms,
            frame_cpu_ms,
            frame_update_cpu_ms,
            frame_draw_cpu_ms,
            draw_setup_cpu_ms,
            draw_env_prepare_cpu_ms,
            draw_sort_cpu_ms,
            draw_children_cpu_ms,
            draw_child_count,
            draw_transparent_child_count,
            draw_runtime_body_count,
            draw_geometry_pool_slots,
            draw_geometry_pool_live,
            draw_draw_list_pool_slots,
            draw_draw_list_pool_live,
            draw_texture_pool_slots,
            draw_texture_pool_live,
            draw_depth_mesh_chunk_count,
            draw_recycled_depth_mesh_geometry_count,
            draw_depth_mesh_pending_upsert_count,
            draw_depth_query_retained_hit_count,
            xr_frame_cpu_ms,
            xr_render_cpu_ms,
            xr_depth_readback_cpu_ms,
            xr_frame_cpu_breakdown,
        ) = if let Some(root) = self.ui.borrow::<XrRoot>() {
            (
                root.physics_depth_query_surface_count(),
                root.physics_compute_ms(),
                root.physics_tsdf_query_ms(),
                root.physics_rapier_step_ms(),
                root.frame_cpu_ms(),
                root.frame_update_cpu_ms(),
                root.frame_draw_cpu_ms(),
                root.draw_setup_cpu_ms(),
                root.draw_env_prepare_cpu_ms(),
                root.draw_sort_cpu_ms(),
                root.draw_children_cpu_ms(),
                root.draw_child_count(),
                root.draw_transparent_child_count(),
                root.draw_runtime_body_count(),
                root.draw_geometry_pool_slots(),
                root.draw_geometry_pool_live(),
                root.draw_draw_list_pool_slots(),
                root.draw_draw_list_pool_live(),
                root.draw_texture_pool_slots(),
                root.draw_texture_pool_live(),
                root.draw_depth_mesh_chunk_count(),
                root.draw_recycled_depth_mesh_geometry_count(),
                root.draw_depth_mesh_pending_upsert_count(),
                root.draw_depth_query_retained_hit_count(),
                cx.xr_frame_cpu_time_ms(),
                cx.xr_render_cpu_time_ms(),
                cx.xr_depth_readback_cpu_time_ms(),
                cx.xr_frame_cpu_breakdown(),
            )
        } else {
            return;
        };
        if let Some(root) = self.ui.borrow::<XrRoot>() {
            root.write_draw_top_children_text(&mut draw_top_children_text);
        }
        let connected_peers = self
            .ui
            .widget(cx, ids!(xr_peer_sync))
            .borrow::<XrPeerSync>()
            .map(|peer_sync| { 
                (
                    peer_sync.connected_peer_count(),
                    peer_sync.shared_object_count(),
                    peer_sync.touch_sync_status_text(),
                    peer_sync.local_touch_signal_text(),
                    peer_sync.remote_touch_signal_text(),
                    peer_sync.alignment_debug_text().to_string(),
                    peer_sync.alignment_state_text().to_string(),
                    peer_sync.peer_scene_text().to_string(),
                )
            })
            .unwrap_or((
                0,
                0,
                "Touch sync: off".to_string(),
                "TouchRaw: off".to_string(),
                "TouchPeer: off".to_string(),
                "AlignDbg: off".to_string(),
                "AlignState: off".to_string(),
                "PeerMap: off".to_string(),
            ));
        let tsdf_memory_mb = cx
            .xr_tsdf()
            .latest_tsdf_snapshot()
            .as_ref()
            .map(|snapshot| {
                let grid = &snapshot.grid;
                grid.heap_bytes() as f64 / 1_000_000.0
            })
            .unwrap_or(0.0);
        let (depth_frames_seen, depth_frames_dropped) = cx
            .xr_tsdf()
            .state()
            .read()
            .ok()
            .map(|state| (state.stats.frames_seen, state.stats.frames_dropped))
            .unwrap_or((0, 0));
        let depth_frames_kept = depth_frames_seen.saturating_sub(depth_frames_dropped);
        let gpu_time_text = cx
            .xr_gpu_frame_time_ms()
            .map(|gpu_ms| format!("{gpu_ms:.2} ms"))
            .unwrap_or_else(|| "waiting".to_string());
        let xr_frame_cpu_text = xr_frame_cpu_ms
            .map(|cpu_ms| format!("{cpu_ms:.2} ms"))
            .unwrap_or_else(|| "waiting".to_string());
        let xr_render_cpu_text = xr_render_cpu_ms
            .map(|cpu_ms| format!("{cpu_ms:.2} ms"))
            .unwrap_or_else(|| "waiting".to_string());
        let xr_depth_readback_cpu_text = xr_depth_readback_cpu_ms
            .map(|cpu_ms| format!("{cpu_ms:.2} ms"))
            .unwrap_or_else(|| "waiting".to_string());
        let xr_begin_chain = xr_frame_cpu_breakdown
            .map(|cpu| {
                format!(
                    "wait {:.2} > begin {:.2} > loc-space {:.2} > loc-views {:.2} > acq {:.2} > wait-img {:.2} > acq-depth {:.2}",
                    cpu.wait_frame_ms,
                    cpu.begin_frame_ms,
                    cpu.locate_space_ms,
                    cpu.locate_views_ms,
                    cpu.acquire_swapchain_ms,
                    cpu.wait_swapchain_ms,
                    cpu.acquire_depth_ms,
                )
            })
            .unwrap_or_else(|| "waiting".to_string());
        let xr_work_chain = xr_frame_cpu_breakdown
            .map(|cpu| {
                format!(
                    "prep {:.2} > xr {:.2} > next {:.2} > draw {:.2} > shaders {:.2} > repaint {:.2} > readback {:.2} > end {:.2} > resize {:.2} > total {:.2}",
                    cpu.update_prepare_ms,
                    cpu.update_dispatch_ms,
                    cpu.next_frame_ms,
                    cpu.draw_event_ms,
                    cpu.compile_shaders_ms,
                    cpu.repaint_ms,
                    cpu.depth_readback_ms,
                    cpu.end_frame_ms,
                    cpu.resize_projection_ms,
                    cpu.total_ms,
                )
            })
            .unwrap_or_else(|| "waiting".to_string());
        let bytes_to_mb = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);
        let xr_repaint_chain = xr_frame_cpu_breakdown
            .map(|cpu| {
                format!(
                    "wait-fence {:.2} > prep-tex {:.2} > record {:.2} > submit {:.2}",
                    cpu.repaint_wait_inflight_ms,
                    cpu.repaint_prepare_textures_ms,
                    cpu.repaint_record_draw_ms,
                    cpu.repaint_submit_ms,
                )
            })
            .unwrap_or_else(|| "waiting".to_string());
        let xr_repaint_uploads = xr_frame_cpu_breakdown
            .map(|cpu| {
                format!(
                    "tex {:.2} MB/{} > packet {:.2} MB/{} > geom {:.2} MB > desc {}",
                    bytes_to_mb(cpu.repaint_texture_upload_bytes),
                    cpu.repaint_texture_upload_count,
                    bytes_to_mb(cpu.repaint_packet_buffer_bytes),
                    cpu.repaint_packet_buffer_count,
                    bytes_to_mb(cpu.repaint_geometry_upload_bytes),
                    cpu.repaint_descriptor_set_count,
                )
            })
            .unwrap_or_else(|| "waiting".to_string());
        let xr_repaint_draw = xr_frame_cpu_breakdown
            .map(|cpu| {
                format!(
                    "items {} > calls {} > packets {} > instances {} > indices {}",
                    cpu.repaint_draw_items,
                    cpu.repaint_draw_calls,
                    cpu.repaint_packets,
                    cpu.repaint_instances,
                    cpu.repaint_indices,
                )
            })
            .unwrap_or_else(|| "waiting".to_string());
        let mut gamepad_count = 0usize;
        for state in cx.game_input_states() {
            let GameInputState::Gamepad(_gamepad) = state else {
                continue;
            };
            gamepad_count += 1;
        }
        self.debug_text_scratch.clear();
        let _ = write!(
            &mut self.debug_text_scratch,
            "OpenXR frame CPU: {xr_frame_cpu_text}\nOpenXR begin chain: {xr_begin_chain}\nOpenXR work chain: {xr_work_chain}\nOpenXR repaint chain: {xr_repaint_chain}\nOpenXR repaint uploads: {xr_repaint_uploads}\nOpenXR repaint draw: {xr_repaint_draw}\nVulkan XR render CPU: {xr_render_cpu_text}\nDepth readback CPU: {xr_depth_readback_cpu_text}\nUI frame CPU: {frame_cpu_ms:.2} ms\nUI update time: {frame_update_cpu_ms:.2} ms\nUI draw time: {frame_draw_cpu_ms:.2} ms\nUI draw chain: setup {draw_setup_cpu_ms:.2} > env {draw_env_prepare_cpu_ms:.2} > sort {draw_sort_cpu_ms:.2} > children {draw_children_cpu_ms:.2}\nUI top children: {draw_top_children_text}\nUI draw state: children {draw_child_count}/{draw_transparent_child_count} runtime-bodies {draw_runtime_body_count}\nUI pool state: geom {draw_geometry_pool_live}/{draw_geometry_pool_slots} > lists {draw_draw_list_pool_live}/{draw_draw_list_pool_slots} > tex {draw_texture_pool_live}/{draw_texture_pool_slots}\nUI depth state: chunks {draw_depth_mesh_chunk_count} recycled-geoms {draw_recycled_depth_mesh_geometry_count} pending-upserts {draw_depth_mesh_pending_upsert_count} retained-hits {draw_depth_query_retained_hit_count}\nPhysics planes: {surface_count}\nPhysics compute time: {compute_ms:.2} ms\nQuery time: {query_ms:.2} ms\nRapier time: {rapier_ms:.2} ms\nTSDF size: {tsdf_memory_mb:.1} MB\nDepth frames kept: {depth_frames_kept}\nGPU time: {gpu_time_text}\nGamepads: {gamepad_count}\nConnected peers: {}\nShared objects: {}\n{}\n{}\n{}\n{}\n{}\n{}",
            connected_peers.0,
            connected_peers.1,
            connected_peers.2,
            connected_peers.3,
            connected_peers.4,
            connected_peers.5,
            connected_peers.6,
            connected_peers.7,
        );
        if self.last_debug_text != self.debug_text_scratch {
            self.ui
                .widget(cx, ids!(debug_field))
                .set_text(cx, &self.debug_text_scratch);
            self.last_debug_text.clear();
            self.last_debug_text.push_str(&self.debug_text_scratch);
        }
        self.wrist_perf_text_scratch.clear();
        let _ = write!(
            &mut self.wrist_perf_text_scratch,
            "P:{:.1} X:{:.1}",
            compute_ms + query_ms + rapier_ms,
            xr_frame_cpu_ms.unwrap_or(frame_cpu_ms),
        );
        if self.last_wrist_perf_text != self.wrist_perf_text_scratch {
            self.ui
                .widget(cx, ids!(wrist_sync_status))
                .set_text(cx, &self.wrist_perf_text_scratch);
            self.last_wrist_perf_text.clear();
            self.last_wrist_perf_text
                .push_str(&self.wrist_perf_text_scratch);
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
        self.run_scene_sync_pre_event(cx, event);
        self.run_tank_pre_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        self.run_tank_post_event(cx);
        self.run_scene_sync_post_event(cx, event);
        self.refresh_debug_fields(cx);
    }
}
