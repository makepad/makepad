//! Host-only scene graph for the streamed xr_remote path.
//!
//! Quest no longer renders a mirrored local-scene fallback. This module exists
//! only so the Mac host can render the test/tree scenes for the encoded stream.

use crate::protocol::RenderStatePacket;
use makepad_widgets::*;
use makepad_xr::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

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

    mod.widgets.XrRemoteHostScene = XrNode{
        host_scene_select := XrSelect{
            pos: vec3(0.0, -0.02, -0.62)
            scale: vec3(0.5, 0.5, 0.5)
            active_child: @test_scene

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
        }
    }
}

pub fn apply_host_scene_state(content: WidgetRef, cx: &mut Cx, render_state: &RenderStatePacket) {
    if let Some(mut select) = content
        .widget(cx, ids!(host_scene_select))
        .borrow_mut::<XrSelect>()
    {
        let _ = select.set_active_child(cx, render_state.scene.live_id());
    }
    content.redraw(cx);
}
