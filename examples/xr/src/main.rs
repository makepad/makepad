use makepad_widgets;

use makepad_widgets::*;
use makepad_xr::net::{XrNetPeerId, XrSharedHand, XrSharedObjectMode};
use makepad_xr::obj::car::{car_drive_command, CarDriveConfig};
use makepad_xr::obj::cube::Cube;
use makepad_xr::obj::tank::{tank_stick_axes, TankDriveConfig};
use makepad_xr::obj::IcoSphere;
use makepad_xr::scene::*;
use std::collections::HashSet;
use std::fmt::Write as _;

app_main!(App);

const ACTIVITY_POSE_SYNC_INTERVAL_SECONDS: f64 = 0.6;
const DEBUG_VIEW_REFRESH_INTERVAL_SECONDS: f64 = 1.0;
const ACTIVITY_POSE_SYNC_POSITION_EPSILON_METERS: f32 = 0.015;
const ACTIVITY_POSE_SYNC_ROTATION_EPSILON_DEGREES: f32 = 1.5;
const TANK_TURRET_YAW_SPEED_RADPS: f32 = 1.5;
const TANK_TURRET_PITCH_SPEED_RADPS: f32 = 4.2;
const TANK_TURRET_PITCH_MIN_RAD: f32 = -0.35;
const TANK_TURRET_PITCH_MAX_RAD: f32 = 0.55;
const TANK_PROJECTILE_RATE_HZ: f32 = 10.0;
const TANK_PROJECTILE_SPEED_MPS: f32 = 7.5;
const TANK_PROJECTILE_RADIUS_METERS: f32 = 0.024;
const TANK_PROJECTILE_MAX_EMITS_PER_UPDATE: usize = 2;
const TANK_HIT_FLASH_SECONDS: f64 = 0.35;
const TANK_SPAWN_RING_RADIUS_METERS: f32 = 0.06;
const TANK_WHEEL_COUNT: usize = 4;
const TANK_WHEEL_LATERAL_OFFSET_METERS: f32 = 0.113;
const TANK_WHEEL_VERTICAL_OFFSET_METERS: f32 = -0.045;
const TANK_WHEEL_FRONT_OFFSET_METERS: f32 = 0.189;
const TANK_WHEEL_BACK_OFFSET_METERS: f32 = -0.189;
const TANK_BODY_HALF_WIDTH_METERS: f32 = 0.145;
const TANK_BODY_HALF_HEIGHT_METERS: f32 = 0.045;
const TANK_BODY_HALF_DEPTH_METERS: f32 = 0.205;
const TANK_PLATE_TOP_LOCAL_Y_METERS: f32 = -0.02;
const TANK_FOUR_WHEEL_RADIUS_SCALE: f32 = 3.20;
const TANK_FOUR_WHEEL_REST_LENGTH_SCALE: f32 = 0.50;
const TANK_FOUR_WHEEL_RADIUS_MIN_METERS: f32 = 0.036;
const TANK_FOUR_WHEEL_RADIUS_MAX_METERS: f32 = 0.160;
const TANK_FOUR_WHEEL_REST_LENGTH_MIN_METERS: f32 = 0.024;
const TANK_FOUR_WHEEL_REST_LENGTH_MAX_METERS: f32 = 0.110;
const TANK_SPAWN_SUSPENSION_PRELOAD_WORLD_METERS: f32 = 0.004;
const TANK_SPAWN_EXTRA_CLEARANCE_WORLD_METERS: f32 = 0.030;
const TANK_BODY_VISUAL_SUSPENSION_RESPONSE: f32 = 0.0;
const TANK_BODY_VISUAL_AXLE_CLEARANCE_SCALE: f32 = 0.42;
const TANK_BODY_VISUAL_LIFT_MIN_METERS: f32 = 0.0;
const TANK_BODY_VISUAL_LIFT_MAX_METERS: f32 = 0.300;
const TANK_SCENE_STATUS_TEXT: &str =
    "Tank mode: left stick steers, right trigger accelerates, left trigger reverses, right stick aims the turret, A/X fire shells, B resets the tank, and controller grip picks the tank up.";

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
    network_started: bool,
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
    #[rust]
    suppress_activity_broadcast: Option<XrActivityId>,
    #[rust]
    pending_shared_scene_reset: bool,
    #[rust]
    last_activity_pose_sync: Option<Pose>,
    #[rust]
    last_activity_pose_sync_activity: Option<XrActivityId>,
    #[rust]
    last_activity_pose_sync_at: f64,
    #[rust]
    tank_drive: TankDriveConfig,
    #[rust]
    car_drive: CarDriveConfig,
    #[rust]
    tank_turret_yaw: f32,
    #[rust]
    tank_turret_pitch: f32,
    #[rust]
    tank_pool_uids: Vec<WidgetUid>,
    #[rust]
    tank_spawn_requested: bool,
    #[rust]
    primary_tank_widget_uid: Option<WidgetUid>,
    #[rust]
    tank_projectile_pool_uids: Vec<WidgetUid>,
    #[rust]
    tank_projectile_cursor: usize,
    #[rust]
    tank_projectile_next_emit_at: Option<f64>,
    #[rust]
    tank_active_hit_projectiles: HashSet<WidgetUid>,
    #[rust]
    tank_hit_flash_until: f64,
    #[rust]
    last_desktop_tank_drive_at: Option<f64>,
    #[rust]
    last_desktop_tank_reset_pressed: bool,
    #[rust]
    desktop_tank_drive_armed: bool,
    #[rust]
    tank_depth_focus_cube_auto_enabled: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct DesktopTankInput {
    left_stick: Vec2f,
    right_stick: Vec2f,
    left_trigger: f32,
    right_trigger: f32,
    a: f32,
    b: f32,
    x: f32,
}

impl App {
    fn tank_physics_wheel_radius_meters() -> f32 {
        let support_base = TANK_BODY_HALF_WIDTH_METERS
            .min(TANK_BODY_HALF_HEIGHT_METERS)
            .min(TANK_BODY_HALF_DEPTH_METERS)
            .max(0.0005);
        (support_base * TANK_FOUR_WHEEL_RADIUS_SCALE).clamp(
            TANK_FOUR_WHEEL_RADIUS_MIN_METERS,
            TANK_FOUR_WHEEL_RADIUS_MAX_METERS,
        )
    }

    fn tank_physics_wheel_rest_length_meters() -> f32 {
        (Self::tank_physics_wheel_radius_meters() * TANK_FOUR_WHEEL_REST_LENGTH_SCALE).clamp(
            TANK_FOUR_WHEEL_REST_LENGTH_MIN_METERS,
            TANK_FOUR_WHEEL_REST_LENGTH_MAX_METERS,
        )
    }

    fn tank_physics_wheel_min_length_meters() -> f32 {
        let rest_length = Self::tank_physics_wheel_rest_length_meters();
        let min_length_floor = 0.004;
        let travel = (Self::tank_physics_wheel_radius_meters() * 0.50).clamp(0.018, 0.090);
        (rest_length - travel).max(min_length_floor)
    }

    fn tank_physics_wheel_min_clearance_meters() -> f32 {
        (Self::tank_physics_wheel_radius_meters() * 0.28).clamp(0.014, 0.028)
    }

