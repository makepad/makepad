//! Mutex, MutexGuard and Condvar (they share the raw lock).
//! Unsized bounds follow library/std/src/sync/poison/mutex.rs.

use core::cell::UnsafeCell;
use core::fmt;
use core::ops::{Deref, DerefMut};
use core::time::Duration;

use super::poison::{self, LockResult, PoisonError, TryLockError, TryLockResult};
use super::raw::{RawCondvar, RawMutex};
use crate::time::Instant;

pub struct Mutex<T: ?Sized> {
    inner: RawMutex,
    poison: poison::Flag,
    data: UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Send> Send for Mutex<T> {}
unsafe impl<T: ?Sized + Send> Sync for Mutex<T> {}

#[rustc_diagnostic_item = "MutexGuard"]
pub struct MutexGuard<'a, T: ?Sized + 'a> {
    lock: &'a Mutex<T>,
    poison: poison::Guard,
}

unsafe impl<'a, T: ?Sized + Sync> Sync for MutexGuard<'a, T> {}
// library/std/src/sync/poison/mutex.rs:289
impl<T: ?Sized> !Send for MutexGuard<'_, T> {}

impl<T: ?Sized> Mutex<T> {
    pub const fn new(t: T) -> Mutex<T> where T: Sized {
        Mutex { inner: RawMutex::new(), poison: poison::Flag::new(), data: UnsafeCell::new(t) }
    }

    #[rustc_should_not_be_called_on_const_items]
    pub fn lock(&self) -> LockResult<MutexGuard<'_, T>> {
        self.inner.lock();
        MutexGuard::new(self)
    }

    #[rustc_should_not_be_called_on_const_items]
    pub fn try_lock(&self) -> TryLockResult<MutexGuard<'_, T>> {
        if self.inner.try_lock() {
            match MutexGuard::new(self) {
                Ok(g) => Ok(g),
                Err(e) => Err(TryLockError::Poisoned(e)),
            }
        } else {
            Err(TryLockError::WouldBlock)
        }
    }

    #[rustc_should_not_be_called_on_const_items]
    pub fn replace(&self, value: T) -> LockResult<T> where T: Sized {
        match self.lock() {
            Ok(mut g) => Ok(crate::mem::replace(&mut *g, value)),
            Err(e) => {
                let mut g = e.into_inner();
                Err(PoisonError::new(crate::mem::replace(&mut *g, value)))
            }
        }
    }

    #[rustc_should_not_be_called_on_const_items]
    pub fn set(&self, value: T) -> Result<(), PoisonError<T>> where T: Sized {
        self.replace(value).map(drop)
    }

    pub fn is_poisoned(&self) -> bool {
        self.poison.get()
    }

    #[rustc_should_not_be_called_on_const_items]
    pub fn clear_poison(&self) {
        self.poison.clear();
    }

    pub fn into_inner(self) -> LockResult<T> where T: Sized {
        let poisoned = self.poison.get();
        let data = self.data.into_inner();
        if poisoned {
            Err(PoisonError::new(data))
        } else {
            Ok(data)
        }
    }

    pub fn get_mut(&mut self) -> LockResult<&mut T> {
        let poisoned = self.poison.get();
        let data = self.data.get_mut();
        if poisoned {
            Err(PoisonError::new(data))
        } else {
            Ok(data)
        }
    }
}

impl<T> From<T> for Mutex<T> {
    fn from(t: T) -> Mutex<T> {
        Mutex::new(t)
    }
}

impl<T: Default> Default for Mutex<T> {
    fn default() -> Mutex<T> {
        Mutex::new(T::default())
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for Mutex<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut d = f.debug_struct("Mutex");
        match self.try_lock() {
            Ok(guard) => {
                d.field("data", &&*guard);
            }
            Err(TryLockError::Poisoned(err)) => {
                d.field("data", &&**err.get_ref());
            }
            Err(TryLockError::WouldBlock) => {
                d.field("data", &"<locked>");
            }
        }
        d.field("poisoned", &self.poison.get());
        d.finish_non_exhaustive()
    }
}

