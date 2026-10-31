//! Direct-input preferences on the session's Cx thread. Setters queue commands for
//! its next submit; getters read the last host report or the local pending choice.
use std::cell::RefCell;
pub use makepad_hal::direct::{
    DESK_LINK_INPUT_VENDOR, DESK_LINK_POINTER_PRODUCT, POINTER_SPEED_DEFAULT, POINTER_SPEED_MAX, POINTER_SPEED_MIN,
};

struct Choice<T> {
    current: T,
    pending: Option<T>,
    dirty: bool,
}

impl<T: Copy + PartialEq> Choice<T> {
    fn new(current: T) -> Self { Self { current, pending: None, dirty: false } }
    fn get(&self) -> T { self.pending.unwrap_or(self.current) }
    fn set(&mut self, value: T) { self.pending = Some(value); self.dirty = true; }
    fn report(&mut self, value: T) {
        self.current = value;
        if self.pending == Some(value) && !self.dirty { self.pending = None; }
    }
    fn take(&mut self) -> Option<T> {
        if std::mem::take(&mut self.dirty) { self.pending } else { None }
    }
}

struct Preferences {
    speeds: [Choice<u32>; 2],
    /// natural scrolling: mouse wheels, touchpads
    natural: [Choice<bool>; 2],
    tap: Choice<bool>,
}

thread_local! {
    static PREFERENCES: RefCell<Preferences> = RefCell::new(Preferences {
        speeds: [Choice::new(POINTER_SPEED_DEFAULT), Choice::new(POINTER_SPEED_DEFAULT)],
        natural: [Choice::new(true), Choice::new(true)],
        tap: Choice::new(false),
    });
}

pub fn pointer_speed(touchpad: bool) -> u32 {
    PREFERENCES.with(|p| p.borrow().speeds[touchpad as usize].get())
}

pub fn set_pointer_speed(touchpad: bool, hundredths: u32) {
    PREFERENCES.with(|p| p.borrow_mut().speeds[touchpad as usize].set(hundredths.clamp(POINTER_SPEED_MIN, POINTER_SPEED_MAX)));
    crate::thread::SignalToUI::set_ui_signal();
}

/// Natural scrolling (the Mac direction) for touchpads or mouse wheels; the direct backend
/// applies it, so every app under the WM gets scroll already turned.
pub fn natural_scroll(touchpad: bool) -> bool { PREFERENCES.with(|p| p.borrow().natural[touchpad as usize].get()) }

pub fn set_natural_scroll(touchpad: bool, on: bool) {
    PREFERENCES.with(|p| p.borrow_mut().natural[touchpad as usize].set(on));
    crate::thread::SignalToUI::set_ui_signal();
}

pub fn tap_to_click() -> bool { PREFERENCES.with(|p| p.borrow().tap.get()) }

pub fn set_tap_to_click(on: bool) {
    PREFERENCES.with(|p| p.borrow_mut().tap.set(on));
    crate::thread::SignalToUI::set_ui_signal();
}

/// The host's preferences, `[mouse, touchpad]` each.
pub(crate) fn report(speeds: [u32; 2], natural_scroll: [bool; 2], tap_to_click: bool) {
    PREFERENCES.with(|p| {
        let mut p = p.borrow_mut();
        for i in 0..2 {
            p.speeds[i].report(speeds[i]);
            p.natural[i].report(natural_scroll[i]);
        }
        p.tap.report(tap_to_click);
    });
}

pub(crate) fn flush(w: &mut makepad_hal::CmdWriter) {
    PREFERENCES.with(|p| {
        let mut p = p.borrow_mut();
        for (i, speed) in p.speeds.iter_mut().enumerate() {
            if let Some(speed) = speed.take() { w.direct_pointer_speed(i != 0, speed); }
        }
        for (i, natural) in p.natural.iter_mut().enumerate() {
            if let Some(on) = natural.take() { w.direct_natural_scroll(i != 0, on); }
        }
        if let Some(on) = p.tap.take() { w.direct_tap_to_click(on); }
    });
}
