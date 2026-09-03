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
//! via the shared --stdin-loop client runtime every Makepad app has. Either
//! way it exposes bounded read and write tools to the assistant (src/ai.rs): under the WM
//! over the bus, standalone to the F10 overlay in its own window.

pub use makepad_widgets;
use makepad_ai_services::port::{AiServicePort, PortEvent};
use makepad_sheets::{ai, theme, view};
use makepad_widgets::*;

app_main!(
    App,
    font_assets: ["makepad_widgets/resources/jetbrains_mono_variable.ttf"]
);

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
    /// The app's service toward the assistant: the WM's bus when hosted,
    /// the window's own F10 overlay when standalone.
    #[rust]
    ai_port: Option<AiServicePort>,
    /// Last summary sent as volatile context, so unchanged events stay quiet.
    #[rust]
    ai_context: String,
}

impl App {
    /// The sheet facts used by both `summary` and the volatile context line.
    fn ai_summary(&self, cx: &mut Cx) -> String {
        self.ui
            .widget(cx, ids!(sheets))
            .borrow::<view::MpSheets>()
            .map(|sheets| sheets.ai_summary(cx))
            .unwrap_or_else(|| "no sheet is open".to_string())
    }

    fn ai_answer(&self, cx: &mut Cx, call: &makepad_ai_services::wire::ServiceCall) -> makepad_ai_services::wire::ToolResult {
        self.ui
            .widget(cx, ids!(sheets))
            .borrow_mut::<view::MpSheets>()
            .map(|mut sheets| sheets.ai_answer(cx, call))
            .unwrap_or_else(|| makepad_ai_services::wire::ToolResult::failed(&call.call_id, "no sheet is open"))
    }

    fn refresh_ai_context(&mut self, cx: &mut Cx) {
        if self.ai_port.is_none() {
            return;
        }
        let text = self.ai_summary(cx);
        if text == self.ai_context {
            return;
        }
        self.ai_context = text.clone();
        if let Some(port) = self.ai_port.as_ref() {
            port.set_context(&text);
        }
    }

    fn drain_ai_port(&mut self, cx: &mut Cx, event: &Event) {
        let events = match self.ai_port.as_mut() {
            Some(port) => port.handle_event(cx, event),
            None => return,
        };
        for ev in events {
            match ev {
                PortEvent::Registered(endpoint) => {
                    log!("sheets: AI service registered as {}", endpoint.as_str());
                    self.ai_context.clear();
                    self.refresh_ai_context(cx);
                }
                PortEvent::Call(call) => {
                    let result = self.ai_answer(cx, &call);
                    if let Some(port) = self.ai_port.as_ref() {
                        port.reply(result);
                    }
                }
                // Nothing here runs long enough to cancel, and the sheet has
                // no chat of its own to step aside.
                PortEvent::Cancel { .. } | PortEvent::ChatOpen { .. } => {}
            }
        }
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.ai_port = AiServicePort::open(cx, ai::manifest());
        makepad_wm_api::set_title(cx, "Sheets");
    }

    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        // Retint the stock widgets from the WM palette, then publish the same
        // palette as `mod.sheets` for this app's own splash.
        makepad_wm_theme::apply(vm);
        // The assistant's panel and overlay root, so the window's F10 slot
        // finds `mod.widgets.AiChatOverlay` by name.
        makepad_aichat::script_mod(vm);
        theme::install(vm);
        view::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        // The window manager asked politely (SUPER+W): go now.
        if let Event::Custom(json) = event {
            if let Some(makepad_wm_api::WmEvent::CloseRequested) = makepad_wm_api::WmEvent::parse(json) {
                cx.quit();
                return;
            }
        }
        self.drain_ai_port(cx, event);
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        // Selection and active-sheet changes are observed inside MpSheets;
        // comparing after dispatch publishes only genuinely changed facts.
        self.refresh_ai_context(cx);
    }
}
