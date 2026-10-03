//! ReentrantLock: a mutex the owning thread may lock again (stdout uses it).

use core::cell::UnsafeCell;
use core::fmt;
use core::ops::Deref;
use core::sync::atomic::AtomicUsize;
use core::sync::atomic::Ordering::Relaxed;

use super::raw::RawMutex;

pub struct ReentrantLock<T> {
    mutex: RawMutex,
    owner: AtomicUsize, // pthread_self of the owner, 0 = none
    lock_count: UnsafeCell<u32>,
    data: T,
}

unsafe impl<T: Send> Send for ReentrantLock<T> {}
unsafe impl<T: Send> Sync for ReentrantLock<T> {}

pub struct ReentrantLockGuard<'a, T> {
    lock: &'a ReentrantLock<T>,
}

fn current_thread() -> usize {
    unsafe { crate::sys::pthread_self() }
}

impl<T> ReentrantLock<T> {
    pub const fn new(t: T) -> ReentrantLock<T> {
        ReentrantLock { mutex: RawMutex::new(), owner: AtomicUsize::new(0), lock_count: UnsafeCell::new(0), data: t }
    }

    pub fn lock(&self) -> ReentrantLockGuard<'_, T> {
        let this_thread = current_thread();
        unsafe {
            if self.owner.load(Relaxed) == this_thread {
                self.increment_lock_count();
            } else {
                self.mutex.lock();
                self.owner.store(this_thread, Relaxed);
                *self.lock_count.get() = 1;
            }
        }
        ReentrantLockGuard { lock: self }
    }

    pub fn try_lock(&self) -> Option<ReentrantLockGuard<'_, T>> {
        let this_thread = current_thread();
        unsafe {
            if self.owner.load(Relaxed) == this_thread {
                self.increment_lock_count();
                Some(ReentrantLockGuard { lock: self })
            } else if self.mutex.try_lock() {
                self.owner.store(this_thread, Relaxed);
                *self.lock_count.get() = 1;
                Some(ReentrantLockGuard { lock: self })
            } else {
                None
            }
        }
    }

    pub fn get_mut(&mut self) -> &mut T {
        &mut self.data
    }

    pub fn into_inner(self) -> T {
        self.data
    }

    unsafe fn increment_lock_count(&self) {
        let c = &mut *self.lock_count.get();
        *c = match c.checked_add(1) {
            Some(n) => n,
            None => panic!("lock count overflow in reentrant mutex"),
        };
    }
}

impl<'a, T> Deref for ReentrantLockGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.lock.data
    }
}

impl<'a, T> Drop for ReentrantLockGuard<'a, T> {
    fn drop(&mut self) {
        unsafe {
            let c = &mut *self.lock.lock_count.get();
            *c -= 1;
            if *c == 0 {
                self.lock.owner.store(0, Relaxed);
                self.lock.mutex.unlock();
            }
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for ReentrantLock<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut d = f.debug_struct("ReentrantLock");
        match self.try_lock() {
            Some(v) => d.field("data", &&*v),
            None => d.field("data", &format_args!("<locked>")),
        };
        d.finish_non_exhaustive()
    }
}
