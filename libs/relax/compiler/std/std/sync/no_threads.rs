//! rustc's sys/sync no_threads primitives. No OS waits or atomic lock state.
use core::cell::Cell;
use core::time::Duration;

pub struct RawMutex { locked: Cell<bool> }
unsafe impl Send for RawMutex {}
unsafe impl Sync for RawMutex {}
impl RawMutex {
    pub const fn new() -> Self { Self { locked: Cell::new(false) } }
    pub fn lock(&self) { assert!(!self.locked.replace(true), "cannot recursively acquire mutex"); }
    pub fn try_lock(&self) -> bool { !self.locked.replace(true) }
    pub unsafe fn unlock(&self) { self.locked.set(false); }
}

pub struct RawRwLock { mode: Cell<isize> }
unsafe impl Send for RawRwLock {}
unsafe impl Sync for RawRwLock {}
impl RawRwLock {
    pub const fn new() -> Self { Self { mode: Cell::new(0) } }
    pub fn read(&self) {
        let m = self.mode.get();
        if m < 0 { crate::process::abort(); }
        self.mode.set(m.checked_add(1).expect("rwlock overflowed read locks"));
    }
    pub fn try_read(&self) -> bool {
        let m = self.mode.get();
        if m < 0 || m == isize::MAX { return false; }
        self.mode.set(m + 1);
        true
    }
    pub fn write(&self) {
        if self.mode.get() != 0 { crate::process::abort(); }
        self.mode.set(-1);
    }
    pub fn try_write(&self) -> bool {
        if self.mode.get() != 0 { return false; }
        self.mode.set(-1);
        true
    }
    pub unsafe fn read_unlock(&self) { assert!(self.mode.replace(self.mode.get() - 1) > 0, "rwlock has not been locked for reading"); }
    pub unsafe fn write_unlock(&self) { assert_eq!(self.mode.replace(0), -1, "rwlock has not been locked for writing"); }
    pub unsafe fn downgrade(&self) { assert_eq!(self.mode.replace(1), -1, "rwlock has not been locked for writing"); }
}

pub struct RawCondvar {}
impl RawCondvar {
    pub const fn new() -> Self { Self {} }
    pub fn notify_one(&self) {}
    pub fn notify_all(&self) {}
    pub unsafe fn wait(&self, _mutex: &RawMutex, timeout: Option<Duration>) -> bool {
        match timeout {
            Some(dur) => { crate::thread::sleep(dur); false }
            None => panic!("condvar wait not supported"),
        }
    }
}

pub struct Parker {}
impl Parker {
    pub const fn new() -> Self { Self {} }
    pub fn park(&self) {}
    pub fn park_timeout(&self, _dur: Duration) {}
    pub fn unpark(&self) {}
}

/// thread::sleep (no wait registry without threads).
pub(crate) fn sleep(dur: Duration) {
    crate::sys::time::sleep(dur)
}
