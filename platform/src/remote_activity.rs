//! Native interaction tracking for the standalone remote interface. The
//! ledger belongs to the `Cx` (`Cx::remote_activity`) and is only touched on
//! the UI thread; the HTTP threads see one number, the user sequence, through
//! the handle the remote service takes when it starts.

use crate::cx::Cx;
use crate::cx_api::CxOsApi;
use crate::event::{DigitDevice, Event, KeyCode, MouseButton, PinchPhase, TouchState};
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

const QUIET_PERIOD: Duration = Duration::from_secs(2);
// A missing native release must stop gating commands after 30 seconds of input silence.
const HELD_STALE: Duration = Duration::from_secs(30);

/// The user sequence as the command threads see it: the HAL's count, advanced on every native
/// input (not inside the `Cx`), so the endpoint's command threads and the semantic tree's consumers
/// read it without asking the UI thread.
pub(crate) fn user_seq_now() -> u64 {
    makepad_hal::a11y::user_seq()
}

/// What the person has done to this instance natively, and what remote input
/// is in flight. Owned by the `Cx`, so an instance's bookkeeping lives and
/// dies with it.
#[derive(Default)]
pub(crate) struct RemoteActivity {
    local_user_seq: u64,
    last_input: Option<Instant>,
    kind: &'static str,
    window: Option<usize>,
    held_window: Option<usize>,
    stale_logged: Cell<bool>,
    buttons: u32,
    keys: Vec<KeyCode>,
    touches: Vec<u64>,
    dragging: bool,
    /// A native trackpad pinch between its Begin and End phases.
    pinching: bool,
    remote_buttons: Vec<MouseButton>,
    remote_keys: Vec<KeyCode>,
    /// True while an injected event is being dispatched (see
    /// `remote::remote_input_scope`); shared so the scope guard needs no
    /// borrow of the `Cx` across the dispatch.
    pub(crate) remote_input: Rc<Cell<bool>>,
    /// Who sent the injected input being dispatched (`remote::remote_input_scope_by`; empty: not
    /// known, or none).
    pub(crate) remote_by: Rc<std::cell::RefCell<String>>,
}

impl RemoteActivity {
    pub(crate) fn local_user_seq(&self) -> u64 {
        self.local_user_seq
    }

    pub(crate) fn user_seq(&self) -> u64 {
        user_seq_now()
    }

    /// Pointer, touch, drag and pinch input has a release the window server
    /// delivers. Keys do not count: a modifier released while a menu or a
    /// panel is key never reaches the app, and a key ledger then blocked every
    /// remote request until the window lost focus. Typing still counts as
    /// activity through the quiet period.
    fn has_held(&self) -> bool {
        self.buttons != 0 || !self.touches.is_empty() || self.dragging || self.pinching
    }

    pub(crate) fn clear_held(&mut self) {
        self.buttons = 0;
        self.keys.clear();
        self.touches.clear();
        self.dragging = false;
        self.pinching = false;
        self.held_window = None;
        self.stale_logged.set(false);
    }

    fn log_stale_once(&self, now: Instant) {
        if !self.stale_logged.replace(true) {
            crate::log!(
                "remote held state expired: held_window={:?} last_window={:?} buttons={:#x} touches={} dragging={} pinching={} age_ms={}",
                self.held_window, self.window, self.buttons, self.touches.len(),
                self.dragging, self.pinching,
                now.saturating_duration_since(self.last_input.unwrap()).as_millis()
            );
        }
    }

    pub(crate) fn held_at(&self, now: Instant) -> bool {
        let Some(last) = self.last_input else { return false };
        if !self.has_held() {
            return false;
        }
        if now.saturating_duration_since(last) >= HELD_STALE {
            self.log_stale_once(now);
            return false;
        }
        true
    }

    pub(crate) fn active_at(&self, now: Instant) -> bool {
        self.held_at(now)
            || self.last_input.is_some_and(|t| now.saturating_duration_since(t) < QUIET_PERIOD)
    }

    fn active(&self) -> bool {
        self.active_at(Instant::now())
    }

