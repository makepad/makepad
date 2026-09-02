//! sheets — a spreadsheet as a plain full-window Makepad app, part of the
//! Makepad family (wm hosts it as a tile; terminal, browser, files and task
//! are its siblings).
//!
//! The grid is the stock `DataGrid` widget. Everything that makes it a
//! spreadsheet — the formula engine, the dependency graph, cell formats, undo,
//! CSV — lives in this crate, and none of it depends on Makepad, so
//! `cargo test -p sheets` covers it without a window.
//!
//! sheets runs standalone, and unmodified inside makepad-wm / Studio tiles
//! via the shared --stdin-loop client runtime every Makepad app has.

pub use makepad_widgets;
use makepad_widgets::*;

mod formula;
mod docs;
mod sheet;
mod theme;
mod view;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1320, 840)
                window.title: "sheets"
                // The stock window clears to `theme.color_bg_app` as captured
                // when the widgets module was evaluated — before this app
                // retinted it — so state the sheet's own ground explicitly.
                pass +: { clear_color: mod.sheets.bg }
                body +: {
                    padding: 0.
                    margin: 0.
                    spacing: 0.
                    sheets := MpSheets{}
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
        // Retint the stock widgets from the WM palette, then publish the same
        // palette as `mod.sheets` for this app's own splash.
        makepad_wm_theme::apply(vm);
        crate::theme::install(vm);
        crate::view::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
