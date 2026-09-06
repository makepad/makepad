use super::AssistantController;
use makepad_widgets::{Cx, WidgetRef};

/// Native keeps the existing Claude/local-LLM/voice controller in `App`.
/// This seam owns the profile-specific availability policy used by the
/// shared transcript/input/mic/speaker chrome.
#[derive(Default)]
pub struct AssistantService;

impl AssistantController for AssistantService {
    fn configure_ui(&self, _cx: &mut Cx, _ui: &WidgetRef) {}

    fn unavailable_reply(&self, _prompt: &str) -> Option<&'static str> {
        None
    }
}
