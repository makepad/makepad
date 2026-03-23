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
        ui:  XrRoot{
            window.inner_size: vec2(1400, 900)
            pass.clear_color: #x0b1118
            camera.fov_y: 52.0
            camera.distance: 2.8
            env.gravity: 9.8
            env.env_cube: false
            env.depth_mesh: false

            control_strip := XrView{
                pos: vec3(0.0, 0.62, -0.86)
                logical_size: vec2(560, 92)
                pixel_scale: 0.0009
                dpi_factor: 2.0
                RoundedView{
                    width: Fill
                    height: Fill
                    flow: Right
                    padding: 16
                    spacing: 12
                    align: Align{y: 0.5}
                    draw_bg.color: #x162331ee
                    draw_bg.border_radius: 18.0

                    title := Label{
                        text: "XR Preview"
                        draw_text.color: #xeff7ff
                        draw_text.text_style.font_size: 18.0
                    }

                    detail := Label{
                        width: Fill
                        text: "Forward-facing fake headset view with a 3D-hosted UI strip."
                        draw_text.color: #xb8c8d8
                    }

                    depth_toggle_button := Button{
                        width: 160
                        text: "Toggle Depth Mesh"
                        on_press: || ui.depth_toggle()
                    }
                }
            }

            platform := Platform{
                size: vec3(2.2, 0.08, 1.08)
                corner_radius: 0.03
                color: #x2d455d
                pos: vec3(0.0, -0.48, -1.42)
            }

            back_wall := Cube{
                body: mod.widgets.XrBodyKind.Fixed
                size: vec3(2.4, 1.3, 0.08)
                corner_radius: 0.05
                roughness: 0.88
                metallic: 0.0
                color: #x152538
                pos: vec3(0.0, 0.12, -2.12)
            }

            left_stack := Block{
                body: mod.widgets.XrBodyKind.Fixed
                color: #xff6a4d
                size: vec3(0.24, 0.24, 0.24)
                pos: vec3(-0.72, -0.20, -1.18)
            }

            center_stack := Block{
                body: mod.widgets.XrBodyKind.Fixed
                color: #x58d68d
                size: vec3(0.26, 0.36, 0.26)
                pos: vec3(0.0, -0.14, -1.34)
            }

            right_stack := Block{
                body: mod.widgets.XrBodyKind.Fixed
                color: #x68a8ff
                size: vec3(0.24, 0.24, 0.24)
                pos: vec3(0.74, -0.20, -1.18)
            }

            accent_cube := Cube{
                body: mod.widgets.XrBodyKind.Fixed
                size: vec3(0.20, 0.20, 0.20)
                corner_radius: 0.02
                roughness: 0.12
                metallic: 0.04
                color: #xffff7a
                pos: vec3(0.0, 0.34, -1.12)
            }
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
