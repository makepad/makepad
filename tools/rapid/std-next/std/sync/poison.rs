//! Lock poisoning. Under Rapid a panic never unwinds through a guard's Drop (R11), so
//! locks are never poisoned there; the API and the rustc behaviour are kept.

use core::error::Error;
use core::fmt;
use core::sync::atomic::AtomicBool;
use core::sync::atomic::Ordering::Relaxed;

pub struct Flag {
    failed: AtomicBool,
}

/// Whether the thread was already panicking when the guard was taken.
pub struct Guard {
    panicking: bool,
}

impl Flag {
    pub const fn new() -> Flag {
        Flag { failed: AtomicBool::new(false) }
    }
    pub fn guard(&self) -> LockResult<Guard> {
        let ret = Guard { panicking: crate::thread::panicking() };
        if self.get() {
            Err(PoisonError::new(ret))
        } else {
            Ok(ret)
        }
    }
    pub fn done(&self, guard: &Guard) {
        if !guard.panicking && crate::thread::panicking() {
            self.failed.store(true, Relaxed);
        }
    }
    pub fn get(&self) -> bool {
        self.failed.load(Relaxed)
    }
    pub fn clear(&self) {
        self.failed.store(false, Relaxed)
    }
}

pub fn map_result<T, U, F: FnOnce(T) -> U>(result: LockResult<T>, f: F) -> LockResult<U> {
    match result {
        Ok(t) => Ok(f(t)),
        Err(PoisonError { data }) => Err(PoisonError::new(f(data))),
    }
}

pub struct PoisonError<T> {
    data: T,
}

pub enum TryLockError<T> {
    Poisoned(PoisonError<T>),
    WouldBlock,
}

pub type LockResult<T> = Result<T, PoisonError<T>>;
pub type TryLockResult<T> = Result<T, TryLockError<T>>;

impl<T> PoisonError<T> {
    pub fn new(data: T) -> PoisonError<T> {
        PoisonError { data }
    }
    pub fn into_inner(self) -> T {
        self.data
    }
    pub fn get_ref(&self) -> &T {
        &self.data
    }
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.data
    }
}

impl<T> fmt::Debug for PoisonError<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("PoisonError").finish_non_exhaustive()
    }
}

impl<T> fmt::Display for PoisonError<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("poisoned lock: another task failed inside")
    }
}

impl<T> Error for PoisonError<T> {}

impl<T> From<PoisonError<T>> for TryLockError<T> {
    fn from(err: PoisonError<T>) -> TryLockError<T> {
        TryLockError::Poisoned(err)
    }
}

impl<T> fmt::Debug for TryLockError<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            TryLockError::Poisoned(..) => f.write_str("Poisoned(..)"),
            TryLockError::WouldBlock => f.write_str("WouldBlock"),
        }
    }
}

impl<T> fmt::Display for TryLockError<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            TryLockError::Poisoned(..) => f.write_str("poisoned lock: another task failed inside"),
            TryLockError::WouldBlock => f.write_str("try_lock failed because the operation would block"),
        }
    }
}

impl<T> Error for TryLockError<T> {}
