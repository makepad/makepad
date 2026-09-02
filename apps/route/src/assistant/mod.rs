//! Feature-selected assistant controller. The UI is deliberately outside
//! this module so native and demo compile the same widget tree.

use makepad_widgets::{Cx, WidgetRef};

pub trait AssistantController {
    fn configure_ui(&self, cx: &mut Cx, ui: &WidgetRef);
    fn unavailable_reply(&self, prompt: &str) -> Option<&'static str>;
}

#[cfg(feature = "native")]
mod native;
#[cfg(feature = "demo")]
mod unavailable;

#[cfg(feature = "native")]
pub use native::AssistantService;
#[cfg(feature = "demo")]
pub use unavailable::AssistantService;

#[cfg(feature = "demo")]
pub const UNAVAILABLE_REPLY: &str = "assistant is not available in this build";