    fn tank_physics_body_collider_bottom_meters() -> f32 {
        let physics_half_height = TANK_BODY_HALF_HEIGHT_METERS * 0.70;
        let physics_center_offset = TANK_BODY_HALF_HEIGHT_METERS * 0.20;
        physics_center_offset - physics_half_height
    }

    fn tank_physics_wheel_local_pose(index: usize) -> Option<Pose> {
        let x = if index < 2 {
            -TANK_WHEEL_LATERAL_OFFSET_METERS
        } else {
            TANK_WHEEL_LATERAL_OFFSET_METERS
        };
        let z = match index % 2 {
            0 => TANK_WHEEL_FRONT_OFFSET_METERS,
            1 => TANK_WHEEL_BACK_OFFSET_METERS,
            _ => return None,
        };
        Some(Pose::new(
            Quat::default(),
            vec3f(x, TANK_WHEEL_VERTICAL_OFFSET_METERS, z),
        ))
    }

    fn tank_physics_body_mount_lift_meters() -> f32 {
        -Self::tank_physics_body_collider_bottom_meters() + Self::tank_physics_wheel_radius_meters()
    }

    fn tank_support_world_metrics(&self, cx: &mut Cx) -> (f32, f32) {
        let scene_scale = self
            .tank_scene_spawn_basis(cx)
            .map(|(_, _, scale)| scale)
            .unwrap_or(vec3f(1.0, 1.0, 1.0));
        let half_extents = vec3f(
            TANK_BODY_HALF_WIDTH_METERS * scene_scale.x,
            TANK_BODY_HALF_HEIGHT_METERS * scene_scale.y,
            TANK_BODY_HALF_DEPTH_METERS * scene_scale.z,
        );
        let support_base = half_extents
            .x
            .min(half_extents.y)
            .min(half_extents.z)
            .max(0.0005);
        let radius = (support_base * TANK_FOUR_WHEEL_RADIUS_SCALE).clamp(
            TANK_FOUR_WHEEL_RADIUS_MIN_METERS,
            TANK_FOUR_WHEEL_RADIUS_MAX_METERS,
        );
        let rest_length = (radius * TANK_FOUR_WHEEL_REST_LENGTH_SCALE).clamp(
            TANK_FOUR_WHEEL_REST_LENGTH_MIN_METERS,
            TANK_FOUR_WHEEL_REST_LENGTH_MAX_METERS,
        );
        (radius, rest_length)
    }

    fn tank_spawn_support_clearance_meters(&self, cx: &mut Cx) -> f32 {
        let scene_scale_y = self
            .tank_scene_spawn_basis(cx)
            .map(|(_, _, scale)| scale.y.abs())
            .filter(|scale| *scale > 1.0e-4)
            .unwrap_or(1.0);
        let (support_radius_world, support_rest_world) = self.tank_support_world_metrics(cx);
        let support_radius_local = support_radius_world / scene_scale_y;
        let support_rest_local = support_rest_world / scene_scale_y;
        let preload_local = TANK_SPAWN_SUSPENSION_PRELOAD_WORLD_METERS / scene_scale_y;
        let extra_clearance_local = TANK_SPAWN_EXTRA_CLEARANCE_WORLD_METERS / scene_scale_y;
        TANK_PLATE_TOP_LOCAL_Y_METERS
            + TANK_BODY_HALF_HEIGHT_METERS
            + support_rest_local
            + support_radius_local
            + extra_clearance_local
            - preload_local
    }

    fn desktop_tank_input_is_neutral(input: DesktopTankInput) -> bool {
        input.right_stick.length() <= 0.16
            && input.left_stick.length() <= 0.16
            && input.left_trigger <= 0.08
            && input.right_trigger <= 0.08
            && input.a <= 0.5
            && input.b <= 0.5
            && input.x <= 0.5
    }

    fn tank_wheel_pivot_ref(
        &self,
        cx: &mut Cx,
        tank_widget: &WidgetRef,
        index: usize,
    ) -> WidgetRef {
        match index {
            0 => tank_widget.widget(cx, ids!(tank_wheel_0)),
            1 => tank_widget.widget(cx, ids!(tank_wheel_1)),
            2 => tank_widget.widget(cx, ids!(tank_wheel_2)),
            3 => tank_widget.widget(cx, ids!(tank_wheel_3)),
            _ => WidgetRef::default(),
        }
    }

    fn tank_wheel_mesh_ref(&self, cx: &mut Cx, tank_widget: &WidgetRef, index: usize) -> WidgetRef {
        match index {
            0 => tank_widget.widget(cx, ids!(tank_wheel_0.wheel_mesh)),
            1 => tank_widget.widget(cx, ids!(tank_wheel_1.wheel_mesh)),
            2 => tank_widget.widget(cx, ids!(tank_wheel_2.wheel_mesh)),
            3 => tank_widget.widget(cx, ids!(tank_wheel_3.wheel_mesh)),
            _ => WidgetRef::default(),
        }
    }

    fn default_tank_wheel_local_pose(index: usize) -> Option<Pose> {
        Self::tank_physics_wheel_local_pose(index)
    }

    fn sync_tank_wheel_widgets(
        &self,
        cx: &mut Cx,
        tank_widget: &WidgetRef,
        tank_body: &XrRuntimeBodyState,
    ) {
        for index in 0..TANK_WHEEL_COUNT {
            let Some(local_pose) = tank_body.linked_support_local_poses[index]
                .or_else(|| Self::default_tank_wheel_local_pose(index))
            else {
                continue;
            };
            let steering = tank_body.linked_support_steer_angles[index].unwrap_or(0.0);
            let spin = tank_body.linked_support_spin_angles[index].unwrap_or(0.0);
            if let Some(mut pivot) = self
                .tank_wheel_pivot_ref(cx, tank_widget, index)
                .borrow_mut::<XrNode>()
            {
                pivot.set_pos(cx, local_pose.position);
                pivot.set_rot(cx, vec3f(0.0, steering, 0.0));
            }
            if let Some(mut wheel) = self
                .tank_wheel_mesh_ref(cx, tank_widget, index)
                .borrow_mut::<IcoSphere>()
            {
                wheel.set_radius(cx, Self::tank_physics_wheel_radius_meters());
                wheel.set_rot(cx, vec3f(spin, 0.0, 0.0));
            }
        }
    }

    fn tank_body_visual_lift(tank_body: &XrRuntimeBodyState) -> f32 {
        let mut wheel_y_sum = 0.0;
        let mut wheel_count = 0.0;
        for index in 0..TANK_WHEEL_COUNT {
            if let Some(local_pose) = tank_body.linked_support_local_poses[index]
                .or_else(|| Self::default_tank_wheel_local_pose(index))
            {
                wheel_y_sum += local_pose.position.y;
                wheel_count += 1.0;
            }
        }
        let average_wheel_y = if wheel_count > 0.0 {
            wheel_y_sum / wheel_count
        } else {
            TANK_WHEEL_VERTICAL_OFFSET_METERS
        };
        (Self::tank_physics_body_mount_lift_meters()
            + (TANK_WHEEL_VERTICAL_OFFSET_METERS - average_wheel_y)
                * TANK_BODY_VISUAL_SUSPENSION_RESPONSE)
            .clamp(
                TANK_BODY_VISUAL_LIFT_MIN_METERS,
                TANK_BODY_VISUAL_LIFT_MAX_METERS,
            )
    }

