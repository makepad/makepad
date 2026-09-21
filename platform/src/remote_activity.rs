//! Native interaction tracking for the standalone remote interface. The
//! ledger belongs to the `Cx` (`Cx::remote_activity`) and is only touched on
//! the UI thread; the HTTP threads see one number, the user sequence, through
//! the handle the remote service takes when it starts.

use crate::cx::Cx;
use crate::cx_api::CxOsApi;
use crate::event::{Event, KeyCode, MouseButton, PinchPhase, TouchState};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const QUIET_PERIOD: Duration = Duration::from_secs(2);

/// What the person has done to this instance natively, and what remote input
/// is in flight. Owned by the `Cx`, so an instance's bookkeeping lives and
/// dies with it.
#[derive(Default)]
pub(crate) struct RemoteActivity {
    last_input: Option<Instant>,
    kind: &'static str,
    window: Option<usize>,
    buttons: Vec<(usize, u32)>,
    keys: Vec<KeyCode>,
    touches: Vec<(usize, u64)>,
    dragging: bool,
    /// A native trackpad pinch between its Begin and End phases.
    pinching: bool,
    remote_buttons: Vec<MouseButton>,
    remote_keys: Vec<KeyCode>,
    /// Advances on every native input; the remote service shares this
    /// handle with its request threads for headers and conflict checks.
    seq: Arc<AtomicU64>,
    /// True while an injected event is being dispatched (see
    /// `remote::remote_input_scope`); shared so the scope guard needs no
    /// borrow of the `Cx` across the dispatch.
    pub(crate) remote_input: Rc<Cell<bool>>,
}

impl RemoteActivity {
    pub(crate) fn user_seq(&self) -> u64 {
        self.seq.load(Ordering::Acquire)
    }

    /// The counter the HTTP threads read.
    pub(crate) fn seq_handle(&self) -> Arc<AtomicU64> {
        self.seq.clone()
    }

    /// Pointer, touch, drag and pinch input has a release the window server
    /// delivers. Keys do not count: a modifier released while a menu or a
    /// panel is key never reaches the app, and a key ledger then blocked every
    /// remote request until the window lost focus. Typing still counts as
    /// activity through the quiet period.
    fn held(&self) -> bool {
        !self.buttons.is_empty()
            || !self.touches.is_empty()
            || self.dragging
            || self.pinching
    }

    fn active(&self) -> bool {
        self.held()
            || self
                .last_input
                .is_some_and(|time| time.elapsed() < QUIET_PERIOD)
    }

    fn json(&self) -> String {
        let idle_ms = self.last_input.map(|time| time.elapsed().as_millis());
        format!(
            "{{\"user_active\":{},\"user_seq\":{},\"idle_ms\":{},\"quiet_ms\":{},\"held\":{},\"last_input\":{},\"window\":{}}}",
            self.active(),
            self.user_seq(),
            idle_ms.map_or_else(|| "null".into(), |ms| ms.to_string()),
            QUIET_PERIOD.as_millis(),
            self.held(),
            if self.kind.is_empty() { "null".into() } else { format!("\"{}\"", self.kind) },
            self.window.map_or_else(|| "null".into(), |window| window.to_string()),
        )
    }
}

pub(super) fn json(cx: &Cx) -> String {
    cx.remote_activity.json()
}

/// `expected` is the `if_user_seq` the request carried; without one only the
/// quiet period gates the mutation.
pub(super) fn conflict(cx: &Cx, expected: Option<u64>) -> Option<String> {
    let activity = &cx.remote_activity;
    let error = if activity.active() {
        "user_interacting"
    } else if expected.is_some_and(|expected| expected != activity.user_seq()) {
        "user_intervened"
    } else {
        return None;
    };
    Some(format!(
        "{{\"err\":\"{error}\",\"applied\":false,\"activity\":{}}}",
        activity.json()
    ))
}

pub(super) fn interrupted(cx: &Cx) -> String {
    format!(
        "{{\"err\":\"user_intervened\",\"applied\":true,\"activity\":{}}}",
        json(cx)
    )
}

