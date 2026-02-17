use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1400, 900)
                body +: {
                    View{
                        width: Fill
                        height: Fill
                        flow: Down
                        View{
                            width: Fill
                            height: 220
                            flow: Right
                            candlestick := CandlestickChart{
                                width: Fill
                                height: Fill
                            }
                            ohlc := OhlcChart{
                                width: Fill
                                height: Fill
                            }
                        }
                        View{
                            width: Fill
                            height: 220
                            flow: Right
                            line := LineChart{
                                width: Fill
                                height: Fill
                            }
                            area := AreaChart{
                                width: Fill
                                height: Fill
                            }
                        }
                        View{
                            width: Fill
                            height: Fill
                            chart3d := Chart3D{
                                width: Fill
                                height: Fill
                                animating: false
                                camera_distance: 12.0
                                camera_distance_min: 8.0
                                camera_distance_max: 30.0
                                camera_fov_y: 36.0
                                camera_target: vec3(0.0, 1.8, 0.0)
                                BarChart3D{
                                    base_y: -0.01
                                    spacing: 1.0
                                    bar_size: 0.34
                                    height_scale: 2.8
                                    show_axes: false
                                    show_labels: false
                                    billboard_labels: true
                                }
                            }
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
