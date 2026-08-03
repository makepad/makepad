// Makepad Arcade — networked AI game sandbox. Plan: repo-root game.md.
pub use makepad_widgets;

pub mod arcade_view;
pub mod capability;
pub mod library;
pub mod pairing;
pub mod pair_server;
pub mod ai;
pub mod intent;
pub mod settings;
pub mod xr_input;

use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.ArcadeView
    use mod.widgets.ArcadeSettings

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1000, 700)
                body +: {
                    View {
                        width: Fill
                        height: Fill
                        flow: Down

                        View {
                            width: Fill
                            height: Fit
                            flow: Right
                            padding: 6
                            spacing: 8
                            align: Align{y: 0.5}

                            View { width: Fill height: 1 }
                            settings_button := Button { text: "Settings" }
                        }

                        // Hidden until asked for: this is a game, not a
                        // control panel.
                        settings_panel := ArcadeSettings { visible: false }

                        arcade_view := ArcadeView{}
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
    #[rust]
    settings_open: bool,
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(settings_button)).clicked(actions) {
            self.settings_open = !self.settings_open;
            self.ui
                .widget(cx, ids!(settings_panel))
                .set_visible(cx, self.settings_open);
            self.ui.redraw(cx);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        makepad_game_render::script_mod(vm);
        crate::arcade_view::script_mod(vm);
        crate::settings::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
