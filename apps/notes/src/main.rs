//! notes — the standalone window around NotesView.

use makepad_ai_services::port::{AiServicePort, PortEvent};
use makepad_notes::{ai, view::NotesView};
use makepad_strict_json::{self as json, Value};
pub use makepad_widgets;
use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Notes"
                window.inner_size: vec2(1240, 800)
                pass +: {clear_color: theme.color_bg_app}
                body +: {
                    keyboard_resize: true
                    padding: 0
                    margin: 0
                    spacing: 0
                    notes := NotesView{}
                }
            }
        }
    }
}

fn is_close_requested(json: &str) -> bool {
    match json::parse(json.as_bytes()) {
        Ok(Value::Obj(fields)) => fields
            .iter()
            .any(|(k, v)| k == "wm" && v.as_str() == Some("CloseRequested")),
        _ => false,
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
    fn notes(&self, cx: &mut Cx) -> WidgetRef {
        self.ui.widget(cx, ids!(notes))
    }

    fn ai_summary(&self, cx: &mut Cx) -> String {
        self.notes(cx)
            .borrow::<NotesView>()
            .map(|view| view.ai_summary())
            .unwrap_or_default()
    }

    fn ai_answer(
        &self,
        cx: &mut Cx,
        call: &makepad_ai_services::wire::ServiceCall,
    ) -> makepad_ai_services::wire::ToolResult {
        self.notes(cx)
            .borrow::<NotesView>()
            .map(|view| view.ai_answer(call))
            .unwrap_or_else(|| {
                makepad_ai_services::wire::ToolResult::unavailable(
                    &call.call_id,
                    "the notes app is gone",
                )
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
                    log!("notes: AI service registered as {}", endpoint.as_str());
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
        if let Some(mut view) = self.notes(cx).borrow_mut::<NotesView>() {
            view.set_storage(cx.storage("notes"));
        }
    }

    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        makepad_notes::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Custom(json) = event {
            if is_close_requested(json) {
                if let Some(mut view) = self.notes(cx).borrow_mut::<NotesView>() {
                    if view.request_close(cx) {
                        cx.quit();
                    }
                    return;
                }
                cx.quit();
                return;
            }
        }
        self.drain_ai_port(cx, event);
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        if let Some(mut view) = self.notes(cx).borrow_mut::<NotesView>() {
            if view.take_quit() {
                cx.quit();
            }
        }
        self.refresh_ai_context(cx);
    }
}
