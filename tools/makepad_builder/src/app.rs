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
                // Plain black like the default terminal, chrome included: the
                // TUI uses the terminal's own colours.
                pass +: { clear_color: #000000 }
                caption_bar +: { draw_bg.color: #000000 }
                body +: {
                    // The terminal's fonts without the colour-emoji member: the
                    // Builder never draws emoji or CJK, so its ZIP ships neither
                    // NotoColorEmoji nor the LXGW WenKai fallback (about 48 MB).
                    term := MpTerm{
                        width: Fill height: Fill
                        draw_text +: {
                            text_style: TextStyle{
                                font_family: FontFamily{
                                    latin := FontMember{res: crate_resource("makepad_widgets:resources/jetbrains_mono_variable.ttf") asc: 0.0 desc: 0.0 weight: 400.0}
                                    icons := FontMember{res: crate_resource("makepad_widgets:resources/fa-solid-900.ttf") asc: 0.0 desc: 0.0}
                                    symbols := FontMember{res: crate_resource("makepad_widgets:resources/Inter.ttf") asc: 0.0 desc: 0.0}
                                }
                                line_spacing: 1.0
                            }
                        }
                        bold_text_style: TextStyle{
                            font_family: FontFamily{
                                latin := FontMember{res: crate_resource("makepad_widgets:resources/jetbrains_mono_variable.ttf") asc: 0.0 desc: 0.0 weight: 800.0}
                                icons := FontMember{res: crate_resource("makepad_widgets:resources/fa-solid-900.ttf") asc: 0.0 desc: 0.0}
                                symbols := FontMember{res: crate_resource("makepad_widgets:resources/Inter.ttf") asc: 0.0 desc: 0.0}
                            }
                            line_spacing: 1.0
                        }
                    }
                }
            }
        }
    }
}

/// `MAKEPAD_TERMINAL_COLORS` for the Builder window (see MpTerm::spawn).
const BUILDER_COLORS: &str = "background=#000000;foreground=#d8d8d8;cursor=#d8d8d8;\
    color0=#000000;color1=#c23621;color2=#25bc24;color3=#adad27;color4=#492ee1;color5=#d338d3;color6=#33bbc8;color7=#d8d8d8;\
    color8=#818383;color9=#fc391f;color10=#31e722;color11=#eaec23;color12=#5833ff;color13=#f935f8;color14=#14f0f0;color15=#f2f2f2";

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
        // The Builder's own palette: black background, light grey text and
        // the macOS Terminal colours, so dim text stays legible. Read by MpTerm
        // when it spawns the session; only this host sets it.
        std::env::set_var("MAKEPAD_TERMINAL_COLORS", BUILDER_COLORS);
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
