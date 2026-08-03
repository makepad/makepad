// Makepad Arcade — networked AI game sandbox. Plan: repo-root game.md.
pub use makepad_widgets;

pub mod arcade_view;
pub mod capability;
pub mod library;
pub mod pairing;
pub mod pair_server;
pub mod ai;
pub mod coedit;
pub mod intent;
pub mod settings;
pub mod browser;
pub mod xr_input;

use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.ArcadeView
    use mod.widgets.ArcadeSettings
    use mod.widgets.ArcadeBrowser

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
                            games_button := Button { text: "Games" }
                            settings_button := Button { text: "Settings" }
                        }

                        // Hidden until asked for: this is a game, not a
                        // control panel.
                        settings_panel := ArcadeSettings { visible: false }
                        browser_panel := ArcadeBrowser { visible: false }

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
    #[rust]
    browser_open: bool,
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
        if self.ui.button(cx, ids!(games_button)).clicked(actions) {
            self.browser_open = !self.browser_open;
            self.ui
                .widget(cx, ids!(browser_panel))
                .set_visible(cx, self.browser_open);
            self.ui.redraw(cx);
        }

        let browser = self.ui.widget(cx, ids!(browser_panel));
        let action = match browser.borrow_mut::<crate::browser::ArcadeBrowser>() {
            Some(mut b) => b.handle_actions(cx, actions),
            None => crate::browser::BrowserAction::None,
        };
        if let crate::browser::BrowserAction::Play(slug) = action {
            // Anything the browser lists was installed from a package, so it
            // runs sandboxed — a game from a stranger is untrusted code.
            let path = crate::browser::games_root()
                .join(&slug)
                .join(makepad_game_pkg::GAME_FILE);
            let view = self.ui.widget(cx, ids!(arcade_view));
            let err = view
                .borrow_mut::<crate::arcade_view::ArcadeView>()
                .and_then(|mut v| {
                    v.load_game_with_trust(cx, &path, makepad_game_script::Trust::Downloaded)
                });
            if let Some(err) = err {
                log!("arcade: {slug} failed to load: {err}");
            }
            self.browser_open = false;
            self.ui
                .widget(cx, ids!(browser_panel))
                .set_visible(cx, false);
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
        crate::browser::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
