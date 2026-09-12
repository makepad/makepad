//! finance — personal finance on Makepad, as a plain full-window app: one
//! window that is a desktop app when it is wide and a phone app when it is
//! narrow.
//!
//! The ledger, budgets, reports, charts and CSV import all live in the
//! library crate (`makepad_finance`), around one root widget
//! ([`makepad_finance::view::Finance`]); this binary is just a `Window`
//! around it, plus the one thing only a checkout-relative run needs: the
//! standalone database path. Run from the repo root and the file lives at
//! `local/finance/finance.db` — a first run fills it with a generated
//! household so there is something to click. The same crate's module
//! (`makepad_finance::module`) seats the same root in-process, one
//! instance per isolate, with the database under the shared makepad home
//! instead (see `Finance::set_db_path`).

pub use makepad_widgets;

use makepad_finance::{chart, theme, view};
use makepad_widgets::*;

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
                    finance := Finance{}
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
        // Only the standalone window, run from a checkout, uses this path;
        // every other host (the module) keeps `Finance`'s own default —
        // the makepad home's `finance/finance.db` — set in `module.rs`.
        if let Some(mut finance) = self.ui.widget(cx, ids!(finance)).borrow_mut::<view::Finance>() {
            finance.set_db_path(std::path::PathBuf::from("local/finance/finance.db"));
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        makepad_wm_theme::apply(vm);
        theme::install(vm);
        chart::script_mod(vm);
        view::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

#[cfg(test)]
mod desktop_style_tests {
    include!("../../../widgets/tests/support/app_style.rs");
}
