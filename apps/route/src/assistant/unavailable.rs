use super::AssistantController;
use makepad_widgets::*;

/// Demo assistant: the native panel remains interactive, but no model,
/// voice runtime, or credential lookup is linked into this profile.
#[derive(Default)]
pub struct AssistantService;

impl AssistantController for AssistantService {
    fn configure_ui(&self, cx: &mut Cx, ui: &WidgetRef) {
        ui.button(cx, ids!(mic_button)).set_disabled(cx, true);
        ui.button(cx, ids!(speaker_button)).set_disabled(cx, true);
    }

    fn unavailable_reply(&self, _prompt: &str) -> Option<&'static str> {
        Some(super::UNAVAILABLE_REPLY)
    }
}