    /// Returns whether this event counts as native input for the quiet period and sequence.
    pub(crate) fn note_native_at(&mut self, event: &Event, cancel_taken_away: bool, now: Instant) -> bool {
        // Discard old edges before new input can refresh their lease.
        if self.has_held() && self.last_input.is_some_and(|t| now.saturating_duration_since(t) >= HELD_STALE) {
            self.log_stale_once(now);
            self.clear_held();
        }
        let (kind, window) = match event {
            Event::MouseDown(event) => {
                self.held_window = Some(event.window_id.id());
                self.buttons |= event.button.bits();
                ("mouse_down", Some(event.window_id.id()))
            }
            Event::MouseUp(event) => {
                self.buttons &= !event.button.bits();
                ("mouse_up", Some(event.window_id.id()))
            }
            Event::MouseMove(event) => ("mouse_move", Some(event.window_id.id())),
            Event::Scroll(event) => ("scroll", Some(event.window_id.id())),
            // An injected pinch (`/m?k=pinch`) arrives inside the remote input
            // scope above and never reaches here; this is the trackpad.
            Event::Pinch(event) => {
                self.pinching = !matches!(event.phase, PinchPhase::End);
                if self.pinching {
                    self.held_window = Some(event.window_id.id());
                }
                ("pinch", Some(event.window_id.id()))
            }
            Event::KeyDown(event) => {
                if !self.keys.contains(&event.key_code) {
                    self.keys.push(event.key_code);
                }
                ("key_down", None)
            }
            Event::KeyUp(event) => {
                self.keys.retain(|key| *key != event.key_code);
                ("key_up", None)
            }
            Event::FingerCancel(event) if cancel_taken_away => {
                match event.device {
                    DigitDevice::Touch { uid } => self.touches.retain(|touch| *touch != uid),
                    DigitDevice::Mouse { button } => {
                        self.buttons &= !button.bits();
                        if button.is_primary() {
                            self.dragging = false;
                        }
                    }
                    _ => {}
                }
                ("pointer_cancel", Some(event.window_id.id()))
            }
            Event::TouchUpdate(event) => {
                let mut changed = false;
                for touch in &event.touches {
                    match touch.state {
                        TouchState::Start | TouchState::Move => {
                            changed = true;
                            self.held_window = Some(event.window_id.id());
                            if !self.touches.contains(&touch.uid) {
                                self.touches.push(touch.uid);
                            }
                        }
                        TouchState::Stop | TouchState::Cancel => {
                            changed = true;
                            self.touches.retain(|uid| *uid != touch.uid);
                        }
                        TouchState::Stable => {}
                    }
                }
                if !changed {
                    return false;
                }
                ("touch", Some(event.window_id.id()))
            }
            Event::Drag(event) => {
                self.held_window = Some(event.window_id.id());
                self.dragging = true;
                ("drag", None)
            }
            Event::Drop(_) => {
                self.dragging = false;
                ("drop", None)
            }
            Event::DragEnd => {
                self.dragging = false;
                return false;
            }
            Event::TextInput(_) | Event::TextRangeReplace(_) | Event::ImeAction(_) => {
                ("text", None)
            }
            Event::TextCopy(_) | Event::TextCut(_) => ("clipboard", None),
            Event::LongPress(_) | Event::SelectionHandleDrag(_) => ("touch", None),
            Event::MacosMenuCommand(_) | Event::TrayMenu(_) | Event::TrayClick(_) => ("menu", None),
            Event::NotificationClick(_) => ("notification", None),
            Event::WindowCloseRequested(event) => ("close", Some(event.window_id.id())),
            Event::WindowLostFocus(_) | Event::WindowClosed(_) | Event::PopupDismissed(_)
            | Event::HostAttached | Event::Background | Event::Pause => {
                self.clear_held();
                return false;
            }
            _ => return false,
        };
        self.kind = kind;
        self.window = window;
        self.last_input = Some(now);
        true
    }

