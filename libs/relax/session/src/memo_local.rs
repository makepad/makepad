//! Per-thread compiler memos that a settle rebuild retires: each thread drops its value by value,
//! on its own thread, at its first access after the rebuild (`retire_all`), and starts again
//! from `Default`. Nothing is forgotten or freed in bulk: a value is dropped by the thread that
//! owns it, as any thread-local is. A thread that never accesses its memo again keeps the old
//! value until it ends (then it is dropped as usual).
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::{AccessError, LocalKey};

static EPOCH: AtomicU64 = AtomicU64::new(0);

/// Retires every thread's memos: each one is dropped at that thread's next access.
pub fn retire_all() {
    EPOCH.fetch_add(1, Ordering::AcqRel);
}

/// An epoch-stamped thread-local memo. Interior mutability in T keeps recursive shared accesses
/// valid; the outer borrow keeps a value from being replaced while an access still holds it.
pub struct MemoCell<T: Default> {
    epoch: Cell<u64>,
    value: RefCell<T>,
}

impl<T: Default> Default for MemoCell<T> {
    fn default() -> Self {
        Self { epoch: Cell::new(EPOCH.load(Ordering::Acquire)), value: RefCell::new(T::default()) }
    }
}

impl<T: Default> MemoCell<T> {
    /// The memo, a fresh `T::default()` on the first access after a retire (the old one dropped here).
    #[inline]
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        let epoch = EPOCH.load(Ordering::Acquire);
        if self.epoch.get() != epoch {
            // construct first: a panicking default leaves the old stamp and value intact
            let fresh = T::default();
            let old = std::mem::replace(&mut *self.value.borrow_mut(), fresh);
            self.epoch.set(epoch);
            drop(old);
        }
        f(&self.value.borrow())
    }
}

/// A memo TLS key; each access checks the epoch before exposing its value.
pub struct MemoLocal<T: Default + 'static>(&'static LocalKey<MemoCell<T>>);

impl<T: Default + 'static> MemoLocal<T> {
    pub const fn new(key: &'static LocalKey<MemoCell<T>>) -> Self {
        Self(key)
    }

    #[inline]
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.0.with(|cell| cell.with(f))
    }

    #[inline]
    pub fn try_with<R>(&self, f: impl FnOnce(&T) -> R) -> Result<R, AccessError> {
        self.0.try_with(|cell| cell.with(f))
    }
}

/// Declares thread-local compiler memos whose initial value is Default::default() and that a
/// settle rebuild retires (`retire_all`).
#[macro_export]
macro_rules! memo_local {
    ($( $(#[$attr:meta])* $vis:vis static $name:ident: $ty:ty; )*) => {
        $(
            $(#[$attr])*
            $vis const $name: $crate::memo_local::MemoLocal<$ty> = {
                std::thread_local! {
                    static CELL: $crate::memo_local::MemoCell<$ty> = Default::default();
                }
                $crate::memo_local::MemoLocal::new(&CELL)
            };
        )*
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{atomic::AtomicUsize, Arc};

    #[derive(Default)]
    struct Counted(Option<Arc<AtomicUsize>>);
    impl Drop for Counted {
        fn drop(&mut self) {
            if let Some(drops) = &self.0 {
                drops.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    crate::memo_local! { static VALUE: RefCell<Counted>; }

    #[test]
    fn a_retired_memo_is_dropped_at_the_next_access_and_at_thread_exit() {
        let drops = Arc::new(AtomicUsize::new(0));
        let (ready, receive) = std::sync::mpsc::channel();
        let (resume, wait) = std::sync::mpsc::channel();
        let count = drops.clone();
        let thread = std::thread::spawn(move || {
            VALUE.with(|v| v.borrow_mut().0 = Some(count.clone()));
            ready.send(()).unwrap();
            wait.recv().unwrap();
            VALUE.with(|v| {
                // the old value went at this access, on this thread
                assert!(v.borrow().0.is_none());
                assert_eq!(count.load(Ordering::Relaxed), 1);
                VALUE.with(|nested| assert!(nested.borrow().0.is_none()));
                v.borrow_mut().0 = Some(count);
            });
        });
        receive.recv().unwrap();
        retire_all();
        resume.send(()).unwrap();
        thread.join().unwrap();
        // the second value went at thread exit
        assert_eq!(drops.load(Ordering::Relaxed), 2);
    }
}
