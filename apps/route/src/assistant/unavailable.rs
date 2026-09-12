//! No brain linked: the hosted demo and the module a window manager seats
//! in-process. The panel remains interactive — `/tool` commands run
//! against the local registry, a host's assistant reaches the same tools
//! over the bus — but no model, voice runtime, or credential lookup is in
//! this build, and the prompt line says so.

use super::{AssistantController, AssistantEvent, BeginTool, PromptOutcome, UNAVAILABLE_REPLY};
use makepad_widgets::*;

#[derive(Default)]
pub struct AssistantService;

impl AssistantController for AssistantService {
    fn configure_ui(&self, cx: &mut Cx, ui: &WidgetRef) {
        ui.button(cx, ids!(mic_button)).set_disabled(cx, true);
        ui.button(cx, ids!(speaker_button)).set_disabled(cx, true);
    }

    fn unavailable_reply(&self, _prompt: &str) -> Option<&'static str> {
        Some(UNAVAILABLE_REPLY)
    }
}

impl AssistantService {
    pub fn start(&mut self, _cx: &mut Cx) {}

    pub fn status_text(&self) -> String {
        UNAVAILABLE_REPLY.to_string()
    }

    pub fn is_busy(&self) -> bool {
        false
    }

    pub fn send_prompt(&mut self, _cx: &mut Cx, _prompt: &str) -> PromptOutcome {
        PromptOutcome::NoAgent
    }

    pub fn interrupt(&mut self, _cx: &mut Cx) -> bool {
        false
    }

    pub fn send_tool_result(&mut self, _cx: &mut Cx, _tool_use_id: &str, _text: &str, _is_error: bool) {}

    pub fn begin_tool(&mut self, _cx: &mut Cx, _tool_use_id: &str, _name: &str, _input: &str) -> BeginTool {
        BeginTool::Execute
    }

    pub fn start_image_search(
        &mut self,
        _cx: &mut Cx,
        _query: &str,
        _tool_use_id: Option<String>,
    ) -> Result<(), String> {
        Err("image search is not available in this build".to_string())
    }

    pub fn speak(&self, _text: &str) {}

    pub fn stop_speech(&mut self) {}

    pub fn submit_utterance(&mut self, _text: String, _recent: Vec<String>) {}

    pub fn handle_event(&mut self, _cx: &mut Cx, _ui: &WidgetRef, _event: &Event) -> Vec<AssistantEvent> {
        Vec::new()
    }

    pub fn handle_actions(&mut self, _cx: &mut Cx, _ui: &WidgetRef, _actions: &Actions) -> Vec<AssistantEvent> {
        Vec::new()
    }
}
