//! Futex-based lock primitives (Rust std's sys/sync/{mutex,condvar,rwlock}/futex.rs and
//! thread_parking/futex.rs algorithms, MIT/Apache-2.0), over sys::os::futex_*.

use core::sync::atomic::AtomicU32;
use core::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use core::time::Duration;

use crate::sys::os::{futex_wait, futex_wake, futex_wake_all};

fn spin_loop() {
    core::hint::spin_loop();
}

// ---- Mutex: 0 unlocked, 1 locked, 2 locked with waiters

pub struct RawMutex {
    futex: AtomicU32,
}

const UNLOCKED: u32 = 0;
const LOCKED: u32 = 1;
const CONTENDED: u32 = 2;

impl RawMutex {
    pub const fn new() -> RawMutex {
        RawMutex { futex: AtomicU32::new(UNLOCKED) }
    }

    pub fn try_lock(&self) -> bool {
        self.futex.compare_exchange(UNLOCKED, LOCKED, Acquire, Relaxed).is_ok()
    }

    pub fn lock(&self) {
        if self.futex.compare_exchange(UNLOCKED, LOCKED, Acquire, Relaxed).is_err() {
            self.lock_contended();
        }
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
            if state != LOCKED || spin == 0 {
                return state;
            }
            spin_loop();
            spin -= 1;
        }
    }

    /// # Safety: the calling thread holds the lock.
    pub unsafe fn unlock(&self) {
        if self.futex.swap(UNLOCKED, Release) == CONTENDED {
            futex_wake(&self.futex);
        }
    }
}

// ---- Condvar: a sequence counter

pub struct RawCondvar {
    futex: AtomicU32,
}

impl RawCondvar {
    pub const fn new() -> RawCondvar {
        RawCondvar { futex: AtomicU32::new(0) }
    }
    pub fn notify_one(&self) {
        self.futex.fetch_add(1, Relaxed);
        futex_wake(&self.futex);
    }
    pub fn notify_all(&self) {
        self.futex.fetch_add(1, Relaxed);
        futex_wake_all(&self.futex);
    }
    /// # Safety: `mutex` is locked by this thread. Returns false on timeout.
    pub unsafe fn wait(&self, mutex: &RawMutex, timeout: Option<Duration>) -> bool {
        let value = self.futex.load(Relaxed);
        mutex.unlock();
        let r = futex_wait(&self.futex, value, timeout);
        mutex.lock();
        r
    }
}

// ---- RwLock

pub struct RawRwLock {
    state: AtomicU32,
    writer_notify: AtomicU32,
}

const READ_LOCKED: u32 = 1;
const MASK: u32 = (1 << 30) - 1;
const WRITE_LOCKED: u32 = MASK;
const DOWNGRADE: u32 = READ_LOCKED.wrapping_sub(WRITE_LOCKED);
const MAX_READERS: u32 = MASK - 1;
const READERS_WAITING: u32 = 1 << 30;
const WRITERS_WAITING: u32 = 1 << 31;

fn is_unlocked(state: u32) -> bool {
    state & MASK == 0
}
fn is_write_locked(state: u32) -> bool {
    state & MASK == WRITE_LOCKED
}
fn has_readers_waiting(state: u32) -> bool {
    state & READERS_WAITING != 0
}
fn has_writers_waiting(state: u32) -> bool {
    state & WRITERS_WAITING != 0
}
fn is_read_lockable(state: u32) -> bool {
    state & MASK < MAX_READERS && !has_readers_waiting(state) && !has_writers_waiting(state)
}
fn is_read_lockable_after_wakeup(state: u32) -> bool {
    state & MASK < MAX_READERS && !has_readers_waiting(state) && !is_write_locked(state) && !is_unlocked(state)
}
fn has_reached_max_readers(state: u32) -> bool {
    state & MASK == MAX_READERS
}

impl RawRwLock {
    pub const fn new() -> RawRwLock {
        RawRwLock { state: AtomicU32::new(0), writer_notify: AtomicU32::new(0) }
    }

    pub fn try_read(&self) -> bool {
        let mut state = self.state.load(Relaxed);
        loop {
            if !is_read_lockable(state) {
                return false;
            }
            match self.state.compare_exchange_weak(state, state + READ_LOCKED, Acquire, Relaxed) {
                Ok(_) => return true,
                Err(s) => state = s,
            }
        }
    }

    pub fn read(&self) {
        let state = self.state.load(Relaxed);
        if !is_read_lockable(state) || self.state.compare_exchange_weak(state, state + READ_LOCKED, Acquire, Relaxed).is_err() {
            self.read_contended();
        }
    }

    /// # Safety: read-locked by this thread.
    pub unsafe fn read_unlock(&self) {
        let state = self.state.fetch_sub(READ_LOCKED, Release) - READ_LOCKED;
        if is_unlocked(state) && has_writers_waiting(state) {
            self.wake_writer_or_readers(state);
        }
    }

    #[cold]
    fn read_contended(&self) {
        let mut has_slept = false;
        let mut state = self.spin_read();
        loop {
            if (has_slept && is_read_lockable_after_wakeup(state)) || is_read_lockable(state) {
                match self.state.compare_exchange_weak(state, state + READ_LOCKED, Acquire, Relaxed) {
                    Ok(_) => return,
                    Err(s) => {
                        state = s;
                        continue;
                    }
                }
            }
            if has_reached_max_readers(state) {
                panic!("too many active read locks on RwLock");
            }
            if !has_readers_waiting(state) {
                if let Err(s) = self.state.compare_exchange(state, state | READERS_WAITING, Relaxed, Relaxed) {
                    state = s;
                    continue;
                }
            }
            futex_wait(&self.state, state | READERS_WAITING, None);
            has_slept = true;
            state = self.spin_read();
        }
    }

