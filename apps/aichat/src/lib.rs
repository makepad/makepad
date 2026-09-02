//! aichat as a library: the panel widget that owns the engine, the WM-bus
//! client half, and the settings. A host links this, calls
//! [`script_mod`] after the widgets' own, and puts `AiChatPanel{}` where
//! the chat goes — the standalone binary, the WM pane child, the Window
//! overlay and the superbuild all do exactly that.

pub use makepad_widgets;
use makepad_widgets::*;

pub mod bus;
pub mod panel;
pub mod settings;

pub use bus::ServiceBus;
pub use panel::{AiChatPanel, AiChatPanelAction};
pub use settings::AiSettings;

/// Register the panel widget. Call once after `makepad_widgets::script_mod`.
pub fn script_mod(vm: &mut ScriptVm) {
    crate::panel::script_mod(vm);
}
