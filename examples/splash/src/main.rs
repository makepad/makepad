pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    startup() do #(App::script_component(vm)) {
        ui: Root {
            main_window := Window {
                body +: {
                    View {
                        width: Fill
                        height: Fill
                        show_bg: true
                        draw_bg +: {
                            line_a: instance(0.0)
                            line_b: instance(0.0)

                            pixel: fn() {
                                let p = self.pos
                                let c = 0.5 + 0.5 * sin(self.time * 1.0 + p.yx * 10.0)
                                let color = #f04.mix(#0af, c.x)
                                return Pal.premul(vec4(color.rgb, 1.0))
                            }
                        }
                    }
                }
            }
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live] ui: WidgetRef,
}

impl MatchEvent for App {
    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {
        
    }
}
