//! Once, OnceState, OnceLock, LazyLock (futex Once algorithm from Rust std,
//! sys/sync/once/futex.rs, MIT/Apache-2.0).

use core::cell::{Cell, UnsafeCell};
use core::fmt;
use core::ops::Deref;
use core::sync::atomic::AtomicU32;
use core::sync::atomic::Ordering::{Acquire, Relaxed, Release};

use crate::sys::os::{futex_wait, futex_wake_all};

const INCOMPLETE: u32 = 3;
const POISONED: u32 = 2;
const RUNNING: u32 = 1;
const COMPLETE: u32 = 0;
const QUEUED: u32 = 4;
const STATE_MASK: u32 = 0b11;

pub struct Once {
    state_and_queued: AtomicU32,
}

pub struct OnceState {
    poisoned: bool,
    set_state_to: Cell<u32>,
}

impl OnceState {
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }
}

impl fmt::Debug for OnceState {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OnceState").field("poisoned", &self.is_poisoned()).finish()
    }
}

/// Publishes the final state and wakes waiters (also when the closure panics under rustc).
struct CompletionGuard<'a> {
    state_and_queued: &'a AtomicU32,
    set_state_on_drop_to: u32,
}

impl<'a> Drop for CompletionGuard<'a> {
    fn drop(&mut self) {
        if self.state_and_queued.swap(self.set_state_on_drop_to, Release) & QUEUED != 0 {
            futex_wake_all(self.state_and_queued);
        }
    }
}

impl Once {
    pub const fn new() -> Once {
        Once { state_and_queued: AtomicU32::new(INCOMPLETE) }
    }

    pub fn is_completed(&self) -> bool {
        self.state_and_queued.load(Acquire) == COMPLETE
    }

    pub fn call_once<F: FnOnce()>(&self, f: F) {
        if self.is_completed() {
            return;
        }
        let mut f = Some(f);
        self.call(false, &mut |_: &OnceState| match f.take() {
            Some(f) => f(),
            None => {}
        });
    }

    pub fn call_once_force<F: FnOnce(&OnceState)>(&self, f: F) {
        if self.is_completed() {
            return;
        }
        let mut f = Some(f);
        self.call(true, &mut |p: &OnceState| match f.take() {
            Some(f) => f(p),
            None => {}
        });
    }

    pub fn wait(&self) {
        if !self.is_completed() {
            self.wait_inner(false);
        }
    }

    pub fn wait_force(&self) {
        if !self.is_completed() {
            self.wait_inner(true);
        }
    }

    #[cold]
    fn wait_inner(&self, ignore_poisoning: bool) {
        let mut state_and_queued = self.state_and_queued.load(Acquire);
        loop {
            let state = state_and_queued & STATE_MASK;
            let queued = state_and_queued & QUEUED != 0;
            if state == COMPLETE {
                return;
            }
            if state == POISONED && !ignore_poisoning {
                panic!("Once instance has previously been poisoned");
            }
            if !queued {
                state_and_queued += QUEUED;
                if let Err(new) = self.state_and_queued.compare_exchange_weak(state, state_and_queued, Relaxed, Acquire) {
                    state_and_queued = new;
                    continue;
                }
            }
            futex_wait(&self.state_and_queued, state_and_queued, None);
            state_and_queued = self.state_and_queued.load(Acquire);
        }
    }

