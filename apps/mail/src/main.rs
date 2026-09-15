//! Mail — a local demo mailbox as a standalone Makepad window.

pub use makepad_widgets;
use makepad_ai_services::port::{AiServicePort, PortEvent};
use makepad_mail::{ai, view::MailView};
use makepad_strict_json::Value;
use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Mail"
                window.inner_size: vec2(1240, 800)
                pass +: { clear_color: theme.color_bg_app }
                body +: {
                    padding: 0 margin: 0 spacing: 0
                    mail := MailView{}
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
    #[rust]
    closing: bool,
}

impl App {
    fn ai_summary(&self, cx: &mut Cx) -> String {
        self.ui
            .widget(cx, ids!(mail))
            .borrow::<MailView>()
            .map(|view| view.ai_summary())
            .unwrap_or_default()
    }

    fn ai_answer(&self, cx: &mut Cx, call: &makepad_ai_services::wire::ServiceCall) -> makepad_ai_services::wire::ToolResult {
        self.ui
            .widget(cx, ids!(mail))
            .borrow::<MailView>()
            .map(|view| view.ai_answer(call))
            .unwrap_or_else(|| {
                makepad_ai_services::wire::ToolResult::unavailable(&call.call_id, "mail is gone")
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
                    log!("mail: AI service registered as {}", endpoint.as_str());
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

fn is_close_requested(json: &str) -> bool {
    let Ok(value) = makepad_strict_json::parse(json.as_bytes()) else {
        return false;
    };
    let Some(wm) = value.get("wm") else {
        return false;
    };
    match wm {
        Value::Obj(pairs) => pairs.iter().any(|(k, v)| {
            k == "CloseRequested" && matches!(v, Value::Arr(items) if items.is_empty())
        }),
        _ => false,
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.ai_port = AiServicePort::open(cx, ai::manifest());
        if let Some(mut view) = self.ui.widget(cx, ids!(mail)).borrow_mut::<MailView>() {
            view.set_storage(cx.storage("mail"));
        }
    }

    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        makepad_wm_theme::apply(vm);
        makepad_mail::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Custom(json) = event {
            if is_close_requested(json) {
                let ready = self
                    .ui
                    .widget(cx, ids!(mail))
                    .borrow_mut::<MailView>()
                    .map(|mut view| view.request_close(cx))
                    .unwrap_or(true);
                if ready {
                    cx.quit();
                } else {
                    self.closing = true;
                }
                return;
            }
        }
        self.drain_ai_port(cx, event);
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        if self.closing {
            let quit = self
                .ui
                .widget(cx, ids!(mail))
                .borrow::<MailView>()
                .map(|view| view.should_quit())
                .unwrap_or(true);
            if quit {
                cx.quit();
            }
        }
        self.refresh_ai_context(cx);
    }
}
