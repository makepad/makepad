//! What a browser view's owner and the browser host say to each other, as
//! JSON envelopes on the hosted-app custom channel (`StudioToApp::Custom`
//! down, `AppToStudio::Custom` up). Every message names its view.

use makepad_platform::makepad_micro_serde::*;

/// The registry id of the browser host app.
pub const HOST_APP: &str = "browser_host";

/// The host's environment variable naming the profile folder a browser
/// host keeps its cookies and storage in.
pub const PROFILE_ENV: &str = "MAKEPAD_CEF_PROFILE_DIR";

/// From the owner to the browser host.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub enum ViewOp {
    /// Make the view (a window of the host) and load `url`.
    Open { url: String, dark: bool, background: u32 },
    Navigate { url: String },
    Back,
    Forward,
    Reload,
    Stop,
    /// The page takes (or loses) the keyboard.
    Focus { on: bool },
    /// A hidden page stops painting.
    Hidden { on: bool },
    /// The page's color scheme and the ground under a transparent page.
    Appearance { dark: bool, background: u32 },
    /// Evaluate `expression` in the page; the answer comes back as
    /// `ViewEvent::Evaluated` with the same `id`.
    Eval { id: u64, expression: String },
    /// Stop capturing the page's sound (started by an audio ring setup).
    AudioStop,
    /// CEF-level input, exactly as the page gets it (see `cef_input`): for
    /// an owner that maps its own events (a recorder's locked page, scripted
    /// input) instead of handing the view's hits to its window. Positions
    /// are in the page's points.
    MouseMove { x: i32, y: i32, modifiers: u32, leave: bool },
    MouseClick { x: i32, y: i32, modifiers: u32, button: i32, up: bool, clicks: i32 },
    MouseWheel { x: i32, y: i32, modifiers: u32, dx: i32, dy: i32 },
    Key { kind: i32, modifiers: u32, windows_key_code: i32, native_key_code: i32, character: u16, unmodified_character: u16, system: bool },
    /// Text committed as an input method would.
    Text { text: String },
    /// A pressed button's gesture ends with no release (nothing clicks).
    CaptureLost,
    /// Close the view and its window.
    Close,
}

/// From the browser host to the owner.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub enum ViewEvent {
    /// The view's window: its frames arrive as that hosted window's.
    Window { window_id: usize },
    /// What the chrome shows, and the committed main-frame navigation
    /// (`epoch`, what frames are stamped with too).
    State { title: String, url: String, loading: bool, can_go_back: bool, can_go_forward: bool, epoch: u64 },
    /// The page asked for a new window.
    Popup { url: String },
    /// A line the page wrote to its console.
    Console { level: i32, message: String, source: String, line: i32 },
    /// The answer to `ViewOp::Eval`: the value as JSON, or the exception.
    Evaluated { id: u64, ok: bool, value: String },
    /// The page (or the engine) failed; the view shows nothing more.
    Error { message: String },
    /// The view is gone (after `ViewOp::Close`).
    Closed,
}

#[derive(SerJson, DeJson)]
struct OpEnvelope {
    makepad_browser_view: u64,
    op: ViewOp,
}

#[derive(SerJson, DeJson)]
struct EventEnvelope {
    makepad_browser_event: u64,
    event: ViewEvent,
}

pub fn op_to_json(view: u64, op: &ViewOp) -> String {
    OpEnvelope { makepad_browser_view: view, op: op.clone() }.serialize_json()
}

pub fn parse_op(json: &str) -> Option<(u64, ViewOp)> {
    if !json.contains("\"makepad_browser_view\"") {
        return None;
    }
    OpEnvelope::deserialize_json(json).ok().map(|e| (e.makepad_browser_view, e.op))
}

pub fn event_to_json(view: u64, event: &ViewEvent) -> String {
    EventEnvelope { makepad_browser_event: view, event: event.clone() }.serialize_json()
}

pub fn parse_event(json: &str) -> Option<(u64, ViewEvent)> {
    if !json.contains("\"makepad_browser_event\"") {
        return None;
    }
    EventEnvelope::deserialize_json(json).ok().map(|e| (e.makepad_browser_event, e.event))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ops_round_trip() {
        for op in [
            ViewOp::Open { url: "https://makepad.nl/?a=\"b\"".into(), dark: true, background: 0xff102030 },
            ViewOp::Back,
            ViewOp::Focus { on: true },
            ViewOp::Eval { id: 7, expression: "1 + 1".into() },
            ViewOp::Close,
        ] {
            let json = op_to_json(3, &op);
            assert_eq!(parse_op(&json), Some((3, op)));
            assert_eq!(parse_event(&json), None);
        }
    }

    #[test]
    fn events_round_trip() {
        for event in [
            ViewEvent::Window { window_id: 2 },
            ViewEvent::State { title: "t".into(), url: "u".into(), loading: true, can_go_back: false, can_go_forward: true, epoch: 3 },
            ViewEvent::Evaluated { id: 7, ok: true, value: "2".into() },
            ViewEvent::Closed,
        ] {
            let json = event_to_json(5, &event);
            assert_eq!(parse_event(&json), Some((5, event)));
            assert_eq!(parse_op(&json), None);
        }
    }
}
