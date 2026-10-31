//! Locks whose synchronization state stays inline, including on macOS. A lazy pthread box
//! could otherwise outlive the compiler generation in which its first lock allocated it.
pub use std::sync::{LockResult, PoisonError, TryLockError, TryLockResult};

#[cfg(not(target_os = "macos"))]
pub use std::sync::{Condvar, Mutex, MutexGuard, WaitTimeoutResult};
#[cfg(target_os = "macos")]
pub use macos::{Condvar, Mutex, MutexGuard, WaitTimeoutResult};

#[cfg(target_os = "macos")]
mod macos {
    use super::{LockResult, PoisonError, TryLockError, TryLockResult};
    use std::cell::UnsafeCell;
    use std::ffi::{c_char, c_int, c_void, CStr};
    use std::fmt;
    use std::marker::PhantomData;
    use std::ops::{Deref, DerefMut};
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering::{Acquire, Relaxed, Release}};
    use std::time::{Duration, Instant};

    type OsWait = unsafe extern "C" fn(*mut c_void, u64, usize, u32) -> c_int;
    type OsWaitTimeout = unsafe extern "C" fn(*mut c_void, u64, usize, u32, u32, u64) -> c_int;
    type OsWake = unsafe extern "C" fn(*mut c_void, usize, u32) -> c_int;

    struct OsSync {
        wait: OsWait,
        wait_timeout: OsWaitTimeout,
        wake_one: OsWake,
        wake_all: OsWake,
    }

    const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;
    const UL_COMPARE_AND_WAIT: u32 = 1;
    const ULF_WAKE_ALL: u32 = 0x100;
    const OS_CLOCK_MACH_ABSOLUTE_TIME: u32 = 32;
    const EINTR: c_int = 4;
    const ETIMEDOUT: c_int = 60;

    #[link(name = "System")]
    unsafe extern "C" {
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        fn __error() -> *mut c_int;
        fn __ulock_wait(operation: u32, addr: *mut c_void, value: u64, timeout_us: u32) -> c_int;
        fn __ulock_wake(operation: u32, addr: *mut c_void, wake_value: u64) -> c_int;
    }

    fn os_sync() -> Option<OsSync> {
        // Match std's weak dlsym cache: racing first users may resolve the same libSystem
        // symbol, but no initializer parks a thread (std's parker can itself allocate).
        const UNKNOWN: *mut c_void = 1usize as *mut c_void;
        static WAIT: AtomicPtr<c_void> = AtomicPtr::new(UNKNOWN);
        static WAIT_TIMEOUT: AtomicPtr<c_void> = AtomicPtr::new(UNKNOWN);
        static WAKE_ONE: AtomicPtr<c_void> = AtomicPtr::new(UNKNOWN);
        static WAKE_ALL: AtomicPtr<c_void> = AtomicPtr::new(UNKNOWN);
        fn lookup(slot: &AtomicPtr<c_void>, name: &CStr) -> *mut c_void {
            let mut ptr = slot.load(Acquire);
            if ptr == UNKNOWN {
                ptr = unsafe { dlsym(RTLD_DEFAULT, name.as_ptr()) };
                slot.store(ptr, Release);
            }
            ptr
        }
        // Resolve the whole family so wait and wake always choose the same backend.
        let wait = lookup(&WAIT, c"os_sync_wait_on_address");
        let wait_timeout = lookup(&WAIT_TIMEOUT, c"os_sync_wait_on_address_with_timeout");
        let wake_one = lookup(&WAKE_ONE, c"os_sync_wake_by_address_any");
        let wake_all = lookup(&WAKE_ALL, c"os_sync_wake_by_address_all");
        if wait.is_null() || wait_timeout.is_null() || wake_one.is_null() || wake_all.is_null() {
            None
        } else {
            // The signatures are those in macOS's os/os_sync_wait_on_address.h.
            Some(unsafe { OsSync {
                wait: std::mem::transmute::<*mut c_void, OsWait>(wait),
                wait_timeout: std::mem::transmute::<*mut c_void, OsWaitTimeout>(wait_timeout),
                wake_one: std::mem::transmute::<*mut c_void, OsWake>(wake_one),
                wake_all: std::mem::transmute::<*mut c_void, OsWake>(wake_all),
            } })
        }
    }

    /// Returns false only when the full timeout elapsed; other errors are spurious wakes.
    fn futex_wait(futex: &AtomicU32, expected: u32, timeout: Option<Duration>) -> bool {
        // As in std, an unrepresentable deadline means an infinite wait. Keep one deadline
        // across EINTR and the shorter waits needed by ulock's u32 microsecond timeout.
        let deadline = timeout.and_then(|d| Instant::now().checked_add(d));
        loop {
            if futex.load(Relaxed) != expected {
                return true;
            }
            let remaining = match deadline {
                Some(end) => match end.checked_duration_since(Instant::now()) {
                    Some(d) if !d.is_zero() => Some(d),
                    _ => return false,
                },
                None => None,
            };
            let addr = futex.as_ptr().cast();
            let r = unsafe {
                if let Some(api) = os_sync() {
                    match remaining {
                        Some(d) => (api.wait_timeout)(addr, expected as u64, 4, 0,
                            OS_CLOCK_MACH_ABSOLUTE_TIME, d.as_nanos().min(u64::MAX as u128) as u64),
                        None => (api.wait)(addr, expected as u64, 4, 0),
                    }
                } else {
                    // Zero means forever; round finite waits up and split long ones.
                    let us = remaining.map_or(0, |d| d.as_nanos().div_ceil(1000).min(u32::MAX as u128) as u32);
                    __ulock_wait(UL_COMPARE_AND_WAIT, addr, expected as u64, us)
                }
            };
            if r >= 0 {
                return true;
            }
            match unsafe { *__error() } {
                EINTR => continue,
                ETIMEDOUT => {
                    if deadline.is_some_and(|end| Instant::now() >= end) {
                        return false;
                    }
                }
                _ => return true,
            }
        }
    }

    fn futex_wake(futex: &AtomicU32, all: bool) {
        let addr = futex.as_ptr().cast();
        let api = os_sync();
        loop {
            let r = unsafe {
                if let Some(api) = &api {
                    let wake = if all { api.wake_all } else { api.wake_one };
                    wake(addr, 4, 0)
                } else {
                    __ulock_wake(UL_COMPARE_AND_WAIT | if all { ULF_WAKE_ALL } else { 0 }, addr, 0)
                }
            };
            // An interrupted wake must be retried or an unlocked mutex can strand waiters.
            if r >= 0 || unsafe { *__error() } != EINTR {
                return;
            }
        }
    }

    // State transitions and orderings from Rust std's sys/sync/mutex/futex.rs
    // (MIT OR Apache-2.0). Only the platform wait/wake above differs.
    struct RawMutex {
        futex: AtomicU32,
    }
    const UNLOCKED: u32 = 0;
    const LOCKED: u32 = 1;
    const CONTENDED: u32 = 2;

    impl RawMutex {
        const fn new() -> Self { Self { futex: AtomicU32::new(UNLOCKED) } }

        #[inline]
        fn try_lock(&self) -> bool {
            self.futex.compare_exchange(UNLOCKED, LOCKED, Acquire, Relaxed).is_ok()
        }

        #[inline]
        fn lock(&self) {
            if !self.try_lock() { self.lock_contended(); }
        }

        #[cold]
        fn lock_contended(&self) {
            let mut state = self.spin();
            if state == UNLOCKED {
                match self.futex.compare_exchange(UNLOCKED, LOCKED, Acquire, Relaxed) {
                    Ok(_) => return,
                    Err(s) => state = s,
                }
            }
            loop {
                if state != CONTENDED && self.futex.swap(CONTENDED, Acquire) == UNLOCKED {
                    return;
                }
                futex_wait(&self.futex, CONTENDED, None);
                state = self.spin();
            }
        }

        fn spin(&self) -> u32 {
            let mut spin = 100;
            loop {
                let state = self.futex.load(Relaxed);
                if state != LOCKED || spin == 0 { return state; }
                std::hint::spin_loop();
                spin -= 1;
            }
        }

        #[inline]
        unsafe fn unlock(&self) {
            if self.futex.swap(UNLOCKED, Release) == CONTENDED { self.wake(); }
        }

        #[cold]
        fn wake(&self) { futex_wake(&self.futex, false); }
    }

    /// A poisoning mutex with inline synchronization state.
    pub struct Mutex<T: ?Sized> {
        inner: RawMutex,
        poisoned: AtomicBool,
        data: UnsafeCell<T>,
    }

    // Only the owner of the acquired mutex can access its data.
    unsafe impl<T: ?Sized + Send> Send for Mutex<T> {}
    unsafe impl<T: ?Sized + Send> Sync for Mutex<T> {}
    impl<T: ?Sized> UnwindSafe for Mutex<T> {}
    impl<T: ?Sized> RefUnwindSafe for Mutex<T> {}

    impl<T> Mutex<T> {
        pub const fn new(value: T) -> Self {
            Self { inner: RawMutex::new(), poisoned: AtomicBool::new(false), data: UnsafeCell::new(value) }
        }

        pub fn into_inner(self) -> LockResult<T> {
            let poisoned = self.is_poisoned();
            let value = self.data.into_inner();
            if poisoned { Err(PoisonError::new(value)) } else { Ok(value) }
        }
    }

    impl<T: ?Sized> Mutex<T> {
        #[inline]
        pub fn lock(&self) -> LockResult<MutexGuard<'_, T>> {
            self.inner.lock();
            unsafe { MutexGuard::new(self) }
        }

        pub fn try_lock(&self) -> TryLockResult<MutexGuard<'_, T>> {
            if self.inner.try_lock() {
                unsafe { MutexGuard::new(self) }.map_err(TryLockError::Poisoned)
            } else {
                Err(TryLockError::WouldBlock)
            }
        }

        pub fn get_mut(&mut self) -> LockResult<&mut T> {
            let poisoned = self.is_poisoned();
            let value = self.data.get_mut();
            if poisoned { Err(PoisonError::new(value)) } else { Ok(value) }
        }

        pub fn is_poisoned(&self) -> bool { self.poisoned.load(Relaxed) }
        pub fn clear_poison(&self) { self.poisoned.store(false, Relaxed); }
    }

    impl<T: Default> Default for Mutex<T> {
        fn default() -> Self { Self::new(T::default()) }
    }
    impl<T> From<T> for Mutex<T> {
        fn from(value: T) -> Self { Self::new(value) }
    }
    impl<T: ?Sized + fmt::Debug> fmt::Debug for Mutex<T> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            let mut d = f.debug_struct("Mutex");
            match self.try_lock() {
                Ok(guard) => { d.field("data", &&*guard); }
                Err(TryLockError::Poisoned(e)) => { d.field("data", &&**e.get_ref()); }
                Err(TryLockError::WouldBlock) => { d.field("data", &"<locked>"); }
            }
            d.field("poisoned", &self.is_poisoned()).finish_non_exhaustive()
        }
    }

    /// Unlocks on drop and records a panic that began while the guard was held.
    #[must_use = "if unused the Mutex will immediately unlock"]
    pub struct MutexGuard<'a, T: ?Sized> {
        lock: &'a Mutex<T>,
        panicking: bool,
        // Keep guards on their acquiring thread, as std requires.
        _not_send: PhantomData<*mut ()>,
    }
    unsafe impl<T: ?Sized + Sync> Sync for MutexGuard<'_, T> {}

    impl<'a, T: ?Sized> MutexGuard<'a, T> {
        /// The caller holds the raw mutex and transfers ownership of it to this guard.
        unsafe fn new(lock: &'a Mutex<T>) -> LockResult<Self> {
            let guard = Self { lock, panicking: std::thread::panicking(), _not_send: PhantomData };
            if lock.is_poisoned() { Err(PoisonError::new(guard)) } else { Ok(guard) }
        }
    }
    impl<T: ?Sized> Deref for MutexGuard<'_, T> {
        type Target = T;
        fn deref(&self) -> &T { unsafe { &*self.lock.data.get() } }
    }
    impl<T: ?Sized> DerefMut for MutexGuard<'_, T> {
        fn deref_mut(&mut self) -> &mut T { unsafe { &mut *self.lock.data.get() } }
    }
    impl<T: ?Sized> Drop for MutexGuard<'_, T> {
        #[inline]
        fn drop(&mut self) {
            if !self.panicking && std::thread::panicking() {
                self.lock.poisoned.store(true, Relaxed);
            }
            unsafe { self.lock.inner.unlock(); }
        }
    }
    impl<T: ?Sized + fmt::Debug> fmt::Debug for MutexGuard<'_, T> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { fmt::Debug::fmt(&**self, f) }
    }
    impl<T: ?Sized + fmt::Display> fmt::Display for MutexGuard<'_, T> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { fmt::Display::fmt(&**self, f) }
    }

    /// A notification sequence counter; the associated mutex supplies memory ordering.
    pub struct Condvar {
        futex: AtomicU32,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct WaitTimeoutResult(bool);
    impl WaitTimeoutResult {
        pub fn timed_out(&self) -> bool { self.0 }
    }

    impl Condvar {
        pub const fn new() -> Self { Self { futex: AtomicU32::new(0) } }

        pub fn notify_one(&self) {
            self.futex.fetch_add(1, Relaxed);
            futex_wake(&self.futex, false);
        }

        pub fn notify_all(&self) {
            self.futex.fetch_add(1, Relaxed);
            futex_wake(&self.futex, true);
        }

        // Rust std's sys/sync/condvar/futex.rs: read the sequence before unlocking so a
        // notification between unlock and wait cannot be lost. The caller owns the mutex.
        unsafe fn wait_optional_timeout(&self, mutex: &RawMutex, timeout: Option<Duration>) -> bool {
            let value = self.futex.load(Relaxed);
            unsafe { mutex.unlock(); }
            let success = futex_wait(&self.futex, value, timeout);
            mutex.lock();
            success
        }

        pub fn wait<'a, T>(&self, guard: MutexGuard<'a, T>) -> LockResult<MutexGuard<'a, T>> {
            unsafe { self.wait_optional_timeout(&guard.lock.inner, None); }
            if guard.lock.is_poisoned() { Err(PoisonError::new(guard)) } else { Ok(guard) }
        }

        pub fn wait_while<'a, T, F>(&self, mut guard: MutexGuard<'a, T>, mut condition: F) -> LockResult<MutexGuard<'a, T>>
        where F: FnMut(&mut T) -> bool {
            while condition(&mut *guard) { guard = self.wait(guard)?; }
            Ok(guard)
        }

        pub fn wait_timeout<'a, T>(&self, guard: MutexGuard<'a, T>, dur: Duration) -> LockResult<(MutexGuard<'a, T>, WaitTimeoutResult)> {
            let success = unsafe { self.wait_optional_timeout(&guard.lock.inner, Some(dur)) };
            let poisoned = guard.lock.is_poisoned();
            let result = (guard, WaitTimeoutResult(!success));
            if poisoned { Err(PoisonError::new(result)) } else { Ok(result) }
        }

        pub fn wait_timeout_while<'a, T, F>(&self, mut guard: MutexGuard<'a, T>, dur: Duration, mut condition: F) -> LockResult<(MutexGuard<'a, T>, WaitTimeoutResult)>
        where F: FnMut(&mut T) -> bool {
            let start = Instant::now();
            loop {
                if !condition(&mut *guard) { return Ok((guard, WaitTimeoutResult(false))); }
                let timeout = match dur.checked_sub(start.elapsed()) {
                    Some(timeout) => timeout,
                    None => return Ok((guard, WaitTimeoutResult(true))),
                };
                guard = self.wait_timeout(guard, timeout)?.0;
            }
        }
    }
    impl Default for Condvar {
        fn default() -> Self { Self::new() }
    }
    impl fmt::Debug for Condvar {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.debug_struct("Condvar").finish_non_exhaustive() }
    }
}