    #[cold]
    fn call(&self, ignore_poisoning: bool, f: &mut dyn FnMut(&OnceState)) {
        let mut state_and_queued = self.state_and_queued.load(Acquire);
        loop {
            let state = state_and_queued & STATE_MASK;
            let queued = state_and_queued & QUEUED != 0;
            if state == COMPLETE {
                return;
            }
            if state == POISONED && !ignore_poisoning {
                panic!("Once instance has previously been poisoned");
            }
            if state == INCOMPLETE || state == POISONED {
                let next = RUNNING + if queued { QUEUED } else { 0 };
                if let Err(new) = self.state_and_queued.compare_exchange_weak(state_and_queued, next, Acquire, Acquire) {
                    state_and_queued = new;
                    continue;
                }
                let mut waiter_queue = CompletionGuard { state_and_queued: &self.state_and_queued, set_state_on_drop_to: POISONED };
                let f_state = OnceState { poisoned: state == POISONED, set_state_to: Cell::new(COMPLETE) };
                f(&f_state);
                waiter_queue.set_state_on_drop_to = f_state.set_state_to.get();
                return;
            }
            // RUNNING on another thread: queue up and wait
            if !queued {
                state_and_queued += QUEUED;
                if let Err(new) = self.state_and_queued.compare_exchange_weak(state, state_and_queued, Relaxed, Acquire) {
                    state_and_queued = new;
                    continue;
                }
            }
            futex_wait(&self.state_and_queued, state_and_queued, None);
            state_and_queued = self.state_and_queued.load(Acquire);
        }
    }
}

impl fmt::Debug for Once {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Once").finish_non_exhaustive()
    }
}

pub struct OnceLock<T> {
    once: Once,
    value: UnsafeCell<Option<T>>,
}

unsafe impl<T: Sync + Send> Sync for OnceLock<T> {}
unsafe impl<T: Send> Send for OnceLock<T> {}

impl<T> OnceLock<T> {
    pub const fn new() -> OnceLock<T> {
        OnceLock { once: Once::new(), value: UnsafeCell::new(None) }
    }

    pub fn get(&self) -> Option<&T> {
        if self.once.is_completed() {
            unsafe { (*self.value.get()).as_ref() }
        } else {
            None
        }
    }

    pub fn get_mut(&mut self) -> Option<&mut T> {
        self.value.get_mut().as_mut()
    }

    pub fn wait(&self) -> &T {
        self.once.wait_force();
        match unsafe { (*self.value.get()).as_ref() } {
            Some(v) => v,
            None => panic!("OnceLock completed without a value"),
        }
    }

    pub fn set(&self, value: T) -> Result<(), T> {
        match self.try_insert(value) {
            Ok(_) => Ok(()),
            Err((_, value)) => Err(value),
        }
    }

    pub fn try_insert(&self, value: T) -> Result<&T, (&T, T)> {
        let mut value = Some(value);
        let res = self.get_or_init(|| match value.take() {
            Some(v) => v,
            None => panic!("unreachable"),
        });
        match value {
            None => Ok(res),
            Some(value) => Err((res, value)),
        }
    }

    pub fn get_or_init<F: FnOnce() -> T>(&self, f: F) -> &T {
        if let Some(v) = self.get() {
            return v;
        }
        self.initialize(f);
        match unsafe { (*self.value.get()).as_ref() } {
            Some(v) => v,
            None => panic!("OnceLock completed without a value"),
        }
    }

    pub fn get_mut_or_init<F: FnOnce() -> T>(&mut self, f: F) -> &mut T {
        if self.get().is_none() {
            self.initialize(f);
        }
        match self.value.get_mut().as_mut() {
            Some(v) => v,
            None => panic!("OnceLock completed without a value"),
        }
    }

    pub fn get_or_try_init<E, F: FnOnce() -> Result<T, E>>(&self, f: F) -> Result<&T, E> {
        if let Some(v) = self.get() {
            return Ok(v);
        }
        let mut res: Result<(), E> = Ok(());
        let slot = &self.value;
        let mut f = Some(f);
        self.once.call(true, &mut |p: &OnceState| {
            let f = match f.take() {
                Some(f) => f,
                None => return,
            };
            match f() {
                Ok(value) => unsafe {
                    *slot.get() = Some(value);
                },
                Err(e) => {
                    res = Err(e);
                    p.set_state_to.set(INCOMPLETE);
                }
            }
        });
        match res {
            Ok(()) => match unsafe { (*self.value.get()).as_ref() } {
                Some(v) => Ok(v),
                None => panic!("OnceLock completed without a value"),
            },
            Err(e) => Err(e),
        }
    }

    pub fn into_inner(mut self) -> Option<T> {
        self.take()
    }

