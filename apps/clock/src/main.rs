//! clock — current local time, a stopwatch and a countdown timer, as a
//! plain full-window Makepad app, part of the Makepad family (wm hosts it
//! as a tile, or in-process as a module; terminal, files and sheets are
//! its siblings).
//!
//! Everything that makes it a clock — the tick and fast timers, the
//! stopwatch and countdown state, the alarm book and its persistence —
//! lives in `ClockView` (`src/view.rs`), and none of it depends on this
//! binary, so `cargo test -p makepad-clock` covers it without a window.
//!
//! clock runs standalone, and unmodified inside makepad-wm / Studio tiles
//! via the shared --stdin-loop client runtime every Makepad app has.
//! Either way it exposes its one bounded read tool to the assistant
//! (src/ai.rs): under the WM over the bus, standalone to the F10 overlay
//! in its own window.

pub use makepad_widgets;
use makepad_ai_services::port::{AiServicePort, PortEvent};
use makepad_clock::{ai, view::ClockView};
use makepad_widgets::desktop_style::{self, DesktopStyle, StyleSheet};
use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Clock" window.inner_size: vec2(420,800)
                pass +: {clear_color: theme.color_bg_app}
                body +: {
                    padding: 0 spacing: 0
                    clock := ClockView{}
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
    /// Dev: the `--switch=<style>` appearance change, applied a moment after
    /// startup the way a host applies one to a running instance.
    #[rust]
    switch: Option<(Timer, String)>,
}

impl App {
    fn ai_summary(&self, cx: &mut Cx) -> String {
        self.ui.widget(cx, ids!(clock)).borrow::<ClockView>().map(|view| view.ai_summary()).unwrap_or_default()
    }

    fn ai_answer(&self, cx: &mut Cx, call: &makepad_ai_services::wire::ServiceCall) -> makepad_ai_services::wire::ToolResult {
        self.ui
            .widget(cx, ids!(clock))
            .borrow::<ClockView>()
            .map(|view| ai::answer(&view, call))
            .unwrap_or_else(|| makepad_ai_services::wire::ToolResult::unavailable(&call.call_id, "the clock is gone"))
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
                    log!("clock: AI service registered as {}", endpoint.as_str());
                    self.ai_context.clear();
                    self.refresh_ai_context(cx);
                }
                PortEvent::Call(call) => {
                    let result = self.ai_answer(cx, &call);
                    if let Some(port) = self.ai_port.as_ref() {
                        port.reply(result);
                    }
                }
                // Nothing here runs long enough to cancel, and the clock has
                // no chat of its own to step aside.
                PortEvent::Cancel { .. } | PortEvent::ChatOpen { .. } => {}
                PortEvent::Subscribe { .. } | PortEvent::Unsubscribe { .. } => {}
            }
        }
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        // Dev flags for looking at the faces in a plain window: `--phone`
        // sizes the window like the phone viewport, `--tile` like the home
        // tile and asks the view for its compact face, the way a host does.
        let args: Vec<String> = std::env::args().collect();
        let window = self.ui.window(cx, ids!(main_window));
        if args.iter().any(|a| a == "--phone" || a == "--tile") {
            // The host gives a face the whole rect; the window's own caption
            // bar would take the top of it.
            let mut win = self.ui.widget(cx, ids!(main_window));
            script_apply_eval!(cx, win, { show_caption_bar: false });
        }
        if args.iter().any(|a| a == "--phone") {
            window.resize(cx, dvec2(402.0, 780.0));
        }
        if let Some(name) = args.iter().find_map(|a| a.strip_prefix("--switch=")) {
            self.switch = Some((cx.start_timeout(2.0), name.to_string()));
        }
        if args.iter().any(|a| a == "--tile") {
            window.resize(cx, dvec2(178.0, 178.0));
            self.ui.widget(cx, ids!(clock)).handle_event(cx, &Event::Custom(HostedViewMode::Tile.to_json()), &mut Scope::empty());
        }
        self.ai_port = AiServicePort::open(cx, ai::manifest());
        makepad_wm_api::set_title(cx, "Clock");
        if let Some(mut view) = self.ui.widget(cx, ids!(clock)).borrow_mut::<ClockView>() {
            view.set_storage(cx.storage("clock"));
        }
    }

    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        makepad_wm_theme::apply(vm);
        // The assistant's panel and overlay root, so the window's F10 slot
        // finds `mod.widgets.AiChatOverlay` by name.
        makepad_aichat::script_mod(vm);
        makepad_clock::face::script_mod(vm);
        makepad_clock::wheel::script_mod(vm);
        makepad_clock::alarm_list::script_mod(vm);
        makepad_clock::digits::script_mod(vm);
        makepad_clock::tabs::script_mod(vm);
        makepad_clock::ring::script_mod(vm);
        makepad_clock::laps::script_mod(vm);
        makepad_clock::view::script_mod(vm);
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
        if let Some((timer, name)) = &self.switch {
            if timer.is_event(event).is_some() {
                // The same receive path a host's appearance change takes.
                if let Some(style) = DesktopStyle::parse(name) {
                    let sheet = StyleSheet::load_with_appearance(style, name.ends_with("-dark"));
                    desktop_style::handle_event(cx, &Event::Custom(sheet.to_json()));
                }
                self.switch = None;
            }
        }
        self.drain_ai_port(cx, event);
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        self.refresh_ai_context(cx);
    }
}

#[cfg(test)]
mod desktop_style_tests {
    include!("../../../widgets/tests/support/app_style.rs");
}
