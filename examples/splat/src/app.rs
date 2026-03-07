use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1400, 900)
                pass.clear_color: #x0d1118
                body +: {
                    scene := Scene3D{
                        width: Fill
                        height: Fill
                        animating: true
                        spin_speed: 0.04
                        camera_fov_y: 46.0
                        camera_distance: 5.6
                        camera_near: 0.02
                        camera_far: 400.0
                        draw_bg +: {
                            color: #x111823
                            draw_depth: -400.0
                        }

                        ground := Grid3D{
                            size: 18.0
                            position: vec3(0.0, -1.25, 0.0)
                            color: vec4(0.56, 0.58, 0.61, 1.0)
                        }

                        helmet := Gltf3D{
                            src: crate_resource("self://../gltf/resources/DamagedHelmet.glb")
                            env_src: crate_resource("self://../gltf/resources/royal_esplanade_4k.jpg")
                            position: vec3(-1.35, -0.1, 0.0)
                            rotation: vec3(0.0, 1.2, 0.0)
                            scale: vec3(0.38, 0.38, 0.38)
                        }

                        sog := ViewSplat{
                            src: crate_resource("self://../../local/toy-cat.sog")
                            position: vec3(-1.35, 0.0, 0.0)
                            scale: vec3(1.0, -1.0, 1.0)
                            normalize_fit: 2.3
                            max_splats: 0
                            radius_scale: 1.1
                            min_radius: 0.0012
                        }

                        ply := ViewSplat{
                            src: crate_resource("self://../../local/biker.ply")
                            position: vec3(1.35, -0.1, 0.0)
                            scale: vec3(1.0, -1.0, 1.0)
                            normalize_fit: 2.0
                            max_splats: 0
                            radius_scale: 1.1
                            min_radius: 0.0012
                        }
                    }
                }
            }
        }
    }
}

impl App {
    fn run(vm: &mut ScriptVm) -> Self {
        println!("{}", std::mem::size_of::<Button>());
        crate::makepad_widgets::script_mod(vm);
        App::from_script_mod(vm, self::script_mod)
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl MatchEvent for App {
    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
