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
                            height: Fill
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
                            height: Fill
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
                            flow: Right
                            bar := BarChart{
                                width: Fill
                                height: Fill
                            }
                            scatter := ScatterChart{
                                width: Fill
                                height: Fill
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

#[derive(Script)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl ScriptHook for App {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        vm.set_ui(&self.ui);
    }
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
