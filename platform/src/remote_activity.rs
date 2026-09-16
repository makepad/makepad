//! Native interaction tracking for the standalone remote interface.
//! All mutable state belongs to the UI thread. HTTP threads only read the epoch.

use crate::cx::Cx;
use crate::cx_api::CxOsApi;
use crate::event::{Event, KeyCode, MouseButton, PinchPhase, TouchState};
use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const QUIET_PERIOD: Duration = Duration::from_secs(2);
static USER_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
struct Activity {
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
}

thread_local! {
    static ACTIVITY: RefCell<Activity> = RefCell::new(Activity::default());
}

pub(super) fn user_seq() -> u64 {
    USER_SEQ.load(Ordering::Acquire)
}

impl Activity {
    fn held(&self) -> bool {
        !self.buttons.is_empty()
            || !self.keys.is_empty()
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
            user_seq(),
            idle_ms.map_or_else(|| "null".into(), |ms| ms.to_string()),
            QUIET_PERIOD.as_millis(),
            self.held(),
            if self.kind.is_empty() { "null".into() } else { format!("\"{}\"", self.kind) },
            self.window.map_or_else(|| "null".into(), |window| window.to_string()),
        )
    }
}

pub(super) fn json() -> String {
    ACTIVITY.with_borrow(Activity::json)
}

pub(super) fn conflict(expected: u64) -> Option<String> {
    ACTIVITY.with_borrow(|activity| {
        let error = if activity.active() {
            "user_interacting"
        } else if expected != user_seq() {
            "user_intervened"
        } else {
            return None;
        };
        Some(format!(
            "{{\"err\":\"{error}\",\"applied\":false,\"activity\":{}}}",
            activity.json()
        ))
    })
}

pub(super) fn interrupted() -> String {
    format!(
        "{{\"err\":\"user_intervened\",\"applied\":true,\"activity\":{}}}",
        json()
    )
}

/// Called at the shared dispatch boundary, before widget code sees native input.
/// Focus/geometry/paint are deliberately excluded: remote input, grabs and the OS
/// also generate them. No text, key values or pointer coordinates are published.
pub(crate) fn note_user_event(cx: &mut Cx, event: &Event) {
    if !super::is_active() {
        return;
    }
    let remote = super::REMOTE_INPUT.with(|origin| origin.get());
    let cancelled = ACTIVITY.with_borrow_mut(|activity| {
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
            return None;
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
                    return None;
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
                return None;
            }
            _ => return None,
        };
        activity.kind = kind;
        activity.window = window;
        activity.last_input = Some(Instant::now());
        USER_SEQ.fetch_add(1, Ordering::Release);
        if activity.remote_buttons.is_empty() && activity.remote_keys.is_empty() {
            None
        } else {
            Some((
                std::mem::take(&mut activity.remote_buttons),
                std::mem::take(&mut activity.remote_keys),
            ))
        }
    });
    if let Some((buttons, keys)) = cancelled {
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
}