    pub fn take(&mut self) -> Option<T> {
        if self.once.is_completed() {
            self.once = Once::new();
            self.value.get_mut().take()
        } else {
            None
        }
    }

    #[cold]
    fn initialize<F: FnOnce() -> T>(&self, f: F) {
        let slot = &self.value;
        let mut f = Some(f);
        self.once.call(true, &mut |_p: &OnceState| {
            if let Some(f) = f.take() {
                let value = f();
                unsafe {
                    *slot.get() = Some(value);
                }
            }
        });
    }
}

impl<T> Default for OnceLock<T> {
    fn default() -> OnceLock<T> {
        OnceLock::new()
    }
}

impl<T: fmt::Debug> fmt::Debug for OnceLock<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut d = f.debug_tuple("OnceLock");
        match self.get() {
            Some(v) => d.field(v),
            None => d.field(&format_args!("<uninit>")),
        };
        d.finish()
    }
}

impl<T: Clone> Clone for OnceLock<T> {
    fn clone(&self) -> OnceLock<T> {
        let cell = OnceLock::new();
        if let Some(value) = self.get() {
            let _ = cell.set(value.clone());
        }
        cell
    }
}

impl<T> From<T> for OnceLock<T> {
    fn from(value: T) -> OnceLock<T> {
        let cell = OnceLock::new();
        let _ = cell.set(value);
        cell
    }
}

impl<T: PartialEq> PartialEq for OnceLock<T> {
    fn eq(&self, other: &OnceLock<T>) -> bool {
        self.get() == other.get()
    }
}

impl<T: Eq> Eq for OnceLock<T> {}

/// A value initialized on first access by `F`.
pub struct LazyLock<T, F = fn() -> T> {
    once: Once,
    init: UnsafeCell<Option<F>>,
    value: UnsafeCell<Option<T>>,
}

unsafe impl<T: Sync + Send, F: Send> Sync for LazyLock<T, F> {}

impl<T, F: FnOnce() -> T> LazyLock<T, F> {
    pub const fn new(f: F) -> LazyLock<T, F> {
        LazyLock { once: Once::new(), init: UnsafeCell::new(Some(f)), value: UnsafeCell::new(None) }
    }

    pub fn force(this: &LazyLock<T, F>) -> &T {
        if !this.once.is_completed() {
            let init = &this.init;
            let value = &this.value;
            this.once.call(false, &mut |_p: &OnceState| {
                let f = unsafe { (*init.get()).take() };
                match f {
                    Some(f) => unsafe { *value.get() = Some(f()) },
                    None => panic!("LazyLock instance has previously been poisoned"),
                }
            });
        }
        match unsafe { (*this.value.get()).as_ref() } {
            Some(v) => v,
            None => panic!("LazyLock instance has previously been poisoned"),
        }
    }

    pub fn into_inner(this: LazyLock<T, F>) -> Result<T, F> {
        let LazyLock { once, init, value } = this;
        if once.is_completed() {
            match value.into_inner() {
                Some(v) => Ok(v),
                None => panic!("LazyLock instance has previously been poisoned"),
            }
        } else {
            match init.into_inner() {
                Some(f) => Err(f),
                None => panic!("LazyLock instance has previously been poisoned"),
            }
        }
    }

    pub fn get(this: &LazyLock<T, F>) -> Option<&T> {
        if this.once.is_completed() {
            unsafe { (*this.value.get()).as_ref() }
        } else {
            None
        }
    }
}

impl<T, F: FnOnce() -> T> Deref for LazyLock<T, F> {
    type Target = T;
    fn deref(&self) -> &T {
        LazyLock::force(self)
    }
}

impl<T: Default> Default for LazyLock<T> {
    fn default() -> LazyLock<T> {
        LazyLock::new(T::default)
    }
}

impl<T: fmt::Debug, F> fmt::Debug for LazyLock<T, F> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut d = f.debug_tuple("LazyLock");
        let v = if self.once.is_completed() { unsafe { (*self.value.get()).as_ref() } } else { None };
        match v {
            Some(v) => d.field(v),
            None => d.field(&format_args!("<uninit>")),
        };
        d.finish()
    }
}
