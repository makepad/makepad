//! aichat as a library: the panel widget that owns the engine, the WM-bus
//! client half, the settings, and the module root. A host links this,
//! calls [`script_mod`] after the widgets' own, and puts `AiChatPanel{}`
//! where the chat goes — the standalone binary and the WM pane child do
//! that themselves; a plain `Window` does it for free through its F10
//! slot, which instantiates `mod.widgets.AiChatOverlay{}` by name; the
//! superbuild seats the same overlay in its pane.
//!
//! Linking this crate also gives the bridge its `/ai` routes: `script_mod`
//! installs `Cx::ai_callback`, the way the widgets crate installs the
//! tweaker's, so `/ai?on=1`, `/ai?say=…` and `/ai/transcript` drive and
//! read the chat in a hidden instance.

pub use makepad_widgets;
use makepad_widgets::ai_slot::AiSlotRequests;
use makepad_widgets::makepad_platform::ScriptVmCx;
use makepad_widgets::*;

pub mod bus;
pub mod gen;
pub mod overlay;
pub mod panel;
pub mod settings;

pub use bus::ServiceBus;
pub use overlay::{AiChatOverlay, AiTranscript};
pub use panel::{AiChatPanel, AiChatPanelAction};
pub use settings::AiSettings;

/// Register the panel and the overlay, and give the bridge its `/ai`
/// routes. Call once after `makepad_widgets::script_mod`.
pub fn script_mod(vm: &mut ScriptVm) {
    crate::panel::script_mod(vm);
    crate::overlay::script_mod(vm);
    vm.cx_mut().ai_callback = Some(ai_callback);
}

fn arg<'a>(args: &'a [(String, String)], keys: &[&str]) -> Option<&'a str> {
    for key in keys {
        if let Some((_, value)) = args.iter().find(|(k, _)| k == key) {
            return Some(value.as_str());
        }
    }
    None
}

/// The bridge's `/ai` dispatcher. `toggle` (`on=1|0`, or flip) and `say`
/// (`say=TEXT`, opening the overlay if it is closed) are requests the
/// slot and the overlay take on their next event; `transcript` is what
/// the overlay last published. Never borrows a widget.
fn ai_callback(cx: &mut Cx, op: &str, args: &[(String, String)]) -> Result<String, String> {
    match op {
        "toggle" => {
            let is_open = cx.global::<AiSlotRequests>().is_open;
            let on = match arg(args, &["on"]) {
                Some(value) => !matches!(value, "0" | "false" | "off" | "no"),
                None => !is_open,
            };
            cx.global::<AiSlotRequests>().open = Some(on);
            // The slot takes the request on its next event: make one.
            cx.new_next_frame();
            cx.redraw_all();
            Ok(format!("{{\"on\":{}}}", on as u8))
        }
        "say" => {
            let text = arg(args, &["say", "t"]).unwrap_or("").trim().to_string();
            if text.is_empty() {
                return Err("need say=TEXT".into());
            }
            let req = cx.global::<AiSlotRequests>();
            if !req.is_open {
                req.open = Some(true);
            }
            req.say.push(text);
            cx.new_next_frame();
            cx.redraw_all();
            Ok("{\"ok\":1}".into())
        }
        "transcript" => {
            let json = cx.global::<AiTranscript>().json.clone();
            if json.is_empty() {
                Ok("{\"status\":\"closed\",\"provider\":\"\",\"apps\":[],\"entries\":[],\"generation\":0}".into())
            } else {
                Ok(json)
            }
        }
        other => Err(format!("no ai op `{other}`; there are toggle, say, transcript")),
    }
}
