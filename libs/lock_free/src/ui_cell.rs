//! State owned by exactly one thread behind a shared handle.
//! Not a lock: entering it twice panics.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, Ordering};

/// State owned by exactly one thread, kept behind a shared handle. Not a
/// lock: entering it while it is already entered — from another thread,
/// or re-entrantly — is a programming error and panics.
pub struct UiCell<T> {
    entered: AtomicBool,
    value: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for UiCell<T> {}
unsafe impl<T: Send> Sync for UiCell<T> {}

impl<T> UiCell<T> {
    pub fn new(value: T) -> UiCell<T> {
        UiCell { entered: AtomicBool::new(false), value: UnsafeCell::new(value) }
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        assert!(
            !self.entered.swap(true, Ordering::Acquire),
            "UiCell entered twice: it belongs to one thread and is not re-entrant"
        );
        struct Leave<'a>(&'a AtomicBool);
        impl Drop for Leave<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _leave = Leave(&self.entered);
        // SAFETY: the flag above admits one caller at a time and panics on
        // any overlap, so this is the only live reference.
        f(unsafe { &mut *self.value.get() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "UiCell entered twice")]
    fn ui_cell_refuses_reentry() {
        let cell = UiCell::new(0u32);
        cell.with(|_| cell.with(|_| ()));
    }
}
