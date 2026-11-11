//! A value one thread replaces while another uses it in place, neither blocked by a lock.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, Ordering};

/// A value one thread (the owner: a UI or command thread) replaces and another (the user: a
/// realtime thread) uses in place. The user never waits: a use that finds a replace under way,
/// or no value, is skipped. A replace waits only for the one use already admitted, then hands
/// the old value back, so it is dropped on the replacing thread and is never used again once
/// `replace` has returned.
pub struct Slot<T> {
    busy: AtomicBool,
    changing: AtomicBool,
    value: UnsafeCell<Option<T>>,
}

// SAFETY: the value is reached only by the one thread holding `busy`.
unsafe impl<T: Send> Send for Slot<T> {}
unsafe impl<T: Send> Sync for Slot<T> {}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Slot::new(None)
    }
}

impl<T> Slot<T> {
    pub const fn new(value: Option<T>) -> Slot<T> {
        Slot { busy: AtomicBool::new(false), changing: AtomicBool::new(false), value: UnsafeCell::new(value) }
    }

    /// Set (or clear) the value and get the old one back. One replacing thread at a time.
    pub fn replace(&self, value: Option<T>) -> Option<T> {
        self.changing.store(true, Ordering::SeqCst);
        while self.busy.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            std::hint::spin_loop();
        }
        // SAFETY: `busy` is ours, so no use is reading the value
        let old = std::mem::replace(unsafe { &mut *self.value.get() }, value);
        self.busy.store(false, Ordering::SeqCst);
        self.changing.store(false, Ordering::SeqCst);
        old
    }

    /// Use the value unless there is none or a replace is under way (`None` then). Never waits.
    #[inline]
    pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> Option<R> {
        if self.changing.load(Ordering::SeqCst)
            || self.busy.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err()
        {
            return None;
        }
        struct Leave<'a>(&'a AtomicBool);
        impl Drop for Leave<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }
        let _leave = Leave(&self.busy);
        // SAFETY: `busy` is ours until `_leave` drops, so no replace writes the value
        unsafe { &mut *self.value.get() }.as_mut().map(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use std::sync::Arc;

    /// A user thread uses the slot as fast as it can while the value is replaced under it:
    /// once a replace returns the old value is never used again and comes back to the replacing
    /// thread, the new one is used, and a cleared slot uses nothing.
    #[test]
    fn a_replace_under_running_uses_retires_the_old_value_at_once() {
        let slot: Arc<Slot<Arc<AtomicU64>>> = Arc::new(Slot::default());
        let counters = [Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0))];
        let stop = Arc::new(AtomicBool::new(false));
        let user = {
            let (slot, stop) = (slot.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    slot.with(|counter| counter.fetch_add(1, Ordering::SeqCst));
                }
            })
        };
        let wait_for_uses = |counter: &AtomicU64| {
            let start = counter.load(Ordering::SeqCst);
            while counter.load(Ordering::SeqCst) < start + 100 {
                std::hint::spin_loop();
            }
        };
        assert!(slot.replace(Some(counters[0].clone())).is_none());
        wait_for_uses(&counters[0]);
        for round in 0..200 {
            let (old, new) = (round % 2, 1 - round % 2);
            let back = slot.replace(Some(counters[new].clone())).expect("the old value comes back");
            assert!(Arc::ptr_eq(&back, &counters[old]));
            let frozen = counters[old].load(Ordering::SeqCst);
            wait_for_uses(&counters[new]);
            assert_eq!(counters[old].load(Ordering::SeqCst), frozen, "round {round}: used after replace returned");
        }
        assert!(slot.replace(None).is_some());
        let frozen = [counters[0].load(Ordering::SeqCst), counters[1].load(Ordering::SeqCst)];
        assert_eq!(slot.with(|_| ()), None, "a cleared slot has nothing to use");
        stop.store(true, Ordering::SeqCst);
        user.join().unwrap();
        assert_eq!([counters[0].load(Ordering::SeqCst), counters[1].load(Ordering::SeqCst)], frozen);
        // Only the test holds the counters now: nothing kept a copy.
        assert_eq!(Arc::strong_count(&counters[0]) + Arc::strong_count(&counters[1]), 2);
    }
}
