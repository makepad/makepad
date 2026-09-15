//! files — the file browser as a plain full-window Makepad app, part of the
//! Makepad family (wm hosts it as a tile, or in-process as a module).
//!
//! Everything that makes it a file browser lives in `FilesView`
//! (`src/view.rs`). The standalone binary is a `Window` around that view.

pub use makepad_widgets;
use makepad_files::view::FilesView;
use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Files"
                window.inner_size: vec2(1240, 800)
                pass.clear_color: mod.mpf.bg
                body +: {
                    padding: 0 spacing: 0
                    files := FilesView{}
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
        makepad_wm_api::set_title(cx, "Files");
        if let Some(mut view) = self.ui.widget(cx, ids!(files)).borrow_mut::<FilesView>() {
            view.ensure_started(cx);
        }
    }

    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        makepad_wm_theme::apply(vm);
        makepad_files::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Custom(json) = event {
            if let Some(makepad_wm_api::WmEvent::CloseRequested) = makepad_wm_api::WmEvent::parse(json) {
                cx.quit();
                return;
            }
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

#[cfg(test)]
mod desktop_style_tests {
    include!("../../../widgets/tests/support/app_style.rs");
}
