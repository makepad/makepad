//! Personal finance on Makepad: a ledger, budgets, reports and CSV import
//! over a SQLite file.
//!
//! One window that is a desktop app when it is wide and a phone app when
//! it is narrow. Run it from the repo root — the file lives at
//! `local/finance/finance.db`, and a first run fills it with a generated
//! household so there is something to click.

#![allow(dead_code)] // ledger, import and report surface built ahead of the views that use it
pub use ::makepad_widgets;

use makepad_widgets::*;

mod chart;
mod csv;
mod date;
mod db;
mod import;
mod model;
mod money;
mod report;
mod seed;
mod theme;
mod view;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1440, 900)
                pass.clear_color: vec4(0.051, 0.067, 0.09, 1.0)
                body +: {
                    Finance{}
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
        mp_theme::apply(vm);
        crate::theme::install(vm);
        crate::chart::script_mod(vm);
        crate::view::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
