pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1280, 840)
                pass.clear_color: vec4(0.08, 0.10, 0.12, 1.0)
                body +: {
                    map := MapView{
                        width: Fill
                        height: Fill
                    }
                }
            }
        }
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
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