/// Called at the shared dispatch boundary, before widget code sees native input.
/// Focus/geometry/paint are deliberately excluded: remote input, grabs and the OS
/// also generate them. No text, key values or pointer coordinates are published.
pub(crate) fn note_user_event(cx: &mut Cx, event: &Event) {
    if !super::is_active() {
        return;
    }
    let remote = cx.remote_activity.remote_input.get();
    let activity = &mut cx.remote_activity;
    if remote {
        match event {
            Event::MouseDown(event) => {
                if !activity.remote_buttons.contains(&event.button) {
                    activity.remote_buttons.push(event.button);
                }
            }
            Event::MouseUp(event) => activity
                .remote_buttons
                .retain(|button| *button != event.button),
            Event::KeyDown(event) => {
                if !activity.remote_keys.contains(&event.key_code) {
                    activity.remote_keys.push(event.key_code);
                }
            }
            Event::KeyUp(event) => activity.remote_keys.retain(|key| *key != event.key_code),
            _ => {}
        }
        return;
    }
    let (kind, window) = match event {
        Event::MouseDown(event) => {
            let button = (event.window_id.id(), event.button.bits());
            if !activity.buttons.contains(&button) {
                activity.buttons.push(button);
            }
            ("mouse_down", Some(event.window_id.id()))
        }
        Event::MouseUp(event) => {
            activity
                .buttons
                .retain(|button| *button != (event.window_id.id(), event.button.bits()));
            ("mouse_up", Some(event.window_id.id()))
        }
        Event::MouseMove(event) => ("mouse_move", Some(event.window_id.id())),
        Event::Scroll(event) => ("scroll", Some(event.window_id.id())),
        // An injected pinch (`/m?k=pinch`) arrives inside the remote input
        // scope above and never reaches here; this is the trackpad.
        Event::Pinch(event) => {
            activity.pinching = !matches!(event.phase, PinchPhase::End);
            ("pinch", Some(event.window_id.id()))
        }
        Event::KeyDown(event) => {
            if !activity.keys.contains(&event.key_code) {
                activity.keys.push(event.key_code);
            }
            ("key_down", None)
        }
        Event::KeyUp(event) => {
            activity.keys.retain(|key| *key != event.key_code);
            ("key_up", None)
        }
        Event::TouchUpdate(event) => {
            let mut changed = false;
            for touch in &event.touches {
                let id = (event.window_id.id(), touch.uid);
                match touch.state {
                    TouchState::Start | TouchState::Move => {
                        changed = true;
                        if !activity.touches.contains(&id) {
                            activity.touches.push(id);
                        }
                    }
                    TouchState::Stop => {
                        changed = true;
                        activity.touches.retain(|touch| *touch != id);
                    }
                    TouchState::Stable => {}
                }
            }
            if !changed {
                return;
            }
            ("touch", Some(event.window_id.id()))
        }
        Event::Drag(_) => {
            activity.dragging = true;
            ("drag", None)
        }
        Event::Drop(_) | Event::DragEnd => {
            activity.dragging = false;
            ("drop", None)
        }
        Event::TextInput(_) | Event::TextRangeReplace(_) | Event::ImeAction(_) => {
            ("text", None)
        }
        Event::TextCopy(_) | Event::TextCut(_) => ("clipboard", None),
        Event::LongPress(_) | Event::SelectionHandleDrag(_) => ("touch", None),
        Event::MacosMenuCommand(_) => ("menu", None),
        Event::WindowCloseRequested(event) => ("close", Some(event.window_id.id())),
        Event::WindowLostFocus(_) | Event::WindowClosed(_) => {
            // A release can occur outside this app; don't leave a phantom
            // held key/button blocking remote work indefinitely.
            activity.buttons.clear();
            activity.keys.clear();
            activity.touches.clear();
            activity.dragging = false;
            activity.pinching = false;
            return;
        }
        _ => return,
    };
    activity.kind = kind;
    activity.window = window;
    activity.last_input = Some(Instant::now());
    activity.seq.fetch_add(1, Ordering::Release);
    if activity.remote_buttons.is_empty() && activity.remote_keys.is_empty() {
        return;
    }
    let buttons = std::mem::take(&mut activity.remote_buttons);
    let keys = std::mem::take(&mut activity.remote_keys);
    // Cancel the automation's unfinished gesture without synthesizing an
    // up/click action. The native event that interrupted it is still about
    // to run; restore its down bookkeeping after discarding remote state.
    if !buttons.is_empty() {
        if cx.fingers.has_pinned_capture() {
            cx.dispatch_hw_pin_release();
            cx.push_unique_platform_op(crate::cx_api::CxOsOp::PinMousePointer(false));
        }
        for button in buttons {
            cx.fingers.mouse_up(button);
        }
        if let Event::MouseDown(event) = event {
            cx.fingers.mouse_down(event.button, event.window_id);
        }
        cx.update_pointer_capture_pacing();
    }
    cx.keyboard
        .keys_down
        .retain(|key| !keys.contains(&key.key_code));
    if let Event::KeyDown(event) = event {
        cx.keyboard.process_key_down(event.clone());
    }
    cx.inner_call_event_handler(&Event::ClearHover);
}
