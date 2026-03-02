use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1280, 900)
                pass.clear_color: #x0f141b
                body +: {
                    helmet := Scene3D{
                        width: Fill
                        height: Fill
                        animating: true
                        spin_speed: 0.0
                        camera_fov_y: 42.0
                        camera_distance: 8.0
                        camera_near: 0.05
                        camera_far: 100.0
                        depth_range: vec2(0.0, 1.0)
                        depth_forward_bias: 0.0
                        draw_bg +: {
                            color: #x131922
                            draw_depth: -99.0
                        }
                        model := Gltf3D{
                            src: crate_resource("self://resources/DamagedHelmet.glb")
                            env_src: crate_resource("self://resources/royal_esplanade_4k.jpg")
                        }
                    }
                }
            }
        }
    }
}

impl App {
    fn run(vm: &mut ScriptVm) -> Self {
        crate::makepad_widgets::script_mod(vm);
        App::from_script_mod(vm, self::script_mod)
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust] i:u32
}

impl MatchEvent for App {
    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        println!("QW:KEJRHQWLE:KJRHQWLEKJRHQWELKJRH {}",self.i);
        self.i+=1;
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