    fn rotation_quat(rot: Vec3f) -> Quat {
        let x = Quat::from_axis_angle(vec3f(1.0, 0.0, 0.0), rot.x);
        let y = Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), rot.y);
        let z = Quat::from_axis_angle(vec3f(0.0, 0.0, 1.0), rot.z);
        Quat::multiply(&z, &Quat::multiply(&y, &x))
    }

    fn quat_to_rot(quat: Quat) -> Vec3f {
        // Invert `rotation_quat`, which composes local node rotations as X then Y then Z.
        let x = (2.0 * (quat.w * quat.x - quat.y * quat.z))
            .atan2(1.0 - 2.0 * (quat.x * quat.x + quat.y * quat.y));
        let y = (2.0 * (quat.x * quat.z + quat.w * quat.y))
            .clamp(-1.0, 1.0)
            .asin();
        let z = (2.0 * (quat.w * quat.z - quat.x * quat.y))
            .atan2(1.0 - 2.0 * (quat.y * quat.y + quat.z * quat.z));
        vec3f(x, y, z)
    }

    fn transform_basis_with_node(
        parent_pos: Vec3f,
        parent_ori: Quat,
        parent_scale: Vec3f,
        node: &XrNode,
    ) -> (Vec3f, Quat, Vec3f) {
        let local_pos = vec3f(
            node.pos().x * parent_scale.x,
            node.pos().y * parent_scale.y,
            node.pos().z * parent_scale.z,
        );
        let rotated_pos = parent_ori.rotate_vec3(&local_pos);
        let orientation = Quat::multiply(&Self::rotation_quat(node.rot()), &parent_ori);
        let scale = vec3f(
            parent_scale.x * node.scale().x,
            parent_scale.y * node.scale().y,
            parent_scale.z * node.scale().z,
        );
        (parent_pos + rotated_pos, orientation, scale)
    }

    fn transform_pose_with_basis(
        parent_pos: Vec3f,
        parent_ori: Quat,
        parent_scale: Vec3f,
        local_pose: Pose,
    ) -> Pose {
        let scaled_pos = vec3f(
            local_pose.position.x * parent_scale.x,
            local_pose.position.y * parent_scale.y,
            local_pose.position.z * parent_scale.z,
        );
        Pose::new(
            Quat::multiply(&local_pose.orientation, &parent_ori),
            parent_pos + parent_ori.rotate_vec3(&scaled_pos),
        )
    }

    fn tank_scene_spawn_basis(&self, cx: &mut Cx) -> Option<(Vec3f, Quat, Vec3f)> {
        let scene_select = self.ui.widget(cx, ids!(scene_select));
        let (select_pos, select_ori, select_scale) =
            xr_widget_with_scene_node(&scene_select, |node| {
                (node.pos(), Self::rotation_quat(node.rot()), node.scale())
            })?;
        let tanks_scene = scene_select.widget(cx, ids!(tanks_scene));
        xr_widget_with_scene_node(&tanks_scene, |node| {
            Self::transform_basis_with_node(select_pos, select_ori, select_scale, node)
        })
    }

    fn poses_match(left: Pose, right: Pose) -> bool {
        let translation_delta = (left.position - right.position).length();
        let rotation_dot = left
            .orientation
            .dot(right.orientation)
            .abs()
            .clamp(0.0, 1.0);
        let rotation_delta_degrees = (2.0 * rotation_dot.acos()).to_degrees();
        translation_delta <= ACTIVITY_POSE_SYNC_POSITION_EPSILON_METERS
            && rotation_delta_degrees <= ACTIVITY_POSE_SYNC_ROTATION_EPSILON_DEGREES
    }

    fn current_activity(&self, cx: &mut Cx) -> Option<XrActivityId> {
        self.ui
            .widget(cx, ids!(scene_select))
            .borrow::<XrSelect>()
            .map(|select| select.activity_id())
    }

    fn active_scene_widget(&self, cx: &mut Cx) -> Option<WidgetRef> {
        self.ui
            .widget(cx, ids!(scene_select))
            .borrow::<XrSelect>()
            .and_then(|select| select.active_child_widget_ref())
    }

    fn apply_activity(&mut self, cx: &mut Cx, activity_id: XrActivityId) -> Option<WidgetRef> {
        self.ui
            .widget(cx, ids!(scene_select))
            .borrow_mut::<XrSelect>()
            .and_then(|mut select| select.set_activity(cx, activity_id))
    }

    fn ensure_network_started(&mut self, cx: &mut Cx) {
        if self.network_started {
            return;
        }
        if let Some(mut peer_sync) = self
            .ui
            .widget(cx, ids!(xr_peer_sync))
            .borrow_mut::<XrPeerSync>()
        {
            peer_sync.set_enabled(cx, true);
            self.network_started = true;
        }
    }

    fn ensure_activity_announced(&mut self, cx: &mut Cx) {
        let Some(activity_id) = self.current_activity(cx) else {
            return;
        };
        if let Some(mut peer_sync) = self
            .ui
            .widget(cx, ids!(xr_peer_sync))
            .borrow_mut::<XrPeerSync>()
        {
            if peer_sync.enabled() && peer_sync.current_activity().is_none() {
                let _ = peer_sync.set_local_activity(cx, activity_id);
            }
        }
    }

    fn sync_authoritative_activity_pose(&mut self, cx: &mut Cx) {
        let Some(activity_id) = self.current_activity(cx) else {
            self.last_activity_pose_sync = None;
            self.last_activity_pose_sync_activity = None;
            self.last_activity_pose_sync_at = 0.0;
            return;
        };

        let should_sync = self
            .ui
            .widget(cx, ids!(xr_peer_sync))
            .borrow::<XrPeerSync>()
            .is_some_and(|peer_sync| {
                peer_sync.enabled()
                    && peer_sync.connected_peer_count() != 0
                    && peer_sync.local_is_activity_authority()
            });
        if !should_sync {
            self.last_activity_pose_sync = None;
            self.last_activity_pose_sync_activity = None;
            self.last_activity_pose_sync_at = 0.0;
            return;
        }

        let Some(content_pose) = self
            .ui
            .borrow::<XrRoot>()
            .and_then(|root| root.content_pose())
        else {
            return;
        };
        let now = Cx::time_now();
        let activity_changed = self.last_activity_pose_sync_activity != Some(activity_id);
        let pose_changed = self
            .last_activity_pose_sync
            .is_none_or(|previous| !Self::poses_match(previous, content_pose));
        let interval_elapsed =
            now - self.last_activity_pose_sync_at >= ACTIVITY_POSE_SYNC_INTERVAL_SECONDS;
        if !(activity_changed || pose_changed || interval_elapsed) {
            return;
        }

        if let Some(mut peer_sync) = self
            .ui
            .widget(cx, ids!(xr_peer_sync))
            .borrow_mut::<XrPeerSync>()
        {
            if peer_sync.send_activity_pose_reset(content_pose) {
                self.last_activity_pose_sync = Some(content_pose);
                self.last_activity_pose_sync_activity = Some(activity_id);
                self.last_activity_pose_sync_at = now;
            }
        }
    }

    fn refresh_spawnable_registry(&mut self, cx: &mut Cx, force: bool) {
        let Some(activity_id) = self.current_activity(cx) else {
            return;
        };
        let peer_sync_widget = self.ui.widget(cx, ids!(xr_peer_sync));
        let should_refresh = force
            || peer_sync_widget
                .borrow::<XrPeerSync>()
                .is_some_and(|peer_sync| peer_sync.spawnable_activity() != Some(activity_id));
        if !should_refresh {
            if let Some(mut peer_sync) = peer_sync_widget.borrow_mut::<XrPeerSync>() {
                peer_sync.flush_pending_shared_object_controls(cx);
            }
            return;
        }
        let Some(scene_widget) = self.active_scene_widget(cx) else {
            return;
        };
        let bindings = collect_scene_spawnable_objects(activity_id, &scene_widget);
        {
            if let Some(mut peer_sync) = peer_sync_widget.borrow_mut::<XrPeerSync>() {
                peer_sync.set_spawnable_objects(activity_id, bindings);
                peer_sync.flush_pending_shared_object_controls(cx);
            };
        }
    }

    fn apply_remote_body_spawn(&mut self, cx: &mut Cx, spawn: XrBodySpawn) {
        if let Some(mut root) = self.ui.borrow_mut::<XrRoot>() {
            root.spawn_body(cx, spawn);
        }
    }

    fn apply_remote_body_despawn(&mut self, cx: &mut Cx, widget_uid: WidgetUid) {
        if let Some(mut root) = self.ui.borrow_mut::<XrRoot>() {
            root.despawn_body(cx, widget_uid);
        }
    }

    fn apply_body_impulse(&mut self, cx: &mut Cx, impulse: XrBodyImpulse) {
        if let Some(mut root) = self.ui.borrow_mut::<XrRoot>() {
            root.apply_body_impulse(cx, impulse);
        }
    }

    fn apply_car_control(&mut self, cx: &mut Cx, control: XrCarControl) {
        if let Some(mut root) = self.ui.borrow_mut::<XrRoot>() {
            root.apply_car_control(cx, control);
        }
    }

    fn is_tanks_scene_active(&self, cx: &mut Cx) -> bool {
        self.current_activity(cx) == Some(XrActivityId(live_id!(tanks_scene)))
    }

    fn collect_spawn_pool_widget_uids(widget: &WidgetRef, pool_uids: &mut Vec<WidgetUid>) {
        if !widget.visible() {
            return;
        }
        xr_widget_with_scene_node(widget, |node| {
            if node.spawn_pool() {
                pool_uids.push(widget.widget_uid());
            }
        });
        xr_widget_children(widget, &mut |_, child| {
            Self::collect_spawn_pool_widget_uids(&child, pool_uids)
        });
    }

    fn find_widget_by_uid(widget: &WidgetRef, target: WidgetUid) -> Option<WidgetRef> {
        if widget.widget_uid() == target {
            return Some(widget.clone());
        }
        let mut found = None;
        xr_widget_children(widget, &mut |_, child| {
            if found.is_none() {
                found = Self::find_widget_by_uid(&child, target);
            }
        });
        found
    }

    fn peer_sync_local_peer_id(&self, cx: &mut Cx) -> Option<XrNetPeerId> {
        self.ui
            .widget(cx, ids!(xr_peer_sync))
            .borrow::<XrPeerSync>()
            .and_then(|peer_sync| peer_sync.local_peer_id())
    }

    fn shared_object_authority_for_widget(
        &self,
        cx: &mut Cx,
        widget_uid: WidgetUid,
    ) -> Option<XrNetPeerId> {
        self.ui
            .widget(cx, ids!(xr_peer_sync))
            .borrow::<XrPeerSync>()
            .and_then(|peer_sync| peer_sync.shared_object_authority_for_widget(widget_uid))
    }

    fn widget_is_local_shared_object(&self, cx: &mut Cx, widget_uid: WidgetUid) -> bool {
        self.ui
            .widget(cx, ids!(xr_peer_sync))
            .borrow::<XrPeerSync>()
            .is_some_and(|peer_sync| peer_sync.widget_is_local_shared_object(widget_uid))
    }

    fn refresh_tank_pool(&mut self, cx: &mut Cx) {
        self.tank_pool_uids.clear();
        let tank_slots = self.ui.widget(cx, ids!(tank_slots));
        if tank_slots.borrow::<XrNode>().is_none() {
            return;
        }
        Self::collect_spawn_pool_widget_uids(&tank_slots, &mut self.tank_pool_uids);
    }

    fn local_tank_widget_uid(&mut self, cx: &mut Cx) -> Option<WidgetUid> {
        if self.tank_pool_uids.is_empty() {
            self.refresh_tank_pool(cx);
        }
        if let Some(primary) = self.primary_tank_widget_uid {
            if self.widget_is_local_shared_object(cx, primary) {
                return Some(primary);
            }
            let has_any_local_shared = self
                .tank_pool_uids
                .iter()
                .copied()
                .any(|widget_uid| self.widget_is_local_shared_object(cx, widget_uid));
            if !has_any_local_shared && self.tank_body_state_for_uid(cx, primary).is_some() {
                return Some(primary);
            }
        }
        self.tank_pool_uids
            .iter()
            .copied()
            .find(|widget_uid| self.widget_is_local_shared_object(cx, *widget_uid))
    }

    fn tank_body_state_for_uid(
        &self,
        _cx: &mut Cx,
        tank_widget_uid: WidgetUid,
    ) -> Option<XrRuntimeBodyState> {
        let runtime_bodies = self
            .ui
            .borrow::<XrRoot>()
            .map(|root| root.runtime_bodies())?;
        runtime_bodies.get(&tank_widget_uid).cloned()
    }

    fn local_tank_body_state(&mut self, cx: &mut Cx) -> Option<(WidgetUid, XrRuntimeBodyState)> {
        let tank_widget_uid = self.local_tank_widget_uid(cx)?;
        self.tank_body_state_for_uid(cx, tank_widget_uid)
            .map(|body| (tank_widget_uid, body))
    }

    fn tank_widget_ref(&mut self, cx: &mut Cx, widget_uid: WidgetUid) -> Option<WidgetRef> {
        let tank_slots = self.ui.widget(cx, ids!(tank_slots));
        if tank_slots.borrow::<XrNode>().is_none() {
            return None;
        }
        Self::find_widget_by_uid(&tank_slots, widget_uid)
    }

    fn emit_local_shared_body_spawn(&mut self, cx: &mut Cx, spawn: XrBodySpawn) -> WidgetUid {
        let peer_sync_widget = self.ui.widget(cx, ids!(xr_peer_sync));
        if let Some(mut peer_sync) = peer_sync_widget.borrow_mut::<XrPeerSync>() {
            if let Some(spawn) = peer_sync.send_local_body_spawn(spawn) {
                let widget_uid = spawn.widget_uid;
                self.apply_remote_body_spawn(cx, spawn);
                return widget_uid;
            }
        }
        let widget_uid = spawn.widget_uid;
        self.apply_remote_body_spawn(cx, spawn);
        widget_uid
    }

    fn emit_local_shared_body_spawn_exact(&mut self, cx: &mut Cx, spawn: XrBodySpawn) -> WidgetUid {
        let peer_sync_widget = self.ui.widget(cx, ids!(xr_peer_sync));
        if let Some(mut peer_sync) = peer_sync_widget.borrow_mut::<XrPeerSync>() {
            if let Some(spawn) = peer_sync.send_local_body_spawn_exact(spawn) {
                let widget_uid = spawn.widget_uid;
                self.apply_remote_body_spawn(cx, spawn);
                return widget_uid;
            }
        }
        let widget_uid = spawn.widget_uid;
        self.apply_remote_body_spawn(cx, spawn);
        widget_uid
    }

    fn tank_spawn_pose(&self, cx: &mut Cx) -> Pose {
        let support_clearance = self.tank_spawn_support_clearance_meters(cx);
        let peer_id = self.peer_sync_local_peer_id(cx).unwrap_or_default();
        let hash = peer_id
            .0
            .wrapping_mul(0x9e37_79b9)
            .wrapping_add(0x7f4a_7c15);
        let angle = ((hash & 1023) as f32 / 1024.0) * std::f32::consts::TAU;
        let radius =
            TANK_SPAWN_RING_RADIUS_METERS + (((hash >> 10) & 63) as f32 / 63.0 - 0.5) * 0.015;
        let local_pose = Pose::new(
            Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), angle + std::f32::consts::PI),
            vec3f(
                angle.cos() * radius,
                support_clearance,
                angle.sin() * radius,
            ),
        );
        if let Some((scene_pos, scene_ori, scene_scale)) = self.tank_scene_spawn_basis(cx) {
            Self::transform_pose_with_basis(scene_pos, scene_ori, scene_scale, local_pose)
        } else {
            local_pose
        }
    }

    fn tank_reset_pose_from_controller(&self, cx: &mut Cx, controller: &XrController) -> Pose {
        let support_clearance = self.tank_spawn_support_clearance_meters(cx);
        let pose = controller.grip_pose;
        if !controller.active() || !pose.is_finite() {
            return self.tank_spawn_pose(cx);
        }
        let mut forward = pose.orientation.rotate_vec3(&vec3f(0.0, 0.0, 1.0));
        forward.y = 0.0;
        let yaw = if forward.length() > 1.0e-4 {
            forward = forward.normalize();
            forward.x.atan2(forward.z)
        } else {
            0.0
        };
        Pose::new(
            Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), yaw),
            pose.position + vec3f(0.0, support_clearance, 0.0),
        )
    }

    fn ensure_local_tank_spawned(&mut self, cx: &mut Cx) {
        if !self.is_tanks_scene_active(cx) {
            self.tank_active_hit_projectiles.clear();
            self.tank_spawn_requested = false;
            return;
        }
        if let Some((widget_uid, _)) = self.local_tank_body_state(cx) {
            self.primary_tank_widget_uid = Some(widget_uid);
            self.tank_spawn_requested = false;
            return;
        }
        let scene_ready = self
            .ui
            .borrow::<XrRoot>()
            .is_some_and(|root| root.physics_scene_body_count() > 0);
        if !scene_ready || self.tank_spawn_requested {
            return;
        }
        if self.tank_pool_uids.is_empty() {
            self.refresh_tank_pool(cx);
        }
        let Some(widget_uid) = self
            .primary_tank_widget_uid
            .or_else(|| self.tank_pool_uids.first().copied())
        else {
            return;
        };
        let spawn_pose = self.tank_spawn_pose(cx);
        let widget_uid = self.emit_local_shared_body_spawn_exact(
            cx,
            XrBodySpawn {
                widget_uid,
                shadow: false,
                mode: XrSharedObjectMode::Dynamic,
                pose: spawn_pose,
                linvel: vec3f(0.0, 0.0, 0.0),
                angvel: vec3f(0.0, 0.0, 0.0),
            },
        );
        self.tank_spawn_requested = true;
        self.primary_tank_widget_uid = Some(widget_uid);
    }

    fn sync_tank_depth_mesh_focus(&mut self, cx: &mut Cx) {
        let focus_point = if self.is_tanks_scene_active(cx) {
            self.local_tank_body_state(cx)
                .map(|(_, body)| body.pose.position)
                .filter(|position| position.is_finite())
        } else {
            None
        };
        if let Some(mut root) = self.ui.borrow_mut::<XrRoot>() {
            if focus_point.is_some() {
                if !root.depth_mesh_focus_cube_enabled() {
                    root.toggle_depth_mesh_focus_cube(cx);
                    self.tank_depth_focus_cube_auto_enabled = true;
                }
            } else if self.tank_depth_focus_cube_auto_enabled
                && root.depth_mesh_focus_cube_enabled()
            {
                root.toggle_depth_mesh_focus_cube(cx);
                self.tank_depth_focus_cube_auto_enabled = false;
            }
            root.set_depth_mesh_focus_point(focus_point);
        }
    }

    fn reset_local_tank(&mut self, cx: &mut Cx) -> bool {
        let spawn_pose = self.tank_spawn_pose(cx);
        self.reset_local_tank_at_pose(cx, spawn_pose)
    }

    fn reset_local_tank_at_pose(&mut self, cx: &mut Cx, spawn_pose: Pose) -> bool {
        if !self.is_tanks_scene_active(cx) {
            return false;
        }
        if self.tank_pool_uids.is_empty() {
            self.refresh_tank_pool(cx);
        }
        let Some(widget_uid) = self
            .local_tank_widget_uid(cx)
            .or(self.primary_tank_widget_uid)
            .or_else(|| self.tank_pool_uids.first().copied())
        else {
            return false;
        };
        self.tank_turret_yaw = 0.0;
        self.tank_turret_pitch = 0.0;
        self.tank_projectile_next_emit_at = None;
        self.tank_active_hit_projectiles.clear();
        self.tank_hit_flash_until = 0.0;
        let widget_uid = self.emit_local_shared_body_spawn_exact(
            cx,
            XrBodySpawn {
                widget_uid,
                shadow: false,
                mode: XrSharedObjectMode::Dynamic,
                pose: spawn_pose,
                linvel: vec3f(0.0, 0.0, 0.0),
                angvel: vec3f(0.0, 0.0, 0.0),
            },
        );
        self.tank_spawn_requested = true;
        self.primary_tank_widget_uid = Some(widget_uid);
        cx.redraw_all();
        true
    }

    fn refresh_tank_projectile_pool(&mut self, cx: &mut Cx) {
        self.tank_projectile_pool_uids.clear();
        let projectile_root = self.ui.widget(cx, ids!(tank_projectiles));
        if projectile_root.borrow::<XrNode>().is_none() {
            self.tank_projectile_cursor = 0;
            return;
        }
        Self::collect_spawn_pool_widget_uids(&projectile_root, &mut self.tank_projectile_pool_uids);
        if self.tank_projectile_pool_uids.is_empty() {
            self.tank_projectile_cursor = 0;
        } else {
            self.tank_projectile_cursor %= self.tank_projectile_pool_uids.len();
        }
    }

    fn next_tank_projectile_widget_uid(&mut self, cx: &mut Cx) -> Option<WidgetUid> {
        if self.tank_projectile_pool_uids.is_empty() {
            self.refresh_tank_projectile_pool(cx);
        }
        let len = self.tank_projectile_pool_uids.len();
        if len == 0 {
            return None;
        }
        let widget_uid = self.tank_projectile_pool_uids[self.tank_projectile_cursor % len];
        self.tank_projectile_cursor = (self.tank_projectile_cursor + 1) % len;
        Some(widget_uid)
    }

    fn sync_local_tank_widgets(&mut self, cx: &mut Cx, now: f64) {
        let Some((tank_widget_uid, tank_body)) = self.local_tank_body_state(cx) else {
            self.ui
                .widget(cx, ids!(scene_status))
                .set_text(cx, TANK_SCENE_STATUS_TEXT);
            return;
        };
        let Some(tank_widget) = self.tank_widget_ref(cx, tank_widget_uid) else {
            return;
        };
        if let Some(mut body_mount) = tank_widget
            .widget(cx, ids!(tank_body_mount))
            .borrow_mut::<XrNode>()
        {
            body_mount.set_pos(cx, vec3f(0.0, Self::tank_body_visual_lift(&tank_body), 0.0));
        }
        if let Some(mut turret) = tank_widget
            .widget(cx, ids!(tank_turret_yaw))
            .borrow_mut::<XrNode>()
        {
            turret.set_rot(cx, vec3f(0.0, self.tank_turret_yaw, 0.0));
        }
        if let Some(mut barrel) = tank_widget
            .widget(cx, ids!(tank_barrel_pitch))
            .borrow_mut::<XrNode>()
        {
            barrel.set_rot(cx, vec3f(self.tank_turret_pitch, 0.0, 0.0));
        }
        if let Some(mut hull) = tank_widget
            .widget(cx, ids!(hull_block))
            .borrow_mut::<Cube>()
        {
            let color = if now < self.tank_hit_flash_until {
                vec4f(0.98, 0.36, 0.26, 1.0)
            } else {
                vec4f(0.4157, 0.5137, 0.2157, 1.0)
            };
            hull.set_color(cx, color);
        }
        self.sync_tank_wheel_widgets(cx, &tank_widget, &tank_body);
        let status = if now < self.tank_hit_flash_until {
            format!("Tank hit by a remote shell. {TANK_SCENE_STATUS_TEXT}")
        } else {
            TANK_SCENE_STATUS_TEXT.to_string()
        };
        self.ui.widget(cx, ids!(scene_status)).set_text(cx, &status);
    }

    fn update_tank_turret_with_controller(
        &mut self,
        cx: &mut Cx,
        controller: &XrController,
        dt: f32,
    ) {
        if self.local_tank_body_state(cx).is_none() {
            return;
        }
        let (pitch_input, yaw_input) = tank_stick_axes(controller.stick, self.tank_drive);
        let dt = dt.clamp(1.0 / 240.0, 0.1);
        self.tank_turret_yaw = (self.tank_turret_yaw
            + yaw_input * TANK_TURRET_YAW_SPEED_RADPS * dt)
            .rem_euclid(std::f32::consts::TAU);
        self.tank_turret_pitch = (self.tank_turret_pitch
            + pitch_input * TANK_TURRET_PITCH_SPEED_RADPS * dt)
            .clamp(TANK_TURRET_PITCH_MIN_RAD, TANK_TURRET_PITCH_MAX_RAD);
    }

    fn detect_local_tank_hits(&mut self, cx: &mut Cx, now: f64) {
        let Some(local_tank_widget_uid) = self.local_tank_widget_uid(cx) else {
            self.tank_active_hit_projectiles.clear();
            return;
        };
        let Some(local_authority) =
            self.shared_object_authority_for_widget(cx, local_tank_widget_uid)
        else {
            self.tank_active_hit_projectiles.clear();
            return;
        };
        if self.tank_projectile_pool_uids.is_empty() {
            self.refresh_tank_projectile_pool(cx);
        }
        let projectile_pool: HashSet<WidgetUid> =
            self.tank_projectile_pool_uids.iter().copied().collect();
        let runtime_contacts = self
            .ui
            .borrow::<XrRoot>()
            .map(|root| root.runtime_contacts());
        let Some(runtime_contacts) = runtime_contacts else {
            return;
        };
        let mut active_projectiles = HashSet::new();
        for &(left, right) in runtime_contacts.iter() {
            let projectile_uid =
                if left == local_tank_widget_uid && projectile_pool.contains(&right) {
                    Some(right)
                } else if right == local_tank_widget_uid && projectile_pool.contains(&left) {
                    Some(left)
                } else {
                    None
                };
            let Some(projectile_uid) = projectile_uid else {
                continue;
            };
            let Some(projectile_authority) =
                self.shared_object_authority_for_widget(cx, projectile_uid)
            else {
                continue;
            };
            if projectile_authority == local_authority {
                continue;
            }
            if self.tank_active_hit_projectiles.insert(projectile_uid) {
                self.tank_hit_flash_until = now + TANK_HIT_FLASH_SECONDS;
                crate::log!(
                    "tank hit: local authority {:08x} hit by projectile {:016x} from {:08x}",
                    local_authority.0,
                    projectile_uid.0,
                    projectile_authority.0
                );
            }
            active_projectiles.insert(projectile_uid);
        }
        self.tank_active_hit_projectiles = active_projectiles;
    }

    fn emit_tank_projectiles(&mut self, cx: &mut Cx, now: f64, fire_active: bool) {
        if !fire_active {
            self.tank_projectile_next_emit_at = None;
            return;
        }
        let Some((_, tank_body)) = self.local_tank_body_state(cx) else {
            self.tank_projectile_next_emit_at = None;
            return;
        };

        let interval = (1.0 / TANK_PROJECTILE_RATE_HZ).clamp(0.01, 10.0) as f64;
        let tank_orientation = tank_body.pose.orientation;
        let turret_orientation = Quat::multiply(
            &Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), self.tank_turret_yaw),
            &tank_orientation,
        );
        let barrel_orientation = Quat::multiply(
            &Quat::from_axis_angle(vec3f(1.0, 0.0, 0.0), self.tank_turret_pitch),
            &turret_orientation,
        );
        let tank_position = tank_body.pose.position;
        let tank_scale = tank_body.scale;
        let scale_local = |offset: Vec3f| {
            vec3f(
                offset.x * tank_scale.x,
                offset.y * tank_scale.y,
                offset.z * tank_scale.z,
            )
        };
        let turret_mount =
            tank_position + tank_orientation.rotate_vec3(&scale_local(vec3f(0.0, 0.08, 0.015)));
        let barrel_pivot =
            turret_mount + turret_orientation.rotate_vec3(&scale_local(vec3f(0.0, 0.0, 0.08)));
        let barrel_tip =
            barrel_pivot + barrel_orientation.rotate_vec3(&scale_local(vec3f(0.0, 0.0, 0.28)));
        let direction = barrel_orientation
            .rotate_vec3(&vec3f(0.0, 0.0, 1.0))
            .normalize();
        let projectile_radius = TANK_PROJECTILE_RADIUS_METERS
            * tank_scale.x.min(tank_scale.y).min(tank_scale.z).max(0.0001);
        let mut next_emit_at = self.tank_projectile_next_emit_at.unwrap_or(now);
        let mut emitted = 0usize;

        while now >= next_emit_at && emitted < TANK_PROJECTILE_MAX_EMITS_PER_UPDATE {
            let Some(widget_uid) = self.next_tank_projectile_widget_uid(cx) else {
                self.tank_projectile_next_emit_at = None;
                return;
            };
            let _ = self.emit_local_shared_body_spawn(
                cx,
                XrBodySpawn {
                    widget_uid,
                    shadow: false,
                    mode: XrSharedObjectMode::Dynamic,
                    pose: Pose::new(
                        barrel_orientation,
                        barrel_tip + direction * projectile_radius,
                    ),
                    linvel: tank_body.linvel + direction * TANK_PROJECTILE_SPEED_MPS,
                    angvel: vec3f(0.0, 0.0, 0.0),
                },
            );
            cx.redraw_all();
            next_emit_at += interval;
            emitted += 1;
        }

        self.tank_projectile_next_emit_at = Some(next_emit_at);
    }

    fn desktop_gamepad_tank_input(&mut self, cx: &mut Cx) -> (usize, Option<DesktopTankInput>) {
        let mut gamepad_count = 0usize;
        let mut best_input = None;
        let mut best_score = 0.0f32;
        for state in cx.game_input_states() {
            let GameInputState::Gamepad(gamepad) = state else {
                continue;
            };
            gamepad_count += 1;
            let input = DesktopTankInput {
                left_stick: vec2f(gamepad.left_stick.x as f32, gamepad.left_stick.y as f32),
                right_stick: vec2f(gamepad.right_stick.x as f32, gamepad.right_stick.y as f32),
                left_trigger: gamepad.left_trigger as f32,
                right_trigger: gamepad.right_trigger as f32,
                a: gamepad.a as f32,
                b: gamepad.b as f32,
                x: gamepad.x as f32,
            };
            let score = input.left_stick.length() * 2.0
                + input.right_stick.length()
                + input.left_trigger.max(input.right_trigger)
                + input.a.max(input.x)
                + input.b;
            if score > best_score {
                best_input = Some(input);
                best_score = score;
            }
        }
        (gamepad_count, best_input)
    }

    fn drive_tank_with_controllers(
        &mut self,
        cx: &mut Cx,
        right_controller: &XrController,
        left_controller: &XrController,
    ) -> Option<(bool, bool, bool, bool, Option<XrSharedHand>, Vec3f, Vec3f)> {
        let Some((tank_widget_uid, body)) = self.local_tank_body_state(cx) else {
            return None;
        };
        let control = car_drive_command(
            tank_widget_uid,
            body.held_by,
            left_controller.stick,
            right_controller.trigger,
            left_controller.trigger,
            self.car_drive,
        );
        let forced_dynamic =
            control.is_some() && body.held_by.is_none() && (!body.dynamic_body || body.shadowed);
        if forced_dynamic {
            let widget_uid = self.emit_local_shared_body_spawn_exact(
                cx,
                XrBodySpawn {
                    widget_uid: tank_widget_uid,
                    shadow: false,
                    mode: XrSharedObjectMode::Dynamic,
                    pose: body.pose,
                    linvel: body.linvel,
                    angvel: body.angvel,
                },
            );
            self.primary_tank_widget_uid = Some(widget_uid);
            self.tank_spawn_requested = true;
        }
        let applied = control.is_some();
        if let Some(control) = control {
            self.apply_car_control(cx, control);
        }
        Some((
            applied,
            forced_dynamic,
            body.dynamic_body,
            body.shadowed,
            body.held_by,
            body.linvel,
            body.angvel,
        ))
    }

    fn drive_tank_for_update(&mut self, cx: &mut Cx, update: &XrUpdateEvent) {
        if update.clicked_b() {
            let spawn_pose =
                self.tank_reset_pose_from_controller(cx, &update.state.right_controller);
            self.reset_local_tank_at_pose(cx, spawn_pose);
            self.sync_local_tank_widgets(cx, update.state.time);
            return;
        }
        let dt = (update.state.time - update.last.time).clamp(1.0 / 240.0, 0.1) as f32;
        let _ = self.drive_tank_with_controllers(
            cx,
            &update.state.right_controller,
            &update.state.left_controller,
        );
        self.update_tank_turret_with_controller(cx, &update.state.right_controller, dt);
        self.emit_tank_projectiles(
            cx,
            update.state.time,
            update.state.left_controller.click_a()
                || update.state.left_controller.click_x()
                || update.state.right_controller.click_a()
                || update.state.right_controller.click_x(),
        );
        self.detect_local_tank_hits(cx, update.state.time);
        self.sync_local_tank_widgets(cx, update.state.time);
    }

    fn drive_tank_for_desktop_frame(&mut self, cx: &mut Cx, event: &NextFrameEvent) {
        let dt = self
            .last_desktop_tank_drive_at
            .map(|last| (event.time - last) as f32)
            .unwrap_or(1.0 / 60.0);
        self.last_desktop_tank_drive_at = Some(event.time);
        let (_, best_input) = self.desktop_gamepad_tank_input(cx);
        let Some(input) = best_input else {
            self.last_desktop_tank_reset_pressed = false;
            self.desktop_tank_drive_armed = false;
            self.emit_tank_projectiles(cx, event.time, false);
            self.detect_local_tank_hits(cx, event.time);
            self.sync_local_tank_widgets(cx, event.time);
            return;
        };
        if !self.desktop_tank_drive_armed {
            if Self::desktop_tank_input_is_neutral(input) {
                self.desktop_tank_drive_armed = true;
            } else {
                self.last_desktop_tank_reset_pressed = false;
                self.emit_tank_projectiles(cx, event.time, false);
                self.detect_local_tank_hits(cx, event.time);
                self.sync_local_tank_widgets(cx, event.time);
                return;
            }
        }
        let reset_pressed = input.b > 0.5;
        let reset_clicked = reset_pressed && !self.last_desktop_tank_reset_pressed;
        self.last_desktop_tank_reset_pressed = reset_pressed;
        if reset_clicked {
            self.reset_local_tank(cx);
            self.desktop_tank_drive_armed = false;
            self.sync_local_tank_widgets(cx, event.time);
            return;
        }
        let right_controller = XrController {
            stick: input.right_stick,
            trigger: input.right_trigger,
            buttons: if input.a > 0.5 {
                XrController::CLICK_A
            } else {
                0
            },
            ..XrController::default()
        };
        let left_controller = XrController {
            stick: input.left_stick,
            trigger: input.left_trigger,
            buttons: if input.x > 0.5 {
                XrController::CLICK_X
            } else {
                0
            },
            ..XrController::default()
        };
        let outcome = self.drive_tank_with_controllers(cx, &right_controller, &left_controller);
        self.update_tank_turret_with_controller(cx, &right_controller, dt);
        self.emit_tank_projectiles(
            cx,
            event.time,
            left_controller.click_a()
                || left_controller.click_x()
                || right_controller.click_a()
                || right_controller.click_x(),
        );
        self.detect_local_tank_hits(cx, event.time);
        self.sync_local_tank_widgets(cx, event.time);
        let _ = (dt, outcome);
    }

    fn publish_local_shared_object_states(&mut self, cx: &mut Cx) {
        let runtime_bodies = self.ui.borrow::<XrRoot>().map(|root| root.runtime_bodies());
        let Some(runtime_bodies) = runtime_bodies else {
            return;
        };
        let peer_sync_widget = self.ui.widget(cx, ids!(xr_peer_sync));
        let maybe_peer_sync = peer_sync_widget.borrow_mut::<XrPeerSync>();
        if let Some(mut peer_sync) = maybe_peer_sync {
            peer_sync.publish_local_shared_object_states(cx, runtime_bodies.as_ref());
        }
    }

    fn apply_pending_shared_scene_reset(&mut self, cx: &mut Cx) {
        if !self.pending_shared_scene_reset {
            return;
        }
        let runtime_bodies = self.ui.borrow::<XrRoot>().map(|root| root.runtime_bodies());
        let Some(runtime_bodies) = runtime_bodies else {
            return;
        };
        if runtime_bodies.is_empty() {
            return;
        }
        let reset_applied = {
            let peer_sync_widget = self.ui.widget(cx, ids!(xr_peer_sync));
            let reset_applied =
                if let Some(mut peer_sync) = peer_sync_widget.borrow_mut::<XrPeerSync>() {
                    peer_sync.reset_local_shared_bootstrap_objects(runtime_bodies.as_ref());
                    true
                } else {
                    false
                };
            reset_applied
        };
        if reset_applied {
            self.pending_shared_scene_reset = false;
        }
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

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let root_uid = self.ui.widget_uid();
        let scene_select_uid = self.ui.widget(cx, ids!(scene_select)).widget_uid();
        let peer_sync_widget = self.ui.widget(cx, ids!(xr_peer_sync));
        let peer_sync_uid = peer_sync_widget.widget_uid();

        let mut remote_activity = None;
        let mut remote_body_spawns = Vec::new();
        let mut remote_body_impulses = Vec::new();
        let mut remote_body_despawns = Vec::new();
        let mut remote_activity_pose_reset = None;
        let mut local_activity = None;
        let mut local_body_spawns = Vec::new();
        let mut local_activity_pose_reset = None;
        let mut scene_changed = false;

        for action in actions {
            let Some(widget_action) = action.as_widget_action() else {
                continue;
            };
            if widget_action.widget_uid == peer_sync_uid {
                match widget_action.cast::<XrPeerSyncAction>() {
                    XrPeerSyncAction::ActivityChanged(activity_id) => {
                        remote_activity = Some(activity_id);
                    }
                    XrPeerSyncAction::ActivityPoseReset(pose) => {
                        remote_activity_pose_reset = Some(pose);
                    }
                    XrPeerSyncAction::BodySpawn(spawn) => {
                        remote_body_spawns.push(spawn);
                    }
                    XrPeerSyncAction::BodyImpulse(impulse) => {
                        remote_body_impulses.push(impulse);
                    }
                    XrPeerSyncAction::BodyDespawn(widget_uid) => {
                        remote_body_despawns.push(widget_uid);
                    }
                    XrPeerSyncAction::None => {}
                }
            }
            if widget_action.widget_uid == root_uid {
                match widget_action.cast::<XrRootAction>() {
                    XrRootAction::PhysicsReset => {
                        self.pending_shared_scene_reset = true;
                    }
                    XrRootAction::ContentPoseReset(pose) => {
                        local_activity_pose_reset = Some(pose);
                    }
                    XrRootAction::None => {}
                }
            }
            if widget_action.widget_uid == scene_select_uid {
                if let XrSelectAction::ActiveChildChanged(activity_id) =
                    widget_action.cast::<XrSelectAction>()
                {
                    local_activity = Some(activity_id);
                }
            }
            if let Some(body_spawn) = widget_action.action.downcast_ref::<XrBodySpawn>() {
                local_body_spawns.push(*body_spawn);
            }
            if matches!(
                widget_action.cast::<XrNodeAction>(),
                XrNodeAction::SceneChanged
            ) {
                scene_changed = true;
            }
        }

        if scene_changed {
            self.refresh_spawnable_registry(cx, true);
        }

        if let Some(activity_id) = remote_activity {
            if self.current_activity(cx) != Some(activity_id) {
                self.suppress_activity_broadcast = Some(activity_id);
                if self.apply_activity(cx, activity_id).is_none() {
                    self.suppress_activity_broadcast = None;
                }
            }
            self.refresh_spawnable_registry(cx, true);
        }

        if let Some(activity_id) = local_activity {
            self.refresh_spawnable_registry(cx, true);
            if self.suppress_activity_broadcast == Some(activity_id) {
                self.suppress_activity_broadcast = None;
            } else if let Some(mut peer_sync) = peer_sync_widget.borrow_mut::<XrPeerSync>() {
                let _ = peer_sync.set_local_activity(cx, activity_id);
            }
        }

        if let Some(pose) = remote_activity_pose_reset {
            if let Some(mut root) = self.ui.borrow_mut::<XrRoot>() {
                root.set_content_pose(cx, pose);
            }
            self.pending_shared_scene_reset = true;
        }

        if let Some(pose) = local_activity_pose_reset {
            self.pending_shared_scene_reset = true;
            if let Some(mut peer_sync) = peer_sync_widget.borrow_mut::<XrPeerSync>() {
                let _ = peer_sync.send_activity_pose_reset(pose);
            }
        }

        for widget_uid in remote_body_despawns {
            self.apply_remote_body_despawn(cx, widget_uid);
        }

        for spawn in remote_body_spawns {
            self.apply_remote_body_spawn(cx, spawn);
        }

        for impulse in remote_body_impulses {
            self.apply_body_impulse(cx, impulse);
        }

        if !local_body_spawns.is_empty() {
            self.refresh_spawnable_registry(cx, false);
            if let Some(mut peer_sync) = peer_sync_widget.borrow_mut::<XrPeerSync>() {
                for spawn in local_body_spawns {
                    if let Some(spawn) = peer_sync.send_local_body_spawn(spawn) {
                        self.apply_remote_body_spawn(cx, spawn);
                    }
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
        if let Event::GameInputConnected(ev) = event {
            let _ = ev;
            self.desktop_tank_drive_armed = false;
        }
        self.match_event(cx, event);
        if let Event::XrUpdate(update) = event {
            self.last_desktop_tank_drive_at = None;
            self.drive_tank_for_update(cx, update);
        } else if let Event::NextFrame(next_frame) = event {
            if !cx.in_xr_mode() {
                self.drive_tank_for_desktop_frame(cx, next_frame);
            } else {
                self.last_desktop_tank_drive_at = None;
            }
        }
        self.ui.handle_event(cx, event, &mut Scope::empty());
        if matches!(event, Event::Startup) {
            self.ensure_network_started(cx);
        }
        self.ensure_activity_announced(cx);
        self.sync_authoritative_activity_pose(cx);
        self.refresh_spawnable_registry(cx, false);
        self.apply_pending_shared_scene_reset(cx);
        self.ensure_local_tank_spawned(cx);
        self.sync_tank_depth_mesh_focus(cx);
        if matches!(event, Event::XrUpdate(_))
            || (matches!(event, Event::NextFrame(_)) && !cx.in_xr_mode())
        {
            self.publish_local_shared_object_states(cx);
        }
        self.refresh_debug_fields(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quat_close(a: Quat, b: Quat, tolerance: f32) -> bool {
        let dot = a.dot(b).abs();
        (1.0 - dot) <= tolerance
    }

    #[test]
    fn quat_to_rot_round_trips_x_then_y_then_z_node_rotations() {
        for rotation in [
            vec3f(0.0, 0.0, 0.0),
            vec3f(0.35, 0.0, 0.0),
            vec3f(0.0, -0.42, 0.0),
            vec3f(0.0, 0.0, 0.61),
            vec3f(0.37, -0.48, 0.29),
            vec3f(-1.10, 0.43, 0.72),
        ] {
            let quat = App::rotation_quat(rotation);
            let recovered = App::quat_to_rot(quat);
            let roundtrip = App::rotation_quat(recovered);
            assert!(
                quat_close(quat, roundtrip, 1.0e-4),
                "wheel/node quaternion conversion should preserve mixed-axis orientation: rotation={rotation:?} recovered={recovered:?} quat={quat:?} roundtrip={roundtrip:?}",
            );
        }
    }
}