    pub fn try_write(&self) -> bool {
        let mut state = self.state.load(Relaxed);
        loop {
            if !is_unlocked(state) {
                return false;
            }
            match self.state.compare_exchange_weak(state, state + WRITE_LOCKED, Acquire, Relaxed) {
                Ok(_) => return true,
                Err(s) => state = s,
            }
        }
    }

    pub fn write(&self) {
        if self.state.compare_exchange_weak(0, WRITE_LOCKED, Acquire, Relaxed).is_err() {
            self.write_contended();
        }
    }

    /// # Safety: write-locked by this thread.
    pub unsafe fn write_unlock(&self) {
        let state = self.state.fetch_sub(WRITE_LOCKED, Release) - WRITE_LOCKED;
        if has_writers_waiting(state) || has_readers_waiting(state) {
            self.wake_writer_or_readers(state);
        }
    }

    /// # Safety: write-locked by this thread.
    pub unsafe fn downgrade(&self) {
        let state = self.state.fetch_add(DOWNGRADE, Release);
        if has_readers_waiting(state) {
            self.state.fetch_sub(READERS_WAITING, Relaxed);
            futex_wake_all(&self.state);
        }
    }

    #[cold]
    fn write_contended(&self) {
        let mut state = self.spin_write();
        let mut other_writers_waiting = 0;
        loop {
            if is_unlocked(state) {
                match self.state.compare_exchange_weak(state, state | WRITE_LOCKED | other_writers_waiting, Acquire, Relaxed) {
                    Ok(_) => return,
                    Err(s) => {
                        state = s;
                        continue;
                    }
                }
            }
            if !has_writers_waiting(state) {
                if let Err(s) = self.state.compare_exchange(state, state | WRITERS_WAITING, Relaxed, Relaxed) {
                    state = s;
                    continue;
                }
            }
            other_writers_waiting = WRITERS_WAITING;
            let seq = self.writer_notify.load(Acquire);
            state = self.state.load(Relaxed);
            if is_unlocked(state) || !has_writers_waiting(state) {
                continue;
            }
            futex_wait(&self.writer_notify, seq, None);
            state = self.spin_write();
        }
    }

    #[cold]
    fn wake_writer_or_readers(&self, mut state: u32) {
        if state == WRITERS_WAITING {
            match self.state.compare_exchange(state, 0, Relaxed, Relaxed) {
                Ok(_) => {
                    self.wake_writer();
                    return;
                }
                Err(s) => state = s,
            }
        }
        if state == READERS_WAITING + WRITERS_WAITING {
            if self.state.compare_exchange(state, READERS_WAITING, Relaxed, Relaxed).is_err() {
                return;
            }
            if self.wake_writer() {
                return;
            }
            state = READERS_WAITING;
        }
        if state == READERS_WAITING {
            if self.state.compare_exchange(state, 0, Relaxed, Relaxed).is_ok() {
                futex_wake_all(&self.state);
            }
        }
    }

    fn wake_writer(&self) -> bool {
        self.writer_notify.fetch_add(1, Release);
        futex_wake(&self.writer_notify)
    }

    fn spin_write(&self) -> u32 {
        let mut spin = 100;
        loop {
            let state = self.state.load(Relaxed);
            if is_unlocked(state) || has_writers_waiting(state) || spin == 0 {
                return state;
            }
            spin_loop();
            spin -= 1;
        }
    }

    fn spin_read(&self) -> u32 {
        let mut spin = 100;
        loop {
            let state = self.state.load(Relaxed);
            if !is_write_locked(state) || has_readers_waiting(state) || has_writers_waiting(state) || spin == 0 {
                return state;
            }
            spin_loop();
            spin -= 1;
        }
    }
}

// ---- Parker (per-thread park/unpark token): 0 empty, 1 notified, u32::MAX parked

pub struct Parker {
    state: AtomicU32,
}

const PARK_EMPTY: u32 = 0;
const PARK_NOTIFIED: u32 = 1;
const PARK_PARKED: u32 = u32::MAX;

impl Parker {
    pub const fn new() -> Parker {
        Parker { state: AtomicU32::new(PARK_EMPTY) }
    }
    /// Only the owning thread parks.
    pub fn park(&self) {
        if self.state.fetch_sub(1, Acquire) == PARK_NOTIFIED {
            return;
        }
        loop {
            futex_wait(&self.state, PARK_PARKED, None);
            if self.state.compare_exchange(PARK_NOTIFIED, PARK_EMPTY, Acquire, Acquire).is_ok() {
                return;
            }
        }
    }
    pub fn park_timeout(&self, timeout: Duration) {
        if self.state.fetch_sub(1, Acquire) == PARK_NOTIFIED {
            return;
        }
        futex_wait(&self.state, PARK_PARKED, Some(timeout));
        self.state.swap(PARK_EMPTY, Acquire);
    }
    pub fn unpark(&self) {
        if self.state.swap(PARK_NOTIFIED, Release) == PARK_PARKED {
            futex_wake(&self.state);
        }
    }
}