    fn json(&self) -> String {
        let now = Instant::now();
        let idle_ms = self.last_input.map(|t| now.saturating_duration_since(t).as_millis());
        format!(
            "{{\"user_active\":{},\"user_seq\":{},\"idle_ms\":{},\"quiet_ms\":{},\"held\":{},\"last_input\":{},\"window\":{}}}",
            self.active_at(now),
            self.user_seq(),
            idle_ms.map_or_else(|| "null".into(), |ms| ms.to_string()),
            QUIET_PERIOD.as_millis(),
            self.held_at(now),
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
    if !remote {
        super::input_rec::note(event);
    }
    if crate::makepad_error_log::flight::on() {
        super::input_rec::flight(event, remote);
    }
    let cancel_taken_away = matches!(event, Event::FingerCancel(e) if cx.fingers.press_taken_away(e.digit_id));
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
            Event::FingerCancel(event) if cancel_taken_away => {
                if let DigitDevice::Mouse { button } = event.device {
                    activity.remote_buttons.retain(|held| *held != button);
                }
            }
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
    if !activity.note_native_at(event, cancel_taken_away, Instant::now()) {
        return;
    }
    activity.local_user_seq = activity.local_user_seq.wrapping_add(1);
    makepad_hal::a11y::note_user_input();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{
        DragEvent, DragResponse, FingerCancelEvent, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
        PopupDismissReason, PopupDismissedEvent, WindowClosedEvent,
    };
    use crate::window::WindowId;
    use std::sync::{Arc, Mutex};

    fn primary_down(window_id: WindowId) -> Event {
        Event::MouseDown(MouseDownEvent {
            abs: Default::default(),
            button: MouseButton::PRIMARY,
            window_id,
            modifiers: Default::default(),
            handled: Default::default(),
            time: 0.0,
        })
    }

    #[test]
    fn mouse_up_on_another_window_clears_held_button() {
        let mut activity = RemoteActivity::default();
        let now = Instant::now();
        assert!(activity.note_native_at(&primary_down(WindowId(1, 0)), false, now));
        assert!(activity.held_at(now));
        let up = Event::MouseUp(MouseUpEvent {
            window_id: WindowId(0, 0),
            abs: Default::default(),
            button: MouseButton::PRIMARY,
            time: 0.0,
            modifiers: Default::default(),
        });
        assert!(activity.note_native_at(&up, false, now));
        assert!(!activity.held_at(now));
        assert!(activity.active_at(now), "the release starts the quiet period");
    }

    #[test]
    fn focus_loss_popup_dismiss_and_host_reattach_clear_all_held_state() {
        let now = Instant::now();
        let window_id = WindowId(1, 0);
        for event in [
            Event::WindowLostFocus(window_id),
            Event::WindowClosed(WindowClosedEvent { window_id }),
            Event::PopupDismissed(PopupDismissedEvent { window_id, reason: PopupDismissReason::FocusLost }),
            Event::HostAttached,
            Event::Background,
            Event::Pause,
        ] {
            let mut activity = RemoteActivity::default();
            activity.note_native_at(&primary_down(window_id), false, now);
            activity.keys.push(KeyCode::Space);
            activity.touches.push(11);
            activity.dragging = true;
            activity.pinching = true;
            let later = now + QUIET_PERIOD;
            assert!(!activity.note_native_at(&event, false, later));
            assert!(!activity.held_at(later));
            assert!(activity.keys.is_empty());
            assert_eq!(activity.held_window, None);
            assert_eq!(activity.last_input, Some(now));
            assert!(!activity.active_at(later));
        }
    }

    #[test]
    fn stale_held_button_expires_and_mouse_move_does_not_revive_it() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let expired = Instant::now();
        let last_move = expired - HELD_STALE;
        let down = last_move - Duration::from_secs(1);
        let moved = Event::MouseMove(MouseMoveEvent {
            abs: Default::default(),
            lock_delta: Default::default(),
            window_id: WindowId(0, 0),
            modifiers: Default::default(),
            time: 0.0,
            handled: Default::default(),
        });
        cx.remote_activity.note_native_at(&primary_down(WindowId(1, 0)), false, down);
        cx.remote_activity.note_native_at(&moved, false, last_move);
        assert!(cx.remote_activity.held_at(expired - Duration::from_millis(1)));
        assert!(!cx.remote_activity.stale_logged.get());
        assert!(!cx.remote_activity.held_at(expired));
        assert!(!cx.remote_activity.active_at(expired));
        assert!(json(&cx).contains("\"held\":false"));
        assert!(conflict(&cx, None).is_none(), "a quit immediately after the query is allowed");
        assert!(cx.remote_activity.stale_logged.get(), "one expiry shared by query and guard");

        let resumed = expired + Duration::from_secs(1);
        cx.remote_activity.note_native_at(&moved, false, resumed);
        assert_eq!(cx.remote_activity.buttons, 0);
        assert!(!cx.remote_activity.held_at(resumed));
        assert!(!cx.remote_activity.stale_logged.get(), "the next hold gets its own expiry");
        assert!(cx.remote_activity.active_at(resumed));
        assert!(!cx.remote_activity.active_at(resumed + QUIET_PERIOD));
    }

    #[test]
    fn finger_cancel_taken_away_clears_button_and_drag_across_windows() {
        let mut activity = RemoteActivity::default();
        let now = Instant::now();
        let window_id = WindowId(1, 0);
        activity.note_native_at(&primary_down(window_id), false, now);
        activity.note_native_at(&Event::Drag(DragEvent {
            window_id,
            modifiers: Default::default(),
            handled: Default::default(),
            abs: Default::default(),
            items: Default::default(),
            response: Arc::new(Mutex::new(DragResponse::None)),
        }), false, now);
        let cancel = Event::FingerCancel(FingerCancelEvent {
            window_id: WindowId(0, 0),
            digit_id: Default::default(),
            device: DigitDevice::Mouse { button: MouseButton::PRIMARY },
            abs: Default::default(),
            time: 0.0,
            modifiers: Default::default(),
        });
        assert!(!activity.note_native_at(&cancel, false, now));
        assert!(activity.dragging && activity.buttons != 0);
        assert!(activity.note_native_at(&cancel, true, now));
        assert!(!activity.held_at(now));
    }
}
