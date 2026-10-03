//! RwLock and its guards.

use core::cell::UnsafeCell;
use core::fmt;
use core::ops::{Deref, DerefMut};

use super::poison::{self, LockResult, PoisonError, TryLockError, TryLockResult};
use super::raw::RawRwLock;

pub struct RwLock<T> {
    inner: RawRwLock,
    poison: poison::Flag,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for RwLock<T> {}
unsafe impl<T: Send + Sync> Sync for RwLock<T> {}

pub struct RwLockReadGuard<'a, T> {
    lock: &'a RwLock<T>,
}

pub struct RwLockWriteGuard<'a, T> {
    lock: &'a RwLock<T>,
    poison: poison::Guard,
}

unsafe impl<'a, T: Sync> Sync for RwLockReadGuard<'a, T> {}
unsafe impl<'a, T: Sync> Sync for RwLockWriteGuard<'a, T> {}

impl<T> RwLock<T> {
    pub const fn new(t: T) -> RwLock<T> {
        RwLock { inner: RawRwLock::new(), poison: poison::Flag::new(), data: UnsafeCell::new(t) }
    }

    pub fn read(&self) -> LockResult<RwLockReadGuard<'_, T>> {
        self.inner.read();
        let g = RwLockReadGuard { lock: self };
        if self.poison.get() {
            Err(PoisonError::new(g))
        } else {
            Ok(g)
        }
    }

    pub fn try_read(&self) -> TryLockResult<RwLockReadGuard<'_, T>> {
        if self.inner.try_read() {
            let g = RwLockReadGuard { lock: self };
            if self.poison.get() {
                Err(TryLockError::Poisoned(PoisonError::new(g)))
            } else {
                Ok(g)
            }
        } else {
            Err(TryLockError::WouldBlock)
        }
    }

    pub fn write(&self) -> LockResult<RwLockWriteGuard<'_, T>> {
        self.inner.write();
        poison::map_result(self.poison.guard(), |guard| RwLockWriteGuard { lock: self, poison: guard })
    }

    pub fn try_write(&self) -> TryLockResult<RwLockWriteGuard<'_, T>> {
        if self.inner.try_write() {
            match poison::map_result(self.poison.guard(), |guard| RwLockWriteGuard { lock: self, poison: guard }) {
                Ok(g) => Ok(g),
                Err(e) => Err(TryLockError::Poisoned(e)),
            }
        } else {
            Err(TryLockError::WouldBlock)
        }
    }

    pub fn is_poisoned(&self) -> bool {
        self.poison.get()
    }

    pub fn clear_poison(&self) {
        self.poison.clear();
    }

    pub fn into_inner(self) -> LockResult<T> {
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

impl<T: Default> Default for RwLock<T> {
    fn default() -> RwLock<T> {
        RwLock::new(T::default())
    }
}

impl<T> From<T> for RwLock<T> {
    fn from(t: T) -> RwLock<T> {
        RwLock::new(t)
    }
}

impl<T: fmt::Debug> fmt::Debug for RwLock<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut d = f.debug_struct("RwLock");
        match self.try_read() {
            Ok(guard) => {
                d.field("data", &&*guard);
            }
            Err(TryLockError::Poisoned(err)) => {
                d.field("data", &&**err.get_ref());
            }
            Err(TryLockError::WouldBlock) => {
                d.field("data", &format_args!("<locked>"));
            }
        }
        d.field("poisoned", &self.poison.get());
        d.finish_non_exhaustive()
    }
}

impl<'a, T> Deref for RwLockReadGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<'a, T> Drop for RwLockReadGuard<'a, T> {
    fn drop(&mut self) {
        unsafe { self.lock.inner.read_unlock() }
    }
}

impl<'a, T> Deref for RwLockWriteGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<'a, T> DerefMut for RwLockWriteGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<'a, T> Drop for RwLockWriteGuard<'a, T> {
    fn drop(&mut self) {
        self.lock.poison.done(&self.poison);
        unsafe { self.lock.inner.write_unlock() }
    }
}

impl<'a, T: fmt::Debug> fmt::Debug for RwLockReadGuard<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}
impl<'a, T: fmt::Display> fmt::Display for RwLockReadGuard<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}
impl<'a, T: fmt::Debug> fmt::Debug for RwLockWriteGuard<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}
impl<'a, T: fmt::Display> fmt::Display for RwLockWriteGuard<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}
