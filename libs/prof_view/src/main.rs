//! The profile view in a window of its own: it lists the running makepad apps (`makepad run`
//! apps and native builds with their remote endpoint on) and the saved recordings; a click
//! attaches. devhub opens the same widget from its tray menu and from a JIT row's Profile action.
//! Run it with `makepad run makepad-prof-view`.

pub use makepad_widgets;
use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Profile"
                window.inner_size: vec2(1280, 800)
                body +: {
                    prof := mod.widgets.ProfView{}
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

impl MatchEvent for App {}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        makepad_prof_view::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
