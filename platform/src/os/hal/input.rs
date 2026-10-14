//! Direct-input preferences on the session's Cx thread. Setters queue commands for
//! its next submit; getters read the last host report or the local pending choice.
use std::cell::RefCell;
pub use makepad_hal::direct::{POINTER_SPEED_DEFAULT, POINTER_SPEED_MAX, POINTER_SPEED_MIN};

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
    natural: Choice<bool>,
    tap: Choice<bool>,
}

thread_local! {
    static PREFERENCES: RefCell<Preferences> = RefCell::new(Preferences {
        speeds: [Choice::new(POINTER_SPEED_DEFAULT), Choice::new(POINTER_SPEED_DEFAULT)],
        natural: Choice::new(true),
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

pub fn natural_scroll() -> bool { PREFERENCES.with(|p| p.borrow().natural.get()) }

pub fn set_natural_scroll(on: bool) {
    PREFERENCES.with(|p| p.borrow_mut().natural.set(on));
    crate::thread::SignalToUI::set_ui_signal();
}

pub fn tap_to_click() -> bool { PREFERENCES.with(|p| p.borrow().tap.get()) }

pub fn set_tap_to_click(on: bool) {
    PREFERENCES.with(|p| p.borrow_mut().tap.set(on));
    crate::thread::SignalToUI::set_ui_signal();
}

pub(crate) fn report(mouse_speed: u32, touchpad_speed: u32, natural_scroll: bool, tap_to_click: bool) {
    PREFERENCES.with(|p| {
        let mut p = p.borrow_mut();
        p.speeds[0].report(mouse_speed);
        p.speeds[1].report(touchpad_speed);
        p.natural.report(natural_scroll);
        p.tap.report(tap_to_click);
    });
}

pub(crate) fn flush(w: &mut makepad_hal::CmdWriter) {
    PREFERENCES.with(|p| {
        let mut p = p.borrow_mut();
        for (i, speed) in p.speeds.iter_mut().enumerate() {
            if let Some(speed) = speed.take() { w.direct_pointer_speed(i != 0, speed); }
        }
        if let Some(on) = p.natural.take() { w.direct_natural_scroll(on); }
        if let Some(on) = p.tap.take() { w.direct_tap_to_click(on); }
    });
}
