//! calculator — the standalone window.

pub use makepad_widgets;
use makepad_ai_services::port::{AiServicePort, PortEvent};
use makepad_calculator::{ai, view::CalculatorView};
use makepad_strict_json as json;
use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Calculator"
                window.inner_size: vec2(1240, 800)
                pass +: {clear_color: theme.color_bg_app}
                body +: {
                    padding: 0 margin: 0 spacing: 0
                    calculator := CalculatorView{}
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
    ai_port: Option<AiServicePort>,
    #[rust]
    ai_context: String,
}

impl App {
    fn ai_summary(&self, cx: &mut Cx) -> String {
        self.ui
            .widget(cx, ids!(calculator))
            .borrow::<CalculatorView>()
            .map(|view| view.ai_summary())
            .unwrap_or_default()
    }

    fn ai_answer(
        &self,
        cx: &mut Cx,
        call: &makepad_ai_services::wire::ServiceCall,
    ) -> makepad_ai_services::wire::ToolResult {
        self.ui
            .widget(cx, ids!(calculator))
            .borrow::<CalculatorView>()
            .map(|view| ai::answer(view.doc(), call))
            .unwrap_or_else(|| {
                makepad_ai_services::wire::ToolResult::unavailable(&call.call_id, "the calculator is gone")
            })
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
                    log!("calculator: AI service registered as {}", endpoint.as_str());
                    self.ai_context.clear();
                    self.refresh_ai_context(cx);
                }
                PortEvent::Call(call) => {
                    let result = self.ai_answer(cx, &call);
                    if let Some(port) = self.ai_port.as_ref() {
                        port.reply(result);
                    }
                }
                PortEvent::Cancel { .. } | PortEvent::ChatOpen { .. } => {}
                PortEvent::Subscribe { .. } | PortEvent::Unsubscribe { .. } => {}
            }
        }
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.ai_port = AiServicePort::open(cx, ai::manifest());
        if let Some(mut view) = self.ui.widget(cx, ids!(calculator)).borrow_mut::<CalculatorView>() {
            view.set_storage(cx.storage("calculator"));
        }
    }

    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        makepad_wm_theme::apply(vm);
        makepad_calculator::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Custom(json) = event {
            if is_close_requested(json) {
                let proceed = self
                    .ui
                    .widget(cx, ids!(calculator))
                    .borrow_mut::<CalculatorView>()
                    .map(|mut view| view.handle_close(cx))
                    .unwrap_or(true);
                if proceed {
                    cx.quit();
                }
                return;
            }
        }
        if let Event::WindowCloseRequested(ev) = event {
            let proceed = self
                .ui
                .widget(cx, ids!(calculator))
                .borrow_mut::<CalculatorView>()
                .map(|mut view| view.handle_close(cx))
                .unwrap_or(true);
            if !proceed {
                ev.accept_close.set(false);
            }
        }
        self.drain_ai_port(cx, event);
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        self.refresh_ai_context(cx);
    }
}

fn is_close_requested(raw: &str) -> bool {
    if !raw.contains("CloseRequested") {
        return false;
    }
    let Ok(value) = json::parse(raw.as_bytes()) else {
        return false;
    };
    value.get("wm").and_then(|wm| wm.get("CloseRequested")).is_some()
}
