// Makepad Arcade — networked AI game sandbox. Plan: repo-root game.md.
pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1000, 700)
                body +: {
                    View{
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 12
                        align: Center

                        Label{
                            text: "Makepad Arcade"
                            draw_text.text_style.font_size: 32
                        }
                        status := Label{
                            text: "M0 — engine extraction in progress (game.md)"
                        }
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
    fn handle_startup(&mut self, cx: &mut Cx) {
        let status = format!(
            "M0 — sim core online: tick {}Hz, deterministic math kernel linked",
            makepad_game_sim::TICK_HZ
        );
        self.ui.label(cx, ids!(status)).set_text(cx, &status);
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
