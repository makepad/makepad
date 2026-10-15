//! Futex-based lock primitives (Rust std's sys/sync/{mutex,condvar,rwlock}/futex.rs and
//! thread_parking/futex.rs algorithms, MIT/Apache-2.0), over sys::os::futex_*.

use core::sync::atomic::AtomicU32;
use core::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use core::time::Duration;



// makepad inspect's wait registry (loader entries): a blocking wait names what it waits on and,
// for a mutex, its holder. Called only around a wait that blocks anyway.
extern "C" {
    fn __relax_wait_enter(kind: u32, addr: usize, holder: usize);
    fn __relax_wait_leave();
    // makepad inspect's timeline: a wake is an edge that ends someone's wait (recorded only while
    // a client records; the flag is the loader's)
    fn __relax_wake(addr: usize);
    static __relax_recording: core::sync::atomic::AtomicBool;
}

/// futex_wake, noted on makepad inspect's timeline while it records.
pub(crate) fn futex_wake(futex: &AtomicU32) -> bool {
    if unsafe { __relax_recording.load(Relaxed) } {
        unsafe { __relax_wake(futex.as_ptr() as usize) };
    }
    crate::sys::os::futex_wake(futex)
}

/// futex_wake_all, noted like futex_wake.
pub(crate) fn futex_wake_all(futex: &AtomicU32) {
    if unsafe { __relax_recording.load(Relaxed) } {
        unsafe { __relax_wake(futex.as_ptr() as usize) };
    }
    crate::sys::os::futex_wake_all(futex)
}

pub(crate) const WAIT_MUTEX: u32 = 1;
pub(crate) const WAIT_CONDVAR: u32 = 2;
pub(crate) const WAIT_READ: u32 = 3;
pub(crate) const WAIT_WRITE: u32 = 4;
pub(crate) const WAIT_PARK: u32 = 5;
pub(crate) const WAIT_ONCE: u32 = 6;

/// futex_wait, recorded in the wait registry while it blocks.
pub(crate) fn futex_wait(futex: &AtomicU32, expected: u32, timeout: Option<Duration>, kind: u32, holder: usize) -> bool {
    unsafe { __relax_wait_enter(kind, futex.as_ptr() as usize, holder) };
    let r = crate::sys::os::futex_wait(futex, expected, timeout);
    unsafe { __relax_wait_leave() };
    r
}

fn spin_loop() {
    core::hint::spin_loop();
}

// ---- Mutex: 0 unlocked; otherwise the holder's tag, with CONTENDED set when threads wait
//
// The tag names the holder without a thread-local lookup: its stack page number (bits 1..31,
// bit 0 set so a tag is never 0). makepad inspect maps a tag to the thread whose stack holds that
// page (hang analysis: who holds the lock a thread waits for).

pub struct RawMutex {
    futex: AtomicU32,
}

const UNLOCKED: u32 = 0;
const CONTENDED: u32 = 1 << 31;

/// The calling thread's holder tag.
#[inline(always)]
fn holder_tag() -> u32 {
    let mark = 0u8;
    let sp = core::hint::black_box(&mark) as *const u8 as usize;
    ((((sp >> 12) << 1) | 1) & 0x7fff_ffff) as u32
}

impl RawMutex {
    pub const fn new() -> RawMutex {
        RawMutex { futex: AtomicU32::new(UNLOCKED) }
    }

    pub fn try_lock(&self) -> bool {
        self.futex.compare_exchange(UNLOCKED, holder_tag(), Acquire, Relaxed).is_ok()
    }

    pub fn lock(&self) {
        let me = holder_tag();
        if self.futex.compare_exchange(UNLOCKED, me, Acquire, Relaxed).is_err() {
            self.lock_contended(me);
        }
    }

    #[cold]
    fn lock_contended(&self, me: u32) {
        let mut state = self.spin();
        if state == UNLOCKED {
            match self.futex.compare_exchange(UNLOCKED, me, Acquire, Relaxed) {
                Ok(_) => return,
                Err(s) => state = s,
            }
        }
        loop {
            // after a wait, others may wait too: the lock is taken marked contended
            if state == UNLOCKED {
                match self.futex.compare_exchange(UNLOCKED, me | CONTENDED, Acquire, Relaxed) {
                    Ok(_) => return,
                    Err(s) => {
                        state = s;
                        continue;
                    }
                }
            }
            if state & CONTENDED == 0 {
                if let Err(s) = self.futex.compare_exchange(state, state | CONTENDED, Relaxed, Relaxed) {
                    state = s;
                    continue;
                }
                state |= CONTENDED;
            }
            futex_wait(&self.futex, state, None, WAIT_MUTEX, (state & !CONTENDED) as usize);
            state = self.spin();
        }
    }

    fn spin(&self) -> u32 {
        let mut spin = 100;
        loop {
            let state = self.futex.load(Relaxed);
            if state == UNLOCKED || state & CONTENDED != 0 || spin == 0 {
                return state;
            }
            spin_loop();
            spin -= 1;
        }
    }

    /// # Safety: the calling thread holds the lock.
    pub unsafe fn unlock(&self) {
        if self.futex.swap(UNLOCKED, Release) & CONTENDED != 0 {
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
        let r = futex_wait(&self.futex, value, timeout, WAIT_CONDVAR, 0);
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
            futex_wait(&self.state, state | READERS_WAITING, None, WAIT_READ, 0);
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
            futex_wait(&self.writer_notify, seq, None, WAIT_WRITE, 0);
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
            futex_wait(&self.state, PARK_PARKED, None, WAIT_PARK, 0);
            if self.state.compare_exchange(PARK_NOTIFIED, PARK_EMPTY, Acquire, Acquire).is_ok() {
                return;
            }
        }
    }
    pub fn park_timeout(&self, timeout: Duration) {
        if self.state.fetch_sub(1, Acquire) == PARK_NOTIFIED {
            return;
        }
        futex_wait(&self.state, PARK_PARKED, Some(timeout), WAIT_PARK, 0);
        self.state.swap(PARK_EMPTY, Acquire);
    }
    pub fn unpark(&self) {
        if self.state.swap(PARK_NOTIFIED, Release) == PARK_PARKED {
            futex_wake(&self.state);
        }
    }
}
