//! Windows' portable entry point hosts the real terminal installer and its
//! child shells/agents in MpTerm. Setup never runs on the rendering thread.

use makepad_terminal::widget::{MpTerm, MpTermAction};
pub use makepad_widgets;
use makepad_widgets::*;

app_main!(App, font_assets: ["makepad_widgets/resources/jetbrains_mono_variable.ttf", INTER_FONT_ASSET]);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1000, 740)
                window.title: "Makepad Builder"
                body +: {
                    term := MpTerm{width: Fill height: Fill}
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
    child_timer: Timer,
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.child_timer = cx.start_interval(0.2);
        let executable = std::env::current_exe().expect("Builder executable location");
        // Diagnostic transfers use the same ConPTY and renderer as setup, in
        // an independently owned instance, without touching an installation.
        let mut args = std::env::args();
        let probe = args.any(|arg| arg == "--probe-download").then(|| args.next()).flatten();
        let action = if let Some(url) = probe.filter(|s| s.starts_with("https://") && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._~:/".contains(&b))) {
            format!("download-probe {url} --tui")
        } else { "tui".to_owned() };
        let command = if cfg!(windows) {
            format!("\"{}\" {action}", executable.display())
        } else {
            format!(
                "exec {} {action}",
                makepad_loader::runtime::shell(&executable.to_string_lossy())
            )
        };
        if let Some(mut terminal) = self.ui.widget(cx, ids!(term)).borrow_mut::<MpTerm>() {
            terminal.cwd = std::env::current_dir().ok();
            terminal.command = Some(command);
        }
    }
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let uid = self.ui.widget(cx, ids!(term)).widget_uid();
        for action in actions.filter_widget_actions_cast::<MpTermAction>(uid) {
            if matches!(action, MpTermAction::Exited) {
                cx.quit();
            }
        }
    }
    fn handle_shutdown(&mut self, cx: &mut Cx) {
        if let Some(mut terminal) = self.ui.widget(cx, ids!(term)).borrow_mut::<MpTerm>() {
            terminal.unload(cx);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        makepad_terminal::widget::script_mod(vm);
        self::script_mod(vm)
    }
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if self.child_timer.is_event(event).is_some() {
            if self
                .ui
                .widget(cx, ids!(term))
                .borrow_mut::<MpTerm>()
                .is_some_and(|mut term| term.process_exited())
            {
                cx.quit();
            }
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

// Keep the macro-generated desktop entry point reachable inside this module.
pub fn run() { main(); }