impl<'a, T: ?Sized> MutexGuard<'a, T> {
    fn new(lock: &'a Mutex<T>) -> LockResult<MutexGuard<'a, T>> {
        poison::map_result(lock.poison.guard(), |guard| MutexGuard { lock, poison: guard })
    }
}

impl<'a, T: ?Sized> Deref for MutexGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<'a, T: ?Sized> DerefMut for MutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<'a, T: ?Sized> Drop for MutexGuard<'a, T> {
    fn drop(&mut self) {
        self.lock.poison.done(&self.poison);
        unsafe { self.lock.inner.unlock() }
    }
}

impl<'a, T: ?Sized + fmt::Debug> fmt::Debug for MutexGuard<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<'a, T: ?Sized + fmt::Display> fmt::Display for MutexGuard<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub struct WaitTimeoutResult(bool);

impl WaitTimeoutResult {
    pub fn timed_out(&self) -> bool {
        self.0
    }
}

pub struct Condvar {
    inner: RawCondvar,
}

impl Condvar {
    pub const fn new() -> Condvar {
        Condvar { inner: RawCondvar::new() }
    }

    #[rustc_should_not_be_called_on_const_items]
    pub fn wait<'a, T>(&self, guard: MutexGuard<'a, T>) -> LockResult<MutexGuard<'a, T>> {
        let poisoned = unsafe {
            let lock = guard.lock;
            self.inner.wait(&lock.inner, None);
            lock.poison.get()
        };
        if poisoned {
            Err(PoisonError::new(guard))
        } else {
            Ok(guard)
        }
    }

    #[rustc_should_not_be_called_on_const_items]
    pub fn wait_while<'a, T, F: FnMut(&mut T) -> bool>(&self, mut guard: MutexGuard<'a, T>, mut condition: F) -> LockResult<MutexGuard<'a, T>> {
        while condition(&mut *guard) {
            guard = self.wait(guard)?;
        }
        Ok(guard)
    }

    #[rustc_should_not_be_called_on_const_items]
    pub fn wait_timeout<'a, T>(&self, guard: MutexGuard<'a, T>, dur: Duration) -> LockResult<(MutexGuard<'a, T>, WaitTimeoutResult)> {
        let (poisoned, success) = unsafe {
            let lock = guard.lock;
            let success = self.inner.wait(&lock.inner, Some(dur));
            (lock.poison.get(), success)
        };
        let result = (guard, WaitTimeoutResult(!success));
        if poisoned {
            Err(PoisonError::new(result))
        } else {
            Ok(result)
        }
    }

    #[rustc_should_not_be_called_on_const_items]
    pub fn wait_timeout_ms<'a, T>(&self, guard: MutexGuard<'a, T>, ms: u32) -> LockResult<(MutexGuard<'a, T>, bool)> {
        match self.wait_timeout(guard, Duration::from_millis(ms as u64)) {
            Ok((g, r)) => Ok((g, !r.timed_out())),
            Err(e) => {
                let (g, r) = e.into_inner();
                Err(PoisonError::new((g, !r.timed_out())))
            }
        }
    }

    #[rustc_should_not_be_called_on_const_items]
    pub fn wait_timeout_while<'a, T, F: FnMut(&mut T) -> bool>(&self, mut guard: MutexGuard<'a, T>, dur: Duration, mut condition: F) -> LockResult<(MutexGuard<'a, T>, WaitTimeoutResult)> {
        let start = Instant::now();
        loop {
            if !condition(&mut *guard) {
                return Ok((guard, WaitTimeoutResult(false)));
            }
            let timeout = match dur.checked_sub(start.elapsed()) {
                Some(timeout) => timeout,
                None => return Ok((guard, WaitTimeoutResult(true))),
            };
            guard = self.wait_timeout(guard, timeout)?.0;
        }
    }

    #[rustc_should_not_be_called_on_const_items]
    pub fn notify_one(&self) {
        self.inner.notify_one()
    }

    #[rustc_should_not_be_called_on_const_items]
    pub fn notify_all(&self) {
        self.inner.notify_all()
    }
}

impl fmt::Debug for Condvar {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Condvar").finish_non_exhaustive()
    }
}

impl Default for Condvar {
    fn default() -> Condvar {
        Condvar::new()
    }
}
