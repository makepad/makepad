//! Compiler TLS is forgotten lazily after a quiesced generation swap, including at thread exit.
use std::cell::{Cell, RefCell};
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::{AccessError, LocalKey};

static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Invalidates compiler TLS. Every compiler thread must be quiesced, with no borrowed TLS
/// value or compiler TLS destructor running, before the old arena is destroyed.
pub fn bump() {
    GENERATION.fetch_add(1, Ordering::AcqRel);
}

/// A generation-stamped TLS value. Interior mutability in T keeps recursive shared accesses
/// valid; the outer borrow prevents replacing a value while an access still holds it.
pub struct GenCell<T: Default> {
    generation: Cell<u64>,
    value: RefCell<ManuallyDrop<T>>,
}

impl<T: Default> Default for GenCell<T> {
    fn default() -> Self {
        Self {
            generation: Cell::new(GENERATION.load(Ordering::Acquire)),
            value: RefCell::new(ManuallyDrop::new(T::default())),
        }
    }
}

impl<T: Default> GenCell<T> {
    /// Starts from T::default() on the first access in a new compiler generation. The caller
    /// must enter that generation's compiler phase before accessing compiler TLS.
    #[inline]
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        let generation = GENERATION.load(Ordering::Acquire);
        if self.generation.get() != generation {
            let mut value = self.value.borrow_mut();
            // Overwriting ManuallyDrop never touches the old arena, even if it is destroyed.
            // Construct first: a panicking default leaves the old stamp and value intact.
            *value = ManuallyDrop::new(T::default());
            self.generation.set(generation);
        }
        f(&self.value.borrow())
    }
}

impl<T: Default> Drop for GenCell<T> {
    fn drop(&mut self) {
        if self.generation.get() == GENERATION.load(Ordering::Acquire) {
            // SAFETY: exclusive access at thread exit; stale generations are never visited.
            unsafe { ManuallyDrop::drop(self.value.get_mut()); }
        }
    }
}

/// A compiler TLS key; each access checks the generation before exposing its value.
pub struct GenLocal<T: Default + 'static>(&'static LocalKey<GenCell<T>>);

impl<T: Default + 'static> GenLocal<T> {
    pub const fn new(key: &'static LocalKey<GenCell<T>>) -> Self { Self(key) }

    #[inline]
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.0.with(|cell| cell.with(f))
    }

    #[inline]
    pub fn try_with<R>(&self, f: impl FnOnce(&T) -> R) -> Result<R, AccessError> {
        self.0.try_with(|cell| cell.with(f))
    }
}

/// Declares compiler TLS whose initial value in each generation is Default::default().
#[macro_export]
macro_rules! gen_local {
    ($( $(#[$attr:meta])* $vis:vis static $name:ident: $ty:ty; )*) => {
        $(
            $(#[$attr])*
            $vis const $name: $crate::gen_local::GenLocal<$ty> = {
                std::thread_local! {
                    static CELL: $crate::gen_local::GenCell<$ty> = Default::default();
                }
                $crate::gen_local::GenLocal::new(&CELL)
            };
        )*
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, atomic::AtomicUsize};

    #[derive(Default)]
    struct Counted(Option<Arc<AtomicUsize>>);
    impl Drop for Counted {
        fn drop(&mut self) {
            if let Some(drops) = &self.0 { drops.fetch_add(1, Ordering::Relaxed); }
        }
    }
    crate::gen_local! { static VALUE: RefCell<Counted>; }

    #[test]
    fn generation_forgets_old_tls_and_only_drops_current_values() {
        let drops = Arc::new(AtomicUsize::new(0));
        let (ready, receive) = std::sync::mpsc::channel();
        let (resume, wait) = std::sync::mpsc::channel();
        let count = drops.clone();
        let thread = std::thread::spawn(move || {
            VALUE.with(|value| value.borrow_mut().0 = Some(count.clone()));
            ready.send(()).unwrap();
            wait.recv().unwrap();
            VALUE.with(|value| {
                assert!(value.borrow().0.is_none());
                assert_eq!(count.load(Ordering::Relaxed), 0);
                // Recursive access retains RefCell's original borrowing semantics.
                VALUE.with(|nested| assert!(nested.borrow().0.is_none()));
                value.borrow_mut().0 = Some(count);
            });
        });
        receive.recv().unwrap();
        bump();
        resume.send(()).unwrap();
        thread.join().unwrap();
        assert_eq!(drops.load(Ordering::Relaxed), 1);

        let (ready, receive) = std::sync::mpsc::channel();
        let (resume, wait) = std::sync::mpsc::channel();
        let count = drops.clone();
        let thread = std::thread::spawn(move || {
            VALUE.with(|value| value.borrow_mut().0 = Some(count));
            ready.send(()).unwrap();
            wait.recv().unwrap();
            // Exit without another access: the stale TLS destructor must forget too.
        });
        receive.recv().unwrap();
        bump();
        resume.send(()).unwrap();
        thread.join().unwrap();
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }
}