#[cfg(test)]
mod tests {
    use super::{Condvar, Mutex, TryLockError};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn mutual_exclusion() {
        let count = Mutex::new(0);
        let start = std::sync::Barrier::new(8);
        thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    start.wait();
                    for _ in 0..10_000 { *count.lock().unwrap() += 1; }
                });
            }
        });
        assert_eq!(count.into_inner().unwrap(), 80_000);
    }

    #[test]
    fn condvar_ping_pong() {
        let count = Mutex::new(0);
        let cv = Condvar::new();
        thread::scope(|scope| {
            scope.spawn(|| {
                for _ in 0..1000 {
                    let mut n = cv.wait_while(count.lock().unwrap(), |n| *n % 2 == 0).unwrap();
                    *n += 1;
                    cv.notify_one();
                }
            });
            for _ in 0..1000 {
                let mut n = cv.wait_while(count.lock().unwrap(), |n| *n % 2 != 0).unwrap();
                *n += 1;
                cv.notify_all();
            }
        });
        assert_eq!(count.into_inner().unwrap(), 2000);
    }

    #[test]
    fn wait_timeout() {
        let mutex = Mutex::new(7);
        let cv = Condvar::new();
        let mut guard = mutex.lock().unwrap();
        // Spurious wakes are allowed on every backend; a genuine timeout must be reported.
        loop {
            let (next, result) = cv.wait_timeout(guard, Duration::from_millis(10)).unwrap();
            guard = next;
            if result.timed_out() { break; }
        }
        assert_eq!(*guard, 7);
        let (guard, result) = cv.wait_timeout_while(guard, Duration::from_millis(10), |_| true).unwrap();
        assert!(result.timed_out());
        let (guard, result) = cv.wait_timeout_while(guard, Duration::ZERO, |_| false).unwrap();
        assert!(!result.timed_out());
        assert_eq!(*guard, 7);
    }

    #[test]
    fn poisoning() {
        let mut mutex = Mutex::new(0);
        assert!(std::panic::catch_unwind(|| {
            let mut guard = mutex.lock().unwrap();
            *guard = 7;
            panic!("poison the mutex");
        }).is_err());
        assert!(mutex.is_poisoned());
        assert_eq!(*mutex.lock().unwrap_err().into_inner(), 7);
        match mutex.try_lock() {
            Err(TryLockError::Poisoned(e)) => assert_eq!(*e.into_inner(), 7),
            other => panic!("expected poison, got {other:?}"),
        }
        assert_eq!(*mutex.get_mut().unwrap_err().into_inner(), 7);
        mutex.clear_poison();
        assert!(!mutex.is_poisoned());
        *mutex.get_mut().unwrap() = 8;
        assert_eq!(mutex.into_inner().unwrap(), 8);
    }

    #[test]
    fn const_statics() {
        static MUTEX: Mutex<bool> = Mutex::new(false);
        static CV: Condvar = Condvar::new();
        let mut guard = MUTEX.try_lock().unwrap();
        assert!(matches!(MUTEX.try_lock(), Err(TryLockError::WouldBlock)));
        *guard = true;
        let (guard, result) = CV.wait_timeout(guard, Duration::ZERO).unwrap();
        assert!(result.timed_out());
        assert!(*guard);
        CV.notify_one();
        CV.notify_all();
    }
}
