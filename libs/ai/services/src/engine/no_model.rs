//! The models that answer with nothing. Outside the `engine` feature on
//! purpose: a build without a model runtime (the web page, a host that
//! only wants the tool console) still needs a `Model` to hand the core,
//! and these are it.

use crate::engine::{Model, ModelEvent, ToolDefinition};

/// Nothing answers. Sends fail at once with a plain message; the tool
/// console still works because it never touches the model.
pub struct NoModel;

impl Model for NoModel {
    fn label(&self) -> String {
        "No model".into()
    }
    fn configure(&mut self, _system: &str, _tools: &[ToolDefinition]) -> Result<(), String> {
        Ok(())
    }
    fn send_user(&mut self, _text: &str, _dynamic_context: &str) {}
    fn send_tool_result(&mut self, _call_id: &str, _text: &str, _is_error: bool) {}
    fn cancel(&mut self) {}
    fn reset(&mut self) {}
    fn poll(&mut self) -> Vec<ModelEvent> {
        Vec::new()
    }
}

/// `NoModel` that answers every send with the reason, so the transcript
/// says why nothing happened.
pub struct NoModelWithReason {
    reason: String,
    queued: Vec<ModelEvent>,
}

impl NoModelWithReason {
    pub fn new(reason: impl Into<String>) -> Self {
        NoModelWithReason { reason: reason.into(), queued: Vec::new() }
    }
}

impl Model for NoModelWithReason {
    fn label(&self) -> String {
        "No model".into()
    }
    fn configure(&mut self, _system: &str, _tools: &[ToolDefinition]) -> Result<(), String> {
        Ok(())
    }
    fn send_user(&mut self, _text: &str, _dynamic_context: &str) {
        self.queued.push(ModelEvent::Error(format!(
            "no model is answering ({}). The tools still work from the console: /name {{json}}",
            self.reason
        )));
    }
    fn send_tool_result(&mut self, _call_id: &str, _text: &str, _is_error: bool) {}
    fn cancel(&mut self) {}
    fn reset(&mut self) {}
    fn poll(&mut self) -> Vec<ModelEvent> {
        std::mem::take(&mut self.queued)
    }
}

/// Claude Desktop answers, not this panel: the conversation happens there,
/// through the app's MCP endpoint, and its calls land here as cards. A line
/// typed here is told so; the tool console still works.
pub struct ClaudeDesktopModel {
    queued: Vec<ModelEvent>,
}

impl ClaudeDesktopModel {
    pub fn new() -> Self {
        ClaudeDesktopModel { queued: Vec::new() }
    }
}

impl Model for ClaudeDesktopModel {
    fn label(&self) -> String {
        "Claude Desktop".into()
    }
    fn configure(&mut self, _system: &str, _tools: &[ToolDefinition]) -> Result<(), String> {
        Ok(())
    }
    fn send_user(&mut self, _text: &str, _dynamic_context: &str) {
        self.queued.push(ModelEvent::Error(
            "Claude Desktop answers for this app: type in Claude Desktop; its calls show here. \
             The tools still work from this console: /name {json}"
                .into(),
        ));
    }
    fn send_tool_result(&mut self, _call_id: &str, _text: &str, _is_error: bool) {}
    fn cancel(&mut self) {}
    fn reset(&mut self) {}
    fn poll(&mut self) -> Vec<ModelEvent> {
        std::mem::take(&mut self.queued)
    }
}
